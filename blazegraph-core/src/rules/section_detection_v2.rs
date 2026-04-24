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

        Self {
            _engine: engine,
            text_elements,
            config,
            _document_analysis: document_analysis,
            font_size_analysis,
            _style_data: style_data,
            inclusion_regexes,
            exclusion_regexes,
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
    fn is_bold(element: &PdfTextElement) -> bool {
        element.style_info.font_weight.to_lowercase().contains("bold")
    }

    /// Determine isolation using segment-based neighbor lookup.
    ///
    /// Primary path: find all elements in `text_elements` that share the same
    /// `(band, column, line_number)` as `element` (excluding `element` itself).
    /// Compute the minimum X-gap between element's bbox and each neighbor's bbox.
    /// If the gap is < `isolation_neighbor_gap`, the element is NOT isolated.
    ///
    /// Fallback (when no same-line neighbors exist): use
    /// `text_width / column_width < isolation_threshold`.
    fn is_isolated(&self, element_idx: usize) -> bool {
        let element = &self.text_elements[element_idx];
        let cfg = &self.config.section_detection_v2;

        // Collect same-(band, column, line_number) neighbors
        let band = element.band();
        let col = element.column();
        let line = element.line_number();

        let same_line: Vec<&PdfTextElement> = self
            .text_elements
            .iter()
            .enumerate()
            .filter(|(idx, e)| {
                *idx != element_idx && e.band() == band && e.column() == col && e.line_number() == line
            })
            .map(|(_, e)| e)
            .collect();

        if same_line.is_empty() {
            // No same-line neighbors — use fallback ratio if line info is meaningful
            // (line_number == 0 can mean "not in a band", so always treat as isolated
            // when there's nothing to compare against)
            return true;
        }

        // Compute minimum X-gap between element and any neighbor
        let elem_left = element.bounding_box().x;
        let elem_right = element.bounding_box().x + element.bounding_box().width;

        let min_gap = same_line
            .iter()
            .map(|neighbor| {
                let n_left = neighbor.bounding_box().x;
                let n_right = neighbor.bounding_box().x + neighbor.bounding_box().width;
                // Gap between the two bboxes (positive = gap, negative = overlap)
                let gap_left = n_left - elem_right; // neighbor is to the right
                let gap_right = elem_left - n_right; // neighbor is to the left
                gap_left.max(gap_right).max(0.0)
            })
            .fold(f32::INFINITY, f32::min);

        min_gap >= cfg.isolation_neighbor_gap
    }

    /// Fallback isolation: text_width / column_width < threshold.
    /// Used when isolation_neighbor_gap check is inconclusive.
    /// Kept for completeness; currently superseded by `is_isolated` which handles
    /// the no-same-line-neighbors case by returning `true` (isolated).
    #[allow(dead_code)]
    fn is_isolated_by_ratio(&self, element: &PdfTextElement) -> bool {
        let cfg = &self.config.section_detection_v2;
        let band = element.band();
        let col = element.column();

        // Compute column width as max(x+width) − min(x) for all elements in this (band, col)
        let column_elements: Vec<&PdfTextElement> = self
            .text_elements
            .iter()
            .filter(|e| e.band() == band && e.column() == col)
            .collect();

        if column_elements.is_empty() {
            return true; // No column context → assume isolated
        }

        let min_x = column_elements
            .iter()
            .map(|e| e.bounding_box().x)
            .fold(f32::INFINITY, f32::min);
        let max_right = column_elements
            .iter()
            .map(|e| e.bounding_box().x + e.bounding_box().width)
            .fold(f32::NEG_INFINITY, f32::max);
        let column_width = (max_right - min_x).max(1.0);

        let text_width = element.bounding_box().width;
        text_width / column_width < cfg.isolation_threshold
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
            // Region 2 (moderate): one confirming signal required
            bold || isolated
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
        isolation_neighbor_gap: f32,
    ) -> ParsingConfig {
        let mut cfg = ParsingConfig::default();
        cfg.section_detection_v2 = SectionDetectionV2Config {
            font_size_tolerance: tolerance,
            structural_size_margin: margin,
            structural_size_ratio: ratio,
            isolation_neighbor_gap,
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
        let config = make_config(1.0, 5.0, None, 20.0);
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
        let config = make_config(1.0, 5.0, None, 20.0);
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
        let config = make_config(1.0, 5.0, None, 20.0);
        assert!(classify(&elements, &font_analysis, &config));
    }

    /// Test 4 — Region 2: isolated alone promotes.
    /// delta = 13 - 10 = 3, tolerance = 1, margin = 5 → Region 2
    /// not bold, isolated (no same-line neighbours) → promoted
    #[test]
    fn test_region2_isolated_alone_promotes() {
        let body = 10.0;
        let elements = vec![make_element(13.0, false, "body", 0)];
        let font_analysis = make_font_analysis(body, vec![("body", 100)]);
        let config = make_config(1.0, 5.0, None, 20.0);
        // element is on a unique line (line 0, no neighbours) → is_isolated = true
        assert!(classify(&elements, &font_analysis, &config));
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
        let config = make_config(1.0, 5.0, None, 20.0);
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
        let config = make_config(1.0, 5.0, None, 20.0);
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
        let config = make_config(1.0, 5.0, None, 20.0);
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
        let config = make_config(1.0, 5.0, None, 20.0);
        assert!(!classify(&elements, &font_analysis, &config));
    }

    /// Test 9 — Region 3: non-isolated bold does NOT promote.
    /// |delta| ≤ tolerance, bold but has neighbour within gap → not isolated → NOT promoted
    #[test]
    fn test_region3_non_isolated_bold_does_not_promote() {
        let body = 10.0;
        let elements = vec![
            make_element(10.5, true, "body", 3),
            make_neighbour(3), // same line, gap 10 < 20 → not isolated
        ];
        let font_analysis = make_font_analysis(body, vec![("body", 100)]);
        let config = make_config(1.0, 5.0, None, 20.0);
        assert!(!classify(&elements, &font_analysis, &config));
    }

    /// Test 10 — Reject: below body − tolerance.
    /// delta = 8 - 10 = -2 < -tolerance (-1) → REJECT regardless of signals
    #[test]
    fn test_reject_below_body_minus_tolerance() {
        let body = 10.0;
        let elements = vec![make_element(8.0, true, "body", 0)];
        let font_analysis = make_font_analysis(body, vec![("body", 100)]);
        let config = make_config(1.0, 5.0, None, 20.0);
        assert!(!classify(&elements, &font_analysis, &config));
    }

    /// Test 11 — Proportional mode: structural_size_ratio overrides margin.
    /// ratio = Some(1.5), body = 10 → threshold = 15.
    /// 12pt: delta = 2 > tolerance 1, < threshold 15 → Region 2; not bold, isolated → promoted.
    /// 16pt: delta = 6 > threshold 15... wait: font_size 16 > threshold 15 → Region 1 → promoted.
    #[test]
    fn test_proportional_ratio_overrides_margin() {
        let body = 10.0;
        let font_analysis = make_font_analysis(body, vec![("body", 100)]);
        // margin=0.1 so body+margin=10.1; ratio=Some(1.5) → threshold=15
        // isolation_neighbor_gap = 20; elements at unique lines → isolated
        let config = make_config(1.0, 0.1, Some(1.5), 20.0);

        // 12pt: delta=2 > tolerance 1 → Region 2, isolated, not bold → promoted (isolated suffices)
        let elements_12 = vec![make_element(12.0, false, "body", 0)];
        assert!(classify(&elements_12, &font_analysis, &config));

        // 16pt: font_size 16 > threshold 15 → Region 1 → promoted
        let elements_16 = vec![make_element(16.0, false, "body", 0)];
        assert!(classify(&elements_16, &font_analysis, &config));
    }

    /// Test 12 — arXiv watermark regression.
    /// body=7pt, element=11pt, not bold, has same-line neighbour (gap<20) → not isolated,
    /// common font, default config (margin=5.0, tolerance=1.0).
    /// delta = 11 - 7 = 4, threshold = 7 + 5 = 12; 4 ≤ 5 (margin), 4 > 1 (tolerance) → Region 2.
    /// Region 2: bold OR isolated → neither → NOT promoted.
    #[test]
    fn test_arxiv_watermark_regression() {
        let body = 7.0;
        let elements = vec![
            make_element(11.0, false, "body", 9),
            make_neighbour(9), // gap 10 < 20 → not isolated
        ];
        let font_analysis = make_font_analysis(body, vec![("body", 100)]);
        let config = make_config(1.0, 5.0, None, 20.0);
        assert!(!classify(&elements, &font_analysis, &config));
    }
}
