//! B6 integration tests for the generic markdown channel.
//!
//! Two halves:
//!
//! 1. **Generic markdown round-trip:** `parse → emit → equal`. The
//!    forcing function is `canonical(parse(emit(parse(input)))) ==
//!    canonical(parse(input))` rather than strict
//!    `emit(parse(input)) == input` byte-identity — setext heading
//!    normalization, list-marker normalization, and similar invertible
//!    transforms are allowed. The semantic round-trip is what matters.
//!
//! 2. **bgraph.md round-trip on Amendment F variants:** every variant
//!    in `SemanticElementType` (including the four B6 additions —
//!    CodeBlock, List, Blockquote, Table) survives a full bgraph.md
//!    `emit → parse → canonical-bytes equal` round-trip.
//!
//! 3. **Cross-channel composition:** a graph from `generic_md::parse`
//!    can also be emitted to bgraph.md and parsed back — both wire
//!    formats reach the same `DocumentGraph` shape.

use blazegraph_io_core::graphs::builder::GraphBuilder;
use blazegraph_io_core::graphs::node_id::NodeIdGenerator;
use blazegraph_io_core::graphs::serialization::canonical::canonical_json;
use blazegraph_io_core::graphs::serialization::markdown::emit_markdown as emit_bgraph_md;
use blazegraph_io_core::graphs::serialization::markdown_generic::emit_markdown as emit_generic_md;
use blazegraph_io_core::preprocessors::md::{
    bgraph_md, generic_md, parse_markdown, ParseIdentity, ParseOptions,
};
use blazegraph_io_core::types::*;
use std::path::PathBuf;

// =========================================================================
// Helpers
// =========================================================================

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test_fixtures/markdown")
}

fn parse_generic(input: &str) -> DocumentGraph {
    generic_md::parse(input, ParseOptions::default())
        .expect("generic markdown should parse")
        .graph
}

/// Build a synthetic graph for the Amendment F bgraph.md round-trip
/// tests. Mirrors `bgraph_md::tests::build_synthetic_graph` but lives
/// here so the integration tests can compose the new variants.
fn build_synthetic_graph(nodes_in: Vec<(&str, &str, u32, u32)>) -> DocumentGraph {
    let provenance = ParseProvenance {
        blazegraph_version: "0.7.0-b6-test".to_string(),
        source_format: "markdown".to_string(),
        source_filename: "amendment-f.md".to_string(),
        source_sha256: "amendment-f-source-sha".to_string(),
        config_hash: "amendment-f-config-hash".to_string(),
    };
    let id_gen = NodeIdGenerator::new(&provenance.source_sha256, &provenance.config_hash);
    let elements: Vec<SemanticTreeElement> = nodes_in
        .iter()
        .map(|(node_type, text, depth, text_order)| {
            let element_type = match *node_type {
                "Section" => SemanticElementType::Section,
                "Paragraph" => SemanticElementType::Paragraph,
                "Header" => SemanticElementType::Header,
                "Footer" => SemanticElementType::Footer,
                "Margin" => SemanticElementType::Margin,
                "CodeBlock" => SemanticElementType::CodeBlock,
                "List" => SemanticElementType::List,
                "Blockquote" => SemanticElementType::Blockquote,
                "Table" => SemanticElementType::Table,
                other => panic!("unknown node_type {other:?}"),
            };
            SemanticTreeElement {
                text: text.to_string(),
                element_type,
                hierarchy_level: *depth,
                text_order: *text_order,
                physical_location: None,
                style: None,
                token_count: text.split_whitespace().count(),
            }
        })
        .collect();
    let mut graph = GraphBuilder::new()
        .build_graph_deterministic(elements, &id_gen, provenance)
        .expect("synthetic graph builds");
    graph.structural_profile.flow_type = FlowType::Free;
    graph.compute_structural_profile();
    graph.compute_breadcrumbs();
    graph
}

// =========================================================================
// Generic markdown round-trip
// =========================================================================

/// Compare two graphs ignoring provenance (which is derived from
/// `source_sha256(input_bytes)` and therefore differs when the
/// input bytes differ across parse→emit→parse). What matters for
/// semantic round-trip is the tree shape + body content; IDs being
/// different is a consequence of source-bytes drift, not a structural
/// drift.
fn assert_semantically_equal(g1: &DocumentGraph, g2: &DocumentGraph) {
    let nodes_in_order = |g: &DocumentGraph| {
        let mut nodes: Vec<&DocumentNode> = g
            .nodes
            .values()
            .filter(|n| n.text_order.is_some())
            .collect();
        nodes.sort_by_key(|n| n.text_order.expect("filtered"));
        nodes
            .iter()
            .map(|n| {
                (
                    n.node_type.clone(),
                    n.content.text.clone(),
                    n.location.semantic.depth,
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        nodes_in_order(g1),
        nodes_in_order(g2),
        "semantic round-trip drift (node_type/text/depth shape differs)"
    );
    assert_eq!(
        g1.document_info.document_metadata.title, g2.document_info.document_metadata.title,
        "title drift"
    );
    assert_eq!(
        g1.document_info.document_metadata.author, g2.document_info.document_metadata.author,
        "author drift"
    );
    assert_eq!(
        g1.document_info.document_metadata.date, g2.document_info.document_metadata.date,
        "date drift"
    );
    assert_eq!(
        g1.document_info.document_metadata.tags, g2.document_info.document_metadata.tags,
        "tags drift"
    );
    assert_eq!(
        g1.document_info.document_metadata.draft, g2.document_info.document_metadata.draft,
        "draft drift"
    );
    assert_eq!(
        g1.document_info.document_metadata.extras, g2.document_info.document_metadata.extras,
        "extras drift"
    );
}

#[test]
fn roundtrip_identity_simple_doc() {
    // Forcing function: emit(parse(input)) == input on a simple
    // synthetic doc. If emit's separator/trailing-newline conventions
    // match the parser's slice rule, the bytes round-trip.
    let input = "# Hello\n\nWorld.\n";
    let g1 = parse_generic(input);
    let emitted = emit_generic_md(&g1);
    assert_eq!(
        emitted, input,
        "byte-identical round-trip drift; emitted (left) vs input (right)"
    );
    // Sanity: the emitted output also round-trips through parse
    // semantically.
    let g2 = parse_generic(&emitted);
    assert_semantically_equal(&g1, &g2);
}

#[test]
fn roundtrip_identity_with_frontmatter() {
    // Frontmatter shape survives, but byte-identity requires the
    // emitter to mirror the input order/quoting. The emit-order is
    // fixed (`title`, `author`, `date`, `description`, `draft`,
    // `tags`, then extras); this input matches that order.
    let input = "---\n\
                 title: Roundtrip\n\
                 author: B6\n\
                 tags: [a, b, c]\n\
                 ---\n\
                 # Body\n\
                 \n\
                 Text.\n";
    let g1 = parse_generic(input);
    let emitted = emit_generic_md(&g1);
    assert_eq!(
        emitted, input,
        "byte-identical frontmatter round-trip drift; emitted (left) vs input (right)"
    );
    let g2 = parse_generic(&emitted);
    assert_semantically_equal(&g1, &g2);
}

#[test]
fn roundtrip_identity_codeblock_in_paragraph() {
    let input = "# Code\n\n```rust\nfn main() {}\n```\n";
    let g1 = parse_generic(input);
    // The CodeBlock should land as its own node.
    let body_nodes: Vec<_> = g1
        .nodes
        .values()
        .filter(|n| n.text_order.is_some())
        .collect();
    assert!(
        body_nodes.iter().any(|n| n.node_type == "CodeBlock"),
        "input with a fenced code block should produce a CodeBlock node; got nodes: {:?}",
        body_nodes.iter().map(|n| &n.node_type).collect::<Vec<_>>()
    );
    let emitted = emit_generic_md(&g1);
    assert_eq!(emitted, input, "byte-identical round-trip drift");
    let g2 = parse_generic(&emitted);
    assert_semantically_equal(&g1, &g2);
}

#[test]
fn roundtrip_identity_nested_list() {
    let input = "# Lists\n\n- top one\n  - nested\n  - nested two\n- top two\n";
    let g1 = parse_generic(input);
    let emitted = emit_generic_md(&g1);
    assert_eq!(emitted, input, "byte-identical round-trip drift");
    let g2 = parse_generic(&emitted);
    assert_semantically_equal(&g1, &g2);
}

#[test]
fn roundtrip_identity_table() {
    let input = "# Table\n\n| a | b |\n|---|---|\n| 1 | 2 |\n";
    let g1 = parse_generic(input);
    let emitted = emit_generic_md(&g1);
    assert_eq!(emitted, input, "byte-identical round-trip drift");
    let g2 = parse_generic(&emitted);
    assert_semantically_equal(&g1, &g2);
}

#[test]
fn roundtrip_identity_real_content_sample() {
    // Real-content fixture: an in-tree sample that exercises
    // frontmatter + every variant the generic-md emitter can write.
    // The semantic forcing function is "parse(emit(parse(fixture)))
    // == parse(fixture)" — separator normalization (e.g., blank-line
    // counts between blocks) is invertible.
    let fixture_path = fixtures_dir().join("round_trip_sample.md");
    let input = std::fs::read_to_string(&fixture_path).unwrap_or_else(|e| {
        panic!(
            "fixture missing — expected at {}: {e}",
            fixture_path.display()
        )
    });

    let g1 = parse_generic(&input);
    let emitted = emit_generic_md(&g1);
    let g2 = parse_generic(&emitted);

    // Semantic invariant: same shape, same text per node, same
    // metadata. Byte-identity on the real-content fixture is a
    // separate (stricter) check below — we run it but document
    // deviations rather than panic if separator handling drifts.
    assert_semantically_equal(&g1, &g2);

    // Byte-identity check on the real-content fixture. Setext-style
    // headings, list-marker normalization, and similar invertible
    // transforms could cause drift here even when the semantic shape
    // is preserved; this fixture intentionally uses ATX-only
    // headings + a single list-marker style so byte-identity holds.
    if emitted != input {
        // Diagnostic dump so failure modes are easy to read; the
        // assertion still fires.
        eprintln!("--- INPUT ---\n{input}");
        eprintln!("--- EMITTED ---\n{emitted}");
    }
    assert_eq!(
        emitted, input,
        "real-content fixture byte-identity drift — investigate \
         separator handling or fixture content"
    );
}

// =========================================================================
// bgraph.md round-trip on Amendment F variants (B6 / formerly B7)
// =========================================================================

#[test]
fn bgraph_md_roundtrip_codeblock_identity() {
    let original = build_synthetic_graph(vec![("CodeBlock", "```rust\nfn x() {}\n```", 1, 0)]);
    let md = emit_bgraph_md(&original);
    let result = bgraph_md::parse(&md, ParseOptions::default()).expect("round-trip parses");
    assert!(matches!(result.identity, ParseIdentity::Verified));
    assert_eq!(canonical_json(&result.graph), canonical_json(&original));
}

#[test]
fn bgraph_md_roundtrip_list_identity() {
    let original = build_synthetic_graph(vec![("List", "- one\n- two\n- three", 1, 0)]);
    let md = emit_bgraph_md(&original);
    let result = bgraph_md::parse(&md, ParseOptions::default()).expect("round-trip parses");
    assert!(matches!(result.identity, ParseIdentity::Verified));
    assert_eq!(canonical_json(&result.graph), canonical_json(&original));
}

#[test]
fn bgraph_md_roundtrip_blockquote_identity() {
    let original = build_synthetic_graph(vec![("Blockquote", "> quoted\n> still", 1, 0)]);
    let md = emit_bgraph_md(&original);
    let result = bgraph_md::parse(&md, ParseOptions::default()).expect("round-trip parses");
    assert!(matches!(result.identity, ParseIdentity::Verified));
    assert_eq!(canonical_json(&result.graph), canonical_json(&original));
}

#[test]
fn bgraph_md_roundtrip_table_identity() {
    let original = build_synthetic_graph(vec![("Table", "| a | b |\n|---|---|\n| 1 | 2 |", 1, 0)]);
    let md = emit_bgraph_md(&original);
    let result = bgraph_md::parse(&md, ParseOptions::default()).expect("round-trip parses");
    assert!(matches!(result.identity, ParseIdentity::Verified));
    assert_eq!(canonical_json(&result.graph), canonical_json(&original));
}

#[test]
fn bgraph_md_roundtrip_mixed_variants_identity() {
    // Section + Paragraph + each Amendment F variant in the same doc.
    let original = build_synthetic_graph(vec![
        ("Section", "Intro", 1, 0),
        ("Paragraph", "Prose.", 1, 1),
        ("CodeBlock", "```\ncode body\n```", 1, 2),
        ("List", "- a\n- b", 1, 3),
        ("Blockquote", "> q", 1, 4),
        ("Table", "| h |\n|---|\n| c |", 1, 5),
    ]);
    let md = emit_bgraph_md(&original);
    let result = bgraph_md::parse(&md, ParseOptions::default()).expect("round-trip parses");
    assert!(matches!(result.identity, ParseIdentity::Verified));
    assert_eq!(canonical_json(&result.graph), canonical_json(&original));
}

// =========================================================================
// Cross-channel composition (Section 11.6 of the handoff)
// =========================================================================

#[test]
fn generic_md_then_bgraph_md_canonical_equal() {
    // Cross-channel composition (handoff §11.6).
    //
    // **Important caveat:** bgraph.md v1.0.0 doc-level block only
    // carries `title` from `DocumentMetadata` — author, date, tags,
    // draft, extras are all lost on round-trip through bgraph.md.
    // So `canonical(g) == canonical(bgraph_parse(bgraph_emit(g)))`
    // only holds for graphs whose metadata is empty-or-title-only.
    // The handoff section 11.6 over-promises; the underlying
    // bgraph.md format is the constraint.
    //
    // What we *can* assert: the body shape (nodes + their text, in
    // order) survives the cross-channel composition. That's the
    // load-bearing invariant — bgraph.md doesn't lose body content
    // for any variant in `SemanticElementType` post-Amendment F.
    //
    // We use a synthetic graph here (rather than the rich-frontmatter
    // fixture) so the title-only constraint is satisfied and the
    // full canonical equality holds.
    let original = build_synthetic_graph(vec![
        ("Section", "Intro", 1, 0),
        ("Paragraph", "Prose body.", 1, 1),
        ("CodeBlock", "```rust\nfn ok() {}\n```", 1, 2),
        ("List", "- a\n- b", 1, 3),
        ("Blockquote", "> q", 1, 4),
        ("Table", "| h |\n|---|\n| c |", 1, 5),
    ]);
    let bgraph_str = emit_bgraph_md(&original);
    let result = parse_markdown(&bgraph_str, ParseOptions::default())
        .expect("bgraph.md round-trips through the unified dispatcher");
    assert!(matches!(result.identity, ParseIdentity::Verified));
    assert_eq!(
        canonical_json(&result.graph),
        canonical_json(&original),
        "cross-channel canonical drift on synthetic graph"
    );
}

#[test]
fn generic_md_to_bgraph_md_preserves_body_shape() {
    // Weaker invariant for the rich-frontmatter fixture: body shape
    // survives cross-channel composition, even though metadata fields
    // beyond `title` don't round-trip through bgraph.md v1.0.0.
    let fixture_path = fixtures_dir().join("round_trip_sample.md");
    let input = std::fs::read_to_string(&fixture_path).expect("fixture present");
    let g1 = parse_generic(&input);
    let bgraph_str = emit_bgraph_md(&g1);
    let result = parse_markdown(
        &bgraph_str,
        ParseOptions {
            accept_drift: true, // metadata loss → hash drift; we accept it here
        },
    )
    .expect("bgraph.md from rich-frontmatter graph parses");
    // Verify the node tree shape + body text matches.
    let nodes_shape = |g: &DocumentGraph| {
        let mut nodes: Vec<&DocumentNode> = g
            .nodes
            .values()
            .filter(|n| n.text_order.is_some())
            .collect();
        nodes.sort_by_key(|n| n.text_order.expect("filtered"));
        nodes
            .iter()
            .map(|n| (n.node_type.clone(), n.content.text.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        nodes_shape(&g1),
        nodes_shape(&result.graph),
        "cross-channel body-shape drift"
    );
}

// =========================================================================
// parse_markdown dispatcher
// =========================================================================

#[test]
fn parse_markdown_routes_generic_to_generic_md_parser() {
    let input = "# Heading\n\nBody.\n";
    let result = parse_markdown(input, ParseOptions::default())
        .expect("generic markdown should parse via dispatcher");
    assert!(matches!(result.identity, ParseIdentity::Verified));
    // Generic-md sets provenance.source_format = "markdown" and
    // config_hash = "none" — distinct from a bgraph.md parse where
    // provenance comes from the embedded doc-level block.
    let prov = result
        .graph
        .document_info
        .parse_provenance
        .as_ref()
        .expect("provenance present");
    assert_eq!(prov.source_format, "markdown");
    assert_eq!(prov.config_hash, "none");
}
