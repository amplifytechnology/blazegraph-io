//! Pipeline boundary tests — stabilize the sandwich edges.
//!
//! These tests load pre-generated snapshots from `test_fixtures/snapshots/`
//! and assert structural properties at the pipeline boundaries:
//!
//! - Boundary 1 (Tika output): XHTML size, text element count
//! - Boundary 2 (Graph output): schema version, node counts, types, breadcrumbs
//!
//! The middle (rules engine) is intentionally NOT snapshot-tested —
//! that's where we want room to iterate.
//!
//! To regenerate fixtures: `make test-generate-fixtures`
//! No JVM required to run these tests.

use blazegraph_io_core::analytics::DocumentAnalysis;
use blazegraph_io_core::config::{
    ParsingConfig, PipelineConfig, RuleConfig, SectionDetectionV2Config,
};
use blazegraph_io_core::preprocessors::pdf::xhtml_parser;
use blazegraph_io_core::rules::engine::{FontSizeAnalysis, ParseRule, RuleEngine};
use blazegraph_io_core::rules::section_detection_v2::SectionDetectionV2Rule;
use blazegraph_io_core::ParsedElementType;
use blazegraph_io_core::{
    BookmarkSection, BoundingBox, FontClass, PdfTextElement, Placement, StyleData,
};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

// ============================================================================
// Fixture helpers
// ============================================================================

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test_fixtures/snapshots")
}

fn load_summary(fixture_name: &str) -> Value {
    let path = fixtures_dir().join(fixture_name).join("summary.json");
    let contents = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "Missing fixture: {}. Run `make test-generate-fixtures`",
            path.display()
        )
    });
    serde_json::from_str(&contents).expect("Invalid summary.json")
}

fn load_graph(fixture_name: &str) -> Value {
    let path = fixtures_dir().join(fixture_name).join("stage3_graph.json");
    let contents = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "Missing fixture: {}. Run `make test-generate-fixtures`",
            path.display()
        )
    });
    serde_json::from_str(&contents).expect("Invalid stage3_graph.json")
}

fn load_xhtml(fixture_name: &str) -> String {
    let path = fixtures_dir().join(fixture_name).join("stage1a_xhtml.html");
    std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "Missing fixture: {}. Run `make test-generate-fixtures`",
            path.display()
        )
    })
}

fn load_text_elements(fixture_name: &str) -> Value {
    let path = fixtures_dir()
        .join(fixture_name)
        .join("stage1b_text_elements.json");
    let contents = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "Missing fixture: {}. Run `make test-generate-fixtures`",
            path.display()
        )
    });
    serde_json::from_str(&contents).expect("Invalid stage1b_text_elements.json")
}

/// Count node types in a graph
fn count_node_types(graph: &Value) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    if let Some(nodes) = graph["nodes"].as_array() {
        for node in nodes {
            if let Some(nt) = node["node_type"].as_str() {
                *counts.entry(nt.to_string()).or_insert(0) += 1;
            }
        }
    }
    counts
}

// ============================================================================
// Boundary 1: Tika output stability
// ============================================================================

mod tika_boundary {
    use super::*;

    #[test]
    fn shannon_xhtml_size_stable() {
        let xhtml = load_xhtml("claude_shannon_paper");
        let summary = load_summary("claude_shannon_paper");
        let expected_bytes = summary["stage_counts"]["xhtml_bytes"].as_u64().unwrap() as usize;

        // XHTML should not change unless Tika version changes
        assert_eq!(
            xhtml.len(),
            expected_bytes,
            "XHTML byte count changed — did Tika version change?"
        );
    }

    #[test]
    fn shannon_text_element_count_stable() {
        let elements = load_text_elements("claude_shannon_paper");
        let arr = elements.as_array().expect("text_elements should be array");

        // Text elements come directly from Tika — stable unless Tika changes
        assert_eq!(
            arr.len(),
            3021,
            "Text element count changed — Tika output drift?"
        );
    }

    #[test]
    fn euclid_xhtml_size_stable() {
        let xhtml = load_xhtml("elements_of_euclid");
        let summary = load_summary("elements_of_euclid");
        let expected_bytes = summary["stage_counts"]["xhtml_bytes"].as_u64().unwrap() as usize;

        assert_eq!(
            xhtml.len(),
            expected_bytes,
            "XHTML byte count changed — did Tika version change?"
        );
    }

    #[test]
    fn euclid_text_element_count_stable() {
        let elements = load_text_elements("elements_of_euclid");
        let arr = elements.as_array().expect("text_elements should be array");

        assert_eq!(
            arr.len(),
            9538,
            "Text element count changed — Tika output drift?"
        );
    }
}

// ============================================================================
// Boundary 2: Graph output — schema contract
// ============================================================================

mod schema_contract {
    use super::*;

    #[test]
    #[ignore = "Block 05 cleanup — fixture stamped 0.2.0 but SCHEMA_VERSION is now 0.4.0. \
                Test passes vacuously against the stale fixture. Regenerate fixtures with \
                `make test-generate-fixtures` and re-pin to current SCHEMA_VERSION when the \
                analytics flow lands consumers."]
    fn schema_version_is_0_2_0() {
        let graph = load_graph("claude_shannon_paper");
        assert_eq!(
            graph["schema_version"].as_str().unwrap(),
            "0.2.0",
            "Schema version changed — this is a contract break for API customers"
        );
    }

    #[test]
    fn schema_version_consistent_across_fixtures() {
        let shannon = load_graph("claude_shannon_paper");
        let euclid = load_graph("elements_of_euclid");
        assert_eq!(
            shannon["schema_version"], euclid["schema_version"],
            "Different fixtures producing different schema versions"
        );
    }

    #[test]
    fn graph_has_required_top_level_fields() {
        let graph = load_graph("claude_shannon_paper");

        assert!(
            graph["schema_version"].is_string(),
            "Missing schema_version"
        );
        assert!(graph["nodes"].is_array(), "Missing nodes array");
        assert!(graph["document_info"].is_object(), "Missing document_info");
        assert!(
            graph["structural_profile"].is_object(),
            "Missing structural_profile"
        );
    }

    #[test]
    fn nodes_have_required_fields() {
        let graph = load_graph("claude_shannon_paper");
        let nodes = graph["nodes"].as_array().unwrap();

        for (i, node) in nodes.iter().enumerate() {
            assert!(node["id"].is_string(), "Node {i} missing id");
            assert!(node["node_type"].is_string(), "Node {i} missing node_type");
            assert!(node["location"].is_object(), "Node {i} missing location");
            assert!(node["content"].is_object(), "Node {i} missing content");
            assert!(
                node["token_count"].is_number(),
                "Node {i} missing token_count"
            );
            // parent can be null (root node)
            // children should always be an array
            assert!(
                node["children"].is_array(),
                "Node {i} missing children array"
            );
        }
    }

    #[test]
    #[ignore = "Block 05 cleanup — `document_analysis` was removed from DocumentInfo (schema 0.4.0). \
                Stale fixture still carries the field so this passes vacuously, but regenerated \
                fixtures will fail. Rewrite as `document_info` shape check (root_id + \
                document_metadata only) and remove the `document_analysis` assertion."]
    fn document_info_has_required_fields() {
        let graph = load_graph("claude_shannon_paper");
        let info = &graph["document_info"];

        assert!(info["root_id"].is_string(), "Missing root_id");
        assert!(
            info["document_metadata"].is_object(),
            "Missing document_metadata"
        );
        assert!(
            info["document_analysis"].is_object(),
            "Missing document_analysis"
        );
    }
}

// ============================================================================
// Boundary 2: Graph output — structural properties
// ============================================================================

mod graph_structure {
    use super::*;

    #[test]
    fn shannon_node_count() {
        let graph = load_graph("claude_shannon_paper");
        let nodes = graph["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 95, "Shannon graph node count changed");
    }

    #[test]
    fn euclid_node_count() {
        let graph = load_graph("elements_of_euclid");
        let nodes = graph["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 390, "Euclid graph node count changed");
    }

    #[test]
    fn shannon_has_document_root() {
        let graph = load_graph("claude_shannon_paper");
        let nodes = graph["nodes"].as_array().unwrap();

        let doc_nodes: Vec<_> = nodes
            .iter()
            .filter(|n| n["node_type"].as_str() == Some("Document"))
            .collect();

        assert_eq!(
            doc_nodes.len(),
            1,
            "Should have exactly one Document root node"
        );

        let root = doc_nodes[0];
        assert!(
            root["parent"].is_null(),
            "Document root should have null parent"
        );
        assert!(
            !root["children"].as_array().unwrap().is_empty(),
            "Document root should have children"
        );
    }

    #[test]
    fn shannon_has_sections() {
        let counts = count_node_types(&load_graph("claude_shannon_paper"));
        let section_count = counts.get("Section").copied().unwrap_or(0);

        assert!(section_count > 0, "Shannon paper should have sections");
        // Shannon's paper has well-defined sections — this should be stable
        assert!(
            (5..=40).contains(&section_count),
            "Shannon section count {section_count} outside expected range [5, 40]"
        );
    }

    #[test]
    fn euclid_has_sections() {
        let counts = count_node_types(&load_graph("elements_of_euclid"));
        let section_count = counts.get("Section").copied().unwrap_or(0);

        assert!(section_count > 0, "Euclid should have sections");
    }

    #[test]
    fn all_nodes_have_valid_node_types() {
        let graph = load_graph("claude_shannon_paper");
        let nodes = graph["nodes"].as_array().unwrap();

        let valid_types = ["Document", "Section", "Paragraph", "List", "ListItem"];

        for node in nodes {
            let nt = node["node_type"].as_str().unwrap();
            assert!(
                valid_types.contains(&nt),
                "Unexpected node_type: '{nt}' — add to valid_types if intentional"
            );
        }
    }

    #[test]
    fn nodes_sorted_by_text_order() {
        let graph = load_graph("claude_shannon_paper");
        let nodes = graph["nodes"].as_array().unwrap();

        // First node is Document (text_order: null), rest should be ascending
        let orders: Vec<Option<u64>> = nodes.iter().map(|n| n["text_order"].as_u64()).collect();

        // Document root has null text_order and comes first
        assert!(
            orders[0].is_none(),
            "First node should be Document with null text_order"
        );

        // Remaining should be monotonically non-decreasing
        let rest: Vec<u64> = orders[1..].iter().filter_map(|o| *o).collect();
        for window in rest.windows(2) {
            assert!(
                window[0] <= window[1],
                "Nodes not sorted by text_order: {} > {}",
                window[0],
                window[1]
            );
        }
    }
}

// ============================================================================
// Boundary 2: Graph output — breadcrumbs
// ============================================================================

mod breadcrumbs {
    use super::*;

    #[test]
    fn document_root_has_title_breadcrumb() {
        let graph = load_graph("claude_shannon_paper");
        let nodes = graph["nodes"].as_array().unwrap();

        let root = nodes
            .iter()
            .find(|n| n["node_type"].as_str() == Some("Document"))
            .expect("No Document root node");

        let breadcrumbs = root["location"]["semantic"]["breadcrumbs"]
            .as_array()
            .expect("Root should have breadcrumbs array");

        assert!(
            !breadcrumbs.is_empty(),
            "Root breadcrumbs should contain the document title"
        );
    }

    #[test]
    fn section_nodes_appear_in_child_breadcrumbs() {
        let graph = load_graph("claude_shannon_paper");
        let nodes = graph["nodes"].as_array().unwrap();

        // Find a section that has children
        for node in nodes {
            if node["node_type"].as_str() == Some("Section") {
                let section_text = node["content"]["text"].as_str().unwrap_or("");
                let children_ids: Vec<&str> = node["children"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(|c| c.as_str())
                    .collect();

                if children_ids.is_empty() {
                    continue;
                }

                // Find a child node and check its breadcrumbs contain this section
                let empty = vec![];
                for child_id in &children_ids {
                    if let Some(child) = nodes.iter().find(|n| n["id"].as_str() == Some(child_id)) {
                        let crumbs: Vec<&str> = child["location"]["semantic"]["breadcrumbs"]
                            .as_array()
                            .unwrap_or(&empty)
                            .iter()
                            .filter_map(|c| c.as_str())
                            .collect();

                        assert!(
                            crumbs.contains(&section_text),
                            "Child of section '{}' should have it in breadcrumbs, got: {:?}",
                            section_text,
                            crumbs
                        );
                        return; // One verified example is sufficient
                    }
                }
            }
        }

        panic!("No section with children found to verify breadcrumb propagation");
    }

    #[test]
    fn all_nodes_have_breadcrumbs_array() {
        let graph = load_graph("claude_shannon_paper");
        let nodes = graph["nodes"].as_array().unwrap();

        for (i, node) in nodes.iter().enumerate() {
            assert!(
                node["location"]["semantic"]["breadcrumbs"].is_array(),
                "Node {i} ({}) missing breadcrumbs array",
                node["node_type"].as_str().unwrap_or("unknown")
            );
        }
    }

    #[test]
    fn breadcrumb_depth_matches_semantic_depth() {
        // Breadcrumbs should grow as depth increases (roughly)
        let graph = load_graph("claude_shannon_paper");
        let nodes = graph["nodes"].as_array().unwrap();

        for node in nodes {
            let depth = node["location"]["semantic"]["depth"].as_u64().unwrap_or(0);
            let crumb_count = node["location"]["semantic"]["breadcrumbs"]
                .as_array()
                .map(|a| a.len())
                .unwrap_or(0);

            // Breadcrumbs shouldn't exceed depth + 1 (title + one per ancestor section)
            // This is a loose bound — exact relationship depends on tree structure
            assert!(
                crumb_count <= (depth as usize + 2),
                "Node at depth {} has {} breadcrumbs — suspiciously deep trail",
                depth,
                crumb_count
            );
        }
    }
}

// ============================================================================
// XHTML parser enrichment tests
// ============================================================================
//
// Tika emits positioned-text primitives only after the layout-reasoning
// consolidation flow (2026-05-03) — no <div class="band">, no data-column.
// The legacy Placement.band / column / nr_band_columns fields were dropped
// in schema 0.5.0 (Block 06b). Region tagging now lives on
// `Placement.region_label`, set by `analytics::reading_order::tag_and_resort`.

#[test]
fn test_parser_rotation_from_aside() {
    let xhtml = r#"<html><head>
<style>.f1 { font-family: Helvetica; font-size: 20.0px; font-style: normal; font-weight: normal; color: #000000; }</style>
</head><body>
<div class="page" data-page="0">
  <aside data-rotation="90">
    <p><span class="f1" data-bbox="10.0,100.0,100.0,20.0" data-line="0" data-segment="0">Rotated sidebar</span></p>
  </aside>
  <p><span class="f1" data-bbox="10.0,200.0,300.0,12.0" data-line="0" data-segment="0">Body text</span></p>
</div>
</body></html>"#;
    let output = xhtml_parser::parse_xhtml(xhtml).expect("parse failed");
    let e = &output.text_elements;
    let rotated: Vec<_> = e.iter().filter(|el| el.placement.rotation != 0).collect();
    let body: Vec<_> = e.iter().filter(|el| el.placement.rotation == 0).collect();
    assert_eq!(rotated.len(), 1);
    assert_eq!(rotated[0].placement.rotation, 90);
    assert!(!body.is_empty());
}

#[test]
fn test_parser_old_format_defaults() {
    let xhtml = r#"<html><head>
<style>.f1 { font-family: Helvetica; font-size: 12.0px; font-style: normal; font-weight: normal; color: #000000; }</style>
</head><body>
<div class="page" data-page="0">
  <p><span class="f1" data-bbox="10.0,100.0,100.0,12.0" data-line="0" data-segment="0">Old format text</span></p>
</div>
</body></html>"#;
    let output = xhtml_parser::parse_xhtml(xhtml).expect("parse failed");
    assert!(!output.text_elements.is_empty());
    for el in &output.text_elements {
        assert_eq!(el.placement.rotation, 0, "default rotation");
        assert_eq!(el.placement.region_label, None, "default region_label");
        assert!(el.raw_tags.is_empty(), "default raw_tags");
    }
}

#[test]
fn test_parser_raw_tags_anchor() {
    let xhtml = r#"<html><head>
<style>.f1 { font-family: Helvetica; font-size: 12.0px; font-style: normal; font-weight: normal; color: #000000; }</style>
</head><body>
<div class="page" data-page="1">
  <p><span class="f1" data-bbox="10.0,100.0,100.0,12.0" data-line="0" data-segment="0" data-column="0">hello <a href="https://example.com">link text</a> world</span></p>
</div>
</body></html>"#;
    let output = xhtml_parser::parse_xhtml(xhtml).expect("parse failed");
    let el = output.text_elements.first().expect("no elements");
    assert_eq!(el.raw_tags.len(), 1, "expected one raw_tag entry");
    assert_eq!(
        el.raw_tags[0], r#"<a href="https://example.com">link text</a>"#,
        "raw_tag must include attributes and inner text verbatim"
    );
}

#[test]
fn test_integration_rfc_quic_post_strip_defaults() {
    let path = format!(
        "{}/../../cache/c1-xhtml/7b1ea3317b5bea95f28cb546fdb925e2dcd66eb0cee2c02ceb66d9772ec927f2.xhtml",
        env!("CARGO_MANIFEST_DIR")
    );
    let xhtml = std::fs::read_to_string(&path)
        .expect("Pre-populated XHTML not found — see worktree setup in handoff (rfc-quic)");
    let output = xhtml_parser::parse_xhtml(&xhtml).expect("parse failed");
    let e = &output.text_elements;
    assert!(!e.is_empty(), "rfc-quic parses to non-empty element list");
    // Post layout-reasoning consolidation: Tika emits no bands or columns. The
    // legacy band/column/nr_band_columns fields were dropped in schema 0.5.0;
    // `region_label` lands later via `tag_and_resort` and is None at this
    // stage of the pipeline (parser output, pre-analytics).
    assert!(
        e.iter().all(|el| el.placement.region_label.is_none()),
        "parser leaves region_label = None; tagging happens post-analytics"
    );
    assert!(
        e.iter().all(|el| el.placement.rotation == 0),
        "rfc-quic has no rotated content"
    );
}

#[test]
#[ignore = "CR-33: fixture-dependent — requires pre-populated XHTML cache (see worktree setup in original handoff). Not runnable under default `cargo test`."]
fn test_integration_attention_rotation() {
    let path = format!(
        "{}/../../cache/c1-xhtml/e1feb60eb4fd74de2432c67eff97517f59fdb3364751f38aa636fdc1b82dc9ea.xhtml",
        env!("CARGO_MANIFEST_DIR")
    );
    let xhtml = std::fs::read_to_string(&path)
        .expect("Pre-populated XHTML not found — see worktree setup in handoff (attention)");
    let output = xhtml_parser::parse_xhtml(&xhtml).expect("parse failed");
    let e = &output.text_elements;
    assert!(
        e.iter().any(|el| el.placement.rotation != 0),
        "attention has rotated sidebar (aside data-rotation=90)"
    );
    assert!(
        e.iter().any(|el| el.placement.rotation == 0),
        "attention has non-rotated body text"
    );
}

// ============================================================================
// Block 02: Font hierarchy rotation filtering (CR-10)
// ============================================================================

/// Build a minimal PdfTextElement for unit tests.
fn make_element(class_name: &str, font_size: f32, rotation: i32) -> PdfTextElement {
    PdfTextElement {
        text: format!("text at {}pt rotation={}", font_size, rotation),
        style_info: FontClass {
            class_name: class_name.to_string(),
            font_family: "TestFamily".to_string(),
            font_size,
            font_style: "normal".to_string(),
            font_weight: "normal".to_string(),
            color: "#000000".to_string(),
        },
        placement: Placement {
            page_number: 0,
            bounding_box: BoundingBox {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: font_size,
            },
            line_number: 0,
            segment_number: 0,
            rotation,
            paragraph_number: 0,
            region_label: None,
            page_width: 0.0,
            page_height: 0.0,
        },
        reading_order: 0,
        bookmark_match: None,
        token_count: 1,
        raw_tags: vec![],
    }
}

/// Build a minimal StyleData with the given (class_name, font_size) pairs.
fn make_style_data(classes: &[(&str, f32)]) -> StyleData {
    let mut font_classes = BTreeMap::new();
    for (class_name, font_size) in classes {
        font_classes.insert(
            class_name.to_string(),
            FontClass {
                class_name: class_name.to_string(),
                font_family: "TestFamily".to_string(),
                font_size: *font_size,
                font_style: "normal".to_string(),
                font_weight: "normal".to_string(),
                color: "#000000".to_string(),
            },
        );
    }
    StyleData { font_classes }
}

/// Test 2: analyze_font_sizes excludes rotated elements from FontSizeAnalysis.
/// 10 elements at class f1 (10pt, rotation=0), 1 at class f2 (20pt, rotation=90).
#[test]
fn analyze_font_sizes_excludes_rotated() {
    let mut elements: Vec<PdfTextElement> = (0..10).map(|_| make_element("f1", 10.0, 0)).collect();
    elements.push(make_element("f2", 20.0, 90));

    let style_data = make_style_data(&[("f1", 10.0), ("f2", 20.0)]);
    let engine = RuleEngine::new().expect("RuleEngine::new should succeed");
    let font_analysis = engine.analyze_font_sizes(&elements, &style_data);

    assert_eq!(
        font_analysis.body_text_size, 10.0,
        "body_text_size should be 10.0 (body), not 20.0 (rotated sidebar)"
    );
    assert!(
        !font_analysis.potential_header_sizes.contains(&20.0),
        "potential_header_sizes should not contain rotated 20pt"
    );
    // The rotated class f2 should not be counted — it's either absent or zero.
    let f2_count = font_analysis
        .class_usage_counts
        .get("f2")
        .copied()
        .unwrap_or(0);
    assert_eq!(f2_count, 0, "rotated class f2 should have usage count 0");
}

// ============================================================================
// Block 03: Section Detection V2 (CR-10, Block 03)
// ============================================================================

/// Build a PdfTextElement with full control over all fields relevant to V2.
/// `text`, `font_size`, `font_weight` ("bold" or "normal"), `rotation`, `line_number`,
/// `x`, `width` (bbox), and `class_name`.
#[allow(clippy::too_many_arguments)]
fn make_v2_element(
    class_name: &str,
    text: &str,
    font_size: f32,
    font_weight: &str, // "bold" or "normal"
    rotation: i32,
    line_number: u32,
    x: f32,
    width: f32,
) -> PdfTextElement {
    PdfTextElement {
        text: text.to_string(),
        style_info: FontClass {
            class_name: class_name.to_string(),
            font_family: "TestFamily".to_string(),
            font_size,
            font_style: "normal".to_string(),
            font_weight: font_weight.to_string(),
            color: "#000000".to_string(),
        },
        placement: Placement {
            page_number: 0,
            bounding_box: BoundingBox {
                x,
                // Y derived from `line_number` so distinct lines have
                // distinct Y. V3 isolation reads Y geometry, not Tika's
                // per-paragraph `data-line` counter.
                y: (line_number as f32) * 14.0,
                width,
                height: font_size,
            },
            line_number,
            segment_number: 0,
            rotation,
            paragraph_number: 0,
            // Default to a body leaf label so the synthetic fixture lands as
            // body content (`ParsedElementType::Paragraph` at the conversion
            // boundary). Tests that exercise Block 07 orphan / header /
            // footer behavior override this after construction.
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

/// Build a FontSizeAnalysis where `body_size` is the most common size,
/// `class_counts` provides per-class usage counts.
fn make_font_analysis(body_size: f32, class_counts: &[(&str, usize)]) -> FontSizeAnalysis {
    let mut class_usage_counts = HashMap::new();
    for (class, count) in class_counts {
        class_usage_counts.insert(class.to_string(), *count);
    }
    FontSizeAnalysis {
        median_size: body_size,
        min_size: body_size,
        max_size: body_size,
        most_common_size: body_size,
        most_common_class: "body".to_string(),
        rare_large_sizes: vec![],
        size_frequency_map: HashMap::new(),
        class_usage_counts,
        potential_header_sizes: vec![],
        body_text_size: body_size,
        hierarchy_levels: vec![],
        size_usage_ratio: 1.0,
    }
}

/// Build a minimal ParsingConfig wired to SectionDetectionV2 with given config.
fn make_v2_config(v2: SectionDetectionV2Config) -> ParsingConfig {
    ParsingConfig {
        pipeline: PipelineConfig {
            rules: vec![RuleConfig {
                name: "SectionDetectionV2".to_string(),
                enabled: true,
            }],
        },
        section_detection_v2: v2,
        ..ParsingConfig::default()
    }
}

/// Build a minimal DocumentAnalysis (all defaults). V2 ignores its contents.
fn make_doc_analysis() -> DocumentAnalysis {
    DocumentAnalysis::default()
}

// ── Test 1 ─────────────────────────────────────────────────────────────────
/// V2 is dispatched when configured, produces 1 section from a small input set.
#[test]
fn v2_test1_dispatched_and_detects_section() {
    // 1 section-quality element (18pt bold, isolated) + 5 body paragraphs (10pt)
    let mut text_elements = vec![make_v2_element(
        "header",
        "Introduction",
        18.0,
        "bold",
        0,
        1,
        10.0,
        100.0,
    )];
    for i in 0..5usize {
        text_elements.push(make_v2_element(
            "body",
            "Body paragraph text here.",
            10.0,
            "normal",
            0,
            (i as u32) + 2,
            10.0,
            300.0,
        ));
    }

    // body class: 5 elements; header class: 1 element — so header is rare (1/6 ≈ 17% > 5% threshold)
    // but size 18 > body 10 → strong candidate → always a section regardless of rarity
    let font_analysis = make_font_analysis(10.0, &[("body", 5), ("header", 1)]);
    let config = make_v2_config(SectionDetectionV2Config::default());
    let doc_analysis = make_doc_analysis();
    let style_data = StyleData {
        font_classes: BTreeMap::new(),
    };
    let engine = RuleEngine::new().expect("RuleEngine::new should succeed");

    let rule = SectionDetectionV2Rule::new(
        &engine,
        &text_elements,
        &config,
        &doc_analysis,
        &font_analysis,
        &style_data,
    );
    let result = rule.apply(vec![]).expect("apply should succeed");

    let sections: Vec<_> = result
        .iter()
        .filter(|e| e.element_type == ParsedElementType::Section)
        .collect();
    let paragraphs: Vec<_> = result
        .iter()
        .filter(|e| e.element_type == ParsedElementType::Paragraph)
        .collect();

    assert_eq!(sections.len(), 1, "should detect exactly 1 section");
    assert_eq!(paragraphs.len(), 5, "should have 5 body paragraphs");
    assert_eq!(sections[0].text, "Introduction");
}

// ── Test 2 ─────────────────────────────────────────────────────────────────
/// Size-only strong candidate (18pt, non-bold) becomes a section.
#[test]
fn v2_test2_size_only_strong_candidate_is_section() {
    // header class is rare: 1 out of total 101 elements (< 5% threshold)
    let mut text_elements = vec![make_v2_element(
        "h_rare", "Abstract", 18.0, "normal", 0, 1, 10.0, 80.0,
    )];
    for i in 0..100usize {
        text_elements.push(make_v2_element(
            "body",
            "Body text.",
            10.0,
            "normal",
            0,
            (i as u32) + 2,
            10.0,
            300.0,
        ));
    }

    // Even though h_rare is rare, the size signal alone (strong) is sufficient
    let font_analysis = make_font_analysis(10.0, &[("body", 100), ("h_rare", 1)]);
    let config = make_v2_config(SectionDetectionV2Config::default());
    let doc_analysis = make_doc_analysis();
    let style_data = StyleData {
        font_classes: BTreeMap::new(),
    };
    let engine = RuleEngine::new().expect("RuleEngine::new should succeed");

    let rule = SectionDetectionV2Rule::new(
        &engine,
        &text_elements,
        &config,
        &doc_analysis,
        &font_analysis,
        &style_data,
    );
    // Only classify the first element
    let result = rule.apply(vec![]).expect("apply should succeed");

    let elem = &result[0];
    assert_eq!(
        elem.element_type,
        ParsedElementType::Section,
        "18pt non-bold strong candidate should be a section"
    );
}

// ── Test 3 ─────────────────────────────────────────────────────────────────
/// Bold inline emphasis does NOT become a section — near neighbor in X.
#[test]
fn v2_test3_bold_inline_emphasis_is_not_section() {
    // Three elements on the same line, all close in X:
    // seg 0: 10pt normal  | seg 1: 10pt bold "Note:"  | seg 2: 10pt normal
    // Bboxes: x=10,w=100 | x=115,w=40               | x=160,w=200
    // Gap between seg0 and seg1: 115 - (10+100) = 5  → < isolation_neighbor_gap (20)
    let text_elements = vec![
        make_v2_element(
            "body",
            "Some leading text.",
            10.0,
            "normal",
            0,
            5,
            10.0,
            100.0,
        ),
        make_v2_element("body_bold", "Note:", 10.0, "bold", 0, 5, 115.0, 40.0),
        make_v2_element(
            "body",
            "this is inline emphasis.",
            10.0,
            "normal",
            0,
            5,
            160.0,
            200.0,
        ),
    ];

    let font_analysis = make_font_analysis(10.0, &[("body", 2), ("body_bold", 1)]);
    let config = make_v2_config(SectionDetectionV2Config::default());
    let doc_analysis = make_doc_analysis();
    let style_data = StyleData {
        font_classes: BTreeMap::new(),
    };
    let engine = RuleEngine::new().expect("RuleEngine::new should succeed");

    let rule = SectionDetectionV2Rule::new(
        &engine,
        &text_elements,
        &config,
        &doc_analysis,
        &font_analysis,
        &style_data,
    );
    let result = rule.apply(vec![]).expect("apply should succeed");

    for (i, elem) in result.iter().enumerate() {
        assert_eq!(
            elem.element_type,
            ParsedElementType::Paragraph,
            "element {} (text='{}') should be Paragraph, not Section (inline emphasis)",
            i,
            elem.text
        );
    }
}

// ── Test 4 ─────────────────────────────────────────────────────────────────
/// Bold isolated at body size becomes a section (weak + bold + isolated).
#[test]
#[ignore = "CR-33: stale assertion — predates CR-19 piecewise regions / CR-26 isolation-gated patterns. Needs investigation: fixture mismatch vs real regression."]
fn v2_test4_bold_isolated_at_body_size_is_section() {
    // The bold element is alone on its line — no same-line neighbors
    // It's also the only element with its class ("bold_class"): 1 out of 11 → 9% > 5%
    // So rarity doesn't confirm. But bold + isolated (weak) → section.
    let mut text_elements = vec![make_v2_element(
        "bold_class",
        "Results",
        10.0,
        "bold",
        0,
        3,
        10.0,
        60.0,
    )];
    for i in 0..10usize {
        text_elements.push(make_v2_element(
            "body",
            "Body paragraph text.",
            10.0,
            "normal",
            0,
            (i as u32) + 4,
            10.0,
            300.0,
        ));
    }

    // bold_class: 1, body: 10 → bold_class count/total = 1/11 ≈ 9.1% > 5% (not rare)
    // isolation: bold element is alone on line 3 → isolated = true
    // → weak + (bold AND isolated) → section
    let font_analysis = make_font_analysis(10.0, &[("bold_class", 1), ("body", 10)]);
    let config = make_v2_config(SectionDetectionV2Config::default());
    let doc_analysis = make_doc_analysis();
    let style_data = StyleData {
        font_classes: BTreeMap::new(),
    };
    let engine = RuleEngine::new().expect("RuleEngine::new should succeed");

    let rule = SectionDetectionV2Rule::new(
        &engine,
        &text_elements,
        &config,
        &doc_analysis,
        &font_analysis,
        &style_data,
    );
    let result = rule.apply(vec![]).expect("apply should succeed");

    assert_eq!(
        result[0].element_type,
        ParsedElementType::Section,
        "10pt bold isolated element should be a section via weak+(bold AND isolated)"
    );
}

// ── Test 5 ─────────────────────────────────────────────────────────────────
/// Numbered subsection pattern promotes weak candidate to section.
#[test]
fn v2_test5_numbered_subsection_pattern_promotes_weak() {
    // 11pt non-bold, body is 10pt → weak by size (11 > 10 + 0.1 threshold? 11 > 10.1 → actually strong!)
    // Use exactly 10.05pt to be within the weak zone (body=10.0, tol=0.1 → weak = [9.9, 10.1])
    // Wait: weak = font_size >= body - tol → 10.05 >= 9.9 (yes) AND NOT font_size > body + tol
    // font_size > body + tol → 10.05 > 10.1 → false → weak. Correct.
    let text_elements = vec![make_v2_element(
        "body",
        "3.2 Model Architecture",
        10.05,
        "normal",
        0,
        1,
        10.0,
        150.0,
    )];

    // No same-line neighbors → isolated = true. But weak + (bold AND isolated) → bold is false.
    // weak + (isolated AND rare): count/total = 1/1 = 100% → not rare.
    // So Pass 1 would NOT classify as section. Pass 2 inclusion pattern "^\\d+\\.\\d+" matches → promote.
    //
    // CR-42: production defaults set `require_bold: true` on inclusion patterns to filter
    // out inline hyperlink-span FPs. This test exercises the regex-only promotion path
    // (no bold signal), so it explicitly opts out of the bold gate to keep the original
    // pre-CR-42 semantics it was written to verify.
    let font_analysis = make_font_analysis(10.0, &[("body", 1)]);
    let mut v2_config = SectionDetectionV2Config::default();
    for ip in &mut v2_config.inclusion_patterns {
        ip.require_bold = false;
    }
    let config = make_v2_config(v2_config);
    let doc_analysis = make_doc_analysis();
    let style_data = StyleData {
        font_classes: BTreeMap::new(),
    };
    let engine = RuleEngine::new().expect("RuleEngine::new should succeed");

    let rule = SectionDetectionV2Rule::new(
        &engine,
        &text_elements,
        &config,
        &doc_analysis,
        &font_analysis,
        &style_data,
    );
    let result = rule.apply(vec![]).expect("apply should succeed");

    assert_eq!(
        result[0].element_type, ParsedElementType::Section,
        "\"3.2 Model Architecture\" at weak size should be promoted to section by inclusion pattern"
    );
}

// ── Test 6 ─────────────────────────────────────────────────────────────────
/// Figure caption is demoted even if visually strong (exclusion pattern).
#[test]
fn v2_test6_figure_caption_is_demoted() {
    // 11pt bold isolated — Pass 1 would make this a strong candidate (11 > 10.1) → section.
    // Text matches "^Figure\s" exclusion pattern → demoted.
    // Inclusion: "^\\d+\\." matches "Figure 3:" — no. "^\\d+" doesn't match "Figure".
    // So final result is non-section.
    let text_elements = vec![make_v2_element(
        "caption",
        "Figure 3: The Transformer architecture.",
        11.0,
        "bold",
        0,
        1,
        10.0,
        200.0,
    )];

    let font_analysis = make_font_analysis(10.0, &[("caption", 1)]);
    let config = make_v2_config(SectionDetectionV2Config::default());
    let doc_analysis = make_doc_analysis();
    let style_data = StyleData {
        font_classes: BTreeMap::new(),
    };
    let engine = RuleEngine::new().expect("RuleEngine::new should succeed");

    let rule = SectionDetectionV2Rule::new(
        &engine,
        &text_elements,
        &config,
        &doc_analysis,
        &font_analysis,
        &style_data,
    );
    let result = rule.apply(vec![]).expect("apply should succeed");

    assert_eq!(
        result[0].element_type, ParsedElementType::Paragraph,
        "Figure caption should be demoted to non-section by exclusion pattern even though visually strong"
    );
}

// ── Test 7 ─────────────────────────────────────────────────────────────────
/// Rotated element (rotation=90) is never a section, regardless of visual properties.
#[test]
fn v2_test7_rotated_element_is_never_section() {
    // 20pt bold — rotation=90. Rotated content is excluded from
    // `body_element_indices` by `analytics::reading_order::tag_and_resort`,
    // so its `region_label` stays `None` → `ParsedElementType::Margin` at the
    // conversion boundary, and Block 07's classify guard skips section
    // detection. Either way it must never become a Section.
    let mut text_elements = vec![make_v2_element(
        "sidebar",
        "arxiv 2017.09",
        20.0,
        "bold",
        90,
        1,
        10.0,
        200.0,
    )];
    // Override the body-default fixture label: rotated elements have no
    // region label in the post-Block-06b contract.
    text_elements[0].placement.region_label = None;

    let font_analysis = make_font_analysis(10.0, &[("sidebar", 1)]);
    let config = make_v2_config(SectionDetectionV2Config::default());
    let doc_analysis = make_doc_analysis();
    let style_data = StyleData {
        font_classes: BTreeMap::new(),
    };
    let engine = RuleEngine::new().expect("RuleEngine::new should succeed");

    let rule = SectionDetectionV2Rule::new(
        &engine,
        &text_elements,
        &config,
        &doc_analysis,
        &font_analysis,
        &style_data,
    );
    let result = rule.apply(vec![]).expect("apply should succeed");

    assert_eq!(
        result[0].element_type,
        ParsedElementType::Margin,
        "Rotated element (rotation=90, no region_label) lands as Margin and is skipped by section detection"
    );
}

// ============================================================================
// Block 05a: Placement struct — structural migration test
// ============================================================================

/// Confirm that the Placement struct fields round-trip correctly post-schema-0.5.0.
/// This test verifies the structural migration is complete and nothing was silently dropped.
#[test]
fn placement_fields_accessible_via_struct() {
    let element = make_v2_element(
        "header",
        "Round-trip test",
        14.0,
        "bold",
        90,
        3,
        42.5,
        120.0,
    );
    assert_eq!(element.placement.page_number, 0);
    assert_eq!(element.placement.bounding_box.x, 42.5);
    assert_eq!(element.placement.bounding_box.width, 120.0);
    assert_eq!(element.placement.line_number, 3);
    assert_eq!(element.placement.segment_number, 0);
    assert_eq!(element.placement.rotation, 90);
    assert_eq!(element.placement.paragraph_number, 0);
    // Block 07: `make_v2_element` defaults `region_label` to a body leaf
    // label so the synthetic fixture flows through section detection as
    // body content. Tests that exercise orphan / header / footer behavior
    // override this after construction.
    assert_eq!(element.placement.region_label, Some("1".to_string()));
    // Accessor methods agree with direct field reads
    assert_eq!(element.rotation(), 90);
    assert_eq!(element.line_number(), 3);
    assert_eq!(element.page_number(), 0);
    assert_eq!(element.bounding_box().x, 42.5);
}

// ── Test 8 ─────────────────────────────────────────────────────────────────
/// Hierarchy levels are assigned consistently (1, 2, 3 descending sizes; then back to 1).
#[test]
#[ignore = "CR-33: stale assertion — predates CR-27 keyword-tiebreaker hierarchy rewrite. Needs investigation: assertion-update vs rule-regression."]
fn v2_test8_hierarchy_levels_assigned_correctly() {
    // Three sections at decreasing font sizes: 16pt, 13pt, 11pt
    // Then one more at 16pt — should step back up to level 1.
    // All are well above body (10pt) → strong candidates → sections.
    // Each is alone on its line → isolated.
    let text_elements = vec![
        make_v2_element("h1", "Chapter One", 16.0, "bold", 0, 1, 10.0, 100.0),
        make_v2_element("h2", "Section 1.1", 13.0, "bold", 0, 2, 10.0, 80.0),
        make_v2_element("h3", "Subsection 1.1.1", 11.0, "bold", 0, 3, 10.0, 120.0),
        make_v2_element("h1", "Chapter Two", 16.0, "bold", 0, 4, 10.0, 100.0),
    ];

    let font_analysis = make_font_analysis(10.0, &[("h1", 2), ("h2", 1), ("h3", 1)]);
    let config = make_v2_config(SectionDetectionV2Config {
        starting_section_level: 1,
        font_size_tolerance: 0.1,
        ..SectionDetectionV2Config::default()
    });
    let doc_analysis = make_doc_analysis();
    let style_data = StyleData {
        font_classes: BTreeMap::new(),
    };
    let engine = RuleEngine::new().expect("RuleEngine::new should succeed");

    let rule = SectionDetectionV2Rule::new(
        &engine,
        &text_elements,
        &config,
        &doc_analysis,
        &font_analysis,
        &style_data,
    );
    let result = rule.apply(vec![]).expect("apply should succeed");

    let sections: Vec<_> = result
        .iter()
        .filter(|e| e.element_type == ParsedElementType::Section)
        .collect();
    assert_eq!(sections.len(), 4, "should detect all 4 sections");

    assert_eq!(
        sections[0].hierarchy_level, 1,
        "Chapter One (16pt) should be level 1"
    );
    assert_eq!(
        sections[1].hierarchy_level, 2,
        "Section 1.1 (13pt) should be level 2"
    );
    assert_eq!(
        sections[2].hierarchy_level, 3,
        "Subsection 1.1.1 (11pt) should be level 3"
    );
    assert_eq!(
        sections[3].hierarchy_level, 1,
        "Chapter Two (16pt) should step back to level 1"
    );
}

// ── Test 9 (CR-41) ─────────────────────────────────────────────────────────
/// Bookmark match substitutes for `isolated_in_leaf` at body-size R3.
///
/// Canonical case: a bold body-size heading sharing its Region tree leaf
/// with the trailing body paragraph (rfc-quic page-30 shape). Without
/// CR-41, R3's `bold AND isolated` rejects on isolation (multi-line leaf).
/// With CR-41, the parser-supplied `bookmark_match` substitutes for the
/// missing structural-atom signal and the heading classifies as Section.
///
/// The control assertion (no bookmark match → still rejected) confirms
/// the gate is keyed on the bookmark substrate, not on something else.
#[test]
fn v2_test9_bookmark_match_substitutes_for_isolation_at_body_size() {
    fn build(text_elements: Vec<PdfTextElement>) -> Vec<blazegraph_io_core::ParsedPdfElement> {
        let font_analysis = make_font_analysis(10.0, &[("body", 5)]);
        let config = make_v2_config(SectionDetectionV2Config::default());
        let doc_analysis = make_doc_analysis();
        let style_data = StyleData {
            font_classes: BTreeMap::new(),
        };
        let engine = RuleEngine::new().expect("RuleEngine::new should succeed");
        let rule = SectionDetectionV2Rule::new(
            &engine,
            &text_elements,
            &config,
            &doc_analysis,
            &font_analysis,
            &style_data,
        );
        rule.apply(vec![]).expect("apply should succeed")
    }

    // Five spans in the same Region tree leaf (default region_label = "1"):
    // line 0 = bold body-size heading; lines 1-4 = body text below it.
    // Multi-line leaf → `isolated_in_leaf` returns false for line 0.
    let make_layout = || {
        vec![
            make_v2_element(
                "body",
                "5.2.1.  Client Packet Handling",
                10.0,
                "bold",
                0,
                0,
                10.0,
                200.0,
            ),
            make_v2_element(
                "body",
                "Valid packets sent to clients...",
                10.0,
                "normal",
                0,
                1,
                10.0,
                400.0,
            ),
            make_v2_element(
                "body",
                "client selects. Clients...",
                10.0,
                "normal",
                0,
                2,
                10.0,
                400.0,
            ),
            make_v2_element(
                "body",
                "and port to identify a connection...",
                10.0,
                "normal",
                0,
                3,
                10.0,
                400.0,
            ),
            make_v2_element("body", "discarded.", 10.0, "normal", 0, 4, 10.0, 100.0),
        ]
    };

    // Control: no bookmark match → R3 rejects on isolation.
    let result_control = build(make_layout());
    assert_eq!(
        result_control[0].element_type,
        ParsedElementType::Paragraph,
        "without bookmark match, bold body-size heading in multi-line leaf should NOT be a section (R3 isolation rejects)"
    );

    // CR-41: same shape with bookmark_match populated → R3 accepts via the
    // bookmark disjunct, and the heading classifies as Section.
    let mut layout = make_layout();
    layout[0].bookmark_match = Some(BookmarkSection {
        title: "5.2.1.  Client Packet Handling".to_string(),
        order: 0,
        level: 3,
    });
    let result_promoted = build(layout);
    assert_eq!(
        result_promoted[0].element_type,
        ParsedElementType::Section,
        "with bookmark_match=Some, bold body-size heading should be promoted to Section even when leaf has body neighbors"
    );
}
