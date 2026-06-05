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
//!    `same_line`, `same_paragraph`, `same_depth`, `max_y_gap`, plus the
//!    region-aware `ignore_region_label` toggle (drops region from the key —
//!    used for Header / Footer where per-page running chrome is one unit).
//!    Adding a new constraint is a drop-in.

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
/// `region` carries `Placement.region_label` (Block 06b's reading-order resort
/// substrate) when `cfg.ignore_region_label = false`. Header / Footer defaults
/// flip the toggle to drop region from the key so all `H-N` / `F-N` indices on
/// a page collapse into one running-chrome bucket. Orphans (`region_label =
/// None`) are bucketed as singletons by synthesizing a unique discriminator
/// from `reading_order` — the invariant is that reading_order should always
/// assign a region (B-X / H-X / F-X / M-X), but the defensive synthesis means
/// any straggler stays a no-op rather than collapsing with other orphans.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct EquivalenceKey {
    page: u32,
    element_type: ParsedElementType,
    region: Option<RegionDiscriminator>,
    paragraph: Option<u32>,
    line: Option<u32>,
    depth: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum RegionDiscriminator {
    Labeled(String),
    /// Per-element singleton for orphan elements (`region_label = None`).
    /// Carries `reading_order` so each orphan lands in its own bucket.
    Orphan(u32),
}

fn equivalence_key(
    el: &ParsedPdfElement,
    cfg: &NodeTypeMergeConfig,
    drop_paragraph: bool,
) -> EquivalenceKey {
    let p = el.pdf_placement();
    let region = if cfg.ignore_region_label {
        None
    } else {
        Some(match &p.region_label {
            Some(label) => RegionDiscriminator::Labeled(label.clone()),
            None => RegionDiscriminator::Orphan(el.reading_order),
        })
    };
    EquivalenceKey {
        page: p.page_number,
        element_type: el.element_type.clone(),
        region,
        paragraph: if cfg.same_paragraph && !drop_paragraph {
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

/// `(page, element_type, region_label)` — used by the overflow pre-scan to
/// count distinct `paragraph_number`s per region.
type RegionScopeKey = (u32, ParsedElementType, Option<String>);

/// Pre-scan: identify regions where Tika's `paragraph_number` is so fragmented
/// that it should be dropped from the equivalence key (treat the region as if
/// `same_paragraph: false`).
///
/// **CR-38 stopgap.** This is a heuristic for Tika's row-keyed `paragraph_number`
/// in 2-column body layouts. The structurally-correct fix (geometry-derived
/// paragraph detection from bbox data) is filed as CR-38; this knob is the
/// Block 10 ship-it pragma. When the threshold fires, real within-column
/// paragraph breaks are lost — accepted tradeoff until CR-38 lands.
///
/// Returns the set of `(page, element_type, region_label)` triples that
/// exceeded their type's threshold.
fn compute_overflow_regions<'c>(
    elements: &[ParsedPdfElement],
    cfg_for_type: impl Fn(&ParsedElementType) -> &'c NodeTypeMergeConfig,
) -> std::collections::HashSet<RegionScopeKey> {
    let mut paragraphs_per_region: HashMap<RegionScopeKey, std::collections::HashSet<u32>> =
        HashMap::new();

    for el in elements {
        let type_cfg = cfg_for_type(&el.element_type);
        // Only types with same_paragraph + region_overflow_threshold set can
        // overflow — the heuristic is meaningless otherwise.
        if !type_cfg.same_paragraph || type_cfg.region_overflow_threshold.is_none() {
            continue;
        }
        let p = el.pdf_placement();
        let key: RegionScopeKey = (
            p.page_number,
            el.element_type.clone(),
            p.region_label.clone(),
        );
        paragraphs_per_region
            .entry(key)
            .or_default()
            .insert(p.paragraph_number);
    }

    paragraphs_per_region
        .into_iter()
        .filter_map(|(key, set)| {
            let type_cfg = cfg_for_type(&key.1);
            type_cfg.region_overflow_threshold.and_then(|t| {
                if set.len() >= t as usize {
                    Some(key)
                } else {
                    None
                }
            })
        })
        .collect()
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
    // Same-line detection: `line_number` alone is the discriminator. Within a
    // bucket the equivalence key has already constrained things to one
    // `(page, region_label)` (or one `(page, type)` for H/F when
    // `ignore_region_label`), so Tika's per-paragraph-and-per-band line_number
    // reset doesn't collide here — the bucket is already inside one logical
    // structural unit. Table separator (`table_line_separator`) is currently
    // unreachable; preserved as a knob for future per-region column-count
    // detection.
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

    // CR-62: concatenate link annotations from all merged source elements
    // in source order. Reading-order preserved by construction because
    // sorted_elements is already in source order (per DT-02 Tika within-line
    // bbox.x sort + line/paragraph order).
    let merged_links: Vec<_> = sorted_elements
        .iter()
        .flat_map(|el| el.links.iter().cloned())
        .collect();

    // CR-78 (Phase A): the fused node's confidence is the max across the
    // merged fragments — the merged Section is at least as confident as its
    // strongest constituent (e.g. a "3.1" number fragment carrying
    // R3+numbered fuses with its title fragment and the node keeps the higher
    // score). Non-Section merges keep 0 (no fragment carries one).
    let merged_confidence = sorted_elements
        .iter()
        .map(|el| el.confidence)
        .max()
        .unwrap_or(0);

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
        links: merged_links,
        confidence: merged_confidence,
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
            ParsedElementType::Header => &cfg.header,
            ParsedElementType::Footer => &cfg.footer,
            ParsedElementType::Margin => &cfg.margin,
            // CR-79: a whole region tagged Table merges into one Table node
            // (region IS the boundary; line/paragraph/depth gates dropped).
            ParsedElementType::Table => &cfg.table,
        }
    }

    pub fn apply(&self, elements: Vec<ParsedPdfElement>) -> Result<Vec<ParsedPdfElement>> {
        let cfg = &self.config.node_type_clustering;
        let input_count = elements.len();

        if elements.is_empty() {
            return Ok(elements);
        }

        // Pre-scan for the CR-38 paragraph-overflow stopgap: regions where
        // Tika's row-keyed `paragraph_number` produces too many singleton
        // buckets (gpt2-style 2-column body) drop `paragraph_number` from
        // the key.
        let overflow_regions = compute_overflow_regions(&elements, |ty| self.cfg_for_type(cfg, ty));
        if !overflow_regions.is_empty() {
            println!(
                "   ↪️  NodeTypeClustering: {} region(s) hit paragraph-overflow threshold (CR-38 fallback active)",
                overflow_regions.len()
            );
        }

        // Build the equivalence key per element using the per-type config.
        // Equality constraints (region_label, same_paragraph, same_line,
        // same_depth) are baked into the key. Only `max_y_gap` is
        // non-equivalence and is handled by walk-and-split inside each bucket.
        let mut partition_order: Vec<EquivalenceKey> = Vec::new();
        let mut buckets: HashMap<EquivalenceKey, Vec<ParsedPdfElement>> = HashMap::new();

        for el in elements {
            let type_cfg = self.cfg_for_type(cfg, &el.element_type);
            let p = el.pdf_placement();
            let region_scope: RegionScopeKey = (
                p.page_number,
                el.element_type.clone(),
                p.region_label.clone(),
            );
            let drop_paragraph = overflow_regions.contains(&region_scope);
            let key = equivalence_key(&el, type_cfg, drop_paragraph);
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
        // Default region_label "1" — single body region. Tests that need
        // distinct regions use `mk_placement_in_region`.
        mk_placement_in_region(page, paragraph, line, y, Some("1"))
    }

    fn mk_placement_in_region(
        page: u32,
        paragraph: u32,
        line: u32,
        y: f32,
        region_label: Option<&str>,
    ) -> Placement {
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
            region_label: region_label.map(str::to_string),
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
            links: vec![],
            confidence: 0,
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
                ParsedElementType::Header => &cfg.header,
                ParsedElementType::Footer => &cfg.footer,
                ParsedElementType::Margin => &cfg.margin,
                ParsedElementType::Table => &cfg.table,
            }
        };

        let overflow_regions = compute_overflow_regions(&elements, pick);
        let mut buckets: HashMap<EquivalenceKey, Vec<ParsedPdfElement>> = HashMap::new();
        let mut order: Vec<EquivalenceKey> = Vec::new();
        for el in elements {
            let tcfg = pick(&el.element_type);
            let p = el.pdf_placement();
            let region_scope: RegionScopeKey = (
                p.page_number,
                el.element_type.clone(),
                p.region_label.clone(),
            );
            let drop_paragraph = overflow_regions.contains(&region_scope);
            let key = equivalence_key(&el, tcfg, drop_paragraph);
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

    /// Test 4 — Paragraph cross-paragraph isolation preserved. The paragraph
    /// default keeps `same_paragraph: true`, so two Paragraph elements with
    /// different `paragraph_number` in the same region stay distinct.
    #[test]
    fn paragraph_cross_paragraph_isolation_preserved() {
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

    /// Test 5 — `region_label` partition splits. Section elements in different
    /// Region tree leaves on the same page stay separate even when their other
    /// placement fields are identical. Region is the new primary structural
    /// axis (Block 10 — replaces the legacy band/column gate).
    #[test]
    fn same_region_label_constraint_splits() {
        let cfg = crate::config::NodeTypeClusteringConfig::default();
        let p1 = mk_placement_in_region(22, 0, 0, 100.0, Some("1"));
        let p2 = mk_placement_in_region(22, 0, 0, 110.0, Some("2"));
        let els = vec![
            mk_element(ParsedElementType::Section, "Region 1 title", 2, 0, p1),
            mk_element(ParsedElementType::Section, "Region 2 title", 2, 1, p2),
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
    /// matching the surviving constraint subset. `merge_lines: true` (the prior
    /// default) translates to `same_paragraph: true` on the body types.
    /// `merge_bands` / `merge_columns` are silently ignored — they have no
    /// equivalent in the region-aware world.
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
            assert!(!t.same_depth);
            assert!(!t.ignore_region_label);
            assert!(t.max_y_gap.is_none());
        }
        // H/F/M get fresh defaults (no legacy equivalent).
        assert!(cfg.header.ignore_region_label);
        assert!(cfg.footer.ignore_region_label);
        assert!(!cfg.margin.ignore_region_label);
    }

    /// Test 8b — Migration shim: legacy column flag is silently dropped.
    /// `merge_columns: true` historically collapsed paragraph_number; the
    /// region-aware redesign keys on region instead and ignores the flag.
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
        assert!(t.same_paragraph, "merge_lines=true survives translation");
        assert!(!t.ignore_region_label);
    }

    // ─── Region-aware tests (Block 10) ─────────────────────────────────────

    /// Region-aware Section merge: same region, same depth, within proximity
    /// → one node. Cross-region with otherwise-identical placement → split.
    #[test]
    fn section_within_region_merges_across_regions_splits() {
        let cfg = crate::config::NodeTypeClusteringConfig::default();
        let same_region = vec![
            mk_element(
                ParsedElementType::Section,
                "Title line 1",
                2,
                0,
                mk_placement_in_region(1, 0, 0, 100.0, Some("1")),
            ),
            mk_element(
                ParsedElementType::Section,
                "Title line 2",
                2,
                1,
                mk_placement_in_region(1, 0, 1, 120.0, Some("1")),
            ),
        ];
        let out = run_rule(same_region, &cfg);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "Title line 1 Title line 2");

        let cross_region = vec![
            mk_element(
                ParsedElementType::Section,
                "Title A",
                2,
                0,
                mk_placement_in_region(1, 0, 0, 100.0, Some("1")),
            ),
            mk_element(
                ParsedElementType::Section,
                "Title B",
                2,
                1,
                mk_placement_in_region(1, 0, 0, 120.0, Some("2")),
            ),
        ];
        let out = run_rule(cross_region, &cfg);
        assert_eq!(out.len(), 2);
    }

    /// Header default ignores region_label: H-1 and H-2 on the same page
    /// collapse into one running-header bucket.
    #[test]
    fn header_collapses_across_region_labels_within_page() {
        let cfg = crate::config::NodeTypeClusteringConfig::default();
        let els = vec![
            mk_element(
                ParsedElementType::Header,
                "Chapter 3",
                0,
                0,
                mk_placement_in_region(7, 0, 0, 30.0, Some("H-1")),
            ),
            mk_element(
                ParsedElementType::Header,
                "Page 18",
                0,
                1,
                mk_placement_in_region(7, 0, 0, 30.0, Some("H-2")),
            ),
        ];
        let out = run_rule(els, &cfg);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "Chapter 3 Page 18");

        // Cross-page must NOT merge — region is dropped but page is still in
        // the key.
        let cross_page = vec![
            mk_element(
                ParsedElementType::Header,
                "Chapter 3",
                0,
                0,
                mk_placement_in_region(7, 0, 0, 30.0, Some("H-1")),
            ),
            mk_element(
                ParsedElementType::Header,
                "Chapter 3",
                0,
                1,
                mk_placement_in_region(8, 0, 0, 30.0, Some("H-1")),
            ),
        ];
        let out = run_rule(cross_page, &cfg);
        assert_eq!(out.len(), 2);
    }

    /// Margin default keeps region_label: a sidebar block and a footnote
    /// margin in different leaves stay distinct, but multiple fragments
    /// inside one Margin region merge.
    #[test]
    fn margin_partitions_by_region_label() {
        let cfg = crate::config::NodeTypeClusteringConfig::default();
        let els = vec![
            mk_element(
                ParsedElementType::Margin,
                "Sidebar a",
                0,
                0,
                mk_placement_in_region(2, 0, 0, 200.0, Some("M-1")),
            ),
            mk_element(
                ParsedElementType::Margin,
                "Sidebar b",
                0,
                1,
                mk_placement_in_region(2, 0, 1, 220.0, Some("M-1")),
            ),
            mk_element(
                ParsedElementType::Margin,
                "Footnote",
                0,
                2,
                mk_placement_in_region(2, 0, 0, 700.0, Some("M-2")),
            ),
        ];
        let out = run_rule(els, &cfg);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].text, "Sidebar a Sidebar b");
        assert_eq!(out[1].text, "Footnote");
    }

    /// CR-38 overflow stopgap: when a `(page, region_label)` produces ≥N
    /// distinct paragraph_numbers (Tika's row-keyed misbehavior in 2-column
    /// layout), `paragraph_number` is dropped from the key for that region
    /// and all elements in it collapse into one bucket.
    #[test]
    fn paragraph_overflow_threshold_collapses_region() {
        // Default paragraph config has `region_overflow_threshold: Some(10)`.
        // Build 12 elements in one region with 12 distinct paragraph_numbers.
        let cfg = crate::config::NodeTypeClusteringConfig::default();
        let mut els = Vec::new();
        for i in 0..12 {
            els.push(mk_element(
                ParsedElementType::Paragraph,
                &format!("frag{i}"),
                4,
                i,
                mk_placement_in_region(1, i, 0, 100.0 + i as f32 * 14.0, Some("1")),
            ));
        }
        let out = run_rule(els, &cfg);
        assert_eq!(
            out.len(),
            1,
            "12 paragraph_numbers ≥ threshold 10 → collapse"
        );
        // Verify all fragments concatenated in order.
        assert!(out[0].text.starts_with("frag0"));
        assert!(out[0].text.ends_with("frag11"));
    }

    /// CR-38 overflow stopgap: under the threshold, normal per-paragraph
    /// bucketing applies (no collapse).
    #[test]
    fn paragraph_overflow_threshold_below_keeps_paragraphs() {
        let cfg = crate::config::NodeTypeClusteringConfig::default();
        let mut els = Vec::new();
        // 5 paragraph_numbers in one region — under threshold 10.
        for i in 0..5 {
            els.push(mk_element(
                ParsedElementType::Paragraph,
                &format!("frag{i}"),
                4,
                i,
                mk_placement_in_region(1, i, 0, 100.0 + i as f32 * 14.0, Some("1")),
            ));
        }
        let out = run_rule(els, &cfg);
        assert_eq!(out.len(), 5, "5 < threshold 10 → no collapse");
    }

    /// CR-38 overflow stopgap is per-region: one overflowing region collapses,
    /// neighbouring regions in the same page stay normally bucketed.
    #[test]
    fn paragraph_overflow_threshold_is_per_region() {
        let cfg = crate::config::NodeTypeClusteringConfig::default();
        let mut els = Vec::new();
        // Region "1" overflows (12 paragraphs)
        for i in 0..12 {
            els.push(mk_element(
                ParsedElementType::Paragraph,
                &format!("L{i}"),
                4,
                i,
                mk_placement_in_region(1, i, 0, 100.0 + i as f32 * 14.0, Some("1")),
            ));
        }
        // Region "2" stays clean (3 paragraphs, each 1 elem)
        for i in 0..3 {
            els.push(mk_element(
                ParsedElementType::Paragraph,
                &format!("R{i}"),
                4,
                100 + i,
                mk_placement_in_region(1, 1000 + i, 0, 100.0 + i as f32 * 14.0, Some("2")),
            ));
        }
        let out = run_rule(els, &cfg);
        // Region "1" → 1 collapsed node; region "2" → 3 separate nodes.
        assert_eq!(out.len(), 4);
    }

    /// Orphan elements (`region_label = None`) bucket as singletons — never
    /// merge with each other even when other placement fields collide.
    #[test]
    fn orphan_region_label_never_merges() {
        let cfg = crate::config::NodeTypeClusteringConfig::default();
        let els = vec![
            mk_element(
                ParsedElementType::Paragraph,
                "Orphan A",
                4,
                0,
                mk_placement_in_region(3, 0, 0, 100.0, None),
            ),
            mk_element(
                ParsedElementType::Paragraph,
                "Orphan B",
                4,
                1,
                mk_placement_in_region(3, 0, 0, 100.0, None),
            ),
        ];
        let out = run_rule(els, &cfg);
        assert_eq!(out.len(), 2);
    }

    /// CR-79 — a whole region tagged `Table` fuses into exactly ONE Table node
    /// whose bbox is the union, regardless of line / paragraph / Y-gap.
    /// `default_table()` drops every within-region gate, so multiple rows
    /// (distinct paragraph_number + a large inter-row Y-gap) collapse to one.
    #[test]
    fn table_region_fuses_to_one_node_with_union_bbox() {
        let cfg = crate::config::NodeTypeClusteringConfig::default();
        // Three "cells" in one region leaf "2-1": distinct paragraph numbers,
        // distinct lines, and a deliberately large Y-gap between rows. Under
        // the Paragraph default these would split; under Table they must fuse.
        let mut p_a = mk_placement_in_region(5, 0, 0, 100.0, Some("2-1"));
        p_a.bounding_box = BoundingBox {
            x: 0.0,
            y: 100.0,
            width: 50.0,
            height: 10.0,
        };
        let mut p_b = mk_placement_in_region(5, 1, 1, 300.0, Some("2-1"));
        p_b.bounding_box = BoundingBox {
            x: 60.0,
            y: 300.0,
            width: 50.0,
            height: 10.0,
        };
        let mut p_c = mk_placement_in_region(5, 2, 2, 500.0, Some("2-1"));
        p_c.bounding_box = BoundingBox {
            x: 20.0,
            y: 500.0,
            width: 80.0,
            height: 10.0,
        };

        let els = vec![
            mk_element(ParsedElementType::Table, "h1 h2", 3, 0, p_a),
            mk_element(ParsedElementType::Table, "a b", 3, 1, p_b),
            mk_element(ParsedElementType::Table, "c d", 3, 2, p_c),
        ];
        let out = run_rule(els, &cfg);
        assert_eq!(out.len(), 1, "whole table region must fuse into one node");
        assert_eq!(out[0].element_type, ParsedElementType::Table);

        // bbox = union of the three cells: x in [0, 110], y in [100, 510].
        let bb = &out[0].pdf_placement().bounding_box;
        assert!((bb.x - 0.0).abs() < 1e-3, "min x");
        assert!((bb.y - 100.0).abs() < 1e-3, "min y");
        assert!((bb.x + bb.width - 110.0).abs() < 1e-3, "max x = 60 + 50");
        assert!((bb.y + bb.height - 510.0).abs() < 1e-3, "max y = 500 + 10");
    }

    /// CR-79 — two stacked tables in *different* region leaves on the same page
    /// stay distinct (region IS the boundary), and a Table region never fuses
    /// with an adjacent Paragraph region (type differs).
    #[test]
    fn distinct_table_regions_and_paragraphs_stay_separate() {
        let cfg = crate::config::NodeTypeClusteringConfig::default();
        let els = vec![
            mk_element(
                ParsedElementType::Table,
                "table one",
                3,
                0,
                mk_placement_in_region(5, 0, 0, 100.0, Some("2-1")),
            ),
            mk_element(
                ParsedElementType::Table,
                "table two",
                3,
                1,
                mk_placement_in_region(5, 0, 0, 300.0, Some("3-1")),
            ),
            mk_element(
                ParsedElementType::Paragraph,
                "prose between",
                4,
                2,
                mk_placement_in_region(5, 0, 0, 200.0, Some("2-2")),
            ),
        ];
        let out = run_rule(els, &cfg);
        assert_eq!(out.len(), 3, "two table regions + one paragraph = 3 nodes");
        let tables = out
            .iter()
            .filter(|e| e.element_type == ParsedElementType::Table)
            .count();
        assert_eq!(tables, 2);
    }
}
