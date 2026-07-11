//! Integration test for the bgraph.md forward emitter (B2).
//!
//! Exercises `emit_markdown` against a real-shape `DocumentGraph`
//! reconstructed from a stage3 fixture snapshot. Asserts structural
//! invariants (one fence per node, doc-level block parses, etc.) — not
//! byte-equality, since we don't have a frozen Rust-emitter fixture
//! yet (B4 will ship round-trip identity tests).

use blazegraph_io_core::graphs::serialization::markdown::emit_markdown;
use blazegraph_io_core::types::*;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test_fixtures/snapshots")
}

/// Synthetic provenance so the emitter can run against fixtures (Block
/// A: provenance is an explicit emit argument, not graph state).
fn fixture_provenance(name: &str) -> ParseProvenance {
    ParseProvenance {
        blazegraph_version: "0.6.0-test".to_string(),
        source_format: "pdf".to_string(),
        source_filename: format!("{name}.pdf"),
        source_sha256: format!("test-source-sha-{name}"),
        config_hash: "test-config-hash".to_string(),
    }
}

/// Load a stage3 fixture.
fn load_fixture_graph(name: &str) -> (DocumentGraph, Value) {
    let path = fixtures_dir().join(name).join("stage3_graph.json");
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "Missing fixture: {}. Run `make test-generate-fixtures`",
            path.display()
        )
    });
    let raw_value: Value = serde_json::from_str(&raw).expect("fixture parses as JSON");
    // Stage3 fixtures are `SortedDocumentGraph` shape; re-hydrate into
    // an in-memory `DocumentGraph` (HashMap<NodeId, DocumentNode>).
    let sorted: SortedDocumentGraph =
        serde_json::from_value(raw_value.clone()).expect("fixture deserializes as graph");

    let mut nodes: HashMap<NodeId, DocumentNode> = HashMap::with_capacity(sorted.nodes.len());
    for node in sorted.nodes {
        nodes.insert(node.id, node);
    }
    let document_info = sorted.document_info;
    let graph = DocumentGraph {
        nodes,
        document_info,
    };
    (graph, raw_value)
}

#[test]
fn emit_matches_node_counts_for_shannon_fixture() {
    let (graph, raw) = load_fixture_graph("claude_shannon_paper");
    let md = emit_markdown(&graph, &fixture_provenance("claude_shannon_paper"));

    // First line should be the document-level fence open.
    let first_line = md.lines().next().expect("non-empty output");
    assert_eq!(
        first_line, "```bgraph",
        "first line of bgraph.md must be the doc-level fence open",
    );

    // Doc-level JSON line parses as a JSON object with the seven
    // spec-required fields.
    let json_line = md
        .lines()
        .nth(1)
        .expect("doc-level fence has a JSON line below the fence open");
    let parsed: Value =
        serde_json::from_str(json_line).expect("doc-level JSON parses as a JSON object");
    // v2.1.0+ (CR-56 § I.4): `title` moved out of the doc-level block
    // into the bgraph-metadata fence. The remaining six keys are
    // graph-identity only.
    for key in [
        "schema",
        "blazegraph_version",
        "source",
        "flow_type",
        "config_hash",
        "graph_sha256",
    ] {
        assert!(
            parsed.get(key).is_some(),
            "doc-level block missing required key {key:?}",
        );
    }
    assert!(
        parsed.get("title").is_none(),
        "v2.1.0 doc-level block must not carry `title`",
    );

    // Structural assertion: number of `bgraph-section` opening fences
    // matches the number of Section nodes in the graph; same for each
    // body element type.
    let raw_nodes = raw["nodes"].as_array().unwrap();
    for (node_type, fence_tag) in [
        ("Section", "```bgraph-section\n"),
        ("Paragraph", "```bgraph-paragraph\n"),
        ("Header", "```bgraph-header\n"),
        ("Footer", "```bgraph-footer\n"),
        ("Margin", "```bgraph-margin\n"),
    ] {
        let graph_count = raw_nodes
            .iter()
            .filter(|n| n["node_type"].as_str() == Some(node_type))
            .count();
        let md_count = md.matches(fence_tag).count();
        assert_eq!(
            md_count, graph_count,
            "fence count mismatch for {node_type}: graph has {graph_count}, md has {md_count}",
        );
    }

    // Document root (synthetic, text_order = None) must NOT appear as
    // a fence — there's no `bgraph-document` tag.
    assert!(
        !md.contains("```bgraph-document"),
        "Document root should be skipped, not emitted as a fence",
    );
}

/// CR-87 drift-guard: the version axes must not silently diverge again.
///
/// (1) **One schema/format version, two serializations.** The bgraph.md
/// doc-level `schema` field and the json wrapper's `schema_version` are
/// two stampings of the ONE serialization-neutral schema/format version.
/// They must emit the same value, and it must equal
/// `BGRAPH_FORMAT_VERSION`. We emit from a real graph (not just compare
/// consts) so a hardcoded literal slipped into *either* stamp site —
/// `graph.rs::to_sorted_graph` (json) or `markdown.rs` (md) — fails
/// loudly here rather than shipping a mismatch (the exact pre-CR-87 bug:
/// json advertised `0.9.0` while md advertised `5.0.0`).
///
/// (2) **Cache tracks the build (Option A).** `BLAZEGRAPH_VERSION ==
/// crate::VERSION`. If a hand-maintained cache-version literal is ever
/// re-introduced (the drifted `0.1.1` this CR killed), this fails.
#[test]
fn cr87_version_axes_do_not_drift() {
    use blazegraph_io_core::cache::versions::BLAZEGRAPH_VERSION;
    use blazegraph_io_core::{BGRAPH_FORMAT_VERSION, VERSION};

    let graph = DocumentGraph::new();
    let prov = fixture_provenance("drift_guard");

    // md side: `schema` on the doc-level JSON line (second line, under
    // the ```bgraph fence open).
    let md = emit_markdown(&graph, &prov);
    let json_line = md
        .lines()
        .nth(1)
        .expect("doc-level fence has a JSON line below the fence open");
    let doc_block: Value =
        serde_json::from_str(json_line).expect("doc-level JSON parses as an object");
    let md_schema = doc_block["schema"].as_str().expect("`schema` is a string");

    // json side: `schema_version` on the wrapper.
    let sorted = graph.to_sorted_graph(Some(&prov));
    let json_schema_version = sorted.schema_version.as_str();

    assert_eq!(
        md_schema, json_schema_version,
        "CR-87 drift: md doc-level `schema` ({md_schema}) != json `schema_version` \
         ({json_schema_version}) — the two serializations must advertise the ONE \
         schema/format version",
    );
    assert_eq!(
        md_schema, BGRAPH_FORMAT_VERSION,
        "both serializations must stamp BGRAPH_FORMAT_VERSION ({BGRAPH_FORMAT_VERSION})",
    );

    assert_eq!(
        BLAZEGRAPH_VERSION, VERSION,
        "CR-87 drift: cache BLAZEGRAPH_VERSION ({BLAZEGRAPH_VERSION}) != crate::VERSION \
         ({VERSION}) — the graph cache key must track the build so output changes \
         invalidate stale entries",
    );
}
