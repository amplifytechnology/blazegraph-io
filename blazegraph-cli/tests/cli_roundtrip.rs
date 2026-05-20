//! B5 CLI integration tests — exercise the markdown channel end-to-end
//! through the compiled binary.
//!
//! These tests are JNI-free: they construct a synthetic graph through
//! the deterministic builder, emit it to bgraph.md via the in-process
//! `emit_markdown` lib call, write that to a temp file, and then
//! invoke the CLI binary as a subprocess to parse it back. The
//! canonical bytes of the parsed graph are compared to the original.
//!
//! The binary path is resolved via `env!("CARGO_BIN_EXE_blazegraph-io")`,
//! which Cargo populates at test-build time from the `[[bin]]` name in
//! `Cargo.toml`. No PATH manipulation required.

use blazegraph_io_core::graphs::builder::GraphBuilder;
use blazegraph_io_core::graphs::node_id::NodeIdGenerator;
use blazegraph_io_core::graphs::serialization::canonical::canonical_json;
use blazegraph_io_core::graphs::serialization::markdown::emit_markdown;
use blazegraph_io_core::types::*;
use std::path::PathBuf;
use std::process::Command;

// =========================================================================
// Test helpers
// =========================================================================

/// The CLI binary, resolved at test build time. Cargo guarantees this
/// resolves to the bin produced by `[[bin]] name = "blazegraph-io"`.
const BIN: &str = env!("CARGO_BIN_EXE_blazegraph-io");

/// Make a unique temp dir for a test. Uses `std::env::temp_dir()` +
/// a UUIDv4 path component (uuid is already a CLI dep). We do NOT
/// auto-clean — leaving the directory around makes failure diagnosis
/// easier, and `/tmp` is cleared by the OS periodically. Each test
/// invocation gets a fresh dir so reruns don't conflict.
fn unique_temp_dir(test_name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "blazegraph-cli-test-{test_name}-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// Build a small synthetic graph through the deterministic builder.
/// Mirrors `markdown_roundtrip_tests::build_synthetic_graph` but kept
/// self-contained so this file doesn't depend on the core crate's
/// integration-test helpers.
fn build_synthetic_graph() -> DocumentGraph {
    let provenance = ParseProvenance {
        blazegraph_version: "0.6.0-cli-test".to_string(),
        source_format: "markdown".to_string(),
        source_filename: "cli-roundtrip.md".to_string(),
        source_sha256: "cli-test-source-sha".to_string(),
        config_hash: "cli-test-config-hash".to_string(),
    };
    let id_gen = NodeIdGenerator::new(&provenance.source_sha256, &provenance.config_hash);
    let elements = vec![
        SemanticTreeElement {
            text: "Introduction".to_string(),
            element_type: SemanticElementType::Section,
            hierarchy_level: 1,
            text_order: 0,
            physical_location: None,
            style: None,
            message_metadata: None,
            token_count: 1,
        },
        SemanticTreeElement {
            text: "First paragraph body.".to_string(),
            element_type: SemanticElementType::Paragraph,
            hierarchy_level: 1,
            text_order: 1,
            physical_location: None,
            style: None,
            message_metadata: None,
            token_count: 3,
        },
        SemanticTreeElement {
            text: "Background".to_string(),
            element_type: SemanticElementType::Section,
            hierarchy_level: 1,
            text_order: 2,
            physical_location: None,
            style: None,
            message_metadata: None,
            token_count: 1,
        },
        SemanticTreeElement {
            text: "Some background prose.".to_string(),
            element_type: SemanticElementType::Paragraph,
            hierarchy_level: 1,
            text_order: 3,
            physical_location: None,
            style: None,
            message_metadata: None,
            token_count: 3,
        },
        SemanticTreeElement {
            text: "Running header".to_string(),
            element_type: SemanticElementType::Header,
            hierarchy_level: 1,
            text_order: 4,
            physical_location: None,
            style: None,
            message_metadata: None,
            token_count: 2,
        },
        SemanticTreeElement {
            text: "Confidential".to_string(),
            element_type: SemanticElementType::Footer,
            hierarchy_level: 1,
            text_order: 5,
            physical_location: None,
            style: None,
            message_metadata: None,
            token_count: 1,
        },
    ];
    let mut graph = GraphBuilder::new()
        .build_graph_deterministic(elements, &id_gen, provenance)
        .expect("synthetic graph builds");
    graph.document_info.document_metadata.title = Some("CLI Round-Trip Sample".to_string());
    graph.structural_profile.flow_type = FlowType::Free;
    graph.compute_structural_profile();
    graph.compute_breadcrumbs();
    graph
}

/// Load a graph from a path produced by the CLI's `-f graph` output
/// and canonicalize. Mirrors the round-trip-integration helper from
/// the core crate's test file.
fn canonicalize_saved_graph(path: &std::path::Path) -> String {
    let raw = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read {} failed: {e}", path.display()));
    // CLI saves via `save_with_format("graph")` → `save_to_json` →
    // pretty-printed JSON. Round-trip-canonicalize by deserializing
    // and re-canonicalizing.
    let sorted: SortedDocumentGraph =
        serde_json::from_str(&raw).expect("CLI-saved JSON deserializes");
    // Reconstruct a regular DocumentGraph from the sorted form so
    // we can canonicalize.
    let mut nodes = std::collections::HashMap::with_capacity(sorted.nodes.len());
    for node in sorted.nodes {
        nodes.insert(node.id, node);
    }
    let graph = DocumentGraph {
        nodes,
        document_info: sorted.document_info,
        structural_profile: sorted.structural_profile,
    };
    canonical_json(&graph)
}

// =========================================================================
// Tests
// =========================================================================

#[test]
fn cli_roundtrip_markdown_to_graph_canonical_bytes_match() {
    // The "two-call round-trip" demonstration: emit a graph to
    // bgraph.md via the lib, write to a temp file, then invoke the
    // CLI binary to parse that bgraph.md back to graph.json. The
    // canonical bytes of the parsed graph must equal the canonical
    // bytes of the original.
    //
    // This is the load-bearing test: it proves bgraph.md → CLI →
    // graph.json works end-to-end through the compiled binary, not
    // just through the in-process lib API.
    let dir = unique_temp_dir("roundtrip");
    let fixture_md = dir.join("fixture.bgraph.md");
    let roundtrip_json = dir.join("roundtrip.json");

    let original = build_synthetic_graph();
    let md = emit_markdown(&original);
    std::fs::write(&fixture_md, &md).expect("write fixture md");

    let output = Command::new(BIN)
        .args([
            "parse",
            "-i",
            fixture_md.to_str().unwrap(),
            "-o",
            roundtrip_json.to_str().unwrap(),
            "-f",
            "graph",
        ])
        .output()
        .expect("CLI binary spawns");
    assert!(
        output.status.success(),
        "CLI exited non-zero: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Round-trip identity verified"),
        "expected verified-identity log line in stdout; got:\n{stdout}"
    );

    let original_canonical = canonical_json(&original);
    let roundtripped_canonical = canonicalize_saved_graph(&roundtrip_json);
    assert_eq!(
        original_canonical, roundtripped_canonical,
        "CLI round-trip canonical bytes drift: bgraph.md → CLI → graph.json must equal original"
    );
}

#[test]
fn cli_emit_markdown_output_format_writes_bgraph_md() {
    // The other half of the two-call pattern: starting from a
    // bgraph.md, run `parse -f markdown` to emit a new bgraph.md.
    // Since the lib path is `parse_markdown → emit_markdown`, the
    // output should be byte-equal to the input (canonical wire
    // format is deterministic across emit↔parse pairs).
    let dir = unique_temp_dir("emit-markdown");
    let input_md = dir.join("input.bgraph.md");
    let output_md = dir.join("output.bgraph.md");

    let original = build_synthetic_graph();
    let md = emit_markdown(&original);
    std::fs::write(&input_md, &md).expect("write input md");

    let output = Command::new(BIN)
        .args([
            "parse",
            "-i",
            input_md.to_str().unwrap(),
            "-o",
            output_md.to_str().unwrap(),
            "-f",
            "bgraph-md",
        ])
        .output()
        .expect("CLI binary spawns");
    assert!(
        output.status.success(),
        "CLI exited non-zero: {}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr),
    );

    let written = std::fs::read_to_string(&output_md).expect("read output md");
    assert_eq!(
        md, written,
        "emit→parse→emit must produce byte-equal bgraph.md"
    );
}

#[test]
fn cli_strict_mode_errors_on_drift() {
    // Strict-mode (the default — no --accept-drift) bgraph.md parse
    // with a tampered graph_sha256 must produce a clean error
    // pointing at `--accept-drift`, with exit code != 0.
    let dir = unique_temp_dir("strict-drift");
    let fixture_md = dir.join("tampered.bgraph.md");

    let original = build_synthetic_graph();
    let mut md = emit_markdown(&original);
    // Tamper: corrupt the embedded graph_sha256 so parse hits
    // HashMismatch.
    md = md.replace(
        "\"graph_sha256\":\"",
        "\"graph_sha256\":\"00000000000000000000000000000000",
    );
    std::fs::write(&fixture_md, &md).expect("write fixture md");

    let output = Command::new(BIN)
        .args([
            "parse",
            "-i",
            fixture_md.to_str().unwrap(),
            "-o",
            dir.join("tampered.json").to_str().unwrap(),
            "-f",
            "graph",
        ])
        .output()
        .expect("CLI binary spawns");

    assert!(
        !output.status.success(),
        "CLI should exit non-zero on hash mismatch in strict mode"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("graph_sha256 mismatch"),
        "stderr should mention graph_sha256 mismatch; got:\n{stderr}"
    );
    assert!(
        stderr.contains("--accept-drift"),
        "stderr should point at --accept-drift; got:\n{stderr}"
    );
}

#[test]
fn cli_accept_drift_returns_derivative_with_warning() {
    // --accept-drift lets the parse succeed on a hash mismatch and
    // surfaces the derivative provenance on stderr.
    let dir = unique_temp_dir("accept-drift");
    let fixture_md = dir.join("drifted.bgraph.md");
    let output_json = dir.join("drifted.json");

    let original = build_synthetic_graph();
    let mut md = emit_markdown(&original);
    md = md.replace(
        "\"graph_sha256\":\"",
        "\"graph_sha256\":\"00000000000000000000000000000000",
    );
    std::fs::write(&fixture_md, &md).expect("write fixture md");

    let output = Command::new(BIN)
        .args([
            "parse",
            "-i",
            fixture_md.to_str().unwrap(),
            "-o",
            output_json.to_str().unwrap(),
            "-f",
            "graph",
            "--accept-drift",
        ])
        .output()
        .expect("CLI binary spawns");

    assert!(
        output.status.success(),
        "CLI must succeed with --accept-drift even on hash mismatch; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("drifted bgraph.md"),
        "stderr should warn about derivative provenance; got:\n{stderr}"
    );
}

#[test]
fn cli_strip_body_only_removes_all_bgraph_fences() {
    // Explicit `--mode body-only`: every bgraph fence is removed. The
    // Section heading + Paragraph body + (v2.0.0) Header body all
    // survive.
    let dir = unique_temp_dir("strip-body-only");
    let fixture_md = dir.join("fixture.bgraph.md");
    let stripped = dir.join("stripped.md");

    let graph = build_synthetic_graph();
    let md = emit_markdown(&graph);
    std::fs::write(&fixture_md, &md).expect("write fixture md");

    let output = Command::new(BIN)
        .args([
            "strip",
            "-i",
            fixture_md.to_str().unwrap(),
            "-o",
            stripped.to_str().unwrap(),
            "--mode",
            "body-only",
        ])
        .output()
        .expect("CLI binary spawns");
    assert!(
        output.status.success(),
        "strip failed: stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let out = std::fs::read_to_string(&stripped).expect("read stripped file");
    assert!(
        !out.contains("```bgraph"),
        "body-only strip output must not contain any bgraph fence; got:\n{out}"
    );
    assert!(
        out.contains("Introduction") && out.contains("Background"),
        "Section headings should survive body-only strip; got:\n{out}"
    );
    assert!(
        out.contains("First paragraph body."),
        "Paragraph bodies should survive body-only strip; got:\n{out}"
    );
    assert!(
        out.contains("Running header"),
        "v2.0.0: Header body lives outside the fence and survives body-only strip; got:\n{out}"
    );
    // body-only: no YAML frontmatter at the top.
    assert!(
        !out.starts_with("---\n"),
        "body-only must NOT emit YAML frontmatter; got start:\n{}",
        &out.chars().take(40).collect::<String>()
    );
}

#[test]
fn cli_strip_default_mode_emits_frontmatter() {
    // CR-55 default: `--mode body-with-frontmatter`. Strip every fence
    // and lift the doc-level block to YAML frontmatter at the top.
    let dir = unique_temp_dir("strip-default-frontmatter");
    let fixture_md = dir.join("fixture.bgraph.md");
    let stripped = dir.join("stripped.md");

    let graph = build_synthetic_graph();
    let md = emit_markdown(&graph);
    std::fs::write(&fixture_md, &md).expect("write fixture md");

    let output = Command::new(BIN)
        .args([
            "strip",
            "-i",
            fixture_md.to_str().unwrap(),
            "-o",
            stripped.to_str().unwrap(),
        ])
        .output()
        .expect("CLI binary spawns");
    assert!(
        output.status.success(),
        "strip failed: stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let out = std::fs::read_to_string(&stripped).expect("read stripped file");
    assert!(
        out.starts_with("---\n"),
        "default mode must emit YAML frontmatter; got start:\n{}",
        &out.chars().take(80).collect::<String>()
    );
    // Frontmatter must round-trip through serde_yaml.
    let frontmatter = out
        .strip_prefix("---\n")
        .and_then(|s| s.split_once("\n---\n"))
        .map(|(yaml, _)| yaml)
        .expect("frontmatter delimited");
    let yaml_str = format!("{frontmatter}\n");
    let parsed: serde_json::Value = serde_yaml::from_str(&yaml_str)
        .expect("frontmatter must round-trip through serde_yaml");
    assert!(parsed.get("graph_sha256").is_some());
    // Body survives.
    assert!(out.contains("First paragraph body."));
    // Source file untouched (content sanity).
    let src_after =
        std::fs::read_to_string(&fixture_md).expect("read source after strip");
    assert_eq!(md, src_after, "source file must not be modified by strip");
}

#[test]
fn cli_strip_node_types_filters_headers() {
    // CR-55: `--node-types header` removes Header elements entirely
    // (body + fence) via the structural rule; the default-mode
    // frontmatter still emits at the top.
    let dir = unique_temp_dir("strip-node-types-header");
    let fixture_md = dir.join("fixture.bgraph.md");
    let stripped = dir.join("stripped.md");

    let graph = build_synthetic_graph();
    let md = emit_markdown(&graph);
    std::fs::write(&fixture_md, &md).expect("write fixture md");

    let output = Command::new(BIN)
        .args([
            "strip",
            "-i",
            fixture_md.to_str().unwrap(),
            "-o",
            stripped.to_str().unwrap(),
            "--node-types",
            "header",
        ])
        .output()
        .expect("CLI binary spawns");
    assert!(
        output.status.success(),
        "strip failed: stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let out = std::fs::read_to_string(&stripped).expect("read stripped file");
    assert!(out.starts_with("---\n"), "frontmatter still emitted");
    assert!(
        !out.contains("Running header"),
        "Header body must be removed; got:\n{out}"
    );
    assert!(
        !out.contains("```bgraph-header"),
        "Header fence must be removed; got:\n{out}"
    );
}

#[test]
fn cli_strip_rejects_unknown_node_type() {
    // CR-55 Test 11: unknown --node-types value is rejected at the
    // clap layer with a list of valid values.
    let dir = unique_temp_dir("strip-bad-type");
    let fixture_md = dir.join("fixture.bgraph.md");
    let graph = build_synthetic_graph();
    let md = emit_markdown(&graph);
    std::fs::write(&fixture_md, &md).expect("write fixture md");

    let output = Command::new(BIN)
        .args([
            "strip",
            "-i",
            fixture_md.to_str().unwrap(),
            "--node-types",
            "bogus",
        ])
        .output()
        .expect("CLI binary spawns");
    assert!(
        !output.status.success(),
        "unknown --node-types must fail; stdout:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unknown node type") || stderr.contains("bogus"),
        "stderr should name the unknown tag; got:\n{stderr}"
    );
}

#[test]
fn cli_strip_rejects_bgraph_as_node_type() {
    // CR-55 Test 12: `bgraph` (doc-level fence) cannot be a
    // --node-types target; clap rejects with a hint to use
    // `--mode body-only`.
    let dir = unique_temp_dir("strip-bgraph-type");
    let fixture_md = dir.join("fixture.bgraph.md");
    let graph = build_synthetic_graph();
    let md = emit_markdown(&graph);
    std::fs::write(&fixture_md, &md).expect("write fixture md");

    let output = Command::new(BIN)
        .args([
            "strip",
            "-i",
            fixture_md.to_str().unwrap(),
            "--node-types",
            "bgraph",
        ])
        .output()
        .expect("CLI binary spawns");
    assert!(
        !output.status.success(),
        "`--node-types bgraph` must fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("body-only"),
        "stderr should hint at --mode body-only; got:\n{stderr}"
    );
}

#[test]
fn cli_strip_to_stdout_when_no_output_path() {
    // Omitting `-o` writes to stdout.
    let dir = unique_temp_dir("strip-stdout");
    let fixture_md = dir.join("fixture.bgraph.md");

    let graph = build_synthetic_graph();
    let md = emit_markdown(&graph);
    std::fs::write(&fixture_md, &md).expect("write fixture md");

    let output = Command::new(BIN)
        .args(["strip", "-i", fixture_md.to_str().unwrap()])
        .output()
        .expect("CLI binary spawns");
    assert!(output.status.success(), "strip stdout failed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("```bgraph"),
        "stdout body-only must not contain any bgraph fence; got:\n{stdout}"
    );
    assert!(
        stdout.contains("Introduction"),
        "stdout should contain Section heading body; got:\n{stdout}"
    );
}

#[test]
fn cli_generic_markdown_input_is_accepted() {
    // B6 (schema 0.7.0+): generic markdown is now a first-class
    // input. The CLI should parse it cleanly and emit graph.json.
    let dir = unique_temp_dir("generic-md");
    let plain_md = dir.join("plain.md");
    std::fs::write(
        &plain_md,
        "# Plain Heading\n\nGeneric prose with no bgraph fences.\n",
    )
    .expect("write plain md");

    let out_json = dir.join("plain.json");
    let output = Command::new(BIN)
        .args([
            "parse",
            "-i",
            plain_md.to_str().unwrap(),
            "-o",
            out_json.to_str().unwrap(),
            "-f",
            "graph",
        ])
        .output()
        .expect("CLI binary spawns");

    assert!(
        output.status.success(),
        "CLI should accept generic markdown input; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        out_json.exists(),
        "graph.json output should be written for generic markdown input"
    );
}

#[test]
fn cli_generic_markdown_roundtrip_via_dash_f_markdown() {
    // `-f markdown` (B6) emits via the generic-markdown emitter.
    // The lib's parse → emit pair is the round-trip; the CLI
    // exercises it end-to-end through the binary.
    let dir = unique_temp_dir("generic-md-roundtrip");
    let input_md = dir.join("input.md");
    let output_md = dir.join("output.md");
    let input = "# Heading\n\nProse text.\n\n- item one\n- item two\n";
    std::fs::write(&input_md, input).expect("write input md");

    let output = Command::new(BIN)
        .args([
            "parse",
            "-i",
            input_md.to_str().unwrap(),
            "-o",
            output_md.to_str().unwrap(),
            "-f",
            "markdown",
        ])
        .output()
        .expect("CLI binary spawns");

    assert!(
        output.status.success(),
        "CLI generic-markdown round-trip failed; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let written = std::fs::read_to_string(&output_md).expect("read output md");
    assert_eq!(
        written, input,
        "byte-identical round-trip drift on generic markdown"
    );
}

#[test]
fn cli_unknown_input_format_errors_with_clear_message() {
    let dir = unique_temp_dir("unknown-format");
    let unknown = dir.join("data.xyz");
    std::fs::write(&unknown, "not a recognized file format\n").expect("write file");

    let output = Command::new(BIN)
        .args([
            "parse",
            "-i",
            unknown.to_str().unwrap(),
            "-o",
            dir.join("ignored.json").to_str().unwrap(),
        ])
        .output()
        .expect("CLI binary spawns");

    assert!(
        !output.status.success(),
        "CLI must reject unknown file format"
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        combined.contains("not recognized") || combined.contains("Unknown"),
        "expected a clear 'unsupported format' message; got:\n{combined}"
    );
}
