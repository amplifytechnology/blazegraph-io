//! bgraph.md forward emitter — `DocumentGraph` → markdown string.
//!
//! Wire-format spec is the source of truth:
//! `docs/P2/core/architecture/08-bgraph-md-format.md`. The emitted
//! `schema` field is sourced from
//! [`crate::preprocessors::md::BGRAPH_MD_FORMAT_VERSION`].
//!
//! Public surface: [`emit_markdown`]. Everything else is private.

use super::canonical;
use crate::preprocessors::md::BGRAPH_MD_FORMAT_VERSION;
use crate::types::*;
use serde::Serialize;

/// Emit a `DocumentGraph` to bgraph.md format. Targets the current
/// [`BGRAPH_MD_FORMAT_VERSION`].
///
/// # Panics
///
/// Panics if `graph.document_info.parse_provenance` is `None`. Build
/// the graph via
/// `GraphBuilder::build_graph_deterministic(elements, &id_gen, provenance)`
/// — the legacy `build_graph` path (random UUIDv4 IDs, no provenance)
/// is incompatible with round-trip identity and must not reach this
/// emitter.
pub fn emit_markdown(graph: &DocumentGraph) -> String {
    let provenance = graph.document_info.parse_provenance.as_ref().expect(
        "emit_markdown requires graph.document_info.parse_provenance; \
         build the graph via GraphBuilder::build_graph_deterministic with provenance",
    );

    let mut parts: Vec<String> = Vec::with_capacity(graph.nodes.len() + 4);
    parts.push(emit_document_level_block(graph, provenance));
    parts.push(String::new()); // blank line after doc-level block

    // Optional `bgraph-bookmarks` fence — emitted only when the source
    // graph carries an outline. Placed immediately after the doc-level
    // block so the doc-level block stays a single readable JSON line
    // even when the bookmark payload is large (rfc-quic ~14 KB).
    if let Some(bookmarks_block) = emit_bookmarks_block(graph) {
        parts.push(bookmarks_block);
        parts.push(String::new());
    }

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

/// Document-level bookmarks block. Tag: `bgraph-bookmarks`. Optional —
/// returns `None` when `graph.document_info.bookmark_data` is `None`.
/// JSON shape mirrors `BookmarkData` exactly (one `serde_json::to_string`
/// pass, compact, single line).
fn emit_bookmarks_block(graph: &DocumentGraph) -> Option<String> {
    let bookmarks = graph.document_info.bookmark_data.as_ref()?;
    let json = serde_json::to_string(bookmarks).expect("BookmarkData is always serializable");
    Some(format!("```bgraph-bookmarks\n{json}\n```"))
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
        schema: BGRAPH_MD_FORMAT_VERSION,
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
/// Body placement follows spec convention C-3: content fences carry
/// body on the line(s) preceding the fence; metadata fences (doc-level,
/// bookmarks) have no body outside. Section gains an `#`-prefix heading
/// line; all other content variants emit body verbatim.
fn emit_node(node: &DocumentNode) -> Option<String> {
    let meta = node_metadata_json(node);
    let text = &node.content.text;
    match node.node_type.as_str() {
        "Document" => None, // synthetic root; not a content node
        "Section" => {
            let prefix = heading_prefix(node.location.semantic.depth as usize);
            Some(format!("{prefix} {text}\n```bgraph-section\n{meta}\n```"))
        }
        // All content variants share the body-outside shape under v2.0.0
        // (spec convention C-3). Tag derives from node_type lowercased
        // per C-2.
        "Paragraph" | "Header" | "Footer" | "Margin" | "CodeBlock" | "List" | "Blockquote"
        | "Table" => {
            let tag = node.node_type.to_ascii_lowercase();
            Some(format!("{text}\n```bgraph-{tag}\n{meta}\n```"))
        }
        // Defense-in-depth: every variant in `SemanticElementType`
        // should have an explicit arm above. Reaching here means a
        // schema addition snuck through without a corresponding spec
        // amendment and emitter update.
        other => panic!(
            "emit_markdown (bgraph.md): variant '{other}' has no fence-tag mapping; \
             schema added a variant without a corresponding spec amendment + emitter \
             arm in graphs/serialization/markdown.rs",
        ),
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
        token_count: usize,
    }
    let meta = NodeMetadata {
        id: &node.id,
        node_type: &node.node_type,
        location: &node.location,
        text_order: &node.text_order,
        token_count: node.token_count,
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
    fn header_footer_margin_body_is_outside_fence() {
        // v2.0.0 (CR-48): H/F/M unified with Section/Paragraph —
        // body precedes the fence on the immediately adjacent line.
        let graph = build_graph(vec![
            ("Header", "Running header text", 1, 0),
            ("Footer", "Running footer text", 1, 1),
            ("Margin", "Margin note text", 1, 2),
        ]);
        let md = emit_markdown(&graph);
        assert!(
            md.contains("Running header text\n```bgraph-header\n"),
            "header body should precede the fence; got:\n{md}",
        );
        assert!(
            md.contains("Running footer text\n```bgraph-footer\n"),
            "footer body should precede the fence; got:\n{md}",
        );
        assert!(
            md.contains("Margin note text\n```bgraph-margin\n"),
            "margin body should precede the fence; got:\n{md}",
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
             {{\"id\":\"{section_id}\",\"node_type\":\"Section\",\"location\":{{\"semantic\":{{\"path\":\"1\",\"depth\":1,\"breadcrumbs\":[]}},\"physical\":null}},\"text_order\":0,\"token_count\":1}}\n\
             ```\n\
             \n\
             Hello.\n\
             ```bgraph-paragraph\n\
             {{\"id\":\"{paragraph_id}\",\"node_type\":\"Paragraph\",\"location\":{{\"semantic\":{{\"path\":\"2\",\"depth\":1,\"breadcrumbs\":[]}},\"physical\":null}},\"text_order\":1,\"token_count\":1}}\n\
             ```\n\
             ",
        );
        assert_eq!(after_doc, expected, "body shape drifted from template");
    }

    #[test]
    fn emit_includes_token_count_in_per_element_metadata() {
        // Spec Amendment D (v1.0.0): every per-element bgraph block
        // carries `token_count` so external consumers can query
        // token-weighted slices without re-tokenizing body content.
        let graph = build_graph(vec![
            ("Section", "Intro", 1, 0),
            ("Paragraph", "Hello world.", 1, 1),
            ("Header", "Running header", 1, 2),
        ]);
        let md = emit_markdown(&graph);
        // build_graph sets token_count = 1 for every body node, so every
        // per-element block should contain `"token_count":1`.
        let occurrences = md.matches("\"token_count\":1").count();
        assert_eq!(
            occurrences, 3,
            "expected 3 token_count fields (one per body node); got:\n{md}",
        );
    }

    #[test]
    fn bookmarks_fence_is_omitted_when_bookmark_data_is_none() {
        let graph = build_graph(vec![("Section", "Intro", 1, 0)]);
        // build_graph sets bookmark_data: None.
        let md = emit_markdown(&graph);
        assert!(
            !md.contains("```bgraph-bookmarks"),
            "bgraph-bookmarks fence should be omitted when bookmark_data is None; got:\n{md}",
        );
    }

    #[test]
    fn bookmarks_fence_is_emitted_when_bookmark_data_is_present() {
        let mut graph = build_graph(vec![("Section", "Intro", 1, 0)]);
        graph.document_info.bookmark_data = Some(BookmarkData {
            sections: vec![
                BookmarkSection {
                    title: "Introduction".to_string(),
                    order: 0,
                    level: 1,
                },
                BookmarkSection {
                    title: "Background".to_string(),
                    order: 1,
                    level: 2,
                },
            ],
        });
        let md = emit_markdown(&graph);

        // Fence appears.
        assert!(
            md.contains("```bgraph-bookmarks\n"),
            "bgraph-bookmarks fence should be present when bookmark_data is Some; got:\n{md}",
        );

        // Fence content parses as JSON with the expected shape.
        let start = md
            .find("```bgraph-bookmarks\n")
            .expect("fence open present")
            + "```bgraph-bookmarks\n".len();
        let end = md[start..].find("\n```").expect("fence close present") + start;
        let json_line = &md[start..end];
        let parsed: BookmarkData =
            serde_json::from_str(json_line).expect("bookmarks JSON parses as BookmarkData");
        assert_eq!(parsed.sections.len(), 2);
        assert_eq!(parsed.sections[0].title, "Introduction");

        // Placement: bookmarks fence sits between the doc-level block
        // and the first per-element fence.
        let doc_level_close = md.find("```\n\n```bgraph-bookmarks").expect(
            "bookmarks fence should follow the doc-level block, separated by exactly one blank line",
        );
        let first_section = md.find("```bgraph-section").expect("section fence");
        assert!(
            doc_level_close < first_section,
            "bookmarks fence must precede the first per-element fence",
        );
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

    // ----- Amendment F (B6, schema 0.7.0+) emit tests -----------------

    #[test]
    fn emit_codeblock_node_body_outside_fence_metadata_inside() {
        let raw = "```rust\nfn main() {}\n```";
        let graph = build_graph(vec![("CodeBlock", raw, 2, 0)]);
        let md = emit_markdown(&graph);
        // Body precedes the bgraph fence on the immediately adjacent
        // line, same shape as Section/Paragraph.
        assert!(
            md.contains("```\n```bgraph-codeblock\n"),
            "CodeBlock body should be outside the bgraph fence; got:\n{md}"
        );
        // Body itself survives verbatim (fence + language tag + body).
        assert!(
            md.contains("```rust\nfn main() {}\n```"),
            "CodeBlock body should be verbatim; got:\n{md}"
        );
    }

    #[test]
    fn emit_list_node_body_outside() {
        let raw = "- one\n- two";
        let graph = build_graph(vec![("List", raw, 2, 0)]);
        let md = emit_markdown(&graph);
        assert!(
            md.contains("- one\n- two\n```bgraph-list\n"),
            "List body should be outside the bgraph fence; got:\n{md}"
        );
    }

    #[test]
    fn emit_blockquote_node_body_outside() {
        let raw = "> quoted\n> still";
        let graph = build_graph(vec![("Blockquote", raw, 2, 0)]);
        let md = emit_markdown(&graph);
        assert!(
            md.contains("> quoted\n> still\n```bgraph-blockquote\n"),
            "Blockquote body should be outside the fence; got:\n{md}"
        );
    }

    #[test]
    fn emit_table_node_body_outside() {
        let raw = "| a | b |\n|---|---|\n| 1 | 2 |";
        let graph = build_graph(vec![("Table", raw, 2, 0)]);
        let md = emit_markdown(&graph);
        assert!(
            md.contains("| a | b |\n|---|---|\n| 1 | 2 |\n```bgraph-table\n"),
            "Table body should be outside the fence; got:\n{md}"
        );
    }

    #[test]
    #[should_panic(expected = "no fence-tag mapping")]
    fn emit_panics_on_truly_unknown_variant() {
        // Defense-in-depth: a node_type string that has no arm in
        // emit_node must panic — the spec/schema/emitter sync is
        // load-bearing.
        let graph = build_graph(vec![("UnknownVariant", "x", 1, 0)]);
        let _ = emit_markdown(&graph);
    }

    // ========================================================================
    // v2.0.0 Convention enforcement tests + emitter whitespace contract
    // ========================================================================
    //
    // These tests enforce the conventions documented in
    // `docs/P2/core/architecture/08-bgraph-md-format.md` § Conventions
    // and § Emitter whitespace contract. They are the executable form
    // of the spec — a canonical-emit output that violates any
    // convention fails one of these tests.
    //
    // H/F/M body-outside coverage (the full v2.0.0 C-3 assertion) lands
    // with CR-48. The non-H/F/M conventions below hold against current
    // code and serve as the regression guard for the parts of v2.0.0
    // that are already true.
    //
    // See `docs/P2/core/change-requests/CR-48-header-footer-margin-body-outside-unification.md`.

    #[test]
    fn convention_c1_doc_level_fence_is_bare_bgraph() {
        // C-1: The first fence in every bgraph.md file is the literal
        // ```bgraph (no -<suffix>).
        let graph = build_graph(vec![("Paragraph", "body", 1, 0)]);
        let md = emit_markdown(&graph);
        let first_line = md.lines().next().expect("canonical emit cannot be empty");
        assert_eq!(
            first_line, "```bgraph",
            "C-1: first fence must be bare ```bgraph (no dash suffix); got: {first_line:?}"
        );
    }

    #[test]
    fn convention_c2_per_element_fences_use_lowercase_tag() {
        // C-2: Every non-doc-level fence opens with ```bgraph-<tag>
        // where <tag> is the lowercase node_type.
        let variants = [
            "Section",
            "Paragraph",
            "Header",
            "Footer",
            "Margin",
            "CodeBlock",
            "List",
            "Blockquote",
            "Table",
        ];
        for variant in &variants {
            let graph = build_graph(vec![(variant, "text", 1, 0)]);
            let md = emit_markdown(&graph);
            let expected_tag = format!("```bgraph-{}", variant.to_ascii_lowercase());
            assert!(
                md.contains(&expected_tag),
                "C-2: variant {variant} must emit fence {expected_tag}; got:\n{md}"
            );
        }
    }

    #[test]
    fn convention_c3_body_outside_for_all_content_variants() {
        // C-3 (v2.0.0): every content fence has body text on the
        // line(s) immediately preceding the fence open. Covers all 9
        // content variants (CR-48 unified H/F/M with the rest).
        let cases = [
            ("Section", "intro-marker"),
            ("Paragraph", "para-marker"),
            ("Header", "header-marker"),
            ("Footer", "footer-marker"),
            ("Margin", "margin-marker"),
            ("CodeBlock", "code-marker"),
            ("List", "list-marker"),
            ("Blockquote", "quote-marker"),
            ("Table", "table-marker"),
        ];
        for (variant, text_marker) in &cases {
            let graph = build_graph(vec![(variant, text_marker, 1, 0)]);
            let md = emit_markdown(&graph);
            let tag_line = format!("```bgraph-{}", variant.to_ascii_lowercase());
            let lines: Vec<&str> = md.lines().collect();
            let fence_idx = lines
                .iter()
                .position(|l| *l == tag_line)
                .unwrap_or_else(|| panic!("expected to find {tag_line} in:\n{md}"));
            assert!(fence_idx > 0, "C-3: {variant} fence cannot be first line");
            let preceding = lines[fence_idx - 1];
            assert!(
                preceding.contains(text_marker),
                "C-3: {variant} body must precede the fence; got preceding line {preceding:?} in:\n{md}"
            );
        }
    }

    #[test]
    fn convention_c3_bookmarks_metadata_fence_has_no_body_outside() {
        // C-3 (metadata side): bgraph-bookmarks is a metadata fence;
        // no body content precedes it. The line immediately before the
        // fence-open is the blank-line separator from the doc-level
        // block.
        let mut graph = build_graph(vec![("Paragraph", "body", 1, 0)]);
        graph.document_info.bookmark_data = Some(BookmarkData {
            sections: vec![BookmarkSection {
                title: "Intro".to_string(),
                order: 0,
                level: 1,
            }],
        });
        let md = emit_markdown(&graph);
        let lines: Vec<&str> = md.lines().collect();
        let bookmarks_idx = lines
            .iter()
            .position(|l| *l == "```bgraph-bookmarks")
            .expect("bookmarks fence must be emitted when bookmark_data is Some");
        assert!(
            bookmarks_idx > 0,
            "bookmarks fence cannot be the first line"
        );
        assert_eq!(
            lines[bookmarks_idx - 1],
            "",
            "C-3: metadata fence (bookmarks) must be preceded by a blank line, not body content; \
             got preceding line {:?}",
            lines[bookmarks_idx - 1]
        );
    }

    #[test]
    fn convention_c4_per_element_fences_in_text_order_ascending() {
        // C-4: per-element content fences appear in text_order
        // ascending. Construct with text_order out of insertion order;
        // assert output respects ascending order.
        let graph = build_graph(vec![
            ("Paragraph", "third-body", 1, 2),
            ("Paragraph", "first-body", 1, 0),
            ("Paragraph", "second-body", 1, 1),
        ]);
        let md = emit_markdown(&graph);
        let first_pos = md.find("first-body").expect("first-body must appear");
        let second_pos = md.find("second-body").expect("second-body must appear");
        let third_pos = md.find("third-body").expect("third-body must appear");
        assert!(
            first_pos < second_pos && second_pos < third_pos,
            "C-4: per-element fences must appear in text_order ascending; got positions {first_pos}, {second_pos}, {third_pos}"
        );
    }

    #[test]
    fn convention_c5_body_text_is_trimmed_on_construction() {
        // C-5 (trim side): NodeContent::new strips leading/trailing
        // whitespace.
        let nc = NodeContent::new("  trimmed body  \n".to_string());
        assert_eq!(
            nc.text, "trimmed body",
            "C-5: NodeContent::new must trim leading/trailing whitespace"
        );
    }

    #[test]
    fn convention_c5_exactly_one_blank_line_between_top_level_fences() {
        // C-5 (separator side): after every ```close, the next line is
        // either empty (blank-line separator) or EOF. The line after
        // that (if present) must NOT be empty — no double blank lines
        // between top-level fences.
        let graph = build_graph(vec![
            ("Section", "Intro", 1, 0),
            ("Paragraph", "First para.", 1, 1),
            ("Paragraph", "Second para.", 1, 2),
        ]);
        let md = emit_markdown(&graph);
        let lines: Vec<&str> = md.lines().collect();
        for i in 0..lines.len() {
            if lines[i] == "```" {
                if i + 1 < lines.len() {
                    assert_eq!(
                        lines[i + 1], "",
                        "C-5: line after ```close (line {i}) must be blank-line separator; got: {:?}\nFull output:\n{md}",
                        lines[i + 1]
                    );
                }
                if i + 2 < lines.len() {
                    assert_ne!(
                        lines[i + 2], "",
                        "C-5: no double blank lines between fences; got blank at line {}\nFull output:\n{md}",
                        i + 2
                    );
                }
            }
        }
    }

    #[test]
    fn whitespace_contract_no_trailing_whitespace_on_lines() {
        let graph = build_graph(vec![
            ("Section", "Intro", 1, 0),
            ("Paragraph", "Body text.", 1, 1),
        ]);
        let md = emit_markdown(&graph);
        for (i, line) in md.lines().enumerate() {
            assert_eq!(
                line,
                line.trim_end(),
                "whitespace contract: line {i} has trailing whitespace: {line:?}"
            );
        }
    }

    #[test]
    fn whitespace_contract_ends_with_single_newline() {
        let graph = build_graph(vec![("Paragraph", "body", 1, 0)]);
        let md = emit_markdown(&graph);
        assert!(
            md.ends_with('\n'),
            "whitespace contract: canonical emit must end with newline"
        );
        assert!(
            !md.ends_with("\n\n"),
            "whitespace contract: canonical emit must end with exactly one newline (not two)"
        );
    }
}
