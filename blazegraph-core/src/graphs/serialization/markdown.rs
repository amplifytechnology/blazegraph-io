//! bgraph.md forward emitter — `DocumentGraph` → markdown string.
//!
//! Conforms to v1.0.0 of the bgraph.md format spec
//! (`docs/P2/core/architecture/08-bgraph-md-format.md`). Reference
//! implementation in `scripts/bgraph_md_prototype.py`.
//!
//! Output shape:
//!
//! - Top of file: a single `bgraph` fence with the document-level
//!   metadata block (flat JSON, all 7 spec fields populated from
//!   `graph.document_info.parse_provenance` + `graph_sha256` derived
//!   from [`super::canonical::graph_sha256`]).
//! - Body: per-element fences (`bgraph-section`, `bgraph-paragraph`,
//!   `bgraph-header`, `bgraph-footer`, `bgraph-margin`) emitted in
//!   `text_order` ascending order. Section/Paragraph bodies live
//!   *outside* the fence; Header/Footer/Margin bodies live *inside*
//!   the fence above the JSON metadata line. The synthetic Document
//!   root node (`text_order = None`) is skipped.
//! - Elements separated by a single blank line.
//!
//! This module exposes a single public function
//! [`emit_markdown`]; everything else is private.

use super::canonical;
use crate::types::*;
use serde::Serialize;

/// Emit a `DocumentGraph` to bgraph.md format (v1.0.0).
///
/// # Panics
///
/// Panics if `graph.document_info.parse_provenance` is `None`. The
/// emitter requires the (version, source, config) triple to populate
/// the document-level block; the legacy `GraphBuilder::build_graph`
/// path (random UUIDv4 IDs, no provenance) is incompatible with
/// round-trip identity by design and should never reach this emitter.
/// Build the graph via
/// `GraphBuilder::build_graph_deterministic(elements, &id_gen, provenance)`
/// instead.
pub fn emit_markdown(graph: &DocumentGraph) -> String {
    let provenance = graph.document_info.parse_provenance.as_ref().expect(
        "emit_markdown requires graph.document_info.parse_provenance; \
         build the graph via GraphBuilder::build_graph_deterministic with provenance",
    );

    let mut parts: Vec<String> = Vec::with_capacity(graph.nodes.len() + 2);
    parts.push(emit_document_level_block(graph, provenance));
    parts.push(String::new()); // blank line after doc-level block

    // Walk by text_order ascending. Document root has text_order = None
    // and is skipped (filtered out before sorting).
    let mut nodes: Vec<&DocumentNode> = graph
        .nodes
        .values()
        .filter(|n| n.text_order.is_some())
        .collect();
    nodes.sort_by_key(|n| n.text_order.expect("filtered above"));

    for node in nodes {
        if let Some(chunk) = emit_node(node) {
            parts.push(chunk);
            parts.push(String::new()); // blank line between elements
        }
    }

    parts.join("\n")
}

/// Document-level metadata block. Tag: `bgraph` (no suffix). Flat JSON
/// — this is file metadata, not a node, so the schema-mirroring rule
/// for per-element blocks doesn't apply.
///
/// Field order matches the prototype emitter and the bgraph.md spec
/// inline example (`schema, blazegraph_version, source, flow_type,
/// title, config_hash, graph_sha256`). Order is preserved by emitting
/// from a struct with serde-derived `Serialize` rather than a
/// `serde_json::Map` — the latter sorts keys when the
/// `preserve_order` feature is off, which is not what the spec
/// example shows for the file representation.
fn emit_document_level_block(graph: &DocumentGraph, provenance: &ParseProvenance) -> String {
    #[derive(Serialize)]
    struct DocLevelSource<'a> {
        format: &'a str,
        filename: &'a str,
        sha256: &'a str,
    }

    #[derive(Serialize)]
    struct DocLevelBlock<'a> {
        schema: &'static str,
        blazegraph_version: &'a str,
        source: DocLevelSource<'a>,
        flow_type: &'a FlowType,
        title: &'a Option<String>,
        config_hash: &'a str,
        graph_sha256: String,
    }

    let block = DocLevelBlock {
        schema: "1.0.0",
        blazegraph_version: &provenance.blazegraph_version,
        source: DocLevelSource {
            format: &provenance.source_format,
            filename: &provenance.source_filename,
            sha256: &provenance.source_sha256,
        },
        flow_type: &graph.structural_profile.flow_type,
        title: &graph.document_info.document_metadata.title,
        config_hash: &provenance.config_hash,
        graph_sha256: canonical::graph_sha256(graph),
    };
    format!(
        "```bgraph\n{}\n```",
        serde_json::to_string(&block).expect("doc-level block is always serializable"),
    )
}

/// Emit one node. Returns `None` for nodes we skip (Document root).
///
/// - `Section`: heading prefix + body on the line *preceding* the
///   `bgraph-section` fence; metadata-only inside fence.
/// - `Paragraph`: body on the line *preceding* the `bgraph-paragraph`
///   fence; metadata-only inside fence.
/// - `Header` / `Footer` / `Margin`: body *inside* the fence followed
///   by the JSON metadata line. (See "Strip ergonomics" in the spec —
///   the body-inside-fence shape lets the second `sed` variant strip
///   noise + metadata in a single pass.)
fn emit_node(node: &DocumentNode) -> Option<String> {
    let meta = node_metadata_json(node);
    match node.node_type.as_str() {
        "Document" => None, // synthetic root; not a content node
        "Section" => {
            let depth = node.location.semantic.depth as usize;
            let prefix = heading_prefix(depth);
            Some(format!(
                "{prefix} {text}\n```bgraph-section\n{meta}\n```",
                text = node.content.text,
            ))
        }
        "Paragraph" => Some(format!(
            "{text}\n```bgraph-paragraph\n{meta}\n```",
            text = node.content.text,
        )),
        "Header" | "Footer" | "Margin" => {
            let tag = node.node_type.to_ascii_lowercase();
            Some(format!(
                "```bgraph-{tag}\n{text}\n{meta}\n```",
                text = node.content.text,
            ))
        }
        // Defensive — shouldn't reach here with v1.0.0 element types.
        _ => Some(format!(
            "```bgraph-unknown\n{text}\n{meta}\n```",
            text = node.content.text,
        )),
    }
}

/// Per-element JSON metadata. Nested mirror of `DocumentNode` shape, so
/// the reverse parser's deserialization is `serde_json::from_str(line)
/// -> DocumentNodeMetadata` with no flat→nested mapping. Excludes
/// `content.text` (lives in markdown body or inside fence) and
/// `parent`/`children` (derivable from heading structure on reverse
/// parse).
fn node_metadata_json(node: &DocumentNode) -> String {
    #[derive(Serialize)]
    struct NodeMetadata<'a> {
        id: &'a NodeId,
        node_type: &'a String,
        location: &'a NodeLocation,
        text_order: &'a Option<u32>,
    }
    let meta = NodeMetadata {
        id: &node.id,
        node_type: &node.node_type,
        location: &node.location,
        text_order: &node.text_order,
    };
    serde_json::to_string(&meta).expect("DocumentNode subset is always serializable")
}

/// Markdown heading prefix for `depth`: `#` to `######` for depths
/// 1..=6, capped at `######` for depth ≥ 7. Markdown's heading syntax
/// does not extend further. The `depth` field in metadata remains
/// exact, so reverse-parsing reconstructs the true tree even when the
/// visual heading prefix has been clamped (see spec, "Heading depth
/// handling").
fn heading_prefix(depth: usize) -> &'static str {
    match depth {
        0 | 1 => "#",
        2 => "##",
        3 => "###",
        4 => "####",
        5 => "#####",
        _ => "######",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use uuid::Uuid;

    /// Build a minimal graph with a Document root + the supplied body
    /// nodes. `nodes_in` is `(node_type, text, depth, text_order)`.
    fn build_graph(nodes_in: Vec<(&str, &str, u32, u32)>) -> DocumentGraph {
        let root_id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"test-root");
        let mut nodes = HashMap::new();
        let mut child_ids = Vec::new();

        for (node_type, text, depth, text_order) in &nodes_in {
            let id = Uuid::new_v5(
                &Uuid::NAMESPACE_DNS,
                format!("test:{}", text_order).as_bytes(),
            );
            child_ids.push(id);
            nodes.insert(
                id,
                DocumentNode {
                    id,
                    node_type: node_type.to_string(),
                    location: NodeLocation {
                        semantic: SemanticLocation {
                            path: format!("{}", text_order + 1),
                            depth: *depth,
                            breadcrumbs: Vec::new(),
                        },
                        physical: None,
                    },
                    text_order: Some(*text_order),
                    content: NodeContent {
                        text: text.to_string(),
                    },
                    style_info: None,
                    token_count: 1,
                    parent: Some(root_id),
                    children: Vec::new(),
                },
            );
        }

        nodes.insert(
            root_id,
            DocumentNode {
                id: root_id,
                node_type: "Document".to_string(),
                location: NodeLocation {
                    semantic: SemanticLocation {
                        path: String::new(),
                        depth: 0,
                        breadcrumbs: Vec::new(),
                    },
                    physical: None,
                },
                text_order: None,
                content: NodeContent {
                    text: "Document".to_string(),
                },
                style_info: None,
                token_count: 0,
                parent: None,
                children: child_ids,
            },
        );

        DocumentGraph {
            nodes,
            document_info: DocumentInfo {
                root_id,
                document_metadata: DocumentMetadata {
                    title: Some("Synthetic Test Doc".to_string()),
                    ..DocumentMetadata::default()
                },
                bookmark_data: None,
                parse_provenance: Some(ParseProvenance {
                    blazegraph_version: "0.6.0".to_string(),
                    source_format: "markdown".to_string(),
                    source_filename: "synthetic.md".to_string(),
                    source_sha256: "deadbeef".to_string(),
                    config_hash: "cafef00d".to_string(),
                }),
            },
            structural_profile: StructuralProfile::default(),
        }
    }

    #[test]
    fn document_root_is_skipped() {
        // Build with one Section; expect output contains the Section
        // fence but no `bgraph-document` (or any other tag matching the
        // root node).
        let graph = build_graph(vec![("Section", "Intro", 1, 0)]);
        let md = emit_markdown(&graph);
        assert!(md.contains("```bgraph-section"), "missing section fence");
        assert!(
            !md.contains("```bgraph-document"),
            "Document root should not emit a fence; got:\n{md}",
        );
    }

    #[test]
    fn section_paragraph_body_is_outside_fence() {
        // Section: body precedes the fence ("# Intro\n```bgraph-section\n…")
        // Paragraph: body precedes the fence
        let graph = build_graph(vec![
            ("Section", "Intro", 1, 0),
            ("Paragraph", "Hello world.", 1, 1),
        ]);
        let md = emit_markdown(&graph);
        // Section: heading line precedes the fence on the immediately
        // adjacent line.
        assert!(
            md.contains("# Intro\n```bgraph-section\n"),
            "section body should be outside (preceding) the fence; got:\n{md}",
        );
        // Paragraph: body precedes the fence.
        assert!(
            md.contains("Hello world.\n```bgraph-paragraph\n"),
            "paragraph body should be outside (preceding) the fence; got:\n{md}",
        );
    }

    #[test]
    fn header_footer_margin_body_is_inside_fence() {
        // For Header/Footer/Margin: opening fence, then body line, then
        // metadata line, then closing fence.
        let graph = build_graph(vec![
            ("Header", "Running header text", 1, 0),
            ("Footer", "Running footer text", 1, 1),
            ("Margin", "Margin note text", 1, 2),
        ]);
        let md = emit_markdown(&graph);
        assert!(
            md.contains("```bgraph-header\nRunning header text\n"),
            "header body should be inside fence (immediately after fence open); got:\n{md}",
        );
        assert!(
            md.contains("```bgraph-footer\nRunning footer text\n"),
            "footer body should be inside fence; got:\n{md}",
        );
        assert!(
            md.contains("```bgraph-margin\nMargin note text\n"),
            "margin body should be inside fence; got:\n{md}",
        );
    }

    #[test]
    fn heading_prefix_caps_at_six_hashes() {
        // Section at depth 7 should render with "######" but the JSON
        // metadata must preserve "depth":7 exactly.
        let graph = build_graph(vec![("Section", "Deeply Nested", 7, 0)]);
        let md = emit_markdown(&graph);
        assert!(
            md.contains("###### Deeply Nested\n"),
            "depth 7 section should still emit ######; got:\n{md}",
        );
        assert!(
            !md.contains("####### "),
            "should not emit 7 hashes; got:\n{md}",
        );
        assert!(
            md.contains("\"depth\":7"),
            "metadata should preserve exact depth 7; got:\n{md}",
        );
    }

    #[test]
    fn doc_level_block_has_all_seven_fields() {
        let graph = build_graph(vec![("Paragraph", "Body.", 1, 0)]);
        let md = emit_markdown(&graph);

        // First line is the bgraph fence open; second line is the JSON.
        let first_line_end = md.find('\n').expect("multi-line output");
        assert_eq!(
            &md[..first_line_end],
            "```bgraph",
            "first line must be the document-level fence open",
        );

        // Pull out the JSON line (between first and second '\n').
        let after_first = &md[first_line_end + 1..];
        let json_line_end = after_first.find('\n').expect("JSON line + closing fence");
        let json_line = &after_first[..json_line_end];

        // Parse and verify all seven keys are present.
        let parsed: serde_json::Value =
            serde_json::from_str(json_line).expect("doc-level JSON parses");
        for key in [
            "schema",
            "blazegraph_version",
            "source",
            "flow_type",
            "title",
            "config_hash",
            "graph_sha256",
        ] {
            assert!(
                parsed.get(key).is_some(),
                "doc-level block missing required key {key:?}; got {json_line}",
            );
        }
        // source.{format, filename, sha256}
        let source = parsed.get("source").unwrap();
        for key in ["format", "filename", "sha256"] {
            assert!(
                source.get(key).is_some(),
                "doc-level source block missing required key {key:?}; got {json_line}",
            );
        }
        // graph_sha256 is a 64-char lowercase hex
        let h = parsed["graph_sha256"]
            .as_str()
            .expect("graph_sha256 is a string");
        assert_eq!(h.len(), 64, "graph_sha256 should be 64 hex chars; got {h}");
    }

    #[test]
    fn synthetic_one_section_one_paragraph_matches_template() {
        // Whole-output literal check: for a synthetic graph with one
        // Section + one Paragraph, the body shape (after the
        // doc-level block) must be byte-identical to the template
        // below. The doc-level block changes whenever provenance or
        // graph_sha256 changes, so we anchor on the body slice.
        let graph = build_graph(vec![
            ("Section", "Intro", 1, 0),
            ("Paragraph", "Hello.", 1, 1),
        ]);
        let md = emit_markdown(&graph);
        // The body shape after the doc-level block is anchored by the
        // first occurrence of the section heading line. Pull from there
        // to end-of-file.
        let body_start = md
            .find("# Intro\n```bgraph-section\n")
            .expect("section heading should be present");
        let after_doc = &md[body_start..];

        // The section/paragraph fences embed deterministic UUIDs from
        // build_graph's seeding ("test:0", "test:1"). Compute them here
        // so the assertion stays stable as long as the seeding scheme
        // doesn't drift.
        let section_id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"test:0");
        let paragraph_id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"test:1");

        let expected = format!(
            "# Intro\n\
             ```bgraph-section\n\
             {{\"id\":\"{section_id}\",\"node_type\":\"Section\",\"location\":{{\"semantic\":{{\"path\":\"1\",\"depth\":1,\"breadcrumbs\":[]}},\"physical\":null}},\"text_order\":0}}\n\
             ```\n\
             \n\
             Hello.\n\
             ```bgraph-paragraph\n\
             {{\"id\":\"{paragraph_id}\",\"node_type\":\"Paragraph\",\"location\":{{\"semantic\":{{\"path\":\"2\",\"depth\":1,\"breadcrumbs\":[]}},\"physical\":null}},\"text_order\":1}}\n\
             ```\n\
             ",
        );
        assert_eq!(after_doc, expected, "body shape drifted from template");
    }

    #[test]
    #[should_panic(expected = "parse_provenance")]
    fn emit_markdown_panics_without_provenance() {
        let mut graph = build_graph(vec![("Paragraph", "Body.", 1, 0)]);
        graph.document_info.parse_provenance = None;
        let _ = emit_markdown(&graph);
    }

    /// Diagnostic: prints sample emitter output for human review.
    /// Always passes; only useful with `--nocapture`. Kept as a test
    /// rather than a binary so it can stay close to the unit tests.
    #[test]
    fn diagnostic_print_sample_output() {
        let graph = build_graph(vec![
            ("Section", "Introduction", 1, 0),
            (
                "Paragraph",
                "Forward emitter B2 lands on schema 0.6.0.",
                2,
                1,
            ),
            ("Header", "Page 1 — Demo", 2, 2),
            ("Footer", "Confidential", 2, 3),
        ]);
        eprintln!("--- BEGIN emit_markdown sample ---");
        eprintln!("{}", emit_markdown(&graph));
        eprintln!("--- END emit_markdown sample ---");
    }
}
