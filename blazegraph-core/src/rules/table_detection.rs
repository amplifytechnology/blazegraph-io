//! TableDetection rule — CR-79 (Tier 1 table-node detection).
//!
//! Marks **table regions** in the PDF channel as `ParsedElementType::Table`
//! so they emit as first-class `NodeType::Table` nodes (an *identifier* with a
//! bounding box — NOT a content extractor; cells stay flattened, that is Tier 2).
//!
//! ## How
//!
//! The discriminating signal exists per region leaf as
//! [`RegionSignature`](crate::analytics::page_stats::RegionSignature), computed
//! in `analytics/page_stats` and reachable via `DocumentAnalysis.page_stats`.
//! The metric is read, never re-derived. v2 (cv dropped — it was backwards:
//! reference lists are *more* regular than tables) ANDs two complementary
//! signals plus row/column counts:
//!
//! ```text
//! table  ⇔  n_peaks_y          >= min_rows               (multi-row)
//!       &&  aligned_cols       >= min_cols               (signal B: ≥2 columns
//!                                                          recur across rows)
//!       &&  grid_vcuts         >= min_grid_vcuts          (signal A: XY-cut
//!                                                          column grid absorbed)
//!       &&  column_consistency >= min_column_consistency  (signal B: columns
//!                                                          solidly filled)
//!       &&  density            >= min_density             (soft floor, default 0)
//! ```
//!
//! `apply()` builds a `(page, region_label) → &RegionSignature` lookup, then
//! tags every element whose leaf passes the metric `Table`. It **tags, it does
//! not merge** — downstream NodeTypeClustering fuses each Table-tagged region
//! into one node (bbox = union). Purely additive: nothing else is demoted or
//! re-typed.
//!
//! ## Off-wire debug dump
//!
//! When the rule runs it writes a per-doc sidecar JSON (CR-71A `evidence.json`
//! pattern) of per-leaf `{page, region_label, bbox, n_peaks, n_peaks_y,
//! y_peak_cv, density, is_table}` for every body leaf. This is the eyeball
//! tuning surface for `max_y_peak_cv` / `min_density`. It is **never** part of
//! `bgraph.md` or the wire. Gated by `BLAZEGRAPH_TABLE_DUMP` (default on); the
//! cache root comes from `BLAZEGRAPH_CACHE_DIR` (same as CR-71A).

use super::engine::{FontSizeAnalysis, ParseRule, RuleEngine};
use crate::analytics::page_stats::RegionSignature;
use crate::analytics::DocumentAnalysis;
use crate::config::{ParsingConfig, TableDetectionConfig};
use crate::types::{ParsedElementType, ParsedPdfElement, PdfTextElement, StyleData};
use anyhow::Result;
use serde::Serialize;
use std::collections::HashMap;

/// One row of the off-wire debug dump — one body leaf and its table verdict.
#[derive(Debug, Serialize)]
struct TableDumpLeaf {
    page: u32,
    region_label: String,
    bbox: TableDumpBbox,
    n_peaks: u32,
    n_peaks_y: u32,
    y_peak_cv: f32,
    grid_vcuts: u32,
    aligned_cols: u32,
    column_consistency: f32,
    density: f32,
    is_table: bool,
}

#[derive(Debug, Serialize)]
struct TableDumpBbox {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

/// Top-level shape of `<doc>.table.json`.
#[derive(Debug, Serialize)]
struct TableDumpArtifact {
    /// The thresholds in force for this run — so the dump is self-describing.
    config: TableDumpConfig,
    leaves: Vec<TableDumpLeaf>,
}

#[derive(Debug, Serialize)]
struct TableDumpConfig {
    min_rows: u32,
    min_cols: u32,
    min_grid_vcuts: u32,
    min_column_consistency: f32,
    min_density: f32,
}

pub struct TableDetectionRule<'a> {
    #[allow(dead_code)]
    engine: &'a RuleEngine,
    #[allow(dead_code)]
    text_elements: &'a [PdfTextElement],
    config: &'a ParsingConfig,
    document_analysis: &'a DocumentAnalysis,
    #[allow(dead_code)]
    font_size_analysis: &'a FontSizeAnalysis,
    #[allow(dead_code)]
    style_data: &'a StyleData,
}

impl<'a> TableDetectionRule<'a> {
    pub fn new(
        engine: &'a RuleEngine,
        text_elements: &'a [PdfTextElement],
        config: &'a ParsingConfig,
        document_analysis: &'a DocumentAnalysis,
        font_size_analysis: &'a FontSizeAnalysis,
        style_data: &'a StyleData,
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

    /// The CR-79 table metric. Pure function of a signature + the thresholds so
    /// it is unit-testable in isolation.
    pub fn is_table(sig: &RegionSignature, cfg: &TableDetectionConfig) -> bool {
        sig.n_peaks_y >= cfg.min_rows
            && sig.aligned_cols >= cfg.min_cols
            && sig.grid_vcuts >= cfg.min_grid_vcuts
            && sig.column_consistency >= cfg.min_column_consistency
            && sig.density >= cfg.min_density
    }

    /// Build the `(page, region_label) → &RegionSignature` lookup from the
    /// analytics pre-pass output.
    fn signature_lookup(&self) -> HashMap<(u32, &'a str), &'a RegionSignature> {
        self.document_analysis
            .page_stats
            .regions
            .iter()
            .map(|sig| ((sig.page_number, sig.region_label.as_str()), sig))
            .collect()
    }
}

impl<'a> ParseRule for TableDetectionRule<'a> {
    fn apply(&self, elements: Vec<ParsedPdfElement>) -> Result<Vec<ParsedPdfElement>> {
        let cfg = &self.config.table_detection;
        if !cfg.enabled {
            println!("   ⏭️  TableDetection disabled — passing through {} elements", elements.len());
            return Ok(elements);
        }

        let lookup = self.signature_lookup();

        // Tag pass: every element whose leaf passes the metric becomes Table.
        // Additive only — Header / Footer / Margin (and anything without a
        // body region_label) are left untouched.
        let mut tagged_regions: std::collections::HashSet<(u32, String)> =
            std::collections::HashSet::new();
        let out: Vec<ParsedPdfElement> = elements
            .into_iter()
            .map(|el| {
                // Only body leaves are candidates: must have a placement with a
                // region_label, and that leaf must be a Paragraph (the type
                // `from_region_label` assigns to body leaves). Never re-type
                // Header / Footer / Margin / Section.
                if el.element_type != ParsedElementType::Paragraph {
                    return el;
                }
                let Some(p) = el.placement.as_ref() else {
                    return el;
                };
                let Some(label) = p.region_label.as_deref() else {
                    return el;
                };
                if let Some(sig) = lookup.get(&(p.page_number, label)) {
                    if Self::is_table(sig, cfg) {
                        tagged_regions.insert((p.page_number, label.to_string()));
                        return ParsedPdfElement {
                            element_type: ParsedElementType::Table,
                            ..el
                        };
                    }
                }
                el
            })
            .collect();

        println!(
            "   📊 TableDetection: tagged {} region leaf/leaves as Table",
            tagged_regions.len()
        );

        // Off-wire debug dump — every body leaf, with its verdict.
        if std::env::var("BLAZEGRAPH_TABLE_DUMP")
            .map(|v| v != "0" && !v.eq_ignore_ascii_case("false"))
            .unwrap_or(true)
        {
            if let Err(e) = self.emit_debug_dump(&lookup, cfg) {
                // Non-fatal: the dump is a development surface, not the wire.
                eprintln!("   ⚠️  TableDetection: debug dump failed: {e}");
            }
        }

        Ok(out)
    }

    fn name(&self) -> &str {
        "TableDetection"
    }
}

impl<'a> TableDetectionRule<'a> {
    /// Write the off-wire `<doc>.table.json` debug artifact. Mirrors the
    /// CR-71A `evidence.json` path convention: cache root from
    /// `BLAZEGRAPH_CACHE_DIR` (default `cache`), one file under
    /// `{cache}/table/`. The stem is derived from the document metadata title
    /// when present, else `unknown` — the rule has no provenance handle.
    fn emit_debug_dump(
        &self,
        lookup: &HashMap<(u32, &'a str), &'a RegionSignature>,
        cfg: &TableDetectionConfig,
    ) -> std::io::Result<()> {
        // Deterministic order: page-major, then region_label.
        let mut sigs: Vec<&RegionSignature> = lookup.values().copied().collect();
        sigs.sort_by(|a, b| {
            a.page_number
                .cmp(&b.page_number)
                .then_with(|| a.region_label.cmp(&b.region_label))
        });

        let leaves: Vec<TableDumpLeaf> = sigs
            .iter()
            .map(|sig| TableDumpLeaf {
                page: sig.page_number,
                region_label: sig.region_label.clone(),
                bbox: TableDumpBbox {
                    x: sig.x_left,
                    y: sig.y_top,
                    width: (sig.x_right - sig.x_left).max(0.0),
                    height: (sig.y_bottom - sig.y_top).max(0.0),
                },
                n_peaks: sig.n_peaks,
                n_peaks_y: sig.n_peaks_y,
                y_peak_cv: sig.y_peak_cv,
                grid_vcuts: sig.grid_vcuts,
                aligned_cols: sig.aligned_cols,
                column_consistency: sig.column_consistency,
                density: sig.density,
                is_table: Self::is_table(sig, cfg),
            })
            .collect();

        let artifact = TableDumpArtifact {
            config: TableDumpConfig {
                min_rows: cfg.min_rows,
                min_cols: cfg.min_cols,
                min_grid_vcuts: cfg.min_grid_vcuts,
                min_column_consistency: cfg.min_column_consistency,
                min_density: cfg.min_density,
            },
            leaves,
        };

        let cache_root =
            std::env::var("BLAZEGRAPH_CACHE_DIR").unwrap_or_else(|_| "cache".to_string());
        let dir = format!("{cache_root}/table");
        std::fs::create_dir_all(&dir)?;
        let stem = self.doc_stem();
        let path = format!("{dir}/{stem}.table.json");
        let json = serde_json::to_string_pretty(&artifact).map_err(std::io::Error::other)?;
        std::fs::write(&path, json)?;
        println!("🧾 CR-79: table debug dump → {path}");
        Ok(())
    }

    /// File stem for the debug artifact. The rule has no provenance handle, so
    /// it uses the document title from metadata when available, else `unknown`.
    /// Sanitized to a filesystem-safe token.
    fn doc_stem(&self) -> String {
        // `BLAZEGRAPH_TABLE_DUMP_STEM` lets the CLI/harness pin the filename to
        // the source stem (the rule otherwise can't see the source filename).
        let raw = std::env::var("BLAZEGRAPH_TABLE_DUMP_STEM").unwrap_or_else(|_| "unknown".to_string());
        sanitize_stem(&raw)
    }
}

/// Keep ASCII alphanumerics, `-`, `_`, `.`; replace anything else with `_`.
fn sanitize_stem(s: &str) -> String {
    let out: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if out.is_empty() {
        "unknown".to_string()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{BoundingBox, FontClass, Placement};

    fn cfg() -> TableDetectionConfig {
        TableDetectionConfig::default()
    }

    fn table_sig(page: u32, label: &str) -> RegionSignature {
        RegionSignature {
            page_number: page,
            region_label: label.to_string(),
            n_peaks: 3,
            n_peaks_y: 5,            // multi-row
            grid_vcuts: 2,           // signal A: absorbed a column grid
            aligned_cols: 3,         // signal B: 3 columns recur across rows
            column_consistency: 0.9, // solidly filled
            density: 0.6,
            ..RegionSignature::default()
        }
    }

    fn prose_sig(page: u32, label: &str) -> RegionSignature {
        RegionSignature {
            page_number: page,
            region_label: label.to_string(),
            n_peaks: 1,
            n_peaks_y: 12,
            grid_vcuts: 0,           // no column grid
            aligned_cols: 0,         // single column
            column_consistency: 0.0,
            density: 0.5,
            ..RegionSignature::default()
        }
    }

    #[test]
    fn table_shaped_signature_passes_metric() {
        assert!(TableDetectionRule::is_table(&table_sig(1, "2-1"), &cfg()));
    }

    #[test]
    fn prose_signature_fails_metric() {
        assert!(!TableDetectionRule::is_table(&prose_sig(1, "1"), &cfg()));
    }

    #[test]
    fn fewer_than_min_aligned_columns_fails() {
        // Only one column recurs across rows (a reference entry's indent) →
        // not a table. This is the v2 fix for the references false-positive class.
        let mut s = table_sig(1, "2-1");
        s.aligned_cols = 1;
        assert!(!TableDetectionRule::is_table(&s, &cfg()));
    }

    #[test]
    fn no_grid_collapse_fails() {
        // Signal A: a band the XY-cut merge never collapsed (no column gutter
        // absorbed) is not a table even if its elements appear to align.
        let mut s = table_sig(1, "2-1");
        s.grid_vcuts = 0;
        assert!(!TableDetectionRule::is_table(&s, &cfg()));
    }

    #[test]
    fn low_column_consistency_fails() {
        // Signal B: columns that don't recur solidly across rows (a reference
        // entry's ragged indent) fail the consistency floor.
        let mut s = table_sig(1, "2-1");
        s.column_consistency = 0.3;
        assert!(!TableDetectionRule::is_table(&s, &cfg()));
    }

    fn test_font_class() -> FontClass {
        FontClass {
            class_name: "f1".to_string(),
            font_family: "TestFont".to_string(),
            font_size: 10.0,
            font_style: "normal".to_string(),
            font_weight: "normal".to_string(),
            color: "#000000".to_string(),
        }
    }

    fn paragraph_el(position: usize, page: u32, label: &str, reading_order: u32) -> ParsedPdfElement {
        ParsedPdfElement {
            element_type: ParsedElementType::Paragraph,
            text: format!("cell {position}"),
            hierarchy_level: 3,
            position,
            style_info: test_font_class(),
            placement: Some(Placement {
                page_number: page,
                bounding_box: BoundingBox {
                    x: position as f32 * 10.0,
                    y: position as f32 * 10.0,
                    width: 10.0,
                    height: 8.0,
                },
                line_number: position as u32,
                segment_number: 0,
                rotation: 0,
                paragraph_number: position as u32,
                region_label: Some(label.to_string()),
                page_width: 612.0,
                page_height: 792.0,
            }),
            reading_order,
            bookmark_match: None,
            token_count: 2,
            links: vec![],
            confidence: 0,
        }
    }

    #[test]
    fn apply_tags_only_table_region_elements() {
        // Two leaves on page 1: "2-1" is table-shaped, "1" is prose. Every
        // element in "2-1" should become Table; "1" stays Paragraph.
        let mut analysis = DocumentAnalysis::default();
        analysis.page_stats.regions = vec![table_sig(1, "2-1"), prose_sig(1, "1")];

        let engine = RuleEngine::new().expect("engine");
        let style_data = StyleData {
            font_classes: std::collections::BTreeMap::new(),
        };
        let fsa = FontSizeAnalysis::default();
        let parsing = ParsingConfig::default();

        let rule = TableDetectionRule::new(
            &engine,
            &[],
            &parsing,
            &analysis,
            &fsa,
            &style_data,
        );

        let elements = vec![
            paragraph_el(0, 1, "2-1", 0),
            paragraph_el(1, 1, "2-1", 1),
            paragraph_el(2, 1, "1", 2),
        ];
        let out = rule.apply(elements).expect("apply ok");

        assert_eq!(out[0].element_type, ParsedElementType::Table);
        assert_eq!(out[1].element_type, ParsedElementType::Table);
        assert_eq!(out[2].element_type, ParsedElementType::Paragraph);
    }
}
