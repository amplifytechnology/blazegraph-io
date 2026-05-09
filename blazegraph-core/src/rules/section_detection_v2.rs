/// Section Detection V2 — V3 algorithm (Block 09) + CR-41 bookmark substrate.
///
/// Three-tier piecewise classifier on `delta = font_size - body_size`:
///
/// - `delta > structural_size_margin` → ACCEPT (R1: clearly larger than body)
/// - `delta > tolerance`              → bold OR isolated_in_leaf OR bookmark_match (R2: medium)
/// - `|delta| ≤ tolerance`            → bold AND (isolated_in_leaf OR bookmark_match) (R3: at body)
/// - `delta < -tolerance`             → REJECT (below-body noise)
///
/// Pre-gates (rotation rejection + alpha-ratio gate) run before the size
/// classification. Pattern-refinement (`apply_pattern_refinement`) applies
/// exclusion regex demotion + inclusion regex promotion (Article N / CHAPTER N
/// / etc.) as a backup for the size-based pipeline, including for
/// degenerate-font documents where Tika reports body_size = 1pt (CELEX).
///
/// Isolation is leaf-based — `is_isolated_in_leaf` consults the
/// `Placement.region_label` set by `analytics::reading_order::tag_and_resort`
/// (Block 06b). An element is *isolated* when no same-Y neighbor inside its
/// Region tree leaf has a different bold-ness. This catches the canonical
/// false positive (bold emphasis word inside a non-bold body line) at
/// near-zero cost: the leaf substrate is precomputed once per document.
///
/// **Bookmark substrate (CR-41).** The XHTML parser populates
/// `PdfTextElement.bookmark_match` when a span's normalized text exactly
/// equals a PDF outline title. This is an author-declared structural
/// signal that substitutes for `isolated_in_leaf` in R2/R3 when geometry
/// can't see the structure (e.g., a bold body-size heading sharing its
/// leaf with the trailing paragraph). TOC entries naturally do not match
/// because Tika fragments them across multiple spans; only the monolithic
/// body heading hits. PDFs without an outline see no behavior change
/// (`bookmark_match` is `None` everywhere).
///
/// V3 deltas vs the prior pipeline (locked 2026-05-07):
/// - Replaced X-cluster `is_isolated` (per-element O(n) page walk) with
///   `is_isolated_in_leaf` (per-element O(leaf_size) walk). The X-cluster
///   logic was a stand-in that pre-dated `region_label`.
/// - R2 relaxed from `bold AND isolated` to `bold OR isolated` — leaf-based
///   isolation is reliable enough that either signal alone suffices on
///   medium-tier candidates. This catches Computer-Modern / italic /
///   non-bold section titles that V2's AND missed.
/// - R1 threshold raised from `body + 1.5pt` to `body + 4pt` — at smaller
///   deltas the size signal is weaker and the auxiliary-signal requirement
///   in R2 keeps body-emphasis-at-slightly-larger-font from auto-promoting.
/// - Rarity gate dropped — was already dead in production
///   (`font_rarity_threshold: 0.00` made `is_rare_font` always false). The
///   V3 algorithm doesn't need it; isolation+bold cover the "structurally
///   distinctive" notion with cleaner semantics.
use super::engine::{FontSizeAnalysis, ParseRule, RuleEngine};
use crate::analytics::DocumentAnalysis;
use crate::config::{ParsingConfig, SectionDetectionV2Config};
use crate::types::*;
use anyhow::Result;
use regex::Regex;

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
}

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

    /// CR-41: Whether the XHTML parser matched this span's normalized text
    /// exactly against a PDF outline (bookmark) title. Acts as an alternative
    /// to the geometric `isolated_in_leaf` signal in R2/R3 — when the PDF
    /// author has explicitly named this span as a section target, the
    /// structural-atom signal can come from the outline rather than from
    /// the leaf's Y-line count. PDFs without a `BookmarkData` payload yield
    /// `false` for every element by construction.
    fn has_bookmark_match(element: &PdfTextElement) -> bool {
        element.bookmark_match.is_some()
    }

    /// Leaf-based isolation. Two gates compose the predicate:
    ///
    /// 1. **Same-line bold-mismatch** — any same-Y same-(page, leaf)
    ///    neighbor with different bold-ness disqualifies the candidate.
    ///    Catches bold-in-paragraph and symmetric anchor-with-continuation
    ///    cases. Already applied as a hard reject at the top of
    ///    `classify_pass1`; redundant here for defense-in-depth and for
    ///    direct callers (pattern-refinement, tests).
    ///
    /// 2. **Multi-line-leaf rejection** — if the (page, leaf) contains
    ///    ≥ 2 distinct Y-lines (regardless of fontsize), the leaf is a
    ///    flowing multi-line content block (body paragraph, abstract,
    ///    table caption, references entry) and the candidate is part of
    ///    the flow. Genuinely multi-line section titles still classify
    ///    via R1 (large-delta auto-promote) or R2's bold disjunct.
    ///
    /// The single-Y-line constraint is the structural-atom signature: a
    /// leaf produced by XY-cut that contains exactly one visual line is
    /// a header / caption / standalone label. Inline cross-references
    /// like "Section 6 of [QUIC-RECOVERY]" sit in multi-line body
    /// leaves and are correctly rejected — the inclusion-pattern path
    /// (which only ever runs on body-tier candidates) shares this gate.
    ///
    /// The leaf substrate (`Placement.region_label`) is set by
    /// `analytics::reading_order::tag_and_resort` (Block 06b). Elements
    /// with `region_label = None` don't reach this code in production —
    /// they are pre-classified as `Margin` at the `PdfTextElement →
    /// ParsedPdfElement` boundary (Block 07) and the `classify` early-
    /// return guard skips them. Defensive return: `false` for missing labels.
    fn is_isolated_in_leaf(&self, element_idx: usize) -> bool {
        let element = &self.text_elements[element_idx];
        let Some(label) = element.placement.region_label.as_deref() else {
            return false;
        };

        let elem_page = element.page_number();
        let y = element.bounding_box().y;
        let tol = self.config.section_detection_v2.line_height_tolerance;
        let elem_bold = Self::is_bold(element);

        // Distinct Y-lines anywhere in the (page, leaf). Multi-line means
        // ≥ 2 → reject (the candidate is in flowing content, not a
        // structural one-line atom).
        let mut distinct_y_lines: Vec<f32> = Vec::new();

        for (idx, n) in self.text_elements.iter().enumerate() {
            // Region labels reset per page (XY-cut runs per page); filter
            // by `(page_number, region_label)` to avoid cross-page leaf
            // contamination — leaf "4-1" on page 3 is structurally
            // distinct from leaf "4-1" on page 5.
            if n.page_number() != elem_page || n.placement.region_label.as_deref() != Some(label) {
                continue;
            }

            let ny = n.bounding_box().y;

            // Same-Y same-leaf neighbor with different bold-ness → bold-in-
            // paragraph (bold word inside body, or non-bold continuation
            // next to a bold anchor). Already filtered as a hard reject in
            // `classify_pass1::has_same_y_bold_mismatch`; redundant here as
            // defense-in-depth so the predicate also holds when called
            // directly (pattern-refinement path, tests).
            if idx != element_idx && (ny - y).abs() < tol && Self::is_bold(n) != elem_bold {
                return false;
            }

            // Multi-line-leaf bookkeeping. Both strict and lax modes count
            // distinct Y-lines anywhere in the (page, leaf): a leaf with
            // ≥ 2 distinct Y-lines is a flowing content block. The
            // pattern-inclusion path (lax) needs this gate too — without
            // it, inline cross-references like "Section 6 of [...]"
            // matching `(?i)^section\s+\d+` get admitted simply because
            // their body neighbors don't have a different bold-ness, which
            // then mis-promotes the line via post-pass merge.
            if !distinct_y_lines.iter().any(|&yy| (yy - ny).abs() < tol) {
                distinct_y_lines.push(ny);
                if distinct_y_lines.len() >= 2 {
                    return false;
                }
            }
        }

        true
    }

    /// Hard-reject gate for bold-in-paragraph cases. Returns `true` when any
    /// same-Y same-(page, leaf) neighbor of the candidate has a different
    /// bold-ness. Catches:
    ///
    /// - **Bold word inside non-bold body line** (canonical false positive
    ///   in `**Then** under the section…` shape).
    /// - **Non-bold body next to a bold anchor** (the symmetric case where
    ///   the body fragment is the non-bold neighbor of a bold "Article 5"
    ///   structural anchor).
    ///
    /// Lifted out of `is_isolated_in_leaf` so it acts as a hard reject in
    /// `classify_pass1` *before* the size/bold/isolation tier logic. Without
    /// this lift, R2's `bold OR isolated` admits bold lead-ins like
    /// "Encoder:" via the bold disjunct even when isolation correctly
    /// returns false.
    fn has_same_y_bold_mismatch(&self, element_idx: usize) -> bool {
        let element = &self.text_elements[element_idx];
        let Some(label) = element.placement.region_label.as_deref() else {
            return false;
        };

        let elem_page = element.page_number();
        let y = element.bounding_box().y;
        let tol = self.config.section_detection_v2.line_height_tolerance;
        let elem_bold = Self::is_bold(element);

        for (idx, n) in self.text_elements.iter().enumerate() {
            if idx == element_idx
                || n.page_number() != elem_page
                || n.placement.region_label.as_deref() != Some(label)
            {
                continue;
            }
            if (n.bounding_box().y - y).abs() < tol && Self::is_bold(n) != elem_bold {
                return true;
            }
        }
        false
    }

    // ──────────────────────────────────────────────────────────────────────
    // Pass 1: Candidate marking + decision tree
    // ──────────────────────────────────────────────────────────────────────

    /// Core classification: returns `true` if the element should be a section after Pass 1.
    ///
    /// Three-tier piecewise decision on `delta = font_size - body_size`:
    ///
    /// - `delta < -tolerance`            → REJECT (below-body noise)
    /// - `delta > structural_size_margin` → ACCEPT (R1: clearly larger than body)
    /// - `delta > tolerance`             → R2 (medium): bold OR isolated_in_leaf OR bookmark_match
    /// - `|delta| ≤ tolerance`           → R3 (at body): bold AND (isolated_in_leaf OR bookmark_match)
    ///
    /// R1 threshold is `body_size + structural_size_margin` by default
    /// (default 4pt — clearly above body); a `body_size * structural_size_ratio`
    /// override is honored when set, useful for documents with proportional
    /// type scales.
    ///
    /// CR-41: `bookmark_match` substitutes for `isolated_in_leaf` in R2/R3
    /// when the PDF outline names this span as a section target. Required
    /// at body-size R3 only as an alternative to isolation; bold is still
    /// required. PDFs without a bookmark outline see no behavior change.
    ///
    /// Rotated elements are always rejected upfront (matches the Block 02
    /// statistical filter).
    fn classify_pass1(&self, element_idx: usize) -> bool {
        let element = &self.text_elements[element_idx];

        if element.rotation() != 0 {
            return false;
        }

        // Hard reject: bold-in-paragraph case. A bold candidate sharing its
        // Y-line in its (page, leaf) with a non-bold neighbor (or vice
        // versa) is body emphasis, not a structural element. Lifted above
        // the size/bold/isolation logic so it overrides R2's `bold OR
        // isolated` disjunct.
        if self.has_same_y_bold_mismatch(element_idx) {
            return false;
        }

        let cfg = &self.config.section_detection_v2;
        let body_size = self.font_size_analysis.body_text_size;
        let font_size = element.style_info.font_size;
        let tolerance = cfg.font_size_tolerance;
        let delta = font_size - body_size;

        if delta < -tolerance {
            return false;
        }

        if !self.passes_alpha_ratio(&element.text) {
            return false;
        }

        let region1_threshold = match cfg.structural_size_ratio {
            Some(ratio) => body_size * ratio,
            None => body_size + cfg.structural_size_margin,
        };

        if font_size > region1_threshold {
            return true;
        }

        let bold = Self::is_bold(element);
        let isolated = self.is_isolated_in_leaf(element_idx);
        let bookmark_promoted = Self::has_bookmark_match(element);

        if delta > tolerance {
            // R2 (medium): bold, isolation, OR bookmark match (CR-41).
            bold || isolated || bookmark_promoted
        } else {
            // R3 (at-body band): bold required; isolation OR bookmark
            // match (CR-41) supplies the structural-atom signal.
            bold && (isolated || bookmark_promoted)
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
                    if re.is_match(text) && self.is_isolated_in_leaf(element_idx) {
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
        // Header / Footer / Margin elements are pre-classified at the
        // PdfTextElement → ParsedPdfElement boundary from
        // `placement.region_label` (Block 07). Skip section detection on them
        // so running headers, footers, and out-of-region marginalia never get
        // promoted to Section.
        if matches!(
            current_element.element_type,
            ParsedElementType::Header | ParsedElementType::Footer | ParsedElementType::Margin
        ) {
            let content_level = hierarchy_context.get_content_level();
            return (current_element.element_type.clone(), content_level);
        }

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

    /// Promote same-Y same-(page, leaf) sibling fragments to Section when
    /// any one of them already classified as Section, **but only in
    /// single-Y-line leaves**. Catches Tika fragmentation of structural
    /// header lines (canonical example: "3.1" + "Encoder and Decoder
    /// Stacks" — the number-only prefix fails `passes_alpha_ratio`, the
    /// title passes, and the post-pass merges them) without mistakenly
    /// promoting body fragments adjacent to inline cross-references like
    /// "Section 6 of [QUIC-RECOVERY]".
    ///
    /// The single-Y-line gate is the structural-atom signature: a leaf
    /// produced by XY-cut that contains exactly one visual line is a
    /// header / caption / standalone label. Multi-line leaves are flowing
    /// content — even if some fragment in them classifies as Section
    /// (e.g. an inline "Section N" cross-reference matched by the
    /// inclusion pattern), the surrounding fragments are body content and
    /// must not be promoted.
    fn promote_same_line_section_fragments(out: &mut [ParsedPdfElement]) {
        use std::collections::HashMap;

        const Y_TOL: f32 = 3.0;

        // First: collect per-(page, leaf) the distinct Y-lines and the
        // Section-bearing Y-line (if any) plus its hierarchy level.
        struct LeafInfo {
            y_lines: Vec<f32>,
            section_y: Option<(f32, u32)>,
        }
        let mut leaves: HashMap<(u32, String), LeafInfo> = HashMap::new();

        for el in out.iter() {
            let Some(p) = el.placement.as_ref() else {
                continue;
            };
            let Some(label) = p.region_label.as_deref() else {
                continue;
            };
            let key = (p.page_number, label.to_string());
            let info = leaves.entry(key).or_insert(LeafInfo {
                y_lines: Vec::new(),
                section_y: None,
            });
            let y = p.bounding_box.y;
            if !info.y_lines.iter().any(|&yy| (yy - y).abs() < Y_TOL) {
                info.y_lines.push(y);
            }
            if el.element_type == ParsedElementType::Section && info.section_y.is_none() {
                info.section_y = Some((y, el.hierarchy_level));
            }
        }

        // Second pass: promote within single-Y-line leaves that contain a
        // Section. The single-Y constraint is the structural-atom check.
        for el in out.iter_mut() {
            if el.element_type == ParsedElementType::Section {
                continue;
            }
            let Some(p) = el.placement.as_ref() else {
                continue;
            };
            let Some(label) = p.region_label.as_deref() else {
                continue;
            };
            let key = (p.page_number, label.to_string());
            let Some(info) = leaves.get(&key) else {
                continue;
            };
            if info.y_lines.len() != 1 {
                continue;
            }
            let Some((sy, level)) = info.section_y else {
                continue;
            };
            let y = p.bounding_box.y;
            if (sy - y).abs() < Y_TOL {
                el.element_type = ParsedElementType::Section;
                el.hierarchy_level = level;
            }
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
                    element_type: ParsedElementType::from_region_label(
                        te.placement.region_label.as_deref(),
                    ),
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

        // Same-Y same-(page, leaf) section promotion. Tika fragments a
        // single visual line into multiple PdfTextElements (e.g., the
        // section heading "3.1 Encoder and Decoder Stacks" emits two
        // fragments at the same Y: "3.1" and "Encoder and Decoder Stacks").
        // Per-element gates can reject one fragment for contingent reasons
        // (the number prefix "3.1" fails `passes_alpha_ratio`) while
        // accepting its sibling. The fragments share a structural identity:
        // if one is a Section, all of them are. Promote them together.
        Self::promote_same_line_section_fragments(&mut out);

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
    use crate::analytics::DocumentAnalysis;
    use crate::config::{ParsingConfig, SectionDetectionV2Config};
    use crate::rules::engine::FontSizeAnalysis;
    use crate::types::{BoundingBox, FontClass, PdfTextElement, Placement, StyleData};
    use std::collections::{BTreeMap, HashMap};

    // ── Helpers ──────────────────────────────────────────────────────────────

    fn make_placement(line_number: u32, rotation: i32) -> Placement {
        Placement {
            page_number: 1,
            bounding_box: BoundingBox {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 12.0,
            },
            line_number,
            segment_number: 0,
            rotation,
            paragraph_number: 0,
            // Default fixture leaf — body content. Tests that exercise
            // bold-in-paragraph or multi-leaf isolation override this.
            region_label: Some("1".to_string()),
            page_width: 0.0,
            page_height: 0.0,
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
            placement: make_placement(line_number, 0),
            reading_order: 0,
            bookmark_match: None,
            token_count: 1,
            raw_tags: vec![],
        }
    }

    /// Non-bold neighbour element sharing the same Y, same leaf as index 0.
    /// V3 isolation reads "different bold-ness on the same Y in the same leaf
    /// → not isolated", so a non-bold neighbour next to a bold candidate is
    /// the canonical bold-in-paragraph reject case.
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
                    x: 110.0,
                    y: 0.0,
                    width: 50.0,
                    height: 12.0,
                },
                line_number,
                segment_number: 1,
                rotation: 0,
                paragraph_number: 0,
                region_label: Some("1".to_string()),
                page_width: 0.0,
                page_height: 0.0,
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

    fn make_config(tolerance: f32, margin: f32, ratio: Option<f32>) -> ParsingConfig {
        ParsingConfig {
            section_detection_v2: SectionDetectionV2Config {
                font_size_tolerance: tolerance,
                structural_size_margin: margin,
                structural_size_ratio: ratio,
                line_height_tolerance: 3.0,
                ..SectionDetectionV2Config::default()
            },
            ..ParsingConfig::default()
        }
    }

    fn make_style_data() -> StyleData {
        StyleData {
            font_classes: BTreeMap::new(),
        }
    }

    fn make_document_analysis() -> DocumentAnalysis {
        // Rules do not read DocumentAnalysis fields in tests (the parameter is
        // threaded through but unused by V2 — see `_document_analysis` on the
        // rule struct). Default is sufficient.
        DocumentAnalysis::default()
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
        let config = make_config(1.0, 5.0, None);
        assert!(classify(&elements, &font_analysis, &config));
    }

    /// Test 2 — R1 boundary: delta == margin is R2 (not R1).
    /// delta = 15 - 10 = 5 = margin → `font_size > body + margin` is `15 > 15` = false → R2.
    /// To make R2 reject we need `bold OR isolated_in_leaf` to be false. Use a
    /// non-bold candidate whose same-Y same-leaf neighbour IS bold → leaf
    /// isolation flips to false (different bold-ness on the same line). Both
    /// signals false → R2 rejects.
    #[test]
    fn test_region1_boundary_is_region2() {
        let body = 10.0;
        let mut neighbour = make_neighbour(5);
        neighbour.style_info.font_weight = "bold".to_string();
        let elements = vec![make_element(15.0, false, "body", 5), neighbour];
        let font_analysis = make_font_analysis(body, vec![("body", 100)]);
        let config = make_config(1.0, 5.0, None);
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
        let config = make_config(1.0, 5.0, None);
        assert!(classify(&elements, &font_analysis, &config));
    }

    /// Test 4 — R2: isolated alone promotes (V3 OR semantics).
    /// delta = 13 - 10 = 3 → R2; non-bold, alone in leaf → isolated → promoted.
    /// Captures the V3 relaxation (`bold OR isolated`) that lets non-bold but
    /// structurally-isolated medium-tier headers (academic italic / Computer-
    /// Modern section titles) classify cleanly.
    #[test]
    fn test_region2_isolated_alone_promotes() {
        let body = 10.0;
        let elements = vec![make_element(13.0, false, "body", 0)];
        let font_analysis = make_font_analysis(body, vec![("body", 100)]);
        let config = make_config(1.0, 5.0, None);
        assert!(classify(&elements, &font_analysis, &config));
    }

    /// Test 5 — R2: neither bold nor isolated rejects.
    /// delta = 3 → R2; non-bold candidate with a bold same-Y same-leaf
    /// neighbour → leaf isolation flips to false → both signals false → reject.
    /// Captures the bold-anchor-with-non-bold-body case (e.g. bold "Article 5"
    /// with non-bold continuation "of this Regulation").
    #[test]
    fn test_region2_neither_signal_rejects() {
        let body = 10.0;
        let mut neighbour = make_neighbour(7);
        neighbour.style_info.font_weight = "bold".to_string();
        let elements = vec![make_element(13.0, false, "body", 7), neighbour];
        let font_analysis = make_font_analysis(body, vec![("body", 100)]);
        let config = make_config(1.0, 5.0, None);
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
        let config = make_config(1.0, 5.0, None);
        assert!(classify(&elements, &font_analysis, &config));
    }

    // (V2 Test 7 — `isolated AND rare` — removed in V3: rarity gate dropped.)

    /// Test 8 — R3: isolated alone does NOT promote.
    /// |delta| ≤ tolerance, isolated, not bold, common font → NOT promoted
    #[test]
    fn test_region3_isolated_alone_does_not_promote() {
        let body = 10.0;
        // "body" = 90/100 = 90% → not rare
        let elements = vec![make_element(10.5, false, "body", 0)];
        let font_analysis = make_font_analysis(body, vec![("body", 90), ("other", 10)]);
        let config = make_config(1.0, 5.0, None);
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
        let config = make_config(1.0, 5.0, None);
        assert!(!classify(&elements, &font_analysis, &config));
    }

    /// Test 10 — Reject: below body − tolerance.
    /// delta = 8 - 10 = -2 < -tolerance (-1) → REJECT regardless of signals
    #[test]
    fn test_reject_below_body_minus_tolerance() {
        let body = 10.0;
        let elements = vec![make_element(8.0, true, "body", 0)];
        let font_analysis = make_font_analysis(body, vec![("body", 100)]);
        let config = make_config(1.0, 5.0, None);
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
        let config = make_config(1.0, 0.1, Some(1.5));

        // 12pt: delta=2 > tolerance 1 → Region 2, isolated, bold → promoted (both signals)
        let elements_12 = vec![make_element(12.0, true, "body", 0)];
        assert!(classify(&elements_12, &font_analysis, &config));

        // 16pt: font_size 16 > threshold 15 → Region 1 → promoted
        let elements_16 = vec![make_element(16.0, false, "body", 0)];
        assert!(classify(&elements_16, &font_analysis, &config));
    }

    /// Test 12 — arXiv watermark regression (V3 path).
    /// In production, arxiv-style sidebar / watermark content is rotated and
    /// excluded from `body_element_indices` by `tag_and_resort` → its
    /// `region_label` stays `None` → element_type becomes `Margin` at the
    /// conversion boundary → the Block 07 classify guard skips section
    /// detection entirely. As a defense, `classify_pass1` itself rejects
    /// rotated elements upfront. This test pins that defense.
    #[test]
    fn test_arxiv_watermark_regression() {
        let body = 7.0;
        let mut element = make_element(11.0, true, "body", 0);
        element.placement.rotation = 90;
        let elements = vec![element];
        let font_analysis = make_font_analysis(body, vec![("body", 100)]);
        let config = make_config(1.0, 5.0, None);
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
            placement: make_placement(line_number, 0),
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

    // ── V3 leaf-based isolation tests ─────────────────────────────────────────

    /// Build a leaf-tagged element at the given (x, y, leaf, bold). Width
    /// fixed at 80pt for predictable Y-line geometry across the leaf-isolation
    /// fixtures.
    fn make_leaf_element(x: f32, y: f32, leaf: &str, bold: bool) -> PdfTextElement {
        let weight = if bold { "bold" } else { "normal" };
        PdfTextElement {
            text: "fragment".to_string(),
            style_info: FontClass {
                class_name: "body".to_string(),
                font_family: "TestFont".to_string(),
                font_size: 10.0,
                font_style: "normal".to_string(),
                font_weight: weight.to_string(),
                color: "#000000".to_string(),
            },
            placement: Placement {
                page_number: 1,
                bounding_box: BoundingBox {
                    x,
                    y,
                    width: 80.0,
                    height: 12.0,
                },
                line_number: 0,
                segment_number: 0,
                rotation: 0,
                paragraph_number: 0,
                region_label: Some(leaf.to_string()),
                page_width: 0.0,
                page_height: 0.0,
            },
            reading_order: 0,
            bookmark_match: None,
            token_count: 1,
            raw_tags: vec![],
        }
    }

    fn make_leaf_rule<'a>(
        engine: &'a RuleEngine,
        elements: &'a [PdfTextElement],
        config: &'a ParsingConfig,
        document_analysis: &'a DocumentAnalysis,
        font_analysis: &'a FontSizeAnalysis,
        style_data: &'a StyleData,
    ) -> SectionDetectionV2Rule<'a> {
        SectionDetectionV2Rule::new(
            engine,
            elements,
            config,
            document_analysis,
            font_analysis,
            style_data,
        )
    }

    /// V3 Test 1 — single element in a leaf has no same-Y neighbors → isolated.
    #[test]
    fn block09_leaf_singleton_is_isolated() {
        let elements = vec![make_leaf_element(0.0, 0.0, "1", true)];
        let fa = make_font_analysis(10.0, vec![("body", 1)]);
        let cfg = make_config(0.1, 4.0, None);
        let da = make_document_analysis();
        let sd = make_style_data();
        let eng = RuleEngine::new().expect("engine");
        let rule = make_leaf_rule(&eng, &elements, &cfg, &da, &fa, &sd);
        assert!(rule.is_isolated_in_leaf(0));
    }

    /// V3 Test 2 — bold candidate + non-bold same-Y same-leaf neighbor →
    /// not isolated. Canonical bold-emphasis-in-paragraph case.
    #[test]
    fn block09_leaf_bold_with_nonbold_same_line_not_isolated() {
        let elements = vec![
            make_leaf_element(0.0, 0.0, "1", true),   // bold candidate
            make_leaf_element(90.0, 0.0, "1", false), // non-bold body neighbor
        ];
        let fa = make_font_analysis(10.0, vec![("body", 2)]);
        let cfg = make_config(0.1, 4.0, None);
        let da = make_document_analysis();
        let sd = make_style_data();
        let eng = RuleEngine::new().expect("engine");
        let rule = make_leaf_rule(&eng, &elements, &cfg, &da, &fa, &sd);
        assert!(!rule.is_isolated_in_leaf(0));
    }

    /// V3 Test 3 — multi-line leaf rejects isolation regardless of
    /// fontsize. The (page, leaf) substrate identifies a flowing content
    /// block; the candidate is part of the flow. Real bold headers in
    /// big multi-line leaves still classify via R2's bold disjunct or R1
    /// auto-promotion. Inline cross-references (e.g. `Section 6 of [...]`)
    /// in body leaves get correctly rejected too — same gate.
    #[test]
    fn block09_leaf_multi_line_rejects_isolation() {
        let elements = vec![
            make_leaf_element(0.0, 0.0, "1", false),
            make_leaf_element(0.0, 50.0, "1", false),
        ];
        let fa = make_font_analysis(10.0, vec![("body", 2)]);
        let cfg = make_config(0.1, 4.0, None);
        let da = make_document_analysis();
        let sd = make_style_data();
        let eng = RuleEngine::new().expect("engine");
        let rule = make_leaf_rule(&eng, &elements, &cfg, &da, &fa, &sd);
        assert!(!rule.is_isolated_in_leaf(0));
    }

    /// V3 Test 3b — multi-line leaf with mixed fontsizes rejects
    /// isolation. Body content is the canonical case: a math-prose mix
    /// where a fragment at +1pt above body is alone at its size on its
    /// line, but the leaf has body content at body size on other lines.
    #[test]
    fn block09_leaf_multi_line_mixed_size_rejects() {
        let mut larger = make_leaf_element(0.0, 0.0, "1", false);
        larger.style_info.font_size = 11.0;
        let body = make_leaf_element(0.0, 50.0, "1", false);
        let elements = vec![larger, body];
        let fa = make_font_analysis(10.0, vec![("body", 2)]);
        let cfg = make_config(0.1, 4.0, None);
        let da = make_document_analysis();
        let sd = make_style_data();
        let eng = RuleEngine::new().expect("engine");
        let rule = make_leaf_rule(&eng, &elements, &cfg, &da, &fa, &sd);
        assert!(!rule.is_isolated_in_leaf(0));
    }

    /// V3 Test 4 — multi-fragment all-bold same-Y same-leaf → isolated.
    /// A bold title that Tika emits as multiple spans on the same line is
    /// still one structural unit.
    #[test]
    fn block09_leaf_all_bold_multi_fragment_is_isolated() {
        let elements = vec![
            make_leaf_element(0.0, 0.0, "1", true),
            make_leaf_element(90.0, 0.0, "1", true),
            make_leaf_element(180.0, 0.0, "1", true),
        ];
        let fa = make_font_analysis(10.0, vec![("body", 3)]);
        let cfg = make_config(0.1, 4.0, None);
        let da = make_document_analysis();
        let sd = make_style_data();
        let eng = RuleEngine::new().expect("engine");
        let rule = make_leaf_rule(&eng, &elements, &cfg, &da, &fa, &sd);
        assert!(rule.is_isolated_in_leaf(0));
    }

    /// V3 Test 5 — different leaves on the same Y → isolated. The XY-cut
    /// produced two separate leaves at this Y (e.g., left and right column
    /// of a multi-column page). Cross-leaf neighbors don't disqualify.
    #[test]
    fn block09_leaf_cross_leaf_same_y_does_not_disqualify() {
        let elements = vec![
            make_leaf_element(0.0, 0.0, "1", true),    // leaf 1
            make_leaf_element(300.0, 0.0, "2", false), // leaf 2, same Y
        ];
        let fa = make_font_analysis(10.0, vec![("body", 2)]);
        let cfg = make_config(0.1, 4.0, None);
        let da = make_document_analysis();
        let sd = make_style_data();
        let eng = RuleEngine::new().expect("engine");
        let rule = make_leaf_rule(&eng, &elements, &cfg, &da, &fa, &sd);
        assert!(rule.is_isolated_in_leaf(0));
    }

    /// V3 Test 6 — element with no `region_label` (orphan) → not isolated.
    /// In production, Margin elements are skipped at `classify` entry by
    /// the Block 07 guard, so this code path doesn't fire there. Defensive
    /// behavior: orphans never claim isolation.
    #[test]
    fn block09_leaf_missing_label_returns_false() {
        let mut e = make_leaf_element(0.0, 0.0, "1", true);
        e.placement.region_label = None;
        let elements = vec![e];
        let fa = make_font_analysis(10.0, vec![("body", 1)]);
        let cfg = make_config(0.1, 4.0, None);
        let da = make_document_analysis();
        let sd = make_style_data();
        let eng = RuleEngine::new().expect("engine");
        let rule = make_leaf_rule(&eng, &elements, &cfg, &da, &fa, &sd);
        assert!(!rule.is_isolated_in_leaf(0));
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
                line_number: 0,
                segment_number: 0,
                rotation: 0,
                paragraph_number: 0,
                // Body leaf — the inclusion-pattern fixtures probe pattern + isolation
                // semantics; co-leaf neighbours of differing bold-ness are how
                // `is_isolated_in_leaf` distinguishes structural labels from inline.
                region_label: Some("1".to_string()),
                page_width: 0.0,
                page_height: 0.0,
            },
            reading_order: 0,
            bookmark_match: None,
            token_count: 1,
            raw_tags: vec![],
        }
    }

    /// Build a configured rule with custom inclusion/exclusion pattern lists.
    /// All other config values match `make_config(1.0, 5.0, None)`.
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
        let mut config = make_config(1.0, 5.0, None);
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

    /// CR-26 Test 2 — Inclusion match next to a different-bold-ness same-Y
    /// same-leaf neighbour does NOT promote. Under V3 leaf isolation, the
    /// presence of any same-line same-leaf neighbour with mismatched bold-ness
    /// flips `is_isolated_in_leaf` to false — which gates the inclusion-pattern
    /// path. Captures the bold-anchor-with-non-bold-body case (e.g., bold
    /// "Article 5" anchor + non-bold continuation "of this Regulation").
    #[test]
    fn test_cr26_inclusion_paragraph_internal_does_not_promote() {
        let mut anchor = make_inclusion_element("Article 5", 100.0, 80.0);
        anchor.style_info.font_weight = "bold".to_string();
        let continuation =
            make_inclusion_element("of this Regulation, the framework", 185.0, 195.0);
        let elements = vec![anchor, continuation];
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

    // ── Block 07 — Header / Footer / Margin classification skip ───────────────

    /// Build a `PdfTextElement` whose font size and isolation would otherwise
    /// promote it to Section under the Region 1 path (`test_region1_auto_promotes`
    /// is the canonical positive case). Caller chooses the `region_label` to
    /// drive the Block 07 boundary classifier.
    fn make_section_sized_element(region_label: Option<&str>) -> PdfTextElement {
        let mut el = make_element(16.0, false, "body", 0);
        el.placement.region_label = region_label.map(str::to_string);
        el
    }

    /// Apply `SectionDetectionV2Rule` over a freshly-bootstrapped element list
    /// (the bootstrap path picks `element_type` from `region_label`) and return
    /// the resulting element types in order.
    fn run_apply_bootstrap(text_elements: &[PdfTextElement]) -> Vec<ParsedElementType> {
        let body = 10.0;
        let font_analysis = make_font_analysis(body, vec![("body", 100)]);
        let config = make_config(1.0, 5.0, None);
        let document_analysis = make_document_analysis();
        let style_data = make_style_data();
        let engine = RuleEngine::new().expect("engine");
        let rule = SectionDetectionV2Rule::new(
            &engine,
            text_elements,
            &config,
            &document_analysis,
            &font_analysis,
            &style_data,
        );
        rule.apply(Vec::new())
            .expect("apply succeeds")
            .into_iter()
            .map(|e| e.element_type)
            .collect()
    }

    /// Block 07 — `H-*` region label is preserved through section detection.
    /// The element has section-promoting geometry (16pt vs 10pt body, isolated)
    /// so without the skip guard it would land as `Section`.
    #[test]
    fn block07_header_label_skips_section_promotion() {
        let text_elements = vec![make_section_sized_element(Some("H-1"))];
        let types = run_apply_bootstrap(&text_elements);
        assert_eq!(types, vec![ParsedElementType::Header]);
    }

    /// Block 07 — `F-*` region label is preserved through section detection.
    #[test]
    fn block07_footer_label_skips_section_promotion() {
        let text_elements = vec![make_section_sized_element(Some("F-2"))];
        let types = run_apply_bootstrap(&text_elements);
        assert_eq!(types, vec![ParsedElementType::Footer]);
    }

    /// Block 07 — orphan element (region_label = None) classifies as Margin and
    /// is skipped by section detection. Sidebar marginalia / rotated content /
    /// elements outside any Region tree leaf land here.
    #[test]
    fn block07_orphan_label_skips_section_promotion() {
        let text_elements = vec![make_section_sized_element(None)];
        let types = run_apply_bootstrap(&text_elements);
        assert_eq!(types, vec![ParsedElementType::Margin]);
    }

    /// Block 07 — body leaf labels (`"1"`, `"2-1"`, …) follow the normal
    /// section-detection path. This is the regression check for the guard:
    /// the `H-` / `F-` prefix gate must not catch body labels.
    #[test]
    fn block07_body_leaf_label_still_promotes_to_section() {
        let text_elements = vec![make_section_sized_element(Some("2-1"))];
        let types = run_apply_bootstrap(&text_elements);
        assert_eq!(types, vec![ParsedElementType::Section]);
    }
}
