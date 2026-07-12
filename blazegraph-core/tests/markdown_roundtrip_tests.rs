//! B4 round-trip invariant harness for the bgraph.md wire format.
//!
//! Verifies that `canonical(parse(emit(g))) == canonical(g)` on
//! synthetic graphs and the in-tree Shannon + Euclid stage3 fixtures.
//! Also exercises drift-detection (strict vs `accept_drift`) and a
//! handful of edge cases the spec calls out explicitly.
//!
//! Wire-format definition:
//! `docs/P2/core/architecture/08-bgraph-md-format.md` (v1.0.0).

use blazegraph_io_core::graphs::builder::GraphBuilder;
use blazegraph_io_core::graphs::node_id::NodeIdGenerator;
use blazegraph_io_core::graphs::serialization::canonical::{canonical_json, graph_sha256};
use blazegraph_io_core::graphs::serialization::markdown::{
    emit_markdown, emit_markdown_with_options, EmitOptions,
};
use blazegraph_io_core::graphs::serialization::version::{
    canonicalize_as, emit_markdown_as, FormatVersion,
};
use blazegraph_io_core::preprocessors::md::{
    bgraph_md, parse_markdown, ParseError, ParseIdentity, ParseOptions,
};
use blazegraph_io_core::types::*;
use std::path::PathBuf;

// =========================================================================
// Helpers.
// =========================================================================

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test_fixtures/snapshots")
}

/// Build a synthetic graph through the same path the PDF channel uses,
/// so its IDs / paths / breadcrumbs match what the reverse parser
/// derives.
/// Synthetic provenance for emit calls (Block A: provenance is an
/// explicit emit argument, not graph state).
fn synthetic_provenance() -> ParseProvenance {
    ParseProvenance {
        blazegraph_version: "0.6.0-roundtrip".to_string(),
        source_format: "markdown".to_string(),
        source_filename: "roundtrip.md".to_string(),
        source_sha256: "roundtrip-source-sha".to_string(),
        config_hash: "roundtrip-config-hash".to_string(),
    }
}

fn build_synthetic_graph(
    nodes_in: Vec<(&str, &str, u32, u32)>,
    title: Option<&str>,
    bookmarks: Option<BookmarkData>,
) -> DocumentGraph {
    let id_gen = NodeIdGenerator::new(); // CR-83: content+breadcrumb-derived
    let elements: Vec<SemanticTreeElement> = nodes_in
        .iter()
        .map(|(node_type, text, depth, text_order)| {
            let element_type = match *node_type {
                "Section" => SemanticElementType::Section,
                "Paragraph" => SemanticElementType::Paragraph,
                "Header" => SemanticElementType::Header,
                "Footer" => SemanticElementType::Footer,
                "Margin" => SemanticElementType::Margin,
                other => panic!("unsupported test node type {other:?}"),
            };
            SemanticTreeElement {
                text: text.to_string(),
                element_type,
                hierarchy_level: *depth,
                text_order: *text_order,
                physical_location: None,
                style: None,
                token_count: text.split_whitespace().count(),
                internal_refs: vec![],
                external_refs: vec![],
                confidence: 0,
            }
        })
        .collect();
    let mut graph = GraphBuilder::new()
        .build_graph_deterministic(elements, &id_gen)
        .expect("synthetic graph builds");
    graph.document_info.document_metadata.title = title.map(str::to_string);
    graph.document_info.outline_data = bookmarks;
    graph.document_info.flow_type = FlowType::Free;
    graph.compute_breadcrumbs();
    graph
}

/// Load a stage3 fixture and rebuild it through
/// `GraphBuilder::build_graph_deterministic` with synthetic
/// provenance, so the resulting graph carries deterministic UUIDv5
/// IDs (which is what the reverse parser will derive when it
/// reconstructs the graph from the emitted markdown).
///
/// The stage3 fixtures were captured under the legacy random-UUIDv4
/// build path, so their on-disk IDs don't match what
/// `NodeIdGenerator` would produce. Round-trip identity on fixtures
/// means "the structure + metadata + content survive the emit→parse
/// cycle byte-for-byte" — not "the original random IDs survive". By
/// reconstructing the fixture graph through the deterministic builder
/// once before emit, we get a fair comparison: the canonical bytes of
/// the rebuilt original are byte-for-byte the canonical bytes of the
/// parsed reconstruction.
fn fixture_provenance(name: &str) -> ParseProvenance {
    ParseProvenance {
        blazegraph_version: "0.6.0-test".to_string(),
        source_format: "pdf".to_string(),
        source_filename: format!("{name}.pdf"),
        source_sha256: format!("test-source-sha-{name}"),
        config_hash: "test-config-hash".to_string(),
    }
}

fn load_fixture_graph(name: &str) -> DocumentGraph {
    let path = fixtures_dir().join(name).join("stage3_graph.json");
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "Missing fixture: {}. Run `make test-generate-fixtures`",
            path.display()
        )
    });
    let sorted: SortedDocumentGraph =
        serde_json::from_str(&raw).expect("fixture deserializes as SortedDocumentGraph");

    // Project the fixture's nodes back onto SemanticTreeElement.
    // Document root has text_order = None and is the synthetic root —
    // skip it (the builder creates the root itself).
    let mut body_nodes: Vec<&DocumentNode> = sorted
        .nodes
        .iter()
        .filter(|n| n.text_order.is_some())
        .collect();
    body_nodes.sort_by_key(|n| n.text_order.unwrap());

    let elements: Vec<SemanticTreeElement> = body_nodes
        .iter()
        .map(|n| {
            let element_type = match n.node_type.as_str() {
                "Section" => SemanticElementType::Section,
                "Paragraph" => SemanticElementType::Paragraph,
                "Header" => SemanticElementType::Header,
                "Footer" => SemanticElementType::Footer,
                "Margin" => SemanticElementType::Margin,
                other => panic!(
                    "fixture {name} contains node_type {other:?} not yet supported by bgraph.md v1.0.0"
                ),
            };
            SemanticTreeElement {
                text: n.content.text.clone(),
                element_type,
                hierarchy_level: n.location.semantic.depth,
                text_order: n.text_order.unwrap(),
                physical_location: n.location.physical.clone(),
                // v1.0.0 spec does not carry per-element style.
                style: None,
                token_count: n.token_count,
                internal_refs: vec![],
                external_refs: vec![],
                // Block A / A3: DocumentNode no longer carries confidence;
                // the element-side field stays parser-internal (neutral 0).
                confidence: 0,
            }
        })
        .collect();

    let id_gen = NodeIdGenerator::new(); // CR-83: content+breadcrumb-derived

    let mut graph = GraphBuilder::new()
        .build_graph_deterministic(elements, &id_gen)
        .expect("fixture graph rebuilds deterministically");

    // Carry over the fixture's *title* and bookmarks and flow_type.
    // The bgraph.md v1.0.0 doc-level block carries only `title` from
    // `DocumentMetadata` (not author / created / creator_tool / …),
    // so round-trip identity for the bgraph.md wire format means
    // "title + bookmarks + flow_type + per-element data". Other
    // DocumentMetadata fields are intentionally outside the format
    // (they belong to the source-format channel, not the
    // channel-agnostic graph). Resetting them to defaults here so
    // the "original" matches what the parser will reconstruct.
    graph.document_info.document_metadata = DocumentMetadata {
        title: sorted.document_info.document_metadata.title,
        ..DocumentMetadata::default()
    };
    graph.document_info.outline_data = sorted.document_info.outline_data;
    graph.document_info.flow_type = sorted.document_info.flow_type;

    // Re-derive breadcrumbs.
    graph.compute_breadcrumbs();

    graph
}

/// Round-trip the graph and assert canonical-byte equality.
/// Returns the parsed graph on success.
fn assert_roundtrip_identity(
    graph: &DocumentGraph,
    provenance: &ParseProvenance,
) -> DocumentGraph {
    let md = emit_markdown(graph, provenance);
    // Run with `accept_drift = true` so a hash mismatch produces a
    // canonical-bytes diff (more useful than a bare HashMismatch
    // error). The post-assertions still enforce `Verified` identity
    // for round-trip success.
    let result = parse_markdown(&md, ParseOptions { accept_drift: true })
        .unwrap_or_else(|e| panic!("parse failed for emitted markdown: {e}"));

    let orig = canonical_json(graph);
    let parsed = canonical_json(&result.graph);
    if orig != parsed {
        // Loud diagnostic: find the first divergence and dump a window.
        let max = orig.len().min(parsed.len());
        let mut first_diff = max;
        for i in 0..max {
            if orig.as_bytes()[i] != parsed.as_bytes()[i] {
                first_diff = i;
                break;
            }
        }
        let start = first_diff.saturating_sub(80);
        let end_o = (first_diff + 120).min(orig.len());
        let end_p = (first_diff + 120).min(parsed.len());
        panic!(
            "canonical bytes differ at byte {first_diff} (orig={} bytes, parsed={} bytes)\n\
             --- original window ---\n{}\n\
             --- parsed window ---\n{}",
            orig.len(),
            parsed.len(),
            &orig[start..end_o],
            &parsed[start..end_p],
        );
    }
    assert!(
        matches!(result.identity, ParseIdentity::Verified),
        "expected Verified identity, got {:?}",
        result.identity
    );
    assert_eq!(graph_sha256(graph), graph_sha256(&result.graph));
    result.graph
}

// =========================================================================
// Tests — synthetic graphs.
// =========================================================================

#[test]
fn roundtrip_identity_synthetic_small() {
    // 4-node synthetic: Section + Paragraph + Header + Footer.
    let original = build_synthetic_graph(
        vec![
            ("Section", "Introduction", 1, 0),
            ("Paragraph", "Hello world.", 1, 1),
            ("Header", "Running header", 1, 2),
            ("Footer", "Confidential", 1, 3),
        ],
        Some("Synthetic Test Doc"),
        None,
    );
    assert_roundtrip_identity(&original, &synthetic_provenance());
}

#[test]
fn roundtrip_identity_with_style_data_verified() {
    // CR-86 / DT-12: the **style-on** edition round-trips to `Verified`.
    // The default (null-style) edition is covered by every other test in
    // this file (`style: None`) plus the real-PDF golden Test B. This test
    // covers the other edition: populate `style_info` with data on a body
    // node (as the with-style build does), emit — `style` now carries data
    // on the wire — re-parse, and assert the canonical bytes match and the
    // identity is `Verified`. Data on both sides, hash equals wire by
    // construction. (The null-style and this data-style graph have
    // different `graph_sha256` — distinct editions, which is correct.)
    let mut original = build_synthetic_graph(
        vec![
            ("Section", "Introduction", 1, 0),
            ("Paragraph", "Hello world.", 1, 1),
        ],
        Some("Styled Test Doc"),
        None,
    );
    let para_id = original
        .nodes
        .values()
        .find(|n| n.node_type == "Paragraph")
        .map(|n| n.id)
        .expect("Paragraph node present");
    original.nodes.get_mut(&para_id).unwrap().style_info = Some(StyleMetadata {
        font_class: "f7".to_string(),
        font_size: Some(11.5),
        is_bold: true,
        is_italic: false,
        font_family: Some("NimbusRomNo9L".to_string()),
        foreground_color: Some("#101010".to_string()),
        background_color: None,
    });

    // Prove the with-style wire actually carries the data (not `null`).
    let md = emit_markdown(&original, &synthetic_provenance());
    assert!(
        md.contains("\"style\":{\"font_class\":\"f7\""),
        "style-on edition must serialize `style` as data; got:\n{md}"
    );

    // And it round-trips to Verified (data on both sides).
    assert_roundtrip_identity(&original, &synthetic_provenance());
}

#[test]
fn legacy_confidence_key_in_fence_is_tolerated_and_dropped() {
    // v4.0.0 (Block A / Amendment M): `confidence` left the wire. A legacy
    // fence carrying the key must still parse (serde drops unknown fields);
    // the reconstructed graph simply doesn't carry it, and — because the
    // canonical form no longer includes it — the recomputed hash equals the
    // hash of the same content without the key. We prove tolerance by
    // splicing the legacy key into an emitted fence and re-parsing with
    // accept_drift off: the spliced key changes no canonical bytes, so
    // identity still Verifies.
    let original = build_synthetic_graph(
        vec![
            ("Section", "Introduction", 1, 0),
            ("Paragraph", "Hello world.", 1, 1),
        ],
        Some("Confidence Tolerance Doc"),
        None,
    );
    let md = emit_markdown(&original, &synthetic_provenance());
    // Splice a legacy confidence key into the Section fence JSON.
    let spliced = md.replace(
        "\"node_type\":\"Section\"",
        "\"node_type\":\"Section\",\"confidence\":7",
    );
    assert_ne!(md, spliced, "splice must have taken effect");
    let result = parse_markdown(&spliced, ParseOptions::default())
        .expect("legacy confidence key parses cleanly");
    assert!(
        matches!(result.identity, ParseIdentity::Verified),
        "legacy confidence key must not perturb the content-only identity; got {:?}",
        result.identity
    );
}

#[test]
fn roundtrip_identity_synthetic_with_bookmarks() {
    let bookmarks = BookmarkData {
        sections: vec![
            BookmarkSection {
                title: "Introduction".to_string(),
                order: 0,
                level: 1,
            },
            BookmarkSection {
                title: "Background".to_string(),
                order: 1,
                level: 1,
            },
            BookmarkSection {
                title: "Method".to_string(),
                order: 2,
                level: 2,
            },
        ],
    };
    let original = build_synthetic_graph(
        vec![
            ("Section", "Introduction", 1, 0),
            ("Paragraph", "Intro body.", 1, 1),
            ("Section", "Background", 1, 2),
            ("Paragraph", "Background body.", 1, 3),
        ],
        Some("Synthetic Doc with Bookmarks"),
        Some(bookmarks.clone()),
    );

    let parsed = assert_roundtrip_identity(&original, &synthetic_provenance());
    // Spot-check: outline_data round-tripped through the
    // `bgraph-outline` fence.
    let parsed_bm = parsed
        .document_info
        .outline_data
        .expect("outline_data Some after round-trip");
    assert_eq!(parsed_bm.sections.len(), bookmarks.sections.len());
    for (got, expected) in parsed_bm.sections.iter().zip(bookmarks.sections.iter()) {
        assert_eq!(got.title, expected.title);
        assert_eq!(got.order, expected.order);
        assert_eq!(got.level, expected.level);
    }
}

#[test]
fn roundtrip_identity_synthetic_nested_sections() {
    // Exercise the find_parent / hierarchy_level path with nested
    // Section depths.
    let original = build_synthetic_graph(
        vec![
            ("Section", "Chapter 1", 1, 0),
            ("Paragraph", "Chapter 1 intro.", 1, 1),
            ("Section", "Section 1.1", 2, 2),
            ("Paragraph", "Section 1.1 body.", 2, 3),
            ("Section", "Section 1.2", 2, 4),
            ("Paragraph", "Section 1.2 body.", 2, 5),
            ("Section", "Chapter 2", 1, 6),
            ("Paragraph", "Chapter 2 body.", 1, 7),
        ],
        Some("Nested Test"),
        None,
    );
    assert_roundtrip_identity(&original, &synthetic_provenance());
}

#[test]
fn empty_graph_emits_and_parses() {
    // Edge case: a graph with only the Document root and no body
    // nodes. The emitter writes just the doc-level block; the parser
    // must reconstruct an equivalent root-only graph.
    let original = build_synthetic_graph(vec![], None, None);
    assert_roundtrip_identity(&original, &synthetic_provenance());
}

#[test]
fn code_block_in_body_round_trips() {
    // The spec ("Reserved fence prefix") says non-bgraph triple-
    // backtick blocks in body content are valid CommonMark and
    // round-trip as part of the surrounding element's text. We don't
    // *contain* a ```bgraph prefix (that's the reserved one), but a
    // ```rust block should pass through intact.
    let original = build_synthetic_graph(
        vec![("Paragraph", "Some prose. See snippet.", 1, 0)],
        Some("Code-in-body test"),
        None,
    );
    // Round-trip identity holds on this case because the body text
    // doesn't contain any reserved prefix. (The actual ```rust ...```
    // case would require the emitter to write a paragraph whose
    // `content.text` contained a triple-backtick block; the parser
    // already collects body content verbatim regardless of inner
    // fences, but exercising it via a synthetic graph requires
    // mutating the source `text` field, which we'd do once the rules
    // engine starts producing such content. For now the regression
    // shield is that a plain paragraph round-trips cleanly.)
    assert_roundtrip_identity(&original, &synthetic_provenance());
}

// =========================================================================
// Tests — drift detection.
// =========================================================================

#[test]
fn drift_detection_strict_errors() {
    let original = build_synthetic_graph(vec![("Paragraph", "Original body.", 1, 0)], None, None);
    let md = emit_markdown(&original, &synthetic_provenance());
    // Mutate the body. Note: the JSON metadata still says
    // token_count = 2 (matching the original), so the recomputed
    // canonical bytes diverge only by content.text. The recomputed
    // graph_sha256 differs.
    let tampered = md.replace("Original body.", "Tampered body.");
    let result = parse_markdown(&tampered, ParseOptions::default());
    assert!(
        matches!(result, Err(ParseError::HashMismatch { .. })),
        "expected HashMismatch under strict mode, got {result:?}",
    );
}

#[test]
fn drift_detection_accept_drift_returns_derivative() {
    let original = build_synthetic_graph(vec![("Paragraph", "Original body.", 1, 0)], None, None);
    let md = emit_markdown(&original, &synthetic_provenance());
    let original_hash = graph_sha256(&original);
    let tampered = md.replace("Original body.", "Tampered body.");

    let result = parse_markdown(&tampered, ParseOptions { accept_drift: true })
        .expect("parse succeeds under accept_drift");
    match result.identity {
        ParseIdentity::Derivative {
            original_sha256,
            recomputed_sha256,
        } => {
            assert_eq!(
                original_sha256, original_hash,
                "Derivative.original_sha256 should match the embedded value",
            );
            assert_ne!(
                original_sha256, recomputed_sha256,
                "Derivative.recomputed_sha256 should differ from original",
            );
            assert_eq!(
                recomputed_sha256,
                graph_sha256(&result.graph),
                "Derivative.recomputed_sha256 should match the parsed graph's hash",
            );
        }
        other => panic!("expected Derivative, got {other:?}"),
    }
}

#[test]
fn reserved_prefix_in_body_is_handled_on_parse() {
    // Hand-craft a malformed bgraph.md where a free-text body line
    // starts with the reserved `` ```bgraph `` prefix. The scanner
    // should fail loud — either ReservedPrefixInBody or
    // MalformedFence is acceptable per the handoff.
    let bogus = "```bgraph\n\
                 {\"schema\":\"1.0.0\",\"blazegraph_version\":\"0.6.0\",\"source\":{\"format\":\"markdown\",\"filename\":\"x.md\",\"sha256\":\"a\"},\"flow_type\":\"Free\",\"config_hash\":\"b\",\"graph_sha256\":\"c\"}\n\
                 ```\n\
                 \n\
                 ```bgraph-mystery\n\
                 body\n\
                 {\"id\":\"x\"}\n\
                 ```\n";
    let result = parse_markdown(bogus, ParseOptions::default());
    assert!(
        matches!(
            result,
            Err(ParseError::MalformedFence(_)) | Err(ParseError::ReservedPrefixInBody)
        ),
        "expected loud failure on reserved-prefix violation, got {result:?}",
    );
}

// =========================================================================
// Tests — in-tree fixtures.
// =========================================================================

#[test]
fn roundtrip_identity_shannon_fixture() {
    // `load_fixture_graph` reconstructs the fixture through
    // `build_graph_deterministic` so its IDs match what the reverse
    // parser will derive. The round-trip then verifies canonical-byte
    // identity end-to-end on a real-shape graph (94 body nodes).
    let original = load_fixture_graph("claude_shannon_paper");
    assert_roundtrip_identity(&original, &fixture_provenance("claude_shannon_paper"));
}

#[test]
fn roundtrip_identity_euclid_fixture() {
    // 389 body nodes — larger stress test for the line-scan + builder
    // pipeline.
    let original = load_fixture_graph("elements_of_euclid");
    assert_roundtrip_identity(&original, &fixture_provenance("elements_of_euclid"));
}

// =========================================================================
// Tests — direct entry point.
// =========================================================================

#[test]
fn bgraph_md_parse_direct_entry_point_works() {
    // bgraph_md::parse is the lower-level entry point that skips
    // detection. Exercising it covers the case where a caller knows
    // the input is bgraph.md.
    let original = build_synthetic_graph(vec![("Paragraph", "Hello.", 1, 0)], None, None);
    let md = emit_markdown(&original, &synthetic_provenance());
    let result = bgraph_md::parse(&md, ParseOptions::default()).expect("parses");
    assert!(matches!(result.identity, ParseIdentity::Verified));
    assert_eq!(canonical_json(&original), canonical_json(&result.graph));
}

// =========================================================================
// Block C — the honest 1.0.0 reset, the codec seam, and json
// self-verification. (Museum design-flow Block C.)
// =========================================================================

/// Extract the doc-level `bgraph` block JSON from an emitted bgraph.md.
fn doc_level_json(md: &str) -> serde_json::Value {
    let line = md
        .lines()
        .nth(1)
        .expect("doc-level fence has a JSON line below the fence open");
    serde_json::from_str(line).expect("doc-level JSON parses")
}

#[test]
fn block_c_emit_stamps_1_0_0_in_both_serializations() {
    // C.1: the reset. Both serializations advertise the honest inaugural
    // edition `1.0.0`.
    let graph = build_synthetic_graph(vec![("Section", "S", 1, 0)], Some("Doc"), None);
    let md = emit_markdown(&graph, &synthetic_provenance());
    let md_schema = doc_level_json(&md)["schema"].as_str().unwrap().to_string();
    assert_eq!(md_schema, "1.0.0", "md doc-level `schema` must be 1.0.0");

    let sorted = graph.to_sorted_graph(Some(&synthetic_provenance()));
    assert_eq!(
        sorted.schema_version, "1.0.0",
        "json `schema_version` must be 1.0.0"
    );
}

#[test]
fn block_c_1x_roundtrips_verified_non_1x_rejected() {
    // C.1: a 1.x bgraph.md round-trips to Verified; a retired non-1.x
    // schema is a clean UnsupportedSchema (never best-effort-read).
    let graph = build_synthetic_graph(vec![("Paragraph", "Body.", 1, 0)], Some("Doc"), None);
    let md = emit_markdown(&graph, &synthetic_provenance());
    let result = parse_markdown(&md, ParseOptions::default()).expect("1.x parses");
    assert!(matches!(result.identity, ParseIdentity::Verified));

    for retired in ["0.9.0", "2.0.0", "5.0.0"] {
        let tampered = md.replacen("\"schema\":\"1.0.0\"", &format!("\"schema\":\"{retired}\""), 1);
        let result = parse_markdown(&tampered, ParseOptions { accept_drift: true });
        assert!(
            matches!(result, Err(ParseError::UnsupportedSchema(_))),
            "retired schema {retired} must be rejected; got {result:?}"
        );
    }
}

#[test]
fn block_c_seam_is_a_pure_refactor() {
    // C.2: routing emit/canonicalize through the seam
    // (`FormatVersion::CURRENT`) reproduces the direct calls byte-for-
    // byte — the seam threads the version differently, it does not change
    // the content. And the `graph_sha256` *value* is unchanged (it is
    // version-independent — the reset is a renumber, not a canonical-form
    // change).
    let graph = build_synthetic_graph(
        vec![("Section", "Intro", 1, 0), ("Paragraph", "Body text.", 2, 1)],
        Some("Doc"),
        None,
    );
    let prov = synthetic_provenance();

    // emit wrapper == direct emit
    assert_eq!(
        emit_markdown_as(FormatVersion::CURRENT, &graph, &prov, EmitOptions::default()),
        emit_markdown_with_options(&graph, &prov, EmitOptions::default()),
        "the V1_0 emit arm must be the identity of the direct emitter"
    );
    // canonicalize wrapper == direct canonical_json
    assert_eq!(
        canonicalize_as(FormatVersion::CURRENT, &graph),
        canonical_json(&graph),
        "the V1_0 canonicalize arm must be the identity canonicalizer"
    );
    // the doc-level block's graph_sha256 equals the version-independent
    // content-body hash — the seam did not move identity.
    let md = emit_markdown(&graph, &prov);
    let doc_sha = doc_level_json(&md)["graph_sha256"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(doc_sha, graph_sha256(&graph), "graph_sha256 value must not move");
}

#[test]
fn block_c_json_envelope_is_self_verifiable() {
    // C.3: the json envelope now carries `graph_sha256`, equal to the md
    // doc-level block's value for the same graph; a round-tripped json
    // graph verifies to `Verified`.
    let graph = build_synthetic_graph(
        vec![("Section", "Intro", 1, 0), ("Paragraph", "Body.", 2, 1)],
        Some("Doc"),
        None,
    );
    let prov = synthetic_provenance();

    let sorted = graph.to_sorted_graph(Some(&prov));
    // envelope value == content-body hash == md doc-level block value
    assert_eq!(sorted.graph_sha256, graph_sha256(&graph));
    let md = emit_markdown(&graph, &prov);
    assert_eq!(
        sorted.graph_sha256,
        doc_level_json(&md)["graph_sha256"].as_str().unwrap(),
        "json envelope graph_sha256 must equal the md doc-level block's"
    );

    // serialize → deserialize → verify
    let json = serde_json::to_string(&sorted).expect("serializes");
    let reloaded: SortedDocumentGraph = serde_json::from_str(&json).expect("deserializes");
    assert_eq!(
        reloaded.verify_identity(),
        ParseIdentity::Verified,
        "an untampered loaded json graph must verify"
    );
}

#[test]
fn block_c_json_verify_detects_tamper() {
    // C.3: mutate the content body of a loaded json graph while leaving
    // the embedded envelope hash intact → the recompute no longer matches
    // → a `Derivative` verdict (the json-side analogue of the md path's
    // tamper detection).
    let graph = build_synthetic_graph(vec![("Paragraph", "original.", 1, 0)], Some("Doc"), None);
    let mut sorted = graph.to_sorted_graph(Some(&synthetic_provenance()));

    // Tamper a body node's text; the embedded graph_sha256 is unchanged.
    let body = sorted
        .nodes
        .iter_mut()
        .find(|n| n.text_order.is_some())
        .expect("has a body node");
    body.content.text = "tampered.".to_string();

    match sorted.verify_identity() {
        ParseIdentity::Derivative { .. } => {}
        other => panic!("tampered json must yield Derivative, got {other:?}"),
    }
}
