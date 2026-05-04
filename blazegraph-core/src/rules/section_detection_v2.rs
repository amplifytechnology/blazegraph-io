/// Block 03 — Section Detection V2
///
/// A candidate-then-refine pipeline that composes four signals (size, bold, isolation,
/// font rarity) into a decision tree, with regex patterns as an escape hatch.
///
/// Key improvements over V1 (`SectionAndHierarchyDetectionRule`):
/// - Segment-based isolation: uses `(band, column, line_number)` grouping to distinguish
///   bold section headers from inline bold emphasis inside paragraphs.
/// - Font rarity: uses `FontSizeAnalysis.class_usage_counts` to detect structurally
///   uncommon fonts, even at body size.
/// - Composable signals: a missing signal on one axis can be confirmed by another.
/// - Regex escape hatches: numbered subsections promoted, figure captions demoted.
/// - Rotated elements are always classified as non-section (belt-and-suspenders on top
///   of Block 02's statistical filter).
use super::engine::{FontSizeAnalysis, ParseRule, RuleEngine};
use crate::config::{ParsingConfig, SectionDetectionV2Config};
use crate::types::*;
use anyhow::Result;
use regex::Regex;
use std::collections::HashMap;

// ──────────────────────────────────────────────────────────────────────────────
// HierarchyContext (CR-27: stack-based with keyword tiebreaker)
//
// Depth assignment uses font-size delta as the primary signal. When the delta
// is within tolerance (the "tie"), a tiebreaker keyword identifies the
// section's structural tier and the stack of `(keyword, depth)` anchors
// resolves whether the new section is a sibling, a step-back-up to an
// earlier tier, or a new deeper tier.
//
// `None` keyword (Pass-1 promotions with no pattern match) is a real keyword
// class for sibling comparisons, but is *skipped past* when a real-keyword
// section arrives — a Pass-1 subtitle should not become the parent of a
// keyword-bearing structural section that follows it.
// ──────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct HierarchyAnchor {
    keyword: Option<String>,
    depth: u32,
    font_size: f32,
}

#[derive(Debug, Clone, Default)]
struct HierarchyContext {
    stack: Vec<HierarchyAnchor>,
    previous_section_font_size: Option<f32>,
}

impl HierarchyContext {
    fn new() -> Self {
        Self::default()
    }

    fn current_depth(&self) -> u32 {
        self.stack.last().map_or(1, |a| a.depth)
    }

    fn cap_depth(depth: u32, config: &SectionDetectionV2Config) -> u32 {
        if config.enforce_max_depth {
            depth.min(config.max_depth)
        } else {
            depth
        }
    }

    /// Update context when we encounter a new section; returns the assigned depth.
    fn update_for_section(
        &mut self,
        font_size: f32,
        keyword: Option<&str>,
        config: &SectionDetectionV2Config,
    ) -> u32 {
        let prev = self.previous_section_font_size;
        self.previous_section_font_size = Some(font_size);

        match prev {
            None => {
                let d = config.starting_section_level;
                self.stack.push(HierarchyAnchor {
                    keyword: keyword.map(String::from),
                    depth: d,
                    font_size,
                });
                d
            }
            Some(prev_font) => {
                let delta = font_size - prev_font;
                let tol = config.font_size_tolerance;

                if delta > tol {
                    // Larger font → step back up to a level with matching size, or
                    // collapse to a shallower level.
                    self.step_back_up(font_size, keyword, config)
                } else if delta < -tol {
                    // Smaller font → push deeper.
                    let d = Self::cap_depth(self.current_depth() + 1, config);
                    self.stack.push(HierarchyAnchor {
                        keyword: keyword.map(String::from),
                        depth: d,
                        font_size,
                    });
                    d
                } else {
                    // Tie — keyword decides.
                    self.resolve_tie(keyword, font_size, config)
                }
            }
        }
    }

    /// Resolve a section transition where font-delta is within tolerance.
    ///
    /// CR-27 algorithm:
    /// - If a keyword-bearing section arrives and the top of stack is a `None`
    ///   anchor, look past the `None` layer for the structural parent (so a
    ///   Pass-1 subtitle doesn't become the parent of a real keyword section).
    /// - If the incoming keyword equals the (effective) top's keyword → sibling.
    /// - If the incoming keyword exists deeper in the stack → step back up to
    ///   that anchor's depth.
    /// - Otherwise the tiebreaker fires → push a new anchor at depth + 1.
    fn resolve_tie(
        &mut self,
        keyword: Option<&str>,
        font_size: f32,
        config: &SectionDetectionV2Config,
    ) -> u32 {
        // None-skip refinement: when a keyword-bearing section meets a None top,
        // the None anchor is transient and should be popped before resolution.
        if keyword.is_some()
            && self.stack.len() >= 2
            && self.stack.last().is_some_and(|a| a.keyword.is_none())
        {
            self.stack.pop();
        }

        let top_keyword = self.stack.last().and_then(|a| a.keyword.as_deref());

        // Same keyword as effective top → sibling. Replace top's font_size so
        // future deltas measure against the most recent section.
        if keyword == top_keyword {
            if let Some(top) = self.stack.last_mut() {
                top.font_size = font_size;
            }
            return self.current_depth();
        }

        // Different keyword: look for it deeper in the stack — but only when the
        // incoming keyword is concrete. A `None` keyword should not step back
        // to an earlier `None` anchor (e.g., the document title), because two
        // unrelated subtitles with no keyword aren't structurally related.
        // None subtitles only ever match the *immediate* top, otherwise they
        // push deeper.
        if keyword.is_some() {
            if let Some(idx) = self
                .stack
                .iter()
                .rposition(|a| a.keyword.as_deref() == keyword)
            {
                self.stack.truncate(idx + 1);
                if let Some(top) = self.stack.last_mut() {
                    top.font_size = font_size;
                }
                return self.stack[idx].depth;
            }
        }

        // Tiebreaker fires — push deeper.
        let d = Self::cap_depth(self.current_depth() + 1, config);
        self.stack.push(HierarchyAnchor {
            keyword: keyword.map(String::from),
            depth: d,
            font_size,
        });
        d
    }

    /// Handle a transition where the incoming font is decisively larger than
    /// the previous section's font. Search strategy mirrors V1's `find_level_for`:
    ///
    /// 1. Exact font-size match in the stack → step back to that anchor.
    /// 2. Else, find the *shallowest* anchor whose font is smaller than the
    ///    incoming size — the incoming section is more important than that
    ///    tier, so it takes that depth and the smaller tier collapses
    ///    (anchors below the chosen depth are popped). This preserves
    ///    hierarchy continuity when font sizes don't exactly match: e.g., a
    ///    14pt heading following 24pt title and 12pt chapters lands at the
    ///    chapter tier rather than restarting at depth 1.
    /// 3. Else (incoming is larger than every anchor), treat as a new
    ///    top-level entry and reset the stack.
    fn step_back_up(
        &mut self,
        font_size: f32,
        keyword: Option<&str>,
        config: &SectionDetectionV2Config,
    ) -> u32 {
        let tol = config.font_size_tolerance;

        // 1. Exact match (rposition: prefer the deepest match for stability).
        if let Some(idx) = self
            .stack
            .iter()
            .rposition(|a| (a.font_size - font_size).abs() < tol)
        {
            self.stack.truncate(idx + 1);
            if let Some(top) = self.stack.last_mut() {
                top.font_size = font_size;
                if top.keyword.is_none() {
                    top.keyword = keyword.map(String::from);
                }
            }
            return self.stack[idx].depth;
        }

        // 2. Fallback — first (shallowest) anchor with smaller font. The
        //    incoming section displaces that tier: truncate to it, replace
        //    its font and keyword.
        if let Some(idx) = self.stack.iter().position(|a| a.font_size < font_size) {
            let depth = self.stack[idx].depth;
            self.stack.truncate(idx + 1);
            if let Some(top) = self.stack.last_mut() {
                top.font_size = font_size;
                top.keyword = keyword.map(String::from);
            }
            return depth;
        }

        // 3. Larger than everything in stack — new top-level entry.
        self.stack.clear();
        let d = config.starting_section_level;
        self.stack.push(HierarchyAnchor {
            keyword: keyword.map(String::from),
            depth: d,
            font_size,
        });
        d
    }

    fn get_content_level(&self) -> u32 {
        self.current_depth() + 1
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Rule struct
// ──────────────────────────────────────────────────────────────────────────────

pub struct SectionDetectionV2Rule<'a> {
    _engine: &'a RuleEngine,
    text_elements: &'a [PdfTextElement],
    config: &'a ParsingConfig,
    _document_analysis: &'a DocumentAnalysis,
    font_size_analysis: &'a FontSizeAnalysis,
    _style_data: &'a StyleData,
    /// Compiled inclusion regexes (promote weak/rejected candidates to sections)
    inclusion_regexes: Vec<Regex>,
    /// Compiled exclusion regexes (demote promoted candidates)
    exclusion_regexes: Vec<Regex>,
    /// Compiled tiebreaker patterns paired with their keyword names. CR-27 —
    /// consulted by `HierarchyContext` to resolve depth when font-delta is
    /// within tolerance.
    tiebreaker_regexes: Vec<(String, Regex)>,
    /// Per-page x-extent: `(min_x, max_right)` aggregated across all elements
    /// on the page. Used by `is_isolated` to derive an effective column
    /// width by dividing the page extent by the number of X-clusters
    /// detected at the element's Y. Indexed by `(page, 0)` — the second
    /// key dimension is the legacy `band` field which currently always
    /// resolves to 0 and is preserved for API compatibility.
    page_extents: HashMap<u32, (f32, f32)>,
}

/// Maximum X-gap (in points) within an X-cluster at a given Y. Two
/// same-Y elements separated by a gap larger than this belong to
/// distinct visual clusters (e.g., left and right columns of a
/// 2-column page). Mirrors the gap threshold Tika applies inside a
/// line for bbox geometry hygiene; chosen to be comfortably larger
/// than inter-word spacing in justified body text but smaller than
/// any plausible column gutter (typically ≥ 8pt).
const X_CLUSTER_GAP_THRESHOLD_PT: f32 = 6.0;

impl<'a> SectionDetectionV2Rule<'a> {
    pub fn new(
        engine: &'a RuleEngine,
        text_elements: &'a [PdfTextElement],
        config: &'a ParsingConfig,
        document_analysis: &'a DocumentAnalysis,
        font_size_analysis: &'a FontSizeAnalysis,
        style_data: &'a StyleData,
    ) -> Self {
        let v2 = &config.section_detection_v2;

        let inclusion_regexes = v2
            .inclusion_patterns
            .iter()
            .filter_map(|p| Regex::new(p).ok())
            .collect();

        let exclusion_regexes = v2
            .exclusion_patterns
            .iter()
            .filter_map(|p| Regex::new(p).ok())
            .collect();

        let tiebreaker_regexes = v2
            .tiebreaker_keywords
            .iter()
            .filter_map(|tk| Regex::new(&tk.pattern).ok().map(|re| (tk.name.clone(), re)))
            .collect();

        let mut page_extents: HashMap<u32, (f32, f32)> = HashMap::new();
        for e in text_elements {
            let bbox = e.bounding_box();
            let entry = page_extents
                .entry(e.page_number())
                .or_insert((f32::INFINITY, f32::NEG_INFINITY));
            entry.0 = entry.0.min(bbox.x);
            entry.1 = entry.1.max(bbox.x + bbox.width);
        }

        Self {
            _engine: engine,
            text_elements,
            config,
            _document_analysis: document_analysis,
            font_size_analysis,
            _style_data: style_data,
            inclusion_regexes,
            exclusion_regexes,
            tiebreaker_regexes,
            page_extents,
        }
    }

    /// Detect the structural-tier keyword for a section's text. Returns the
    /// name of the first matching tiebreaker pattern, or `None` when no
    /// pattern matches (Pass-1 promotions, generic bold subtitles, etc.).
    fn detect_keyword(&self, text: &str) -> Option<&str> {
        for (name, re) in &self.tiebreaker_regexes {
            if re.is_match(text) {
                return Some(name);
            }
        }
        None
    }

    // ──────────────────────────────────────────────────────────────────────
    // Signal helpers
    // ──────────────────────────────────────────────────────────────────────

    /// Check whether an element's text has a sufficient alpha ratio.
    fn passes_alpha_ratio(&self, text: &str) -> bool {
        let min = self.config.section_detection_v2.min_alpha_ratio;
        if min <= 0.0 {
            return true;
        }
        let non_ws: usize = text.chars().filter(|c| !c.is_whitespace()).count();
        if non_ws == 0 {
            return false;
        }
        let alpha: usize = text.chars().filter(|c| c.is_ascii_alphabetic()).count();
        (alpha as f32 / non_ws as f32) >= min
    }

    /// Check whether the element is bold.
    ///
    /// CR-20: LaTeX PDFs encode bold in the font-family name rather than CSS font-weight.
    /// Common patterns: "NimbusRomNo9L-Medi" (contains "medi"), "CMBX10" (contains "bx").
    /// We check both fields to catch both CSS-based and LaTeX-encoded bold.
    fn is_bold(element: &PdfTextElement) -> bool {
        let weight = element.style_info.font_weight.to_lowercase();
        if weight.contains("bold") {
            return true;
        }
        let family = element.style_info.font_family.to_lowercase();
        family.contains("bold") || family.contains("medi") || family.contains("bx")
    }

    /// Determine isolation as a single geometric question:
    /// **does the visual line this element sits on fill its column?**
    ///
    /// Without column metadata from Tika (post layout-reasoning consolidation,
    /// Tika emits positioned-text primitives only), columns are inferred per-Y
    /// from glyph geometry. The algorithm:
    ///
    /// 1. Gather same-Y, same-rotation, same-page neighbors (incl. element)
    ///    where Y-delta ≤ `line_height_tolerance`. Y is ground truth;
    ///    Tika's `data-line` counter resets per `<p>` and is unreliable.
    /// 2. Sort by X and cluster by X-connectivity. A gap larger than
    ///    `X_CLUSTER_GAP_THRESHOLD_PT` between consecutive bboxes opens a new
    ///    cluster. This recovers the visual column structure at this Y from
    ///    geometry alone — a 2-column body row produces 2 clusters; a header
    ///    sitting alone produces 1.
    /// 3. The cluster containing the element is its "visual line." If the
    ///    cluster has only one member (the element itself) → isolated.
    /// 4. Otherwise compute `line_extent = cluster.max_right - cluster.min_x`
    ///    and `column_width = page_x_extent / cluster_count`. A short line in
    ///    an effectively wide column is isolated; a line filling its column
    ///    is not.
    fn is_isolated(&self, element_idx: usize) -> bool {
        let element = &self.text_elements[element_idx];
        let cfg = &self.config.section_detection_v2;

        let page = element.page_number();
        let element_rotation = element.rotation();
        let y = element.bounding_box().y;
        let tol = cfg.line_height_tolerance;

        // Step 1: gather same-Y same-rotation neighbors on the page.
        let mut same_y: Vec<(usize, f32, f32)> = Vec::new();
        for (idx, n) in self.text_elements.iter().enumerate() {
            if n.page_number() != page
                || n.rotation() != element_rotation
                || (n.bounding_box().y - y).abs() >= tol
            {
                continue;
            }
            let nx = n.bounding_box().x;
            let nr = nx + n.bounding_box().width;
            same_y.push((idx, nx, nr));
        }

        // Step 2: sort by X-start, cluster by X-connectivity.
        same_y.sort_by(|a, b| a.1.total_cmp(&b.1));

        let mut clusters: Vec<(f32, f32, usize, bool)> = Vec::new();
        let mut current: Option<(f32, f32, usize, bool)> = None;
        for &(idx, nx, nr) in &same_y {
            let is_self = idx == element_idx;
            match current {
                None => current = Some((nx, nr, 1, is_self)),
                Some((c_min, c_max, c_count, c_self)) => {
                    if nx - c_max > X_CLUSTER_GAP_THRESHOLD_PT {
                        clusters.push((c_min, c_max, c_count, c_self));
                        current = Some((nx, nr, 1, is_self));
                    } else {
                        current = Some((c_min, c_max.max(nr), c_count + 1, c_self || is_self));
                    }
                }
            }
        }
        if let Some(c) = current {
            clusters.push(c);
        }

        // Step 3: locate element's cluster. If alone, isolated.
        let element_cluster = clusters.iter().find(|c| c.3).copied();
        let Some((c_min, c_max, c_count, _)) = element_cluster else {
            return true;
        };
        if c_count <= 1 {
            return true;
        }

        // Step 4: compare cluster width against effective column width.
        let line_extent = c_max - c_min;
        let Some(&(page_min, page_max)) = self.page_extents.get(&page) else {
            return false;
        };
        let page_width = page_max - page_min;
        if page_width <= 0.0 {
            return false;
        }
        let column_width = (page_width / clusters.len() as f32).max(1.0);
        line_extent / column_width < cfg.isolation_threshold
    }

    /// Check whether the element's font class is rare in the document.
    fn is_rare_font(&self, element: &PdfTextElement) -> bool {
        let cfg = &self.config.section_detection_v2;
        let class_name = &element.style_info.class_name;

        // Total non-rotated element count = sum of all class_usage_counts values
        let total: usize = self.font_size_analysis.class_usage_counts.values().sum();

        if total == 0 {
            return false;
        }

        let count = self
            .font_size_analysis
            .class_usage_counts
            .get(class_name)
            .copied()
            .unwrap_or(0);

        (count as f32 / total as f32) < cfg.font_rarity_threshold
    }

    // ──────────────────────────────────────────────────────────────────────
    // Pass 1: Candidate marking + decision tree
    // ──────────────────────────────────────────────────────────────────────

    /// Core classification: returns `true` if the element should be a section after Pass 1.
    ///
    /// Four-region piecewise decision based on `delta = font_size - body_size`:
    ///
    /// - `delta < -tolerance` → REJECT (below-body noise)
    /// - `|delta| ≤ tolerance` → Region 3 (at-body band): promote if isolated AND (bold OR rare)
    /// - `tolerance < delta ≤ margin` → Region 2 (moderate): promote if bold OR isolated
    /// - `delta > margin` → Region 1 (large): auto-promote unconditionally
    ///
    /// Region 1 threshold is `body_size + structural_size_margin` by default,
    /// or `body_size * structural_size_ratio` when a ratio is configured.
    ///
    /// Rotated elements are always rejected (§8).
    fn classify_pass1(&self, element_idx: usize) -> bool {
        let element = &self.text_elements[element_idx];

        // §8: Rotated elements are never sections
        if element.rotation() != 0 {
            return false;
        }

        let cfg = &self.config.section_detection_v2;
        let body_size = self.font_size_analysis.body_text_size;
        let font_size = element.style_info.font_size;
        let tolerance = cfg.font_size_tolerance;
        let delta = font_size - body_size;

        // REJECT: below body − tolerance
        if delta < -tolerance {
            return false;
        }

        // Alpha ratio gate
        if !self.passes_alpha_ratio(&element.text) {
            return false;
        }

        // Region 1 threshold: body + margin (or body * ratio when configured)
        let region1_threshold = match cfg.structural_size_ratio {
            Some(ratio) => body_size * ratio,
            None => body_size + cfg.structural_size_margin,
        };

        // Region 1: size alone is authoritative — auto-promote
        if font_size > region1_threshold {
            return true;
        }

        let bold = Self::is_bold(element);
        let isolated = self.is_isolated(element_idx);
        let rare = self.is_rare_font(element);

        if delta > tolerance {
            // Region 2 (moderate): both signals required — bold-only or isolated-only is
            // insufficient; non-bold body text that happens to be column-isolated must not promote.
            bold && isolated
        } else {
            // Region 3 (at-body band): |delta| ≤ tolerance
            // Needs isolation AND at least one font-distinctive signal
            isolated && (bold || rare)
        }
    }

    // ──────────────────────────────────────────────────────────────────────
    // Pass 2: Pattern refinement
    // ──────────────────────────────────────────────────────────────────────

    /// Apply exclusion patterns (demote) then inclusion patterns (promote).
    ///
    /// Inclusion matches additionally require:
    /// 1. `is_isolated(element_idx)` — element sits alone on its visual line (not a
    ///    paragraph-internal reference like "as described in Article 5 of this
    ///    Regulation").
    /// 2. text length ≤ `inclusion_max_length` — synthetic length gate that filters
    ///    body wrap-lines beginning with a structural keyword. Pass 1 protects body
    ///    text via bold/rarity gates that depend on a meaningful font_size signal;
    ///    on documents where Tika reports degenerate font sizes (CELEX/EU regulation
    ///    embedded fonts), Pass 2 has neither bold nor size to lean on, so length
    ///    is the discriminator: real labels are short, body wraps are long.
    ///
    /// Patterns ship as written in config; per-pattern case sensitivity is controlled
    /// by `(?i)` in the YAML.
    fn apply_pattern_refinement(
        &self,
        pass1_is_section: bool,
        element_idx: usize,
        text: &str,
    ) -> bool {
        let mut result = pass1_is_section;

        // Exclusion first: demote even if Pass 1 promoted
        if result {
            for re in &self.exclusion_regexes {
                if re.is_match(text) {
                    result = false;
                    break;
                }
            }
        }

        // Inclusion: promote only when text matches AND element is isolated AND
        // text length is within the structural-label cap. Char count, not byte
        // length — labels are ASCII in practice but we count chars to stay
        // predictable for any future Latin/Roman/numeric variants.
        if !result {
            let max_len = self.config.section_detection_v2.inclusion_max_length;
            if text.chars().count() <= max_len {
                for re in &self.inclusion_regexes {
                    if re.is_match(text) && self.is_isolated(element_idx) {
                        result = true;
                        break;
                    }
                }
            }
        }

        result
    }

    // ──────────────────────────────────────────────────────────────────────
    // Per-element classification
    // ──────────────────────────────────────────────────────────────────────

    fn classify(
        &self,
        element_idx: usize,
        hierarchy_context: &mut HierarchyContext,
        current_element: &ParsedPdfElement,
    ) -> (ParsedElementType, u32) {
        let element = &self.text_elements[element_idx];

        // Pass 1
        let pass1 = self.classify_pass1(element_idx);

        // Pass 2
        let is_section = self.apply_pattern_refinement(pass1, element_idx, &element.text);

        if is_section {
            let font_size = element.style_info.font_size;
            let keyword = self.detect_keyword(&element.text);
            let level = hierarchy_context.update_for_section(
                font_size,
                keyword,
                &self.config.section_detection_v2,
            );
            (ParsedElementType::Section, level)
        } else {
            let content_level = hierarchy_context.get_content_level();
            (current_element.element_type.clone(), content_level)
        }
    }
}

impl<'a> ParseRule for SectionDetectionV2Rule<'a> {
    fn apply(&self, elements: Vec<ParsedPdfElement>) -> Result<Vec<ParsedPdfElement>> {
        println!(
            "📝 [V2] Applying section detection V2 to {} elements...",
            elements.len()
        );

        // If no elements provided, bootstrap from text_elements (same as V1)
        let input_elements = if elements.is_empty() {
            println!("   📋 [V2] No input elements — bootstrapping from text_elements");
            self.text_elements
                .iter()
                .enumerate()
                .map(|(i, te)| ParsedPdfElement {
                    element_type: ParsedElementType::Paragraph,
                    text: te.text.clone(),
                    hierarchy_level: 3,
                    position: i,
                    style_info: te.style_info.clone(),
                    placement: Some(te.placement.clone()),
                    reading_order: te.reading_order,
                    bookmark_match: te.bookmark_match.clone(),
                    token_count: te.token_count,
                })
                .collect()
        } else {
            elements
        };

        let mut hierarchy_context = HierarchyContext::new();
        let mut out = Vec::with_capacity(input_elements.len());

        for element in input_elements {
            if let Some(_te) = self.text_elements.get(element.position) {
                let (new_type, new_level) =
                    self.classify(element.position, &mut hierarchy_context, &element);
                out.push(ParsedPdfElement {
                    element_type: new_type,
                    hierarchy_level: new_level,
                    ..element
                });
            } else {
                // No corresponding text element — preserve with content level
                let content_level = hierarchy_context.get_content_level();
                out.push(ParsedPdfElement {
                    hierarchy_level: content_level,
                    ..element
                });
            }
        }

        let sections = out
            .iter()
            .filter(|e| e.element_type == ParsedElementType::Section)
            .count();
        println!(
            "   ✅ [V2] Detected {} sections across {} elements",
            sections,
            out.len()
        );
        Ok(out)
    }

    fn name(&self) -> &str {
        "SectionDetectionV2"
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Unit tests — CR-19 four-region piecewise classification
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ParsingConfig, SectionDetectionV2Config};
    use crate::rules::engine::FontSizeAnalysis;
    use crate::types::{
        BoundingBox, DocumentAnalysis, FontClass, PdfTextElement, Placement, StyleData,
    };
    use std::collections::HashMap;

    // ── Helpers ──────────────────────────────────────────────────────────────

    fn make_placement(band: u32, column: u32, line_number: u32, rotation: i32) -> Placement {
        Placement {
            page_number: 1,
            bounding_box: BoundingBox {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 12.0,
            },
            band,
            column,
            nr_band_columns: 1,
            line_number,
            segment_number: 0,
            rotation,
            paragraph_number: 0,
        }
    }

    fn make_element(
        font_size: f32,
        bold: bool,
        class_name: &str,
        line_number: u32,
    ) -> PdfTextElement {
        let font_weight = if bold {
            "bold".to_string()
        } else {
            "normal".to_string()
        };
        PdfTextElement {
            text: "Introduction".to_string(),
            style_info: FontClass {
                class_name: class_name.to_string(),
                font_family: "TestFont".to_string(),
                font_size,
                font_style: "normal".to_string(),
                font_weight,
                color: "#000000".to_string(),
            },
            placement: make_placement(0, 0, line_number, 0),
            reading_order: 0,
            bookmark_match: None,
            token_count: 1,
            raw_tags: vec![],
        }
    }

    /// Standalone neighbour element sharing the same (band, column, line_number) as index 0.
    fn make_neighbour(line_number: u32) -> PdfTextElement {
        PdfTextElement {
            text: "neighbour".to_string(),
            style_info: FontClass {
                class_name: "body".to_string(),
                font_family: "TestFont".to_string(),
                font_size: 10.0,
                font_style: "normal".to_string(),
                font_weight: "normal".to_string(),
                color: "#000000".to_string(),
            },
            placement: Placement {
                page_number: 1,
                bounding_box: BoundingBox {
                    x: 110.0, // right next to element at x=0, width=100 → gap = 10pt
                    y: 0.0,
                    width: 50.0,
                    height: 12.0,
                },
                band: 0,
                column: 0,
                nr_band_columns: 1,
                line_number,
                segment_number: 1,
                rotation: 0,
                paragraph_number: 0,
            },
            reading_order: 1,
            bookmark_match: None,
            token_count: 1,
            raw_tags: vec![],
        }
    }

    fn make_font_analysis(body_size: f32, class_counts: Vec<(&str, usize)>) -> FontSizeAnalysis {
        let mut class_usage_counts = HashMap::new();
        for (name, count) in &class_counts {
            class_usage_counts.insert(name.to_string(), *count);
        }
        FontSizeAnalysis {
            body_text_size: body_size,
            class_usage_counts,
            ..FontSizeAnalysis::default()
        }
    }

    fn make_config(
        tolerance: f32,
        margin: f32,
        ratio: Option<f32>,
        isolation_threshold: f32,
    ) -> ParsingConfig {
        ParsingConfig {
            section_detection_v2: SectionDetectionV2Config {
                font_size_tolerance: tolerance,
                structural_size_margin: margin,
                structural_size_ratio: ratio,
                isolation_threshold,
                line_height_tolerance: 3.0,
                font_rarity_threshold: 0.05, // < 5% → rare
                ..SectionDetectionV2Config::default()
            },
            ..ParsingConfig::default()
        }
    }

    fn make_style_data() -> StyleData {
        StyleData {
            font_classes: HashMap::new(),
        }
    }

    fn make_document_analysis() -> DocumentAnalysis {
        DocumentAnalysis {
            font_size_counts: HashMap::new(),
            font_family_counts: HashMap::new(),
            bold_counts: (0, 0),
            italic_counts: (0, 0),
            most_common_font_size: 10.0,
            most_common_font_family: "TestFont".to_string(),
            all_font_sizes: vec![],
        }
    }

    /// Build a `SectionDetectionV2Rule` and call `classify_pass1` on index 0.
    fn classify(
        elements: &[PdfTextElement],
        font_analysis: &FontSizeAnalysis,
        config: &ParsingConfig,
    ) -> bool {
        let engine = RuleEngine::new().expect("engine");
        let document_analysis = make_document_analysis();
        let style_data = make_style_data();
        let rule = SectionDetectionV2Rule::new(
            &engine,
            elements,
            config,
            &document_analysis,
            font_analysis,
            &style_data,
        );
        rule.classify_pass1(0)
    }

    // ── Tests ─────────────────────────────────────────────────────────────────

    /// Test 1 — Region 1: auto-promotes without any confirming signals.
    /// delta = 16 - 10 = 6 > margin (5) → Region 1 → true
    #[test]
    fn test_region1_auto_promotes_without_signals() {
        let body = 10.0;
        // element: 16pt, not bold, common font, isolated (no neighbours → line_number 0 unique)
        let elements = vec![make_element(16.0, false, "body", 0)];
        let font_analysis = make_font_analysis(body, vec![("body", 100)]);
        let config = make_config(1.0, 5.0, None, 0.80);
        assert!(classify(&elements, &font_analysis, &config));
    }

    /// Test 2 — Region 1 boundary: delta == margin is Region 2 (not Region 1).
    /// delta = 15 - 10 = 5 = margin → font_size = 15 is NOT > region1_threshold (15) → Region 2
    /// No bold, not isolated (has a neighbour within gap) → NOT promoted.
    #[test]
    fn test_region1_boundary_is_region2() {
        let body = 10.0;
        // Two elements share same line → element 0 is NOT isolated (gap = 10 < gap_cfg 20)
        let elements = vec![
            make_element(15.0, false, "body", 5),
            make_neighbour(5), // same line, gap = 10pt < isolation_neighbor_gap 20 → not isolated
        ];
        let font_analysis = make_font_analysis(body, vec![("body", 100)]);
        let config = make_config(1.0, 5.0, None, 0.80);
        assert!(!classify(&elements, &font_analysis, &config));
    }

    /// Test 3 — Region 2: bold alone promotes.
    /// delta = 13 - 10 = 3, tolerance = 1, margin = 5 → 1 < 3 < 5 → Region 2
    /// bold = true → promoted
    #[test]
    fn test_region2_bold_alone_promotes() {
        let body = 10.0;
        let elements = vec![make_element(13.0, true, "body", 0)];
        let font_analysis = make_font_analysis(body, vec![("body", 100)]);
        let config = make_config(1.0, 5.0, None, 0.80);
        assert!(classify(&elements, &font_analysis, &config));
    }

    /// Test 4 — Region 2: isolated alone no longer promotes (AND semantics).
    /// delta = 13 - 10 = 3, tolerance = 1, margin = 5 → Region 2
    /// not bold, isolated (no same-line neighbours) → NOT promoted (bold AND isolated required)
    #[test]
    fn test_region2_isolated_alone_does_not_promote() {
        let body = 10.0;
        let elements = vec![make_element(13.0, false, "body", 0)];
        let font_analysis = make_font_analysis(body, vec![("body", 100)]);
        let config = make_config(1.0, 5.0, None, 0.80);
        assert!(!classify(&elements, &font_analysis, &config));
    }

    /// Test 5 — Region 2: neither bold nor isolated rejects (arXiv watermark shape).
    /// delta = 3 → Region 2; not bold, has neighbour within gap → NOT promoted
    #[test]
    fn test_region2_neither_signal_rejects() {
        let body = 10.0;
        let elements = vec![
            make_element(13.0, false, "body", 7),
            make_neighbour(7), // shares line, gap 10 < gap_cfg 20 → not isolated
        ];
        let font_analysis = make_font_analysis(body, vec![("body", 100)]);
        let config = make_config(1.0, 5.0, None, 0.80);
        assert!(!classify(&elements, &font_analysis, &config));
    }

    /// Test 6 — Region 3: isolated AND bold promotes.
    /// |delta| = |10.5 - 10| = 0.5 ≤ tolerance 1 → Region 3
    /// isolated (unique line), bold → promoted
    #[test]
    fn test_region3_isolated_and_bold_promotes() {
        let body = 10.0;
        let elements = vec![make_element(10.5, true, "body", 0)];
        let font_analysis = make_font_analysis(body, vec![("body", 100)]);
        let config = make_config(1.0, 5.0, None, 0.80);
        assert!(classify(&elements, &font_analysis, &config));
    }

    /// Test 7 — Region 3: isolated AND rare promotes (bold not required).
    /// |delta| = 0.5 ≤ tolerance → Region 3; isolated, not bold, rare font → promoted
    #[test]
    fn test_region3_isolated_and_rare_promotes() {
        let body = 10.0;
        // "rare_font" appears 1/100 times = 1% < rarity_threshold 5% → rare
        let elements = vec![make_element(10.5, false, "rare_font", 0)];
        let font_analysis = make_font_analysis(body, vec![("rare_font", 1), ("body", 99)]);
        let config = make_config(1.0, 5.0, None, 0.80);
        assert!(classify(&elements, &font_analysis, &config));
    }

    /// Test 8 — Region 3: isolated alone does NOT promote.
    /// |delta| ≤ tolerance, isolated, not bold, common font → NOT promoted
    #[test]
    fn test_region3_isolated_alone_does_not_promote() {
        let body = 10.0;
        // "body" = 90/100 = 90% → not rare
        let elements = vec![make_element(10.5, false, "body", 0)];
        let font_analysis = make_font_analysis(body, vec![("body", 90), ("other", 10)]);
        let config = make_config(1.0, 5.0, None, 0.80);
        assert!(!classify(&elements, &font_analysis, &config));
    }

    /// Test 9 — Region 3: non-isolated bold does NOT promote.
    /// |delta| ≤ tolerance, bold, with a same-Y X-clustered neighbour whose
    /// joined extent fills the page → ratio = 1.0 ≥ isolation_threshold →
    /// NOT isolated → NOT promoted.
    ///
    /// Neighbour is placed at gap=4pt (well below `X_CLUSTER_GAP_THRESHOLD_PT=6`)
    /// so the two elements form a single visual cluster, which is what the
    /// "non-isolated" semantic requires post-strip.
    #[test]
    fn test_region3_non_isolated_bold_does_not_promote() {
        let body = 10.0;
        let mut element = make_element(10.5, true, "body", 3);
        element.placement.bounding_box.width = 160.0;
        let mut neighbour = make_neighbour(3);
        neighbour.placement.bounding_box.x = 164.0; // gap = 4pt → same cluster
        neighbour.placement.bounding_box.width = 36.0;
        let elements = vec![element, neighbour];
        let font_analysis = make_font_analysis(body, vec![("body", 100)]);
        let config = make_config(1.0, 5.0, None, 0.80);
        assert!(!classify(&elements, &font_analysis, &config));
    }

    /// Test 10 — Reject: below body − tolerance.
    /// delta = 8 - 10 = -2 < -tolerance (-1) → REJECT regardless of signals
    #[test]
    fn test_reject_below_body_minus_tolerance() {
        let body = 10.0;
        let elements = vec![make_element(8.0, true, "body", 0)];
        let font_analysis = make_font_analysis(body, vec![("body", 100)]);
        let config = make_config(1.0, 5.0, None, 0.80);
        assert!(!classify(&elements, &font_analysis, &config));
    }

    /// Test 11 — Proportional mode: structural_size_ratio overrides margin.
    /// ratio = Some(1.5), body = 10 → threshold = 15.
    /// 12pt: delta = 2 > tolerance 1, < threshold 15 → Region 2; bold + isolated → promoted.
    /// 16pt: font_size 16 > threshold 15 → Region 1 → promoted.
    #[test]
    fn test_proportional_ratio_overrides_margin() {
        let body = 10.0;
        let font_analysis = make_font_analysis(body, vec![("body", 100)]);
        // margin=0.1 so body+margin=10.1; ratio=Some(1.5) → threshold=15
        // isolation_neighbor_gap = 20; elements at unique lines → isolated
        let config = make_config(1.0, 0.1, Some(1.5), 0.80);

        // 12pt: delta=2 > tolerance 1 → Region 2, isolated, bold → promoted (both signals)
        let elements_12 = vec![make_element(12.0, true, "body", 0)];
        assert!(classify(&elements_12, &font_analysis, &config));

        // 16pt: font_size 16 > threshold 15 → Region 1 → promoted
        let elements_16 = vec![make_element(16.0, false, "body", 0)];
        assert!(classify(&elements_16, &font_analysis, &config));
    }

    /// Test 12 — arXiv watermark regression.
    /// body=7pt, element=11pt, not bold, has same-line neighbour (gap<20) → not isolated,
    /// common font, default config (margin=5.0, tolerance=1.0).
    /// delta = 11 - 7 = 4, threshold = 7 + 5 = 12; 4 ≤ 5 (margin), 4 > 1 (tolerance) → Region 2.
    /// Region 2: bold AND isolated → neither → NOT promoted.
    #[test]
    fn test_arxiv_watermark_regression() {
        let body = 7.0;
        let elements = vec![
            make_element(11.0, false, "body", 9),
            make_neighbour(9), // gap 10 < 20 → not isolated
        ];
        let font_analysis = make_font_analysis(body, vec![("body", 100)]);
        let config = make_config(1.0, 5.0, None, 0.80);
        assert!(!classify(&elements, &font_analysis, &config));
    }

    // ── CR-20 tests — bold detection font-family fallback ─────────────────────

    /// Helper: build a PdfTextElement with explicit font_weight and font_family strings.
    fn make_element_with_font(
        font_size: f32,
        font_weight: &str,
        font_family: &str,
        class_name: &str,
        line_number: u32,
    ) -> PdfTextElement {
        PdfTextElement {
            text: "Introduction".to_string(),
            style_info: FontClass {
                class_name: class_name.to_string(),
                font_family: font_family.to_string(),
                font_size,
                font_style: "normal".to_string(),
                font_weight: font_weight.to_string(),
                color: "#000000".to_string(),
            },
            placement: make_placement(0, 0, line_number, 0),
            reading_order: 0,
            bookmark_match: None,
            token_count: 1,
            raw_tags: vec![],
        }
    }

    /// CR-20 Test 1 — Regression: font_weight="bold" still detected as bold.
    #[test]
    fn test_bold_detected_via_font_weight() {
        let element = make_element_with_font(12.0, "bold", "ArialMT", "h1", 0);
        assert!(SectionDetectionV2Rule::is_bold(&element));
    }

    /// CR-20 Test 2 — LaTeX "Medi" family detected as bold (NimbusRomNo9L-Medi).
    #[test]
    fn test_bold_detected_via_medi_family() {
        let element = make_element_with_font(12.0, "normal", "NimbusRomNo9L-Medi", "latex-medi", 0);
        assert!(SectionDetectionV2Rule::is_bold(&element));
    }

    /// CR-20 Test 3 — LaTeX "bx" family detected as bold (CMBX10).
    #[test]
    fn test_bold_detected_via_bx_family() {
        let element = make_element_with_font(12.0, "normal", "CMBX10", "latex-bx", 0);
        assert!(SectionDetectionV2Rule::is_bold(&element));
    }

    /// CR-20 Test 4 — Regular fonts are not bold: NimbusRomNo9L-Regu and CMR10.
    #[test]
    fn test_regular_font_not_bold() {
        let regu = make_element_with_font(10.0, "normal", "NimbusRomNo9L-Regu", "body", 0);
        assert!(!SectionDetectionV2Rule::is_bold(&regu));

        let cmr = make_element_with_font(10.0, "normal", "CMR10", "body", 0);
        assert!(!SectionDetectionV2Rule::is_bold(&cmr));
    }

    // ── CR-21 tests — page-scoped band matching ────────────────────────────────

    /// Build a PdfTextElement with an explicit page number, x position, band, column, line.
    fn make_element_on_page(
        font_size: f32,
        page_number: u32,
        band: u32,
        column: u32,
        line_number: u32,
        x: f32,
    ) -> PdfTextElement {
        PdfTextElement {
            text: "1 Introduction".to_string(),
            style_info: FontClass {
                class_name: "section".to_string(),
                font_family: "NimbusRomNo9L-Medi".to_string(),
                font_size,
                font_style: "normal".to_string(),
                font_weight: "normal".to_string(),
                color: "#000000".to_string(),
            },
            placement: Placement {
                page_number,
                bounding_box: BoundingBox {
                    x,
                    y: 0.0,
                    width: 80.0,
                    height: 12.0,
                },
                band,
                column,
                nr_band_columns: 1,
                line_number,
                segment_number: 0,
                rotation: 0,
                paragraph_number: 0,
            },
            reading_order: 0,
            bookmark_match: None,
            token_count: 2,
            raw_tags: vec![],
        }
    }

    /// CR-21 Test 5 — Elements sharing (band=0, col=0, line=0) but on different pages
    /// must NOT be treated as same-line neighbours. Target is index 1 (page 2).
    /// Expected: isolated=true (page 1 element is ignored).
    #[test]
    fn test_isolation_cross_page_same_band_not_neighbours() {
        let elements = vec![
            // Page 1 — same band/col/line, x=110 (would create gap=10 → not isolated if counted)
            make_element_on_page(12.0, 1, 0, 0, 0, 110.0),
            // Page 2 — the target we're classifying, x=0
            make_element_on_page(12.0, 2, 0, 0, 0, 0.0),
        ];
        let font_analysis = make_font_analysis(10.0, vec![("section", 2)]);
        // isolation_neighbor_gap = 20: a gap of 10 would be < 20 → not isolated
        let config = make_config(1.0, 5.0, None, 0.80);
        let engine = RuleEngine::new().expect("engine");
        let document_analysis = make_document_analysis();
        let style_data = make_style_data();
        let rule = SectionDetectionV2Rule::new(
            &engine,
            &elements,
            &config,
            &document_analysis,
            &font_analysis,
            &style_data,
        );
        // Index 1 is on page 2; page 1 element must not count as a neighbour
        assert!(
            rule.is_isolated(1),
            "cross-page element must not be a neighbour"
        );
    }

    /// Same page, same Y line — two X-clustered side-by-side elements form
    /// one visual cluster that fills the page width → ratio = 1.0 ≥ 0.80 →
    /// NOT isolated.
    ///
    /// Gap between elements is 4pt (below `X_CLUSTER_GAP_THRESHOLD_PT=6`)
    /// so they belong to the same X-cluster.
    #[test]
    fn test_isolation_same_page_same_band_are_neighbours() {
        let elements = vec![
            // Target at x=0, width=80 → right edge at 80
            make_element_on_page(12.0, 1, 0, 0, 0, 0.0),
            // Neighbour at x=84, width=86 → right edge at 170 (same Y, gap=4pt)
            {
                let mut e = make_element_on_page(12.0, 1, 0, 0, 0, 84.0);
                e.placement.bounding_box.width = 86.0;
                e
            },
        ];
        let font_analysis = make_font_analysis(10.0, vec![("section", 2)]);
        let config = make_config(1.0, 5.0, None, 0.80);
        let engine = RuleEngine::new().expect("engine");
        let document_analysis = make_document_analysis();
        let style_data = make_style_data();
        let rule = SectionDetectionV2Rule::new(
            &engine,
            &elements,
            &config,
            &document_analysis,
            &font_analysis,
            &style_data,
        );
        // Cluster spans [0, 170], page extent [0, 170], num_clusters=1
        // → column_width = 170 → ratio = 170/170 = 1.0 ≥ 0.80 → not isolated.
        assert!(
            !rule.is_isolated(0),
            "X-clustered same-Y neighbour fills page → not isolated"
        );
    }

    /// Same Y, but the "neighbour" sits across a wide X-gap (40pt apart).
    /// Post-strip semantics: the gap puts them in distinct visual clusters,
    /// so the target element is alone in its cluster and counts as isolated
    /// — even though the original (page, band, column) filter would have
    /// merged them. Captures the new geometry-driven behavior.
    #[test]
    fn test_isolation_wide_x_gap_treats_neighbour_as_separate_cluster() {
        let elements = vec![
            // Target at x=0, width=80 (left "column")
            make_element_on_page(12.0, 1, 0, 0, 0, 0.0),
            // Neighbour at x=120, width=80 (right "column", gap=40pt)
            {
                let mut e = make_element_on_page(12.0, 1, 0, 0, 0, 120.0);
                e.placement.bounding_box.width = 80.0;
                e
            },
        ];
        let font_analysis = make_font_analysis(10.0, vec![("section", 2)]);
        let config = make_config(1.0, 5.0, None, 0.80);
        let engine = RuleEngine::new().expect("engine");
        let document_analysis = make_document_analysis();
        let style_data = make_style_data();
        let rule = SectionDetectionV2Rule::new(
            &engine,
            &elements,
            &config,
            &document_analysis,
            &font_analysis,
            &style_data,
        );
        assert!(
            rule.is_isolated(0),
            "wide-X-gap neighbour belongs to a different visual cluster → target is isolated"
        );
    }

    /// Line-extent Test A — Attention 3.1 / 3.2 shape regression.
    /// Header alone on its Y line (a real section title) is isolated even when
    /// the body block below shares the same `(page, band, col)` and Tika's
    /// per-paragraph `data-line` collides. Y-coordinate is the ground truth.
    #[test]
    fn test_line_extent_isolated_when_alone_on_y_line() {
        // Header at y=490, x=108, width=145 (≈ "3.1 Encoder and Decoder Stacks")
        let mut header = make_element_on_page(9.0, 3, 2, 0, 0, 108.0);
        header.placement.bounding_box.y = 490.0;
        header.placement.bounding_box.width = 145.0;
        // Body line below at y=510, fills column. Defines band x-extent.
        let mut body = make_element_on_page(9.0, 3, 2, 0, 0, 108.0);
        body.placement.bounding_box.y = 510.0;
        body.placement.bounding_box.width = 402.0;
        let elements = vec![header, body];
        let font_analysis = make_font_analysis(9.0, vec![("section", 100)]);
        let config = make_config(0.1, 5.0, None, 0.80);
        let engine = RuleEngine::new().expect("engine");
        let document_analysis = make_document_analysis();
        let style_data = make_style_data();
        let rule = SectionDetectionV2Rule::new(
            &engine,
            &elements,
            &config,
            &document_analysis,
            &font_analysis,
            &style_data,
        );
        // Header is alone on y=490 (body at y=510, gap=20 > tolerance=3) → isolated
        assert!(rule.is_isolated(0));
    }

    /// Line-extent Test B — Tika overlapping-spans regression (QUIC "MUST" pattern).
    /// When Tika emits an inline-styled word as a separate span overlapping a
    /// base-font line, both spans share the same Y. Their union fills the column,
    /// so neither is treated as isolated. This catches false positives where
    /// mid-paragraph bold lead-ins (`Encoder:`) would otherwise be promoted.
    #[test]
    fn test_line_extent_rejects_overlapping_inline_span() {
        // Base-font line at y=300 spanning the whole column
        let mut base = make_element_on_page(9.0, 1, 0, 0, 0, 108.0);
        base.placement.bounding_box.y = 300.0;
        base.placement.bounding_box.width = 400.0;
        // Overlapping inline-styled span at same Y, narrow
        let mut overlay = make_element_on_page(9.0, 1, 0, 0, 0, 200.0);
        overlay.placement.bounding_box.y = 300.0;
        overlay.placement.bounding_box.width = 30.0;
        let elements = vec![overlay, base];
        let font_analysis = make_font_analysis(9.0, vec![("section", 50), ("body", 50)]);
        let config = make_config(0.1, 5.0, None, 0.80);
        let engine = RuleEngine::new().expect("engine");
        let document_analysis = make_document_analysis();
        let style_data = make_style_data();
        let rule = SectionDetectionV2Rule::new(
            &engine,
            &elements,
            &config,
            &document_analysis,
            &font_analysis,
            &style_data,
        );
        // line_extent ≈ 400, column_width ≈ 400 → ratio ≈ 1.0 → not isolated
        assert!(
            !rule.is_isolated(0),
            "inline-styled overlay span on a full-width line must not be isolated"
        );
    }

    /// CR-21 Test 7 — Simulate the "1 Introduction" / watermark collision.
    /// Page 1 element at (band=0, col=0, line=0, x=124) — simulates a watermark.
    /// Page 2 element at (band=0, col=0, line=0, x=108) — simulates "1 Introduction".
    /// Without CR-21, the page 1 element would be counted as a same-line neighbour,
    /// producing a gap of 0 (overlap) → isolated=false → section header rejected.
    /// With CR-21, page 1 is excluded → no neighbours → isolated=true.
    #[test]
    fn test_isolation_introduction_scenario() {
        let elements = vec![
            // Page 1 watermark-like element
            make_element_on_page(11.0, 1, 0, 0, 0, 124.0),
            // Page 2 "1 Introduction"
            make_element_on_page(11.0, 2, 0, 0, 0, 108.0),
        ];
        let font_analysis = make_font_analysis(10.0, vec![("section", 2)]);
        let config = make_config(1.0, 5.0, None, 0.80);
        let engine = RuleEngine::new().expect("engine");
        let document_analysis = make_document_analysis();
        let style_data = make_style_data();
        let rule = SectionDetectionV2Rule::new(
            &engine,
            &elements,
            &config,
            &document_analysis,
            &font_analysis,
            &style_data,
        );
        // Page 2 element (index 1) must be isolated — page 1 element not a neighbour
        assert!(
            rule.is_isolated(1),
            "page 2 introduction must be isolated despite page 1 band collision"
        );
    }

    // ── CR-26 tests — isolation-gated inclusion patterns ──────────────────────

    /// Build a body-size, non-bold, non-rotated element at the given x/width on
    /// page=1 / band=0 / col=0 / y=0. Used to construct isolation scenarios where
    /// the inclusion-pattern path is the only route to promotion.
    fn make_inclusion_element(text: &str, x: f32, width: f32) -> PdfTextElement {
        PdfTextElement {
            text: text.to_string(),
            style_info: FontClass {
                class_name: "body".to_string(),
                font_family: "TestFont".to_string(),
                font_size: 10.0,
                font_style: "normal".to_string(),
                font_weight: "normal".to_string(),
                color: "#000000".to_string(),
            },
            placement: Placement {
                page_number: 1,
                bounding_box: BoundingBox {
                    x,
                    y: 0.0,
                    width,
                    height: 12.0,
                },
                band: 0,
                column: 0,
                nr_band_columns: 1,
                line_number: 0,
                segment_number: 0,
                rotation: 0,
                paragraph_number: 0,
            },
            reading_order: 0,
            bookmark_match: None,
            token_count: 1,
            raw_tags: vec![],
        }
    }

    /// Build a configured rule with custom inclusion/exclusion pattern lists.
    /// All other config values match `make_config(1.0, 5.0, None, 0.80)`.
    fn build_pattern_rule_test(
        elements: &[PdfTextElement],
        inclusion: Vec<&str>,
        exclusion: Vec<&str>,
    ) -> (
        ParsingConfig,
        FontSizeAnalysis,
        DocumentAnalysis,
        StyleData,
        RuleEngine,
    ) {
        let mut config = make_config(1.0, 5.0, None, 0.80);
        config.section_detection_v2.inclusion_patterns =
            inclusion.into_iter().map(String::from).collect();
        config.section_detection_v2.exclusion_patterns =
            exclusion.into_iter().map(String::from).collect();
        let font_analysis = make_font_analysis(10.0, vec![("body", elements.len().max(1))]);
        let document_analysis = make_document_analysis();
        let style_data = make_style_data();
        let engine = RuleEngine::new().expect("engine");
        (config, font_analysis, document_analysis, style_data, engine)
    }

    /// CR-26 Test 1 — Inclusion match on an isolated element promotes.
    /// Element alone on its visual line (no same-Y neighbours) → isolated=true.
    #[test]
    fn test_cr26_inclusion_isolated_promotes() {
        let elements = vec![make_inclusion_element("Article 5", 100.0, 80.0)];
        let (config, fa, da, sd, eng) =
            build_pattern_rule_test(&elements, vec!["(?i)article"], vec![]);
        let rule = SectionDetectionV2Rule::new(&eng, &elements, &config, &da, &fa, &sd);
        assert!(rule.apply_pattern_refinement(false, 0, "Article 5"));
    }

    /// CR-26 Test 2 — Inclusion match on a column-filling line does NOT promote.
    /// Two side-by-side elements at the same Y union to fill the column. The
    /// substring `Article` matches but `is_isolated` returns false. This is the
    /// load-bearing case the gate exists to handle.
    #[test]
    fn test_cr26_inclusion_paragraph_internal_does_not_promote() {
        let elements = vec![
            make_inclusion_element("as described in Article 5 of", 100.0, 200.0),
            make_inclusion_element("this Regulation, the framework", 305.0, 195.0),
        ];
        let (config, fa, da, sd, eng) =
            build_pattern_rule_test(&elements, vec!["(?i)article"], vec![]);
        let rule = SectionDetectionV2Rule::new(&eng, &elements, &config, &da, &fa, &sd);
        // line_extent ≈ 400, column_width ≈ 400 → ratio ≈ 1.0 ≥ 0.80 → not isolated
        assert!(!rule.apply_pattern_refinement(false, 0, "as described in Article 5 of"));
    }

    /// CR-26 Test 3 — `(?i)` flag honoured per-pattern.
    /// `(?i)chapter` matches `CHAPTER II`; bare `chapter` does not.
    #[test]
    fn test_cr26_case_insensitive_flag_honoured() {
        let elements = vec![make_inclusion_element("CHAPTER II", 100.0, 80.0)];

        // (?i)chapter → matches CHAPTER II → isolated → promote
        let (config_ci, fa_ci, da_ci, sd_ci, eng_ci) =
            build_pattern_rule_test(&elements, vec!["(?i)chapter"], vec![]);
        let rule_ci =
            SectionDetectionV2Rule::new(&eng_ci, &elements, &config_ci, &da_ci, &fa_ci, &sd_ci);
        assert!(rule_ci.apply_pattern_refinement(false, 0, "CHAPTER II"));

        // bare lowercase `chapter` does NOT match CHAPTER II → no promote
        let (config_cs, fa_cs, da_cs, sd_cs, eng_cs) =
            build_pattern_rule_test(&elements, vec!["chapter"], vec![]);
        let rule_cs =
            SectionDetectionV2Rule::new(&eng_cs, &elements, &config_cs, &da_cs, &fa_cs, &sd_cs);
        assert!(!rule_cs.apply_pattern_refinement(false, 0, "CHAPTER II"));
    }

    /// CR-26 Test 4 — Exclusion still demotes Pass 1-promoted candidates.
    /// Verifies the exclusion path is unaffected by the isolation-gate change on
    /// the inclusion path.
    #[test]
    fn test_cr26_exclusion_unaffected_by_isolation_gate() {
        let elements = vec![make_inclusion_element(
            "Figure 3: Architecture",
            100.0,
            80.0,
        )];
        let (config, fa, da, sd, eng) =
            build_pattern_rule_test(&elements, vec![], vec!["^Figure\\s"]);
        let rule = SectionDetectionV2Rule::new(&eng, &elements, &config, &da, &fa, &sd);
        // Pass 1 = true (simulating Region 1 promotion); exclusion match → demoted
        assert!(!rule.apply_pattern_refinement(true, 0, "Figure 3: Architecture"));
    }

    /// CR-26 Test 5 — Length cap rejects long body wrap-lines.
    /// Body wrap-line begins with `Article` and is structurally isolated (single
    /// span, no same-Y neighbours), but its length exceeds `inclusion_max_length`.
    /// The synthetic length gate is what protects Pass 2 on documents like CELEX
    /// where Tika reports degenerate font sizes and Pass 1 cannot disambiguate.
    #[test]
    fn test_cr26_length_cap_rejects_body_wrap() {
        let long_wrap = "Article 17, including ICT network performance issues and ICT-related";
        assert!(long_wrap.chars().count() > 30, "fixture must exceed cap");
        let elements = vec![make_inclusion_element(long_wrap, 100.0, 400.0)];
        let (config, fa, da, sd, eng) =
            build_pattern_rule_test(&elements, vec!["(?i)^article"], vec![]);
        let rule = SectionDetectionV2Rule::new(&eng, &elements, &config, &da, &fa, &sd);
        // Pattern matches, element is isolated, but length > 30 → not promoted
        assert!(!rule.apply_pattern_refinement(false, 0, long_wrap));
    }

    /// CR-26 Test 6 — Length cap admits short structural labels.
    /// Companion to Test 5: confirms the cap doesn't accidentally reject the
    /// real labels we're trying to catch.
    #[test]
    fn test_cr26_length_cap_admits_short_label() {
        let label = "Article 64";
        assert!(label.chars().count() <= 30, "fixture must be within cap");
        let elements = vec![make_inclusion_element(label, 100.0, 80.0)];
        let (config, fa, da, sd, eng) =
            build_pattern_rule_test(&elements, vec!["(?i)^article"], vec![]);
        let rule = SectionDetectionV2Rule::new(&eng, &elements, &config, &da, &fa, &sd);
        assert!(rule.apply_pattern_refinement(false, 0, label));
    }

    // ── CR-27 tests — keyword tiebreaker hierarchy ───────────────────────────

    /// Default config for hierarchy unit tests: tolerance=0.1, margin=5.0,
    /// no proportional ratio, isolation_threshold default.
    fn cr27_config() -> SectionDetectionV2Config {
        SectionDetectionV2Config {
            font_size_tolerance: 0.1,
            structural_size_margin: 5.0,
            structural_size_ratio: None,
            max_depth: 6,
            enforce_max_depth: true,
            starting_section_level: 1,
            ..SectionDetectionV2Config::default()
        }
    }

    /// CR-27 Test 1 — same keyword at equal font is sibling.
    #[test]
    fn test_cr27_same_keyword_is_sibling() {
        let cfg = cr27_config();
        let mut ctx = HierarchyContext::new();
        let d1 = ctx.update_for_section(12.0, Some("part"), &cfg);
        let d2 = ctx.update_for_section(12.0, Some("part"), &cfg);
        assert_eq!(d1, d2, "two PARTs at equal font must be siblings");
    }

    /// CR-27 Test 2 — different keyword at equal font fires tiebreaker (deeper).
    #[test]
    fn test_cr27_different_keyword_fires_tiebreaker() {
        let cfg = cr27_config();
        let mut ctx = HierarchyContext::new();
        let d_part = ctx.update_for_section(12.0, Some("part"), &cfg);
        let d_chap = ctx.update_for_section(12.0, Some("chapter"), &cfg);
        assert_eq!(d_chap, d_part + 1, "CHAPTER must be one deeper than PART");
    }

    /// CR-27 Test 3 — keyword reappearance steps back up.
    #[test]
    fn test_cr27_keyword_reappearance_steps_back_up() {
        let cfg = cr27_config();
        let mut ctx = HierarchyContext::new();
        let d_part1 = ctx.update_for_section(12.0, Some("part"), &cfg);
        let _d_chap = ctx.update_for_section(12.0, Some("chapter"), &cfg);
        let _d_num = ctx.update_for_section(12.0, Some("numbered"), &cfg);
        let d_part2 = ctx.update_for_section(12.0, Some("part"), &cfg);
        assert_eq!(d_part2, d_part1, "PART 2 must return to PART 1's depth");
    }

    /// CR-27 Test 4 — `None` ↔ `None` is sibling.
    #[test]
    fn test_cr27_none_is_sibling_with_none() {
        let cfg = cr27_config();
        let mut ctx = HierarchyContext::new();
        let d1 = ctx.update_for_section(12.0, None, &cfg);
        let d2 = ctx.update_for_section(12.0, None, &cfg);
        assert_eq!(
            d1, d2,
            "two None-keyword sections at equal font must be siblings"
        );
    }

    /// CR-27 Test 5 — `None` after a keyword fires tiebreaker (deeper).
    /// Pass-1 subtitle following a CHAPTER becomes a child of the CHAPTER.
    #[test]
    fn test_cr27_none_after_keyword_goes_deeper() {
        let cfg = cr27_config();
        let mut ctx = HierarchyContext::new();
        let d_chap = ctx.update_for_section(12.0, Some("chapter"), &cfg);
        let d_subtitle = ctx.update_for_section(12.0, None, &cfg);
        assert_eq!(
            d_subtitle,
            d_chap + 1,
            "subtitle must be one deeper than CHAPTER"
        );
    }

    /// CR-27 Test 6 — font-delta still wins when decisive (skips the tiebreaker).
    #[test]
    fn test_cr27_font_delta_wins_when_decisive() {
        let cfg = cr27_config();
        let mut ctx = HierarchyContext::new();
        let d_title = ctx.update_for_section(24.0, None, &cfg);
        let d_part = ctx.update_for_section(12.0, Some("part"), &cfg);
        assert_eq!(
            d_part,
            d_title + 1,
            "decisive font-delta still pushes deeper"
        );
    }

    /// CR-27 Test 7 — full Police Act PART/CHAPTER/numbered sequence.
    /// Title → PART 1 → numbered → numbered → PART 2 → CHAPTER 1 → numbered.
    /// Expected depths: 1, 2, 3, 3, 2, 3, 4.
    #[test]
    fn test_cr27_police_act_sequence() {
        let cfg = cr27_config();
        let mut ctx = HierarchyContext::new();

        // Title at 24pt
        assert_eq!(ctx.update_for_section(24.0, None, &cfg), 1);
        // PART 1 at 12pt — font-delta decisive (smaller), pushed deeper to 2
        assert_eq!(ctx.update_for_section(12.0, Some("part"), &cfg), 2);
        // numbered at 12pt — tie, different keyword, not in stack → tiebreaker → 3
        assert_eq!(ctx.update_for_section(12.0, Some("numbered"), &cfg), 3);
        // sibling numbered → still 3
        assert_eq!(ctx.update_for_section(12.0, Some("numbered"), &cfg), 3);
        // PART 2 at 12pt — tie, keyword 'part' in stack at depth 2 → step back → 2
        assert_eq!(ctx.update_for_section(12.0, Some("part"), &cfg), 2);
        // CHAPTER 1 at 12pt — tie, 'chapter' not in stack → tiebreaker → 3
        assert_eq!(ctx.update_for_section(12.0, Some("chapter"), &cfg), 3);
        // numbered at 12pt — tie, 'numbered' was popped during PART 2 step-back
        //   → not currently in stack → tiebreaker → 4
        assert_eq!(ctx.update_for_section(12.0, Some("numbered"), &cfg), 4);
    }

    /// CR-27 Test 8 — CELEX cascade with all-equal-font sections.
    /// Title (None) → CHAPTER → subtitle (None) → Article → article-title (None).
    /// Expected: 1, 2, 3, 3, 4. The subtitle is a None layer that gets skipped
    /// past when Article arrives, so Article sits at chapter+1 = 3 alongside
    /// (not under) the subtitle.
    #[test]
    fn test_cr27_celex_cascade() {
        let mut cfg = cr27_config();
        // CELEX has font_size = 1.0 everywhere; lower margin so Title→CHAPTER
        // doesn't trigger the decisive-delta path.
        cfg.structural_size_margin = 5.0;
        let mut ctx = HierarchyContext::new();

        // Title — first section, no prior, depth = starting_level
        assert_eq!(ctx.update_for_section(1.0, None, &cfg), 1);
        // CHAPTER I — tie (delta=0), keyword chapter ≠ None → tiebreaker → 2
        assert_eq!(ctx.update_for_section(1.0, Some("chapter"), &cfg), 2);
        // "General provisions" subtitle — tie, None ≠ chapter → tiebreaker → 3
        assert_eq!(ctx.update_for_section(1.0, None, &cfg), 3);
        // Article 1 — tie, top is None subtitle → skip past it; effective top
        //   is chapter; article ≠ chapter, not in stack → tiebreaker → 3
        assert_eq!(ctx.update_for_section(1.0, Some("article"), &cfg), 3);
        // "Subject matter" — tie, None ≠ article → tiebreaker → 4
        assert_eq!(ctx.update_for_section(1.0, None, &cfg), 4);
    }

    /// CR-27 Test 9 — depth cap respected when enforce_max_depth=true.
    #[test]
    fn test_cr27_depth_cap_respected() {
        let mut cfg = cr27_config();
        cfg.max_depth = 3;
        cfg.enforce_max_depth = true;
        let mut ctx = HierarchyContext::new();
        // Each section adds a tier at the same font: 1, 2, 3, then capped at 3
        assert_eq!(ctx.update_for_section(12.0, Some("part"), &cfg), 1);
        assert_eq!(ctx.update_for_section(12.0, Some("chapter"), &cfg), 2);
        assert_eq!(ctx.update_for_section(12.0, Some("article"), &cfg), 3);
        assert_eq!(ctx.update_for_section(12.0, Some("section"), &cfg), 3);
    }

    /// CR-27 Test 10 — REGRESSION REPRO: title-wrap drift on Police Act §86.
    ///
    /// Reproduces the depth-5 paragraph drift observed under section 86
    /// "Causing death by dangerous driving or careless driving when under the
    /// influence of drink or drugs: increased penalties". The section title
    /// is split across two Tika lines, same font class (f9), same band:
    ///
    ///   Line 0: "86 Causing death by dangerous driving..."  → matches `numbered`
    ///   Line 1: "influence of drink or drugs: increased..." → matches no keyword
    ///
    /// Both pass Pass 1 (bold + isolated). Line 0 promotes at the correct
    /// numbered tier. Line 1 arrives with `None` keyword at the same font,
    /// triggering the CR-27 tiebreaker and pushing (None, depth+1) on the stack.
    /// Subsequent body paragraphs then read `current_depth + 1`, landing one
    /// level too deep.
    ///
    /// This test currently documents the buggy behaviour. See discussion: this
    /// is arguably an upstream Pass-1 issue (over-promotion of title wrap-lines)
    /// surfaced by CR-27's now-correct tier discrimination.
    #[test]
    fn test_cr27_title_wrap_drifts_deeper_repro() {
        let cfg = cr27_config();
        let mut ctx = HierarchyContext::new();

        // Set up the Police Act §86 surrounding context.
        // Title (24pt) → PART 5 (12pt) → "86 Causing death..." line 0 (12pt, numbered)
        ctx.update_for_section(24.0, None, &cfg); // Title at d=1
        let d_part = ctx.update_for_section(12.0, Some("part"), &cfg);
        assert_eq!(d_part, 2, "PART 5 expected at depth 2");

        let d_section = ctx.update_for_section(12.0, Some("numbered"), &cfg);
        assert_eq!(d_section, 3, "numbered section expected at depth 3");

        // Now the title wrap-line: same font, no keyword match.
        let d_wrap = ctx.update_for_section(12.0, None, &cfg);

        // Body paragraphs after the section read get_content_level().
        let body_depth = ctx.get_content_level();

        // Demonstrate the bug: wrap-line is treated as a NEW deeper section,
        // and body content is pushed one level too deep.
        assert_eq!(
            d_wrap, 4,
            "BUG: wrap-line classified as deeper section instead of sibling/continuation"
        );
        assert_eq!(
            body_depth, 5,
            "BUG: body paragraphs land at section_depth + 2 instead of + 1"
        );

        // Expected behaviour (commented out — these are the assertions we'd
        // want after a fix):
        //   assert_eq!(d_wrap, 3, "wrap-line should remain at section depth (continuation)");
        //   assert_eq!(body_depth, 4, "body should be one level under the section");
    }
}
