//! ParagraphClustering rule — Block 05b.
//!
//! Merges `ParsedPdfElement`s using the structural signals that flow through
//! `PdfTextElement.placement` (bands, columns, paragraph_number) rather than
//! spatial gap heuristics. The old `SpatialClustering` rule is preserved
//! unchanged; this rule replaces it in the default pipeline.
//!
//! ## Merge semantics
//!
//! Elements are partitioned into merge groups by a key derived from the active
//! merge level (controlled by `ParagraphClusteringConfig`):
//!
//! - `merge_segments` only  → key = (page, band, column, line_number, element_type)
//! - `merge_lines` (default) → key = (page, band, column, paragraph_number, element_type)
//! - `merge_columns`         → key = (page, band, element_type)
//! - `merge_bands`           → key = (page, element_type)
//!
//! Within each group elements are sorted by `reading_order` (stable), then merged
//! according to §7 of the Block 05b handoff spec.

use super::engine::{FontSizeAnalysis, ParseRule};
use crate::config::{ParsingConfig, ParagraphClusteringConfig};
use crate::types::{BoundingBox, FontClass, ParsedElementType, ParsedPdfElement};
use anyhow::Result;
use std::collections::HashMap;

// ─── Effective merge levels ───────────────────────────────────────────────────

/// Resolved merge scope after cascade promotion.
/// Higher levels imply all lower levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum MergeLevel {
    /// Merge segments on the same (band, column, line_number, element_type).
    Segments,
    /// Merge lines within the same (band, column, paragraph_number, element_type).
    Lines,
    /// Merge across columns within the same (band, element_type).
    Columns,
    /// Merge across bands within the same (page, element_type).
    Bands,
}

impl MergeLevel {
    /// Compute effective level from config, applying cascade promotion with warnings.
    fn resolve(cfg: &ParagraphClusteringConfig) -> Self {
        // Start from the requested top level and auto-promote lower levels as needed.
        if cfg.merge_bands {
            if !cfg.merge_columns {
                eprintln!(
                    "ParagraphClustering: merge_bands=true but merge_columns=false; \
                     auto-promoting merge_columns"
                );
            }
            if !cfg.merge_lines {
                eprintln!(
                    "ParagraphClustering: merge_bands=true but merge_lines=false; \
                     auto-promoting merge_lines"
                );
            }
            if !cfg.merge_segments {
                eprintln!(
                    "ParagraphClustering: merge_bands=true but merge_segments=false; \
                     auto-promoting merge_segments"
                );
            }
            return MergeLevel::Bands;
        }
        if cfg.merge_columns {
            if !cfg.merge_lines {
                eprintln!(
                    "ParagraphClustering: merge_columns=true but merge_lines=false; \
                     auto-promoting merge_lines"
                );
            }
            if !cfg.merge_segments {
                eprintln!(
                    "ParagraphClustering: merge_columns=true but merge_segments=false; \
                     auto-promoting merge_segments"
                );
            }
            return MergeLevel::Columns;
        }
        if cfg.merge_lines {
            if !cfg.merge_segments {
                eprintln!(
                    "ParagraphClustering: merge_lines=true but merge_segments=false; \
                     auto-promoting merge_segments"
                );
            }
            return MergeLevel::Lines;
        }
        if cfg.merge_segments {
            return MergeLevel::Segments;
        }
        // Nothing enabled — treat as no-merge (pass-through). Use Segments as the
        // lowest level but since there's only one segment per key it's a no-op.
        MergeLevel::Segments
    }
}

// ─── Partition key ────────────────────────────────────────────────────────────

/// A hashable key that uniquely identifies a merge partition.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PartitionKey {
    page: u32,
    band: u32,
    column: u32,
    paragraph_number: u32,
    // When merging at `Lines` level this is the paragraph_number.
    // When merging at `Segments` level this is the line_number (stored in paragraph_number slot
    // for simplicity — see build_key()).
    element_type: u8, // 0 = Section, 1 = Paragraph, 2+ = others (no cross-type merges)
}

fn element_type_ordinal(t: &ParsedElementType) -> u8 {
    match t {
        ParsedElementType::Section => 0,
        ParsedElementType::Paragraph => 1,
        ParsedElementType::List => 2,
        ParsedElementType::ListItem => 3,
    }
}

fn build_key(el: &ParsedPdfElement, level: MergeLevel) -> PartitionKey {
    let p = el.pdf_placement();
    match level {
        MergeLevel::Segments => PartitionKey {
            page: p.page_number,
            band: p.band,
            column: p.column,
            paragraph_number: p.line_number, // abuse the slot for line_number
            element_type: element_type_ordinal(&el.element_type),
        },
        MergeLevel::Lines => PartitionKey {
            page: p.page_number,
            band: p.band,
            column: p.column,
            paragraph_number: p.paragraph_number,
            element_type: element_type_ordinal(&el.element_type),
        },
        MergeLevel::Columns => PartitionKey {
            page: p.page_number,
            band: p.band,
            column: 0, // collapse columns
            paragraph_number: 0, // collapse paragraphs
            element_type: element_type_ordinal(&el.element_type),
        },
        MergeLevel::Bands => PartitionKey {
            page: p.page_number,
            band: 0, // collapse bands
            column: 0,
            paragraph_number: 0,
            element_type: element_type_ordinal(&el.element_type),
        },
    }
}

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
    let mut first_occurrence: HashMap<String, usize> = HashMap::new(); // class → position in sorted_elements

    for (pos, el) in sorted_elements.iter().enumerate() {
        let class = &el.style_info.class_name;
        *class_tokens.entry(class.clone()).or_insert(0) += el.token_count;
        first_occurrence.entry(class.clone()).or_insert(pos);
    }

    // Winner: highest token_count; ties broken by first occurrence (lower index wins).
    let winning_class = class_tokens
        .iter()
        .max_by(|(ca, ta), (cb, tb)| {
            ta.cmp(tb).then_with(|| {
                // Tie: prefer earlier first occurrence (lower index = earlier = wins).
                let ia = first_occurrence[*ca];
                let ib = first_occurrence[*cb];
                ib.cmp(&ia) // reverse: lower index → larger ordering → max picks it
            })
        })
        .map(|(class, _)| class.clone())
        .unwrap_or_else(|| sorted_elements[0].style_info.class_name.clone());

    // Return the full FontClass from the first element with the winning class.
    sorted_elements
        .iter()
        .find(|el| el.style_info.class_name == winning_class)
        .map(|el| el.style_info.clone())
        .unwrap_or_else(|| sorted_elements[0].style_info.clone())
}

// ─── Merge a group of sorted elements into a single element ──────────────────

fn merge_group(
    mut sorted_elements: Vec<ParsedPdfElement>,
    cfg: &ParagraphClusteringConfig,
) -> Option<ParsedPdfElement> {
    if sorted_elements.is_empty() {
        return None;
    }
    if sorted_elements.len() == 1 {
        let el = sorted_elements.remove(0);
        if el.text.trim().is_empty() {
            // Drop whitespace-only elements silently (debug-level event).
            return None;
        }
        return Some(el);
    }

    // --- Text concatenation ---
    let mut text = String::new();
    let mut prev_line_number: Option<u32> = None;
    for el in &sorted_elements {
        let line = el.pdf_placement().line_number;
        let nr_cols = el.pdf_placement().nr_band_columns;
        if !text.is_empty() {
            match prev_line_number {
                Some(prev) if prev == line => {
                    // Same line — no separator (segment merge)
                }
                _ => {
                    // Different line — prose or table separator
                    if nr_cols <= 2 {
                        text.push_str(&cfg.prose_line_separator);
                    } else {
                        text.push_str(&cfg.table_line_separator);
                    }
                }
            }
        }
        text.push_str(&el.text);
        prev_line_number = Some(line);
    }

    if text.trim().is_empty() {
        // All constituents were whitespace — drop silently (debug-level event).
        return None;
    }

    // --- Bbox union ---
    let mut union_bbox = sorted_elements[0].pdf_placement().bounding_box.clone();
    for el in &sorted_elements[1..] {
        union_bbox = bbox_union(&union_bbox, &el.pdf_placement().bounding_box);
    }

    // --- Style (majority by token_count, first-occurrence tiebreaker) ---
    let winning_style = majority_style(&sorted_elements);

    // --- reading_order = min ---
    let min_reading_order = sorted_elements
        .iter()
        .map(|el| el.reading_order)
        .min()
        .unwrap_or(0);

    // --- token_count = sum (recompute from merged text if significantly different) ---
    let sum_tokens: usize = sorted_elements.iter().map(|el| el.token_count).sum();
    // Use summed value as approximation; merged text token count not recomputed.
    let token_count = sum_tokens;

    // --- bookmark_match: first Some(...) ---
    let bookmark_match = sorted_elements
        .iter()
        .find_map(|el| el.bookmark_match.clone());

    // --- element_type: all same within partition ---
    let element_type = sorted_elements[0].element_type.clone();

    // --- placement: first element's, with bounding_box updated ---
    let mut merged_placement = sorted_elements[0].pdf_placement().clone();
    merged_placement.bounding_box = union_bbox;

    // --- position: first element's position (reading_order is the ordering key) ---
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
        token_count,
    })
}

// ─── Rule struct ──────────────────────────────────────────────────────────────

pub struct ParagraphClusteringRule<'a> {
    #[allow(dead_code)]
    engine: &'a super::engine::RuleEngine,
    #[allow(dead_code)]
    text_elements: &'a [crate::types::PdfTextElement],
    config: &'a ParsingConfig,
    #[allow(dead_code)]
    document_analysis: &'a crate::types::DocumentAnalysis,
    #[allow(dead_code)]
    font_size_analysis: &'a FontSizeAnalysis,
    #[allow(dead_code)]
    style_data: &'a crate::types::StyleData,
}

impl<'a> ParagraphClusteringRule<'a> {
    pub fn new(
        engine: &'a super::engine::RuleEngine,
        text_elements: &'a [crate::types::PdfTextElement],
        config: &'a ParsingConfig,
        document_analysis: &'a crate::types::DocumentAnalysis,
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

    pub fn apply(&self, elements: Vec<ParsedPdfElement>) -> Result<Vec<ParsedPdfElement>> {
        let cfg = &self.config.paragraph_clustering;
        let level = MergeLevel::resolve(cfg);

        println!(
            "   📦 ParagraphClustering: {} input elements, merge level = {:?}",
            elements.len(),
            level,
        );

        if elements.is_empty() {
            return Ok(elements);
        }

        // Partition elements by merge key.
        // We use a Vec<(PartitionKey, Vec<ParsedPdfElement>)> to preserve insertion order
        // (which is reading_order order from the pipeline) so that after sorting groups
        // by their representative reading_order the output is correctly sequenced.
        let mut partition_order: Vec<PartitionKey> = Vec::new();
        let mut partition_map: HashMap<PartitionKey, Vec<ParsedPdfElement>> = HashMap::new();

        for el in elements {
            let key = build_key(&el, level);
            if !partition_map.contains_key(&key) {
                partition_order.push(key.clone());
            }
            partition_map.entry(key).or_default().push(el);
        }

        // For each partition: sort by reading_order (stable), then merge.
        let mut merged: Vec<ParsedPdfElement> = Vec::with_capacity(partition_order.len());

        for key in &partition_order {
            let mut group = partition_map.remove(key).unwrap_or_default();
            // Stable sort within each group by reading_order.
            group.sort_by_key(|el| el.reading_order);
            if let Some(merged_el) = merge_group(group, cfg) {
                merged.push(merged_el);
            }
        }

        // Sort final output by reading_order so the caller gets a coherent sequence.
        merged.sort_by_key(|el| el.reading_order);

        println!(
            "   ✅ ParagraphClustering: {} output elements (from {} partitions)",
            merged.len(),
            partition_order.len(),
        );

        Ok(merged)
    }
}

impl<'a> ParseRule for ParagraphClusteringRule<'a> {
    fn apply(&self, elements: Vec<ParsedPdfElement>) -> Result<Vec<ParsedPdfElement>> {
        self.apply(elements)
    }

    fn name(&self) -> &str {
        "ParagraphClustering"
    }
}
