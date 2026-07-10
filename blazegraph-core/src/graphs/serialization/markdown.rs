//! bgraph.md forward emitter — `DocumentGraph` → markdown string.
//!
//! Wire-format spec is the source of truth:
//! `docs/P2/core/architecture/08-bgraph-md-format.md`. The emitted
//! `schema` field is sourced from
//! [`crate::preprocessors::md::BGRAPH_MD_FORMAT_VERSION`].
//!
//! Public surface: [`emit_markdown`] (default options) and
//! [`emit_markdown_with_options`] (opt-in flags). Everything else is
//! private.

use super::canonical;
use crate::preprocessors::md::BGRAPH_MD_FORMAT_VERSION;
use crate::types::*;
use serde::Serialize;

/// Emitter options. Defaults are the wire-format default — anything
/// gated behind a flag is opt-in.
///
/// CR-59 (v2.1.0+): `include_style_info` gates whether the per-element
/// JSON carries the `style` field. CR-45 introduced the field but
/// shipped with a default of "always emit"; CR-59 reverted the default
/// to opt-in because the 178-line-per-Shannon bloat outweighed the
/// debug-readability benefit. The in-memory pipeline still populates
/// `DocumentNode.style_info` regardless — library consumers of the
/// `Graph` data structure see style on every PDF-source body node. Only
/// the bgraph.md serializer gates on the flag.
#[derive(Debug, Clone, Copy, Default)]
pub struct EmitOptions {
    /// When `true`, the per-element JSON carries the `style` field for
    /// every node whose `style_info` is `Some(...)`. When `false`
    /// (default), the `style` field is omitted unconditionally. Round-
    /// trip identity holds in both modes; the parser tolerates either
    /// shape on input.
    ///
    /// CR-84 / CR-86: **debug/inspection-only.** This gate exists so
    /// `style_info` can be inspected on the wire; the production emit
    /// path is the default (style omitted). The style round-trip
    /// question is CR-86's, deferred.
    pub include_style_info: bool,
}

/// Emit a `DocumentGraph` to bgraph.md format. Targets the current
/// [`BGRAPH_MD_FORMAT_VERSION`]. Uses [`EmitOptions::default()`] — the
/// wire-format default (no opt-in flags set).
///
/// `provenance` is an explicit, compile-time-required argument (Block A
/// / Amendment M): it feeds only the doc-level *envelope* block — never
/// `graph_sha256`, which covers the content body alone. It used to live
/// on `graph.document_info` (with a runtime `.expect()` here); threading
/// it as a value keeps zero hidden state on `DocumentGraph`.
///
/// For PDF-source graphs whose emitted bgraph.md should carry `style`
/// on every per-element fence, call [`emit_markdown_with_options`] with
/// `EmitOptions { include_style_info: true }`.
pub fn emit_markdown(graph: &DocumentGraph, provenance: &ParseProvenance) -> String {
    emit_markdown_with_options(graph, provenance, EmitOptions::default())
}

/// Emit a `DocumentGraph` to bgraph.md format with explicit options. See
/// [`EmitOptions`] for the available flags and [`emit_markdown`] for the
/// provenance contract.
pub fn emit_markdown_with_options(
    graph: &DocumentGraph,
    provenance: &ParseProvenance,
    opts: EmitOptions,
) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(graph.nodes.len() + 6);
    parts.push(emit_document_level_block(graph, provenance));
    parts.push(String::new()); // blank line after doc-level block

    // v2.1.0+ (CR-56 § I.3): the `bgraph-metadata` fence is REQUIRED on
    // every emitted bgraph.md, even when all fields are null. Placed
    // immediately after the doc-level `bgraph` block, before any
    // `bgraph-outline` fence.
    parts.push(emit_metadata_block(&graph.document_info.document_metadata));
    parts.push(String::new());

    // Optional `bgraph-outline` fence — emitted only when the source
    // graph carries an outline. Placed immediately after `bgraph-metadata`
    // so the doc-level identity block stays a single readable JSON line
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
        if let Some(chunk) = emit_node(node, opts) {
            parts.push(chunk);
            parts.push(String::new()); // blank line between elements
        }
    }

    parts.join("\n")
}

/// Document-level bookmarks block. Tag: `bgraph-outline`. Optional —
/// returns `None` when `graph.document_info.outline_data` is `None`.
/// JSON shape mirrors `BookmarkData` exactly (one `serde_json::to_string`
/// pass, compact, single line).
fn emit_bookmarks_block(graph: &DocumentGraph) -> Option<String> {
    let bookmarks = graph.document_info.outline_data.as_ref()?;
    let json = serde_json::to_string(bookmarks).expect("BookmarkData is always serializable");
    Some(format!("```bgraph-outline\n{json}\n```"))
}

/// Document-extracted metadata block. Tag: `bgraph-metadata`. Carries
/// canonical fields (title, author, description, language, created) plus
/// channel-specific namespaced sub-objects (pdf / md / docx).
///
/// Always emitted by v2.1.0+ even when every field is null — the fence's
/// presence is part of the wire-format contract (CR-56 § I.3).
fn emit_metadata_block(metadata: &DocumentMetadata) -> String {
    let json = serde_json::to_string(metadata).expect("DocumentMetadata is always serializable");
    format!("```bgraph-metadata\n{json}\n```")
}

/// Document-level metadata block. Tag: `bgraph` (no suffix). Flat JSON
/// — this is graph-identity metadata, not a node, so the schema-mirroring
/// rule for per-element blocks doesn't apply.
///
/// CR-57 (v2.1.0+ / Amendment I.4): `title` moves out to the
/// `bgraph-metadata` block. The doc-level `bgraph` block carries only
/// graph identity (schema, version, source, flow_type, config_hash,
/// graph_sha256).
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
        // CR-82: artifact discriminator, emitted right after `schema`.
        // Always present (default `document`); part of graph identity.
        kind: &'a str,
        blazegraph_version: &'a str,
        source: DocLevelSource<'a>,
        flow_type: &'a FlowType,
        // title removed — moved to bgraph-metadata (CR-56 § I.4)
        // CR-49 (v2.1.0+) added `topology` here.
        // CR-60 (2026-05-22) retracted `source_identity` + `supersedes`
        // per the byte-in/byte-out principle (arch doc 11 + DT-04).
        // `topology` stays — parser-known (channel decides) + immutable.
        // Skipped when None so v2.1.0 graphs without the field serialize
        // byte-identical to the pre-CR-49 shape.
        #[serde(skip_serializing_if = "Option::is_none")]
        topology: &'a Option<String>,
        config_hash: &'a str,
        graph_sha256: String,
    }

    let block = DocLevelBlock {
        schema: BGRAPH_MD_FORMAT_VERSION,
        kind: &graph.document_info.kind,
        blazegraph_version: &provenance.blazegraph_version,
        source: DocLevelSource {
            format: &provenance.source_format,
            filename: &provenance.source_filename,
            sha256: &provenance.source_sha256,
        },
        flow_type: &graph.document_info.flow_type,
        topology: &graph.document_info.topology,
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
/// `bgraph-metadata`, `bgraph-outline`) have no body outside. Section
/// gains an `#`-prefix heading line; all other content variants emit
/// body verbatim.
///
/// Fence-tag derivation goes through [`node_type_to_fence_tag`] so
/// multi-word variants get kebab-case per CR-56 § I.5 / F-11
/// (`CodeBlock` → `bgraph-code-block`, `Blockquote` → `bgraph-block-quote`).
fn emit_node(node: &DocumentNode, opts: EmitOptions) -> Option<String> {
    let meta = node_metadata_json(node, opts);
    let text = &node.content.text;
    if node.node_type == "Document" {
        return None; // synthetic root; not a content node
    }
    if node.node_type == "Section" {
        let prefix = heading_prefix(node.location.semantic.depth as usize);
        let tag = node_type_to_fence_tag(&node.node_type);
        return Some(format!("{prefix} {text}\n```bgraph-{tag}\n{meta}\n```"));
    }
    let tag = node_type_to_fence_tag(&node.node_type);
    Some(format!("{text}\n```bgraph-{tag}\n{meta}\n```"))
}

/// Map a graph `node_type` (PascalCase) to its bgraph.md fence-tag
/// (lowercase, kebab-case for multi-word variants per CR-56 § I.5 / F-11).
///
/// Single source of truth for the variant→tag mapping; the parser's
/// dispatch arm in `bgraph_md.rs` accepts exactly these tags. Panics on
/// any variant without an explicit mapping — defense-in-depth so a
/// schema addition cannot reach the emitter without a corresponding spec
/// amendment + arm here.
fn node_type_to_fence_tag(node_type: &str) -> &'static str {
    match node_type {
        "Section" => "section",
        "Paragraph" => "paragraph",
        "Header" => "header",
        "Footer" => "footer",
        "Margin" => "margin",
        "CodeBlock" => "code-block", // F-11 (v2.1.0+; was: codeblock)
        "List" => "list",
        "Blockquote" => "block-quote", // F-11 (v2.1.0+; was: blockquote)
        "Table" => "table",
        // CR-59 (v2.1.0+): the `Message` variant was added by CR-49 as a
        // wire-format precursor to the future stream-topology design slice
        // but had no in-memory carrier path in tree-topology channels.
        // Retracted — `SemanticElementType::Message` survives as an orphan
        // sentinel only; no fence tag, no production path.
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
fn node_metadata_json(node: &DocumentNode, opts: EmitOptions) -> String {
    use crate::types::{ExternalRef, InternalRef};
    #[derive(Serialize)]
    struct NodeMetadata<'a> {
        id: &'a NodeId,
        node_type: &'a String,
        location: &'a NodeLocation,
        text_order: &'a Option<u32>,
        token_count: usize,
        /// CR-62 (v2.3.0+): per-element refs within this document. Omitted
        /// when empty so pre-CR-62 fixtures stay byte-identical when no link
        /// extraction is in play.
        #[serde(skip_serializing_if = "Vec::is_empty")]
        internal_refs: &'a Vec<InternalRef>,
        /// CR-62 (v2.3.0+): per-element refs to external locations.
        #[serde(skip_serializing_if = "Vec::is_empty")]
        external_refs: &'a Vec<ExternalRef>,
        // v4.0.0 (Block A / Amendment M): the CR-78 `confidence` field is
        // gone from the wire — schema-ahead placeholders in the identity
        // form are retired (an empty→populated flip silently churned
        // `graph_sha256`). The parser tolerates it on legacy inputs
        // (unknown fields are dropped).
        /// CR-45: verbatim Tika style projection (foreground / background
        /// color, font_family, font_size, is_bold, is_italic, font_class).
        /// CR-59 (v2.1.0+): gated on `EmitOptions::include_style_info`. When
        /// the flag is `false` (default), this slot is always `None` so
        /// `skip_serializing_if` omits the field entirely — regardless of
        /// whether `node.style_info` is populated. The in-memory carrier
        /// (`DocumentNode.style_info`) stays populated for library
        /// consumers; only the wire-format emission is gated. Shape is
        /// verbatim Tika projection — see DT-03.
        #[serde(skip_serializing_if = "Option::is_none")]
        style: Option<&'a StyleMetadata>,
    }
    // CR-59: style emission is opt-in. When the flag is off we pass
    // `None` regardless of `node.style_info`; `skip_serializing_if`
    // then drops the field.
    let style = if opts.include_style_info {
        node.style_info.as_ref()
    } else {
        None
    };
    let meta = NodeMetadata {
        id: &node.id,
        node_type: &node.node_type,
        location: &node.location,
        text_order: &node.text_order,
        token_count: node.token_count,
        internal_refs: &node.internal_refs,
        external_refs: &node.external_refs,
        style,
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

    /// Synthetic provenance for emit tests. Threaded explicitly into
    /// every emit call (Block A: provenance is an argument, not graph
    /// state).
    fn test_provenance() -> ParseProvenance {
        ParseProvenance {
            blazegraph_version: "0.6.0".to_string(),
            source_format: "markdown".to_string(),
            source_filename: "synthetic.md".to_string(),
            source_sha256: "deadbeef".to_string(),
            config_hash: "cafef00d".to_string(),
        }
    }

    /// Emit with the shared synthetic provenance + default options.
    fn emit(graph: &DocumentGraph) -> String {
        emit_markdown(graph, &test_provenance())
    }

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
                    internal_refs: vec![],
                    external_refs: vec![],
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
                internal_refs: vec![],
                external_refs: vec![],
            },
        );

        DocumentGraph {
            nodes,
            document_info: DocumentInfo {
                root_id,
                kind: crate::types::default_kind(),
                document_metadata: DocumentMetadata {
                    title: Some("Synthetic Test Doc".to_string()),
                    ..DocumentMetadata::default()
                },
                outline_data: None,
                flow_type: FlowType::default(),
                topology: None,
            },
        }
    }

    #[test]
    fn document_root_is_skipped() {
        // Build with one Section; expect output contains the Section
        // fence but no `bgraph-document` (or any other tag matching the
        // root node).
        let graph = build_graph(vec![("Section", "Intro", 1, 0)]);
        let md = emit(&graph);
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
        let md = emit(&graph);
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
    fn confidence_never_appears_on_the_wire_v4() {
        // v4.0.0 (Block A / Amendment M): the CR-78 `confidence` field is
        // gone from DocumentNode and the per-element fence — schema-ahead
        // placeholders in the identity form are retired. No emitted fence
        // may carry the key.
        let graph = build_graph(vec![
            ("Section", "Intro", 1, 0),
            ("Paragraph", "Hello world.", 1, 1),
        ]);
        let md = emit(&graph);
        assert!(
            !md.contains("\"confidence\""),
            "v4.0.0 wire must not carry a confidence key; got:\n{md}",
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
        let md = emit(&graph);
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
        let md = emit(&graph);
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
    fn doc_level_block_has_all_six_identity_fields_no_title() {
        // v2.1.0 (CR-56 § I.4): `title` moved out of the doc-level
        // `bgraph` block into the `bgraph-metadata` block. The `bgraph`
        // block now carries identity-only fields.
        let graph = build_graph(vec![("Paragraph", "Body.", 1, 0)]);
        let md = emit(&graph);

        // First line is the bgraph fence open; second line is the JSON.
        let first_line_end = md.find('\n').expect("multi-line output");
        assert_eq!(
            &md[..first_line_end],
            "```bgraph",
            "first line must be the document-level fence open",
        );

        let after_first = &md[first_line_end + 1..];
        let json_line_end = after_first.find('\n').expect("JSON line + closing fence");
        let json_line = &after_first[..json_line_end];

        let parsed: serde_json::Value =
            serde_json::from_str(json_line).expect("doc-level JSON parses");
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
                "doc-level block missing required key {key:?}; got {json_line}",
            );
        }
        // v2.1.0 contract: `title` MUST NOT appear in the bgraph block.
        assert!(
            parsed.get("title").is_none(),
            "doc-level block must not carry `title` under v2.1.0; got {json_line}"
        );
        let source = parsed.get("source").unwrap();
        for key in ["format", "filename", "sha256"] {
            assert!(
                source.get(key).is_some(),
                "doc-level source block missing required key {key:?}; got {json_line}",
            );
        }
        let h = parsed["graph_sha256"]
            .as_str()
            .expect("graph_sha256 is a string");
        assert_eq!(h.len(), 64, "graph_sha256 should be 64 hex chars; got {h}");
    }

    #[test]
    fn metadata_block_is_emitted_after_doc_level() {
        // v2.1.0 (CR-56 § I.3): the bgraph-metadata fence is REQUIRED on
        // every emitted bgraph.md, immediately after the doc-level block,
        // separated by exactly one blank line.
        let mut graph = build_graph(vec![("Paragraph", "Body.", 1, 0)]);
        graph.document_info.document_metadata.title = Some("My Doc".to_string());
        graph.document_info.document_metadata.author = Some("Alice".to_string());
        let md = emit(&graph);

        // Doc-level close → blank line → bgraph-metadata open.
        assert!(
            md.contains("```\n\n```bgraph-metadata\n"),
            "bgraph-metadata fence must follow the doc-level block with one blank-line separator; \
             got:\n{md}"
        );

        // Payload mirrors DocumentMetadata JSON shape (canonical fields at
        // the top, channel namespaces under named keys).
        let start = md.find("```bgraph-metadata\n").unwrap() + "```bgraph-metadata\n".len();
        let end = md[start..].find("\n```").expect("metadata fence close") + start;
        let json_line = &md[start..end];
        let parsed: serde_json::Value =
            serde_json::from_str(json_line).expect("bgraph-metadata JSON parses");
        assert_eq!(parsed["title"].as_str(), Some("My Doc"));
        assert_eq!(parsed["author"].as_str(), Some("Alice"));
    }

    #[test]
    fn metadata_block_emitted_even_when_metadata_empty() {
        let graph = build_graph(vec![("Paragraph", "Body.", 1, 0)]);
        // build_graph defaults `title: Some("Synthetic Test Doc")` — strip
        // it so we test the truly-empty case too.
        let mut graph = graph;
        graph.document_info.document_metadata = DocumentMetadata::default();
        let md = emit(&graph);
        assert!(
            md.contains("```bgraph-metadata\n"),
            "bgraph-metadata fence MUST be present under v2.1.0 even when all fields are null; \
             got:\n{md}"
        );
    }

    #[test]
    fn synthetic_one_section_one_paragraph_matches_template() {
        // Whole-output literal check: for a synthetic graph with one
        // Section + one Paragraph, the body shape (after the doc-level
        // + bgraph-metadata blocks) must be byte-identical to the
        // template below. The doc-level + metadata blocks change with
        // provenance / graph_sha256 / metadata fields, so we anchor on
        // the section heading line.
        let graph = build_graph(vec![
            ("Section", "Intro", 1, 0),
            ("Paragraph", "Hello.", 1, 1),
        ]);
        let md = emit(&graph);
        let body_start = md
            .find("# Intro\n```bgraph-section\n")
            .expect("section heading should be present");
        let after_doc = &md[body_start..];

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
        let md = emit(&graph);
        // build_graph sets token_count = 1 for every body node, so every
        // per-element block should contain `"token_count":1`.
        let occurrences = md.matches("\"token_count\":1").count();
        assert_eq!(
            occurrences, 3,
            "expected 3 token_count fields (one per body node); got:\n{md}",
        );
    }

    #[test]
    fn bookmarks_fence_is_omitted_when_outline_data_is_none() {
        let graph = build_graph(vec![("Section", "Intro", 1, 0)]);
        // build_graph sets outline_data: None.
        let md = emit(&graph);
        assert!(
            !md.contains("```bgraph-outline"),
            "bgraph-outline fence should be omitted when outline_data is None; got:\n{md}",
        );
    }

    #[test]
    fn bookmarks_fence_is_emitted_when_outline_data_is_present() {
        let mut graph = build_graph(vec![("Section", "Intro", 1, 0)]);
        graph.document_info.outline_data = Some(BookmarkData {
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
        let md = emit(&graph);

        // Fence appears.
        assert!(
            md.contains("```bgraph-outline\n"),
            "bgraph-outline fence should be present when outline_data is Some; got:\n{md}",
        );

        // Fence content parses as JSON with the expected shape.
        let start = md
            .find("```bgraph-outline\n")
            .expect("fence open present")
            + "```bgraph-outline\n".len();
        let end = md[start..].find("\n```").expect("fence close present") + start;
        let json_line = &md[start..end];
        let parsed: BookmarkData =
            serde_json::from_str(json_line).expect("bookmarks JSON parses as BookmarkData");
        assert_eq!(parsed.sections.len(), 2);
        assert_eq!(parsed.sections[0].title, "Introduction");

        // Placement (v2.1.0+): bookmarks fence sits between the
        // bgraph-metadata block and the first per-element fence.
        let metadata_close = md.find("```\n\n```bgraph-outline").expect(
            "bookmarks fence should follow the metadata block, separated by exactly one blank line",
        );
        let first_section = md.find("```bgraph-section").expect("section fence");
        assert!(
            metadata_close < first_section,
            "bookmarks fence must precede the first per-element fence",
        );
    }

    // Block A / Amendment M: `emit_markdown_panics_without_provenance`
    // is gone — provenance is a compile-time-required argument now, so
    // the emitter cannot be reached without it. The type system replaced
    // the runtime panic contract.

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
        eprintln!("{}", emit(&graph));
        eprintln!("--- END emit_markdown sample ---");
    }

    // ----- Amendment F (B6, schema 0.7.0+) emit tests -----------------

    #[test]
    fn emit_codeblock_node_body_outside_fence_metadata_inside() {
        let raw = "```rust\nfn main() {}\n```";
        let graph = build_graph(vec![("CodeBlock", raw, 2, 0)]);
        let md = emit(&graph);
        // F-11 (v2.1.0+): the CodeBlock variant uses kebab-case
        // `bgraph-code-block`.
        assert!(
            md.contains("```\n```bgraph-code-block\n"),
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
        let md = emit(&graph);
        assert!(
            md.contains("- one\n- two\n```bgraph-list\n"),
            "List body should be outside the bgraph fence; got:\n{md}"
        );
    }

    #[test]
    fn emit_blockquote_node_body_outside() {
        let raw = "> quoted\n> still";
        let graph = build_graph(vec![("Blockquote", raw, 2, 0)]);
        let md = emit(&graph);
        // F-11 (v2.1.0+): the Blockquote variant uses kebab-case
        // `bgraph-block-quote`.
        assert!(
            md.contains("> quoted\n> still\n```bgraph-block-quote\n"),
            "Blockquote body should be outside the fence; got:\n{md}"
        );
    }

    #[test]
    fn emit_table_node_body_outside() {
        let raw = "| a | b |\n|---|---|\n| 1 | 2 |";
        let graph = build_graph(vec![("Table", raw, 2, 0)]);
        let md = emit(&graph);
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
        let _ = emit(&graph);
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
        let md = emit(&graph);
        let first_line = md.lines().next().expect("canonical emit cannot be empty");
        assert_eq!(
            first_line, "```bgraph",
            "C-1: first fence must be bare ```bgraph (no dash suffix); got: {first_line:?}"
        );
    }

    #[test]
    fn convention_c2_per_element_fences_use_kebab_case_tag() {
        // C-2 (v2.1.0+ / F-11): Every non-doc-level fence opens with
        // ```bgraph-<tag> where <tag> is the kebab-case lowercased
        // node_type. CodeBlock → bgraph-code-block; Blockquote →
        // bgraph-block-quote; the rest are single-word and unchanged.
        let variants = [
            ("Section", "section"),
            ("Paragraph", "paragraph"),
            ("Header", "header"),
            ("Footer", "footer"),
            ("Margin", "margin"),
            ("CodeBlock", "code-block"),
            ("List", "list"),
            ("Blockquote", "block-quote"),
            ("Table", "table"),
        ];
        for (variant, tag) in &variants {
            let graph = build_graph(vec![(variant, "text", 1, 0)]);
            let md = emit(&graph);
            let expected_tag = format!("```bgraph-{tag}");
            assert!(
                md.contains(&expected_tag),
                "C-2: variant {variant} must emit fence {expected_tag}; got:\n{md}"
            );
        }
    }

    #[test]
    fn convention_c3_body_outside_for_all_content_variants() {
        // C-3 (v2.0.0+ / kebab-case from v2.1.0): every content fence
        // has body text on the line(s) immediately preceding the fence
        // open. Covers all 9 content variants.
        let cases = [
            ("Section", "intro-marker", "section"),
            ("Paragraph", "para-marker", "paragraph"),
            ("Header", "header-marker", "header"),
            ("Footer", "footer-marker", "footer"),
            ("Margin", "margin-marker", "margin"),
            ("CodeBlock", "code-marker", "code-block"),
            ("List", "list-marker", "list"),
            ("Blockquote", "quote-marker", "block-quote"),
            ("Table", "table-marker", "table"),
        ];
        for (variant, text_marker, tag) in &cases {
            let graph = build_graph(vec![(variant, text_marker, 1, 0)]);
            let md = emit(&graph);
            let tag_line = format!("```bgraph-{tag}");
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
        // C-3 (metadata side): bgraph-outline is a metadata fence;
        // no body content precedes it. The line immediately before the
        // fence-open is the blank-line separator from the doc-level
        // block.
        let mut graph = build_graph(vec![("Paragraph", "body", 1, 0)]);
        graph.document_info.outline_data = Some(BookmarkData {
            sections: vec![BookmarkSection {
                title: "Intro".to_string(),
                order: 0,
                level: 1,
            }],
        });
        let md = emit(&graph);
        let lines: Vec<&str> = md.lines().collect();
        let bookmarks_idx = lines
            .iter()
            .position(|l| *l == "```bgraph-outline")
            .expect("bookmarks fence must be emitted when outline_data is Some");
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
        let md = emit(&graph);
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
        let md = emit(&graph);
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
        let md = emit(&graph);
        for (i, line) in md.lines().enumerate() {
            assert_eq!(
                line,
                line.trim_end(),
                "whitespace contract: line {i} has trailing whitespace: {line:?}"
            );
        }
    }

    // ===================================================================
    // CR-49 (v2.1.0+) emit tests: topology doc-level field.
    // CR-60 (2026-05-22) retracted source_identity + supersedes per
    // arch doc 11 + DT-04 (byte-in/byte-out). Only topology remains.
    // ===================================================================

    #[test]
    fn doc_level_block_omits_topology_when_unset() {
        // Default-built graph: no topology.
        // The skip_serializing_if rule must keep it out of the JSON.
        let graph = build_graph(vec![("Paragraph", "body", 1, 0)]);
        let md = emit(&graph);
        assert!(
            !md.contains("\"topology\""),
            "topology must be skipped when None; got:\n{md}"
        );
    }

    #[test]
    fn doc_level_block_emits_topology_when_set() {
        let mut graph = build_graph(vec![("Paragraph", "body", 1, 0)]);
        graph.document_info.topology = Some("stream".to_string());
        let md = emit(&graph);
        // Extract the first JSON line (doc-level block).
        let first_line_end = md.find('\n').expect("multi-line output");
        let after_first = &md[first_line_end + 1..];
        let json_line_end = after_first.find('\n').expect("JSON line + closing fence");
        let json_line = &after_first[..json_line_end];
        let parsed: serde_json::Value =
            serde_json::from_str(json_line).expect("doc-level JSON parses");
        assert_eq!(parsed["topology"].as_str(), Some("stream"));
        // CR-49 position contract: after flow_type, before config_hash.
        let topology_pos = json_line.find("\"topology\"").expect("topology present");
        let flow_pos = json_line.find("\"flow_type\"").expect("flow_type present");
        let config_pos = json_line
            .find("\"config_hash\"")
            .expect("config_hash present");
        assert!(
            flow_pos < topology_pos && topology_pos < config_pos,
            "topology should sit between flow_type and config_hash; got positions {flow_pos}, {topology_pos}, {config_pos}"
        );
    }

    // CR-59 removed `emit_message_node_body_outside_with_variant_metadata`
    // and `non_message_variant_does_not_carry_message_fields` along with
    // the wire-format support for the Message variant. The orphan enum
    // variant + struct remain in `types.rs` as future-design sentinels.

    // ===================================================================
    // CR-59 (v2.1.0+) emit tests: style emit-gating.
    // ===================================================================

    #[test]
    fn style_omitted_by_default_even_when_node_carries_it() {
        // Build a graph and populate `style_info` on its single
        // Paragraph node. With default `EmitOptions` the emitter must
        // omit `style` from the per-element JSON regardless.
        let mut graph = build_graph(vec![("Paragraph", "Body.", 1, 0)]);
        let para_id = graph
            .nodes
            .values()
            .find(|n| n.node_type == "Paragraph")
            .map(|n| n.id)
            .expect("Paragraph node present");
        graph.nodes.get_mut(&para_id).unwrap().style_info = Some(StyleMetadata {
            font_class: "f1".to_string(),
            font_size: Some(10.0),
            is_bold: false,
            is_italic: false,
            font_family: Some("Helvetica".to_string()),
            foreground_color: Some("#000000".to_string()),
            background_color: None,
        });
        let md = emit(&graph);
        assert!(
            !md.contains("\"style\""),
            "default EmitOptions must omit `style` even when node.style_info is Some; got:\n{md}"
        );
    }

    #[test]
    fn style_emitted_when_include_flag_set_and_node_carries_it() {
        let mut graph = build_graph(vec![("Paragraph", "Body.", 1, 0)]);
        let para_id = graph
            .nodes
            .values()
            .find(|n| n.node_type == "Paragraph")
            .map(|n| n.id)
            .expect("Paragraph node present");
        graph.nodes.get_mut(&para_id).unwrap().style_info = Some(StyleMetadata {
            font_class: "f1".to_string(),
            font_size: Some(10.0),
            is_bold: false,
            is_italic: false,
            font_family: Some("Helvetica".to_string()),
            foreground_color: Some("#000000".to_string()),
            background_color: None,
        });
        let md = emit_markdown_with_options(
            &graph,
            &test_provenance(),
            EmitOptions {
                include_style_info: true,
            },
        );
        assert!(
            md.contains("\"style\":{"),
            "include_style_info=true must emit `style` when node.style_info is Some; got:\n{md}"
        );
    }

    #[test]
    fn style_omitted_when_include_flag_set_but_node_lacks_it() {
        // skip_serializing_if=Option::is_none still applies: when the
        // node has no style, the field is absent even with the flag on.
        let graph = build_graph(vec![("Paragraph", "Body.", 1, 0)]);
        let md = emit_markdown_with_options(
            &graph,
            &test_provenance(),
            EmitOptions {
                include_style_info: true,
            },
        );
        assert!(
            !md.contains("\"style\""),
            "with-flag emit must still omit `style` when node.style_info is None; got:\n{md}"
        );
    }

    #[test]
    fn whitespace_contract_ends_with_single_newline() {
        let graph = build_graph(vec![("Paragraph", "body", 1, 0)]);
        let md = emit(&graph);
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
