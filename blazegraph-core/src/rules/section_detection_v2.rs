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
// HierarchyContext (copied from section_detection.rs — same semantics, self-contained)
//
// This is intentionally a private copy so V2 is self-contained and V1 is untouched.
// If a future block extracts this to rules/hierarchy.rs, only imports need to change.
// ──────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct HierarchyContext {
    current_level: u32,
    previous_section_font_size: Option<f32>,
    level_font_sizes: Vec<f32>,
}

impl Default for HierarchyContext {
    fn default() -> Self {
        Self::new()
    }
}

impl HierarchyContext {
    fn new() -> Self {
        Self {
            current_level: 1,
            previous_section_font_size: None,
            level_font_sizes: Vec::new(),
        }
    }

    /// Update context when we encounter a new section; returns the assigned level.
    fn update_for_section(&mut self, font_size: f32, config: &SectionDetectionV2Config) -> u32 {
        let new_level = match self.previous_section_font_size {
            None => {
                self.current_level = config.starting_section_level;
                self.level_font_sizes = vec![font_size];
                config.starting_section_level
            }
            Some(prev_font) => {
                if font_size < prev_font {
                    // Smaller font → subsection (go deeper)
                    let proposed = self.current_level + 1;
                    if config.enforce_max_depth && proposed > config.max_depth {
                        // Cap at max_depth; update the size at current level
                        if let Some(slot) = self.level_font_sizes.get_mut(self.current_level as usize - 1) {
                            *slot = font_size;
                        }
                        self.current_level
                    } else {
                        self.current_level = proposed;
                        while self.level_font_sizes.len() < self.current_level as usize {
                            self.level_font_sizes.push(0.0);
                        }
                        self.level_font_sizes[self.current_level as usize - 1] = font_size;
                        self.current_level
                    }
                } else if (font_size - prev_font).abs() < config.font_size_tolerance {
                    // Same size (within tolerance) → parallel sibling
                    if let Some(slot) = self.level_font_sizes.get_mut(self.current_level as usize - 1) {
                        *slot = font_size;
                    }
                    self.current_level
                } else {
                    // Larger font → step back up
                    self.current_level = self.find_level_for(font_size, config);
                    while self.level_font_sizes.len() < self.current_level as usize {
                        self.level_font_sizes.push(0.0);
                    }
                    self.level_font_sizes[self.current_level as usize - 1] = font_size;
                    self.current_level
                }
            }
        };
        self.previous_section_font_size = Some(font_size);
        new_level
    }

    /// Find the appropriate level for a font size when stepping back up.
    fn find_level_for(&self, font_size: f32, config: &SectionDetectionV2Config) -> u32 {
        // Look for an existing level with matching size
        for (idx, &sz) in self.level_font_sizes.iter().enumerate() {
            if (font_size - sz).abs() < config.font_size_tolerance {
                return (idx + 1) as u32;
            }
        }
        // Fall back: find first level whose size is smaller than this one
        for (idx, &sz) in self.level_font_sizes.iter().enumerate() {
            if font_size > sz {
                return (idx + 1) as u32;
            }
        }
        1
    }

    fn get_content_level(&self) -> u32 {
        self.current_level + 1
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
    /// Per `(page, band)` x-extent: `(min_x, max_right)` aggregated across all elements
    /// in that band. Used to derive a stable column width for ratio-based isolation,
    /// independent of whether the column under inspection has body neighbors.
    band_extents: HashMap<(u32, u32), (f32, f32)>,
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

        let mut band_extents: HashMap<(u32, u32), (f32, f32)> = HashMap::new();
        for e in text_elements {
            let key = (e.page_number(), e.band());
            let bbox = e.bounding_box();
            let entry = band_extents
                .entry(key)
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
            band_extents,
        }
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
        family.contains("bold")
            || family.contains("medi")
            || family.contains("bx")
    }

    /// Determine isolation as a single geometric question:
    /// **does the visual line this element sits on fill its column?**
    ///
    /// "Visual line" = all elements in the same `(page, band, column)` whose Y-coordinate
    /// is within `line_height_tolerance` of `element.bounding_box.y`. Y is ground truth;
    /// Tika's `data-line` counter is unreliable (resets per `<p>`, so a header and the
    /// next paragraph's first body line collide on `line=0` even though they're at
    /// different visual Y).
    ///
    /// An element alone on its visual line → isolated (no other content sharing the
    /// baseline). Otherwise compute the union extent of all bboxes on this line and
    /// compare to `column_width = band_x_extent / nr_band_columns`. A short line in a
    /// wide column is isolated; a line that fills the column is not (mid-line bold
    /// emphasis, justified body, or Tika overlay-style spans for inline styling).
    fn is_isolated(&self, element_idx: usize) -> bool {
        let element = &self.text_elements[element_idx];
        let cfg = &self.config.section_detection_v2;

        let page = element.page_number();
        let band = element.band();
        let col = element.column();
        let y = element.bounding_box().y;
        let tol = cfg.line_height_tolerance;

        let mut min_x = element.bounding_box().x;
        let mut max_right = element.bounding_box().x + element.bounding_box().width;
        let mut had_line_neighbor = false;
        for (idx, n) in self.text_elements.iter().enumerate() {
            if idx == element_idx
                || n.page_number() != page
                || n.band() != band
                || n.column() != col
                || (n.bounding_box().y - y).abs() >= tol
            {
                continue;
            }
            had_line_neighbor = true;
            let n_left = n.bounding_box().x;
            let n_right = n.bounding_box().x + n.bounding_box().width;
            if n_left < min_x {
                min_x = n_left;
            }
            if n_right > max_right {
                max_right = n_right;
            }
        }

        if !had_line_neighbor {
            return true;
        }

        let line_extent = max_right - min_x;
        let column_width = self.column_width_for(page, band, element.placement.nr_band_columns);
        if column_width <= 0.0 {
            return false;
        }
        line_extent / column_width < cfg.isolation_threshold
    }

    /// Column width derived from band x-extent divided by `nr_band_columns`.
    ///
    /// Uses band extent (not per-column extent) so that a column containing only the
    /// header still gets a meaningful denominator from sibling columns' content.
    fn column_width_for(&self, page: u32, band: u32, nr_band_columns: u32) -> f32 {
        let Some(&(min_x, max_right)) = self.band_extents.get(&(page, band)) else {
            return 0.0;
        };
        let band_width = max_right - min_x;
        if band_width <= 0.0 {
            return 0.0;
        }
        let nr_cols = nr_band_columns.max(1) as f32;
        (band_width / nr_cols).max(1.0)
    }

    /// Check whether the element's font class is rare in the document.
    fn is_rare_font(&self, element: &PdfTextElement) -> bool {
        let cfg = &self.config.section_detection_v2;
        let class_name = &element.style_info.class_name;

        // Total non-rotated element count = sum of all class_usage_counts values
        let total: usize = self
            .font_size_analysis
            .class_usage_counts
            .values()
            .sum();

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
    /// - `delta < -tolerance`               → REJECT (below-body noise)
    /// - `|delta| ≤ tolerance`              → Region 3 (at-body band):
    ///                                         promote if isolated AND (bold OR rare)
    /// - `tolerance < delta ≤ margin`       → Region 2 (moderate):
    ///                                         promote if bold OR isolated
    /// - `delta > margin`                   → Region 1 (large): auto-promote unconditionally
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
    /// Ordering: exclusion runs first, then inclusion can override it.
    /// Rationale: for an element that is both "Figure 3: ..." and matches a numbered
    /// pattern, the figure caption intent is more specific — but the inclusion override
    /// keeps the system composable for edge cases. Document the choice clearly.
    fn apply_pattern_refinement(&self, pass1_is_section: bool, text: &str) -> bool {
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

        // Inclusion last: promote (can override exclusion for numbered subsections etc.)
        // Note: inclusion always wins — if both exclusion and inclusion match, inclusion wins.
        // This lets "^\\d+\\.\\d+" promote "Figure 3.1" — review with human if undesirable.
        if !result {
            for re in &self.inclusion_regexes {
                if re.is_match(text) {
                    result = true;
                    break;
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
        let is_section = self.apply_pattern_refinement(pass1, &element.text);

        if is_section {
            let font_size = element.style_info.font_size;
            let level = hierarchy_context
                .update_for_section(font_size, &self.config.section_detection_v2);
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
        BoundingBox, DocumentAnalysis, FontClass, Placement, PdfTextElement, StyleData,
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
        let mut cfg = ParsingConfig::default();
        cfg.section_detection_v2 = SectionDetectionV2Config {
            font_size_tolerance: tolerance,
            structural_size_margin: margin,
            structural_size_ratio: ratio,
            isolation_threshold,
            line_height_tolerance: 3.0,
            font_rarity_threshold: 0.05, // < 5% → rare
            ..SectionDetectionV2Config::default()
        };
        cfg
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
        let font_analysis = make_font_analysis(
            body,
            vec![("rare_font", 1), ("body", 99)],
        );
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
    /// |delta| ≤ tolerance, bold, column-filling AND has same-line neighbour →
    /// fails both segment and width-ratio isolation → NOT promoted.
    ///
    /// Constructs an element wide enough to defeat the ratio check
    /// (text_width=160 in a band of 200 → ratio 0.80 ≥ 0.75) and a same-line
    /// neighbour at gap=10pt < `isolation_neighbor_gap` (20) to defeat the segment check.
    #[test]
    fn test_region3_non_isolated_bold_does_not_promote() {
        let body = 10.0;
        let mut element = make_element(10.5, true, "body", 3);
        element.placement.bounding_box.width = 160.0;
        let mut neighbour = make_neighbour(3);
        neighbour.placement.bounding_box.x = 170.0;
        neighbour.placement.bounding_box.width = 30.0;
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
        let element =
            make_element_with_font(12.0, "normal", "NimbusRomNo9L-Medi", "latex-medi", 0);
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
        assert!(rule.is_isolated(1), "cross-page element must not be a neighbour");
    }

    /// Same page, same (page, band, col), same Y line — line-extent fills column.
    /// Two side-by-side elements at the same Y union to fill the column width →
    /// ratio ≥ isolation_threshold → not isolated.
    #[test]
    fn test_isolation_same_page_same_band_are_neighbours() {
        let elements = vec![
            // Target at x=0, width=80 → right edge at 80
            make_element_on_page(12.0, 1, 0, 0, 0, 0.0),
            // Neighbour at x=90, width=80 → right edge at 170 (same Y=0)
            make_element_on_page(12.0, 1, 0, 0, 0, 90.0),
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
        // line_extent = 170, band_x_extent = 170, nr_band_columns=1 → ratio = 1.0 ≥ 0.80
        assert!(
            !rule.is_isolated(0),
            "same-Y neighbour fills column union → not isolated"
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
}
