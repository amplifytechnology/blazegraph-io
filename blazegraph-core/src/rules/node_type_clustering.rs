//! NodeTypeClustering rule — CR-29 (renamed from ParagraphClustering).
//!
//! Merges fragmented `ParsedPdfElement`s emitted by Tika into the logical lines
//! / paragraphs / sections they came from. Runs after `SectionDetectionV2`.
//!
//! ## Algorithm
//!
//! 1. **Coarse partition** at `(page, element_type)`. Page is the natural top
//!    boundary (cross-page merge is always wrong). Element type is the natural
//!    type boundary (Section ≠ Paragraph by definition).
//!
//! 2. **Walk reading-order-sorted elements** within each partition. Group
//!    consecutive elements until any configured constraint fails between
//!    `last_in_group` and the current element. On failure, emit the current
//!    group and start a new one.
//!
//! 3. Constraints are orthogonal "must match between consecutive elements"
//!    predicates configured per element type via `NodeTypeMergeConfig`:
//!    `same_line`, `same_paragraph`, `same_band`, `same_column`, `same_depth`,
//!    `max_y_gap`. Adding a new constraint is a drop-in.

use super::engine::{FontSizeAnalysis, ParseRule};
use crate::config::{NodeTypeMergeConfig, ParsingConfig};
use crate::types::{BoundingBox, FontClass, ParsedElementType, ParsedPdfElement};
use anyhow::Result;
use std::collections::HashMap;

// ─── Bbox helpers ─────────────────────────────────────────────────────────────

fn bbox_union(a: &BoundingBox, b: &BoundingBox) -> BoundingBox {
    let x = a.x.min(b.x);
    let y = a.y.min(b.y);
    let right = (a.x + a.width).max(b.x + b.width);
    let bottom = (a.y + a.height).max(b.y + b.height);
    BoundingBox {
        x,
        y,
        width: right - x,
        height: bottom - y,
    }
}

// ─── Style selection (majority by token_count, first-occurrence tiebreaker) ──

fn majority_style(sorted_elements: &[ParsedPdfElement]) -> FontClass {
    let mut class_tokens: HashMap<String, usize> = HashMap::new();
    let mut first_occurrence: HashMap<String, usize> = HashMap::new();

    for (pos, el) in sorted_elements.iter().enumerate() {
        let class = &el.style_info.class_name;
        *class_tokens.entry(class.clone()).or_insert(0) += el.token_count;
        first_occurrence.entry(class.clone()).or_insert(pos);
    }

    let winning_class = class_tokens
        .iter()
        .max_by(|(ca, ta), (cb, tb)| {
            ta.cmp(tb).then_with(|| {
                let ia = first_occurrence[*ca];
                let ib = first_occurrence[*cb];
                ib.cmp(&ia)
            })
        })
        .map(|(class, _)| class.clone())
        .unwrap_or_else(|| sorted_elements[0].style_info.class_name.clone());

    sorted_elements
        .iter()
        .find(|el| el.style_info.class_name == winning_class)
        .map(|el| el.style_info.clone())
        .unwrap_or_else(|| sorted_elements[0].style_info.clone())
}

// ─── Partition key + proximity check ─────────────────────────────────────────

/// Equivalence key derived from an element's placement + the active constraints.
/// Elements with the same key share an equivalence class for merging purposes.
/// `max_y_gap` is *not* an equivalence — it's a pairwise proximity check applied
/// during walk-and-split inside each bucket.
///
/// NOTE (Block 06b, schema 0.5.0): the legacy `band` / `column` partition
/// dimensions were dropped along with the matching Placement fields. Both
/// `cfg.same_band` and `cfg.same_column` are read from yaml but no longer
/// influence the equivalence key. The rule is disabled in the active config
/// (`pipeline.NodeTypeClustering.enabled = false`) and merge semantics will
/// be redesigned in a follow-up CR using `Placement.region_label` from the
/// reading-order resort. Until then, the partition is `(page, element_type,
/// paragraph?, line?, depth?)` only.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct EquivalenceKey {
    page: u32,
    element_type: ParsedElementType,
    paragraph: Option<u32>,
    line: Option<u32>,
    depth: Option<u32>,
}

fn equivalence_key(el: &ParsedPdfElement, cfg: &NodeTypeMergeConfig) -> EquivalenceKey {
    let p = el.pdf_placement();
    EquivalenceKey {
        page: p.page_number,
        element_type: el.element_type.clone(),
        paragraph: if cfg.same_paragraph {
            Some(p.paragraph_number)
        } else {
            None
        },
        line: if cfg.same_line {
            Some(p.line_number)
        } else {
            None
        },
        depth: if cfg.same_depth {
            Some(el.hierarchy_level)
        } else {
            None
        },
    }
}

/// Pairwise proximity check between consecutive elements in a bucket. Only
/// `max_y_gap` is non-equivalence — equality constraints are already enforced
/// by the partition key.
fn within_proximity(
    prev: &ParsedPdfElement,
    curr: &ParsedPdfElement,
    cfg: &NodeTypeMergeConfig,
) -> bool {
    if let Some(max_gap) = cfg.max_y_gap {
        let p_prev = prev.pdf_placement();
        let p_curr = curr.pdf_placement();
        let prev_bottom = p_prev.bounding_box.y + p_prev.bounding_box.height;
        let curr_top = p_curr.bounding_box.y;
        let gap = curr_top - prev_bottom;
        if gap > max_gap {
            return false;
        }
    }
    true
}

// ─── Merge a group of sorted elements into a single element ──────────────────

fn merge_group(
    mut sorted_elements: Vec<ParsedPdfElement>,
    cfg: &NodeTypeMergeConfig,
) -> Option<ParsedPdfElement> {
    if sorted_elements.is_empty() {
        return None;
    }
    if sorted_elements.len() == 1 {
        let el = sorted_elements.remove(0);
        if el.text.trim().is_empty() {
            return None;
        }
        return Some(el);
    }

    // --- Text concatenation ---
    //
    // Same-line detection (legacy): pre-Block-06b this used `(band, line_number)`
    // because Tika reset `line_number` per-paragraph-and-per-band so line_number
    // alone collided across bands. With band/column dropped (schema 0.5.0) the
    // discriminator is `line_number` alone — degraded for the cross-band case.
    // The rule is disabled in the active config; a region-aware redesign keyed
    // on `Placement.region_label` is the proper fix and is filed as a follow-up
    // CR. Table separator (`table_line_separator`) is now unreachable here
    // because the dropped `nr_band_columns` always determined that branch.
    //
    // Same-line join rule: exactly one space at the boundary unless one side
    // already provides whitespace.
    let mut text = String::new();
    let mut prev_line: Option<u32> = None;
    for el in &sorted_elements {
        let p = el.pdf_placement();
        let line = p.line_number;

        if !text.is_empty() {
            match prev_line {
                Some(prev) if prev == line => {
                    // Same-line: insert at most one space if neither side has
                    // whitespace at the join boundary.
                    let left_ws = text.ends_with(|c: char| c.is_whitespace());
                    let right_ws = el.text.starts_with(|c: char| c.is_whitespace());
                    if !left_ws && !right_ws {
                        text.push(' ');
                    }
                }
                _ => {
                    text.push_str(&cfg.prose_line_separator);
                }
            }
        }

        // Drop leading whitespace on el.text if we already have whitespace at
        // the boundary — guarantees the join is at most a single separator.
        let to_push = if text.ends_with(|c: char| c.is_whitespace()) {
            el.text.trim_start()
        } else {
            el.text.as_str()
        };
        text.push_str(to_push);
        prev_line = Some(line);
    }
    // Trim accumulated boundary whitespace from the ends; interior preserved.
    let text = text.trim().to_string();

    if text.trim().is_empty() {
        return None;
    }

    // --- Bbox union ---
    let mut union_bbox = sorted_elements[0].pdf_placement().bounding_box.clone();
    for el in &sorted_elements[1..] {
        union_bbox = bbox_union(&union_bbox, &el.pdf_placement().bounding_box);
    }

    let winning_style = majority_style(&sorted_elements);

    let min_reading_order = sorted_elements
        .iter()
        .map(|el| el.reading_order)
        .min()
        .unwrap_or(0);

    let sum_tokens: usize = sorted_elements.iter().map(|el| el.token_count).sum();

    let bookmark_match = sorted_elements
        .iter()
        .find_map(|el| el.bookmark_match.clone());

    let element_type = sorted_elements[0].element_type.clone();

    let mut merged_placement = sorted_elements[0].pdf_placement().clone();
    merged_placement.bounding_box = union_bbox;

    let position = sorted_elements[0].position;

    Some(ParsedPdfElement {
        element_type,
        text,
        hierarchy_level: sorted_elements[0].hierarchy_level,
        position,
        style_info: winning_style,
        placement: Some(merged_placement),
        reading_order: min_reading_order,
        bookmark_match,
        token_count: sum_tokens,
    })
}

// ─── Rule struct ──────────────────────────────────────────────────────────────

pub struct NodeTypeClusteringRule<'a> {
    #[allow(dead_code)]
    engine: &'a super::engine::RuleEngine,
    #[allow(dead_code)]
    text_elements: &'a [crate::types::PdfTextElement],
    config: &'a ParsingConfig,
    #[allow(dead_code)]
    document_analysis: &'a crate::analytics::DocumentAnalysis,
    #[allow(dead_code)]
    font_size_analysis: &'a FontSizeAnalysis,
    #[allow(dead_code)]
    style_data: &'a crate::types::StyleData,
}

impl<'a> NodeTypeClusteringRule<'a> {
    pub fn new(
        engine: &'a super::engine::RuleEngine,
        text_elements: &'a [crate::types::PdfTextElement],
        config: &'a ParsingConfig,
        document_analysis: &'a crate::analytics::DocumentAnalysis,
        font_size_analysis: &'a FontSizeAnalysis,
        style_data: &'a crate::types::StyleData,
    ) -> Self {
        Self {
            engine,
            text_elements,
            config,
            document_analysis,
            font_size_analysis,
            style_data,
        }
    }

    fn cfg_for_type<'c>(
        &self,
        cfg: &'c crate::config::NodeTypeClusteringConfig,
        ty: &ParsedElementType,
    ) -> &'c NodeTypeMergeConfig {
        match ty {
            ParsedElementType::Section => &cfg.section,
            ParsedElementType::Paragraph => &cfg.paragraph,
            ParsedElementType::List => &cfg.list,
            ParsedElementType::ListItem => &cfg.list_item,
            // Block 07 — rule is `enabled: false` in the active config and
            // its band/column-based partitioning is awaiting a region-aware
            // redesign (see ignored tests in this file). Header / Footer /
            // Margin currently fall through to the paragraph config so they
            // compile; the redesign will give them their own config keys
            // alongside the new region_label-aware equivalence key.
            ParsedElementType::Header | ParsedElementType::Footer | ParsedElementType::Margin => {
                &cfg.paragraph
            }
        }
    }

    pub fn apply(&self, elements: Vec<ParsedPdfElement>) -> Result<Vec<ParsedPdfElement>> {
        let cfg = &self.config.node_type_clustering;
        let input_count = elements.len();

        if elements.is_empty() {
            return Ok(elements);
        }

        // Build the equivalence key per element using the per-type config.
        // Equality constraints (same_band, same_column, same_paragraph,
        // same_line, same_depth) are baked into the key. Only `max_y_gap` is
        // non-equivalence and is handled by walk-and-split inside each bucket.
        let mut partition_order: Vec<EquivalenceKey> = Vec::new();
        let mut buckets: HashMap<EquivalenceKey, Vec<ParsedPdfElement>> = HashMap::new();

        for el in elements {
            let type_cfg = self.cfg_for_type(cfg, &el.element_type);
            let key = equivalence_key(&el, type_cfg);
            if !buckets.contains_key(&key) {
                partition_order.push(key.clone());
            }
            buckets.entry(key).or_default().push(el);
        }

        let mut merged: Vec<ParsedPdfElement> = Vec::new();
        let mut total_groups = 0usize;

        for key in &partition_order {
            let mut bucket = buckets.remove(key).unwrap_or_default();
            bucket.sort_by_key(|el| el.reading_order);
            let type_cfg = self.cfg_for_type(cfg, &key.element_type);

            // Walk-and-split on proximity only. Equality constraints are
            // already guaranteed by the bucket key.
            let mut current_group: Vec<ParsedPdfElement> = Vec::new();
            for el in bucket {
                let split = match current_group.last() {
                    None => false,
                    Some(prev) => !within_proximity(prev, &el, type_cfg),
                };
                if split {
                    let group = std::mem::take(&mut current_group);
                    total_groups += 1;
                    if let Some(out) = merge_group(group, type_cfg) {
                        merged.push(out);
                    }
                }
                current_group.push(el);
            }
            if !current_group.is_empty() {
                total_groups += 1;
                if let Some(out) = merge_group(current_group, type_cfg) {
                    merged.push(out);
                }
            }
        }

        merged.sort_by_key(|el| el.reading_order);

        println!(
            "   📦 NodeTypeClustering: {} input elements, {} buckets, {} groups",
            input_count,
            partition_order.len(),
            total_groups,
        );
        println!("   ✅ NodeTypeClustering: {} output elements", merged.len(),);

        Ok(merged)
    }
}

impl<'a> ParseRule for NodeTypeClusteringRule<'a> {
    fn apply(&self, elements: Vec<ParsedPdfElement>) -> Result<Vec<ParsedPdfElement>> {
        self.apply(elements)
    }

    fn name(&self) -> &str {
        "NodeTypeClustering"
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Placement;

    fn mk_style(class_name: &str, font_size: f32) -> FontClass {
        FontClass {
            class_name: class_name.to_string(),
            font_family: "Times-Roman".to_string(),
            font_size,
            font_style: "normal".to_string(),
            font_weight: "normal".to_string(),
            color: "#000000".to_string(),
        }
    }

    fn mk_placement(page: u32, paragraph: u32, line: u32, y: f32) -> Placement {
        Placement {
            page_number: page,
            bounding_box: BoundingBox {
                x: 0.0,
                y,
                width: 100.0,
                height: 17.7,
            },
            line_number: line,
            segment_number: 0,
            rotation: 0,
            paragraph_number: paragraph,
            region_label: None,
            page_width: 0.0,
            page_height: 0.0,
        }
    }

    fn mk_element(
        ty: ParsedElementType,
        text: &str,
        depth: u32,
        reading_order: u32,
        placement: Placement,
    ) -> ParsedPdfElement {
        ParsedPdfElement {
            element_type: ty,
            text: text.to_string(),
            hierarchy_level: depth,
            position: reading_order as usize,
            style_info: mk_style("f27", 17.7),
            placement: Some(placement),
            reading_order,
            bookmark_match: None,
            token_count: 1,
        }
    }

    /// Direct algorithm invocation — bypasses `RuleEngine` to avoid wiring the
    /// full engine context for a unit test. Mirrors `NodeTypeClusteringRule::apply`.
    fn run_rule(
        elements: Vec<ParsedPdfElement>,
        cfg: &crate::config::NodeTypeClusteringConfig,
    ) -> Vec<ParsedPdfElement> {
        let pick = |ty: &ParsedElementType| -> &NodeTypeMergeConfig {
            match ty {
                ParsedElementType::Section => &cfg.section,
                ParsedElementType::Paragraph => &cfg.paragraph,
                ParsedElementType::List => &cfg.list,
                ParsedElementType::ListItem => &cfg.list_item,
                ParsedElementType::Header
                | ParsedElementType::Footer
                | ParsedElementType::Margin => &cfg.paragraph,
            }
        };

        let mut buckets: HashMap<EquivalenceKey, Vec<ParsedPdfElement>> = HashMap::new();
        let mut order: Vec<EquivalenceKey> = Vec::new();
        for el in elements {
            let tcfg = pick(&el.element_type);
            let key = equivalence_key(&el, tcfg);
            if !buckets.contains_key(&key) {
                order.push(key.clone());
            }
            buckets.entry(key).or_default().push(el);
        }

        let mut out: Vec<ParsedPdfElement> = Vec::new();
        for key in &order {
            let mut bucket = buckets.remove(key).unwrap_or_default();
            bucket.sort_by_key(|el| el.reading_order);
            let tcfg = pick(&key.element_type);
            let mut group: Vec<ParsedPdfElement> = Vec::new();
            for el in bucket {
                let split = match group.last() {
                    None => false,
                    Some(prev) => !within_proximity(prev, &el, tcfg),
                };
                if split {
                    let g = std::mem::take(&mut group);
                    if let Some(m) = merge_group(g, tcfg) {
                        out.push(m);
                    }
                }
                group.push(el);
            }
            if !group.is_empty() {
                if let Some(m) = merge_group(group, tcfg) {
                    out.push(m);
                }
            }
        }
        out.sort_by_key(|el| el.reading_order);
        out
    }

    /// Test 1 — Cross-band Section merge (the GraphRAG case):
    /// "Improving" (band 0, y=141.8) + "LLM accuracy" (band 1, y=173.9), same
    /// column, same depth, Y-gap = 14.4pt → merge into one Section.
    #[test]
    fn cross_band_section_merge() {
        let cfg = crate::config::NodeTypeClusteringConfig::default();
        let p1 = mk_placement(22, 0, 0, 141.8);
        let p2 = mk_placement(22, 0, 0, 173.9);
        let els = vec![
            mk_element(ParsedElementType::Section, "Improving", 2, 0, p1),
            mk_element(ParsedElementType::Section, "LLM accuracy", 2, 1, p2),
        ];
        let out = run_rule(els, &cfg);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "Improving LLM accuracy");
    }

    /// Test 2 — Cross-band merge rejected by `max_y_gap`. Same setup but Y-gap
    /// far exceeds the threshold → split into two Sections.
    #[test]
    fn cross_band_section_split_by_proximity() {
        let cfg = crate::config::NodeTypeClusteringConfig::default();
        let p1 = mk_placement(22, 0, 0, 100.0);
        let p2 = mk_placement(22, 0, 0, 500.0); // gap = 500 - 117.7 = 382.3 > 50
        let els = vec![
            mk_element(ParsedElementType::Section, "Section A", 2, 0, p1),
            mk_element(ParsedElementType::Section, "Section B", 2, 1, p2),
        ];
        let out = run_rule(els, &cfg);
        assert_eq!(out.len(), 2);
    }

    /// Test 3 — Cross-band merge rejected by `same_depth`. Different
    /// `hierarchy_level` values → split.
    #[test]
    fn cross_band_section_split_by_depth() {
        let cfg = crate::config::NodeTypeClusteringConfig::default();
        let p1 = mk_placement(22, 0, 0, 100.0);
        let p2 = mk_placement(22, 0, 0, 120.0);
        let els = vec![
            mk_element(ParsedElementType::Section, "Chapter 1", 2, 0, p1),
            mk_element(ParsedElementType::Section, "1.1 Sub", 3, 1, p2),
        ];
        let out = run_rule(els, &cfg);
        assert_eq!(out.len(), 2);
    }

    /// Test 4 — Paragraph isolation across bands preserved by the default
    /// paragraph config (`same_band: true`).
    ///
    /// IGNORED (Block 06b, schema 0.5.0): band was dropped from `EquivalenceKey`
    /// along with the corresponding `Placement` field. Cross-band isolation now
    /// regresses to no-op for the cases this test covers; a region-aware
    /// redesign keyed on `Placement.region_label` is the proper fix and is
    /// tracked as a follow-up CR. The rule is disabled in the active config.
    #[test]
    #[ignore = "Block 06b: band/column dropped — awaiting region-aware redesign"]
    fn paragraph_cross_band_isolation_preserved() {
        let cfg = crate::config::NodeTypeClusteringConfig::default();
        let p1 = mk_placement(22, 0, 0, 200.0);
        let p2 = mk_placement(22, 1, 0, 250.0);
        let els = vec![
            mk_element(ParsedElementType::Paragraph, "Body line A", 4, 0, p1),
            mk_element(ParsedElementType::Paragraph, "Body line B", 4, 1, p2),
        ];
        let out = run_rule(els, &cfg);
        assert_eq!(out.len(), 2);
    }

    /// Test 5 — `same_column` enforcement. Section elements on different
    /// columns of the same page+band stay separate.
    ///
    /// IGNORED (Block 06b, schema 0.5.0): see `paragraph_cross_band_isolation_preserved`.
    #[test]
    #[ignore = "Block 06b: band/column dropped — awaiting region-aware redesign"]
    fn same_column_constraint_splits() {
        let cfg = crate::config::NodeTypeClusteringConfig::default();
        let p1 = mk_placement(22, 0, 0, 100.0);
        let p2 = mk_placement(22, 0, 0, 110.0);
        let els = vec![
            mk_element(ParsedElementType::Section, "Col 0 title", 2, 0, p1),
            mk_element(ParsedElementType::Section, "Col 1 title", 2, 1, p2),
        ];
        let out = run_rule(els, &cfg);
        assert_eq!(out.len(), 2);
    }

    /// Test 6 — Walk-and-split sub-grouping. [A, B, C] where B violates a
    /// constraint with A but C respects all with B → emits [A], [B, C].
    #[test]
    fn walk_and_split_three_element() {
        let cfg = crate::config::NodeTypeClusteringConfig::default();
        let p_a = mk_placement(22, 0, 0, 100.0);
        let p_b = mk_placement(22, 0, 0, 500.0);
        let p_c = mk_placement(22, 0, 0, 520.0);
        let els = vec![
            mk_element(ParsedElementType::Section, "A", 2, 0, p_a),
            mk_element(ParsedElementType::Section, "B", 2, 1, p_b),
            mk_element(ParsedElementType::Section, "C", 2, 2, p_c),
        ];
        let out = run_rule(els, &cfg);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].text, "A");
        assert_eq!(out[1].text, "B C");
    }

    /// Test 7 — Same-band, same-line segments separated by a single space at
    /// the join boundary when neither side carries whitespace. The xhtml
    /// parser preserves Tika's segment-boundary whitespace; on segments where
    /// Tika emitted none (e.g. CR-25 dropped it), the join inserts exactly one.
    #[test]
    fn same_line_segments_join_with_single_space() {
        let cfg = crate::config::NodeTypeClusteringConfig::default();
        let p1 = mk_placement(22, 0, 0, 100.0);
        let p2 = mk_placement(22, 0, 0, 100.0);
        let els = vec![
            mk_element(ParsedElementType::Section, "Title", 2, 0, p1),
            mk_element(ParsedElementType::Section, "continued", 2, 1, p2),
        ];
        let out = run_rule(els, &cfg);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "Title continued");
    }

    /// Test 7b — Inline-styled emphasis case (the rfc-quic MAY pattern).
    /// Tika emits three segments on the same band+line; the parser preserves
    /// the trailing space on the first base-font segment but the bracketing
    /// pre-pass removed the space between the styled word and the resuming
    /// base-font run. Join must yield "implementations MAY choose...".
    #[test]
    fn same_line_inline_emphasis_join() {
        let cfg = crate::config::NodeTypeClusteringConfig::default();
        let p_a = mk_placement(14, 0, 1, 222.4);
        let p_b = mk_placement(14, 0, 1, 222.4);
        let p_c = mk_placement(14, 0, 1, 222.4);
        let els = vec![
            mk_element(ParsedElementType::Paragraph, "implementations ", 4, 0, p_a),
            mk_element(ParsedElementType::Paragraph, "MAY", 4, 1, p_b),
            mk_element(ParsedElementType::Paragraph, "choose to offer", 4, 2, p_c),
        ];
        let out = run_rule(els, &cfg);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "implementations MAY choose to offer");
    }

    /// Test 8 — Migration shim: legacy YAML deserializes into per-type configs
    /// matching the equivalent constraint set. `merge_lines: true` (the prior
    /// default) keys partitions on `(page, band, column, paragraph_number,
    /// element_type)`, so the equivalent constraint set is `same_paragraph,
    /// same_band, same_column` all true.
    #[test]
    fn migration_shim_legacy_default() {
        let yaml = r#"
merge_segments: true
merge_lines: true
merge_columns: false
merge_bands: false
prose_line_separator: " "
table_line_separator: "\n"
"#;
        let cfg: crate::config::NodeTypeClusteringConfig =
            serde_yaml::from_str(yaml).expect("legacy YAML must deserialize");
        for t in &[&cfg.section, &cfg.paragraph, &cfg.list, &cfg.list_item] {
            assert!(!t.same_line, "legacy default does not require same_line");
            assert!(
                t.same_paragraph,
                "merge_lines=true keys on paragraph_number"
            );
            assert!(t.same_band);
            assert!(t.same_column);
            assert!(!t.same_depth);
            assert!(t.max_y_gap.is_none());
        }
    }

    /// Test 8b — Migration shim: `merge_columns: true` collapses paragraph
    /// boundaries (key drops paragraph_number) → `same_paragraph: false`.
    #[test]
    fn migration_shim_legacy_columns() {
        let yaml = r#"
merge_segments: true
merge_lines: true
merge_columns: true
merge_bands: false
"#;
        let cfg: crate::config::NodeTypeClusteringConfig =
            serde_yaml::from_str(yaml).expect("legacy YAML must deserialize");
        let t = &cfg.section;
        assert!(!t.same_paragraph);
        assert!(t.same_band);
        assert!(!t.same_column);
    }
}
