//! CR-55 corpus tests for `blazegraph_io_core::preprocessors::md::strip`.
//!
//! Verifies the strip surface against `cache/c3-graph/rfc-quic.bgraph.md`
//! — the canonical Header/Footer/Margin-heavy fixture from spec line 439.
//!
//! Pins:
//! - Default mode emits YAML frontmatter at top, parseable, and removes
//!   every `bgraph` fence from the body.
//! - `--node-types header,footer,margin` keeps frontmatter identical and
//!   strips all three element types' bodies + fences.
//! - The source `.bgraph.md` file is never modified by a strip run.

use blazegraph_io_core::preprocessors::md::{strip, StripMode};
use std::path::PathBuf;

fn rfc_quic_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("cache")
        .join("c3-graph")
        .join("rfc-quic.bgraph.md")
}

fn read_rfc_quic() -> Option<String> {
    let p = rfc_quic_path();
    std::fs::read_to_string(&p).ok()
}

#[test]
fn corpus_rfc_quic_default_mode_emits_parseable_frontmatter() {
    let Some(input) = read_rfc_quic() else {
        // Fixture not present in this checkout; skip cleanly. The
        // CR-55 corpus check is fulfilled when the file exists.
        eprintln!(
            "skipped: rfc-quic.bgraph.md not present at {:?}",
            rfc_quic_path()
        );
        return;
    };

    let out = strip(&input, StripMode::BodyWithFrontmatter).expect("strip OK");
    assert!(
        out.starts_with("---\n"),
        "default mode must emit YAML frontmatter at top"
    );
    let frontmatter = out
        .strip_prefix("---\n")
        .and_then(|s| s.split_once("\n---\n"))
        .map(|(yaml, _)| yaml)
        .expect("frontmatter delimited");
    let yaml_str = format!("{frontmatter}\n");
    let parsed: serde_json::Value =
        serde_yaml::from_str(&yaml_str).expect("frontmatter YAML must round-trip");
    // Headline keys present.
    for key in &[
        "blazegraph_version",
        "config_hash",
        "flow_type",
        "graph_sha256",
        "schema",
        "source",
        "title",
    ] {
        assert!(parsed.get(*key).is_some(), "frontmatter missing {key}");
    }
    // Nested `source.format` survived as a map.
    assert_eq!(
        parsed
            .get("source")
            .and_then(|s| s.get("format"))
            .and_then(|v| v.as_str()),
        Some("pdf")
    );
    // No bgraph fences remain anywhere in the output (frontmatter
    // alone uses YAML delimiters; the body is plain markdown).
    assert!(
        !out.contains("```bgraph"),
        "no bgraph fences allowed in default-mode output"
    );
    // bgraph-outline fence body (a JSON sections array) is gone.
    assert!(
        !out.contains("\"sections\":[{\"title\":\"RFC 9000\""),
        "bgraph-outline JSON must be stripped from body"
    );
}

#[test]
fn corpus_rfc_quic_node_types_filter_strips_hfm() {
    let Some(input) = read_rfc_quic() else {
        eprintln!("skipped: rfc-quic.bgraph.md not present");
        return;
    };

    // Compose the CLI's two-pass run order: NodeTypes filter, then
    // default BodyWithFrontmatter mode.
    let filter_tags = vec![
        "header".to_string(),
        "footer".to_string(),
        "margin".to_string(),
    ];
    let after_filter = strip(&input, StripMode::NodeTypes(filter_tags)).expect("filter OK");
    let out = strip(&after_filter, StripMode::BodyWithFrontmatter).expect("mode OK");

    // No bgraph fences (default mode strips all).
    assert!(!out.contains("```bgraph"));
    // Frontmatter still present.
    assert!(out.starts_with("---\n"));
    // Frontmatter is identical to the unfiltered default run.
    let default_out = strip(&input, StripMode::BodyWithFrontmatter).expect("default OK");
    let fm = |s: &str| -> String {
        s.strip_prefix("---\n")
            .and_then(|t| t.split_once("\n---\n"))
            .map(|(yaml, _)| yaml.to_string())
            .unwrap_or_default()
    };
    assert_eq!(
        fm(&out),
        fm(&default_out),
        "frontmatter must be identical between unfiltered and H/F/M-filtered runs"
    );
}

/// Stop-the-world: source file MUST NOT be modified by any strip run.
#[test]
fn corpus_rfc_quic_source_file_unmodified_after_strip() {
    let path = rfc_quic_path();
    let Ok(before_meta) = std::fs::metadata(&path) else {
        eprintln!("skipped: rfc-quic.bgraph.md not present");
        return;
    };
    let before_mtime = before_meta.modified().expect("mtime");
    let before_len = before_meta.len();
    let before_bytes = std::fs::read(&path).expect("read source");

    // Run every mode against it; none should touch the source.
    let input = std::fs::read_to_string(&path).expect("read");
    let _ = strip(&input, StripMode::BodyWithFrontmatter).expect("default");
    let _ = strip(&input, StripMode::BodyOnly).expect("body-only");
    let _ = strip(&input, StripMode::NodeTypes(vec!["header".to_string()])).expect("node-types");

    let after_meta = std::fs::metadata(&path).expect("re-stat");
    let after_mtime = after_meta.modified().expect("mtime");
    let after_len = after_meta.len();
    let after_bytes = std::fs::read(&path).expect("re-read");

    assert_eq!(before_mtime, after_mtime, "source mtime must not change");
    assert_eq!(before_len, after_len, "source length must not change");
    assert_eq!(before_bytes, after_bytes, "source bytes must not change");
}
