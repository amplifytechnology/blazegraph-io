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

use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;

// ============================================================================
// Fixture helpers
// ============================================================================

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test_fixtures/snapshots")
}

fn load_summary(fixture_name: &str) -> Value {
    let path = fixtures_dir().join(fixture_name).join("summary.json");
    let contents = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("Missing fixture: {}. Run `make test-generate-fixtures`", path.display()));
    serde_json::from_str(&contents).expect("Invalid summary.json")
}

fn load_graph(fixture_name: &str) -> Value {
    let path = fixtures_dir().join(fixture_name).join("stage3_graph.json");
    let contents = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("Missing fixture: {}. Run `make test-generate-fixtures`", path.display()));
    serde_json::from_str(&contents).expect("Invalid stage3_graph.json")
}

fn load_xhtml(fixture_name: &str) -> String {
    let path = fixtures_dir().join(fixture_name).join("stage1a_xhtml.html");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("Missing fixture: {}. Run `make test-generate-fixtures`", path.display()))
}

fn load_text_elements(fixture_name: &str) -> Value {
    let path = fixtures_dir().join(fixture_name).join("stage1b_text_elements.json");
    let contents = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("Missing fixture: {}. Run `make test-generate-fixtures`", path.display()));
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
            xhtml.len(), expected_bytes,
            "XHTML byte count changed — did Tika version change?"
        );
    }

    #[test]
    fn shannon_text_element_count_stable() {
        let elements = load_text_elements("claude_shannon_paper");
        let arr = elements.as_array().expect("text_elements should be array");

        // Text elements come directly from Tika — stable unless Tika changes
        assert_eq!(arr.len(), 3021, "Text element count changed — Tika output drift?");
    }

    #[test]
    fn euclid_xhtml_size_stable() {
        let xhtml = load_xhtml("elements_of_euclid");
        let summary = load_summary("elements_of_euclid");
        let expected_bytes = summary["stage_counts"]["xhtml_bytes"].as_u64().unwrap() as usize;

        assert_eq!(
            xhtml.len(), expected_bytes,
            "XHTML byte count changed — did Tika version change?"
        );
    }

    #[test]
    fn euclid_text_element_count_stable() {
        let elements = load_text_elements("elements_of_euclid");
        let arr = elements.as_array().expect("text_elements should be array");

        assert_eq!(arr.len(), 9538, "Text element count changed — Tika output drift?");
    }
}

// ============================================================================
// Boundary 2: Graph output — schema contract
// ============================================================================

mod schema_contract {
    use super::*;

    #[test]
    fn schema_version_is_0_2_0() {
        let graph = load_graph("claude_shannon_paper");
        assert_eq!(
            graph["schema_version"].as_str().unwrap(), "0.2.0",
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

        assert!(graph["schema_version"].is_string(), "Missing schema_version");
        assert!(graph["nodes"].is_array(), "Missing nodes array");
        assert!(graph["document_info"].is_object(), "Missing document_info");
        assert!(graph["structural_profile"].is_object(), "Missing structural_profile");
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
            assert!(node["token_count"].is_number(), "Node {i} missing token_count");
            // parent can be null (root node)
            // children should always be an array
            assert!(node["children"].is_array(), "Node {i} missing children array");
        }
    }

    #[test]
    fn document_info_has_required_fields() {
        let graph = load_graph("claude_shannon_paper");
        let info = &graph["document_info"];

        assert!(info["root_id"].is_string(), "Missing root_id");
        assert!(info["document_metadata"].is_object(), "Missing document_metadata");
        assert!(info["document_analysis"].is_object(), "Missing document_analysis");
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

        let doc_nodes: Vec<_> = nodes.iter()
            .filter(|n| n["node_type"].as_str() == Some("Document"))
            .collect();

        assert_eq!(doc_nodes.len(), 1, "Should have exactly one Document root node");

        let root = doc_nodes[0];
        assert!(root["parent"].is_null(), "Document root should have null parent");
        assert!(!root["children"].as_array().unwrap().is_empty(), "Document root should have children");
    }

    #[test]
    fn shannon_has_sections() {
        let counts = count_node_types(&load_graph("claude_shannon_paper"));
        let section_count = counts.get("Section").copied().unwrap_or(0);

        assert!(section_count > 0, "Shannon paper should have sections");
        // Shannon's paper has well-defined sections — this should be stable
        assert!(
            section_count >= 5 && section_count <= 40,
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
        let orders: Vec<Option<u64>> = nodes.iter()
            .map(|n| n["text_order"].as_u64())
            .collect();

        // Document root has null text_order and comes first
        assert!(orders[0].is_none(), "First node should be Document with null text_order");

        // Remaining should be monotonically non-decreasing
        let rest: Vec<u64> = orders[1..].iter()
            .filter_map(|o| *o)
            .collect();
        for window in rest.windows(2) {
            assert!(
                window[0] <= window[1],
                "Nodes not sorted by text_order: {} > {}", window[0], window[1]
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

        let root = nodes.iter()
            .find(|n| n["node_type"].as_str() == Some("Document"))
            .expect("No Document root node");

        let breadcrumbs = root["location"]["semantic"]["breadcrumbs"]
            .as_array()
            .expect("Root should have breadcrumbs array");

        assert!(!breadcrumbs.is_empty(), "Root breadcrumbs should contain the document title");
    }

    #[test]
    fn section_nodes_appear_in_child_breadcrumbs() {
        let graph = load_graph("claude_shannon_paper");
        let nodes = graph["nodes"].as_array().unwrap();

        // Find a section that has children
        for node in nodes {
            if node["node_type"].as_str() == Some("Section") {
                let section_text = node["content"]["text"].as_str().unwrap_or("");
                let children_ids: Vec<&str> = node["children"].as_array().unwrap()
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
                            section_text, crumbs
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
                depth, crumb_count
            );
        }
    }
}
