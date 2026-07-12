//! Block D — the golden-freeze anchor (museum layer ①).
//!
//! Freezes one real document's emitted **bgraph.md** at `1.0.0` as a
//! *reconstruction anchor*: the durable artifact a future version-pinned
//! binary (layer ②, deferred) can use to prove it still reproduces this
//! edition.
//!
//! The freeze check is **JVM-free**. It replays the deterministic identity
//! path (`process_document_with_cache` → `rules_and_graph` →
//! `build_graph_deterministic`, a pure function of `PreprocessorOutput` +
//! config) from a **committed C2 preprocessor cache**. A C2 cache hit skips
//! Tika (extraction) *and* the XHTML parse entirely — the stub preprocessor
//! below panics if either is ever reached, so a JVM/Tika invocation fails
//! the test loudly rather than passing silently.
//!
//! Two tests:
//!   * **A — reproduction (freeze):** regenerate bgraph.md JVM-free from the
//!     committed C2 cache and assert it is **byte-identical** to the frozen
//!     `golden/1.0.0/attention/document.bgraph.md`. Under `BLESS_GOLDEN=1`,
//!     re-freeze (write the md + refresh `PRODUCED_BY`) instead of asserting.
//!   * **B — roundtrip:** `parse_markdown` the frozen md and assert the
//!     identity verdict is `Verified` (doc-level `graph_sha256`
//!     self-consistency).
//!
//! Family layout (all committed):
//!   test_fixtures/golden/1.0.0/attention/attention.pdf        — the source
//!   test_fixtures/golden/1.0.0/attention/config.yaml          — the config
//!   test_fixtures/golden/1.0.0/attention/document.bgraph.md   — the freeze
//!   test_fixtures/golden/1.0.0/attention/PRODUCED_BY          — codebase sha
//!   test_fixtures/snapshots/{c1-xhtml,c2-preprocessor}/<sha>  — the cache
//!
//! Design-flow authority:
//! `docs/P2/core/design-flows/2026-07-06-canonical-versioning-and-fixture-stability.md`
//! (Block D). Purely additive: touches no core types, bumps no version,
//! freezes only bgraph.md (bgraph.json is CR-88).

use blazegraph_io_core::config::ParsingConfig;
use blazegraph_io_core::graphs::serialization::markdown::emit_markdown;
use blazegraph_io_core::preprocessors::md::{parse_markdown, ParseIdentity, ParseOptions};
use blazegraph_io_core::preprocessors::Preprocessor;
use blazegraph_io_core::processor::DocumentProcessor;
use blazegraph_io_core::storage::{CacheDefaults, FileStorage, FreshFrom};
use blazegraph_io_core::types::PreprocessorOutput;
use std::path::{Path, PathBuf};

// =========================================================================
// JVM-free guard: a preprocessor stub that panics if Tika or the XHTML
// parse is ever reached. Reaching either means the committed C2 cache did
// NOT hit — which would make the freeze depend on a live JVM. We want that
// to be a loud test failure, not a silent slow pass.
// =========================================================================

struct NoJvmPreprocessor;

impl Preprocessor for NoJvmPreprocessor {
    fn parse_pdf_to_markup_language(&self, _pdf_bytes: &[u8]) -> anyhow::Result<String> {
        panic!(
            "Tika/JVM extraction was invoked — the golden freeze must replay from the \
             committed C2 preprocessor cache (a C2 hit skips extraction). A cache miss here \
             means the committed C2 tier is absent or its pdf_hash no longer matches \
             attention.pdf. Rebuild the family with `make golden-generate`."
        );
    }

    fn parse_markup_to_preprocessor_output(
        &self,
        _markup: &str,
    ) -> anyhow::Result<PreprocessorOutput> {
        panic!(
            "The XHTML → PreprocessorOutput parse was invoked — the golden freeze must replay \
             from the committed C2 cache, not re-parse C1 XHTML. A C2 hit skips this step."
        );
    }

    fn name(&self) -> &str {
        "no-jvm-golden-freeze-stub"
    }

    fn supports_file_type(&self, _path: &Path) -> bool {
        true
    }
}

// =========================================================================
// Paths.
// =========================================================================

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test_fixtures/golden/1.0.0/attention")
}

/// The `--cache-dir` root. FileStorage lays its own `c1-xhtml/`,
/// `c2-preprocessor/`, `c3-graph/` tiers underneath. Marcus's steer:
/// reuse `snapshots/` as the local fixture cache dir (the stage-snapshot
/// `snapshots/<doc>/` contents live alongside, untouched).
fn cache_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test_fixtures/snapshots")
}

fn golden_md_path() -> PathBuf {
    golden_dir().join("document.bgraph.md")
}

// =========================================================================
// The JVM-free replay.
// =========================================================================

/// Regenerate `attention`'s bgraph.md from the committed C2 cache, JVM-free.
///
/// `FreshFrom::C3` is the exact tier we want: it consults the C2 cache (a
/// hit → skips Tika *and* the XHTML parse) but does **not** read a C3 graph
/// cache, so the deterministic builder (`build_graph_deterministic`) runs on
/// every invocation. That is the reproduction path we are anchoring. (The
/// handoff prose says "FreshFrom::C2"; that variant means "reparse from C1
/// XHTML" — the opposite of a C2 hit. We honor the stated intent — "a C2 hit
/// skips Tika entirely" — with the variant that actually produces it.)
///
/// `CacheDefaults` with every write disabled keeps the run read-only: the
/// committed cache is never mutated and no stray tiers are written.
fn regenerate_bgraph_md() -> String {
    let golden = golden_dir();
    let pdf = golden.join("attention.pdf");
    let config_path = golden.join("config.yaml");

    let config = ParsingConfig::load_from_file(
        config_path
            .to_str()
            .expect("config.yaml path is valid UTF-8"),
    )
    .expect("golden config.yaml loads");

    let storage = FileStorage::new(cache_dir().to_str().expect("cache dir path is valid UTF-8"))
        .expect("FileStorage opens at the committed cache dir");

    let mut processor =
        DocumentProcessor::new_with_dependencies(Box::new(NoJvmPreprocessor), Box::new(storage))
            .expect("DocumentProcessor builds with the no-JVM stub preprocessor");

    // Read-only: consult caches, write nothing.
    let read_only = CacheDefaults {
        c0_pdf: false,
        c1_xhtml: false,
        c2_preprocessor: false,
        c3_graph: false,
    };

    let (graph, provenance) = processor
        .process_document_with_cache(
            pdf.to_str().expect("pdf path is valid UTF-8"),
            &config,
            FreshFrom::C3,
            &read_only,
            false,
        )
        .expect("C2 replay builds the graph deterministically");

    // CR-86 / DT-12: the anchor is the **default null-style `1.0.0`
    // edition**. `style_info` is now an always-present, config-valued node
    // field, gated at *build* time: the golden `config.yaml` leaves
    // `include_style_info` off (the default), so the built graph carries
    // `style_info: None` on every node. `graph_sha256` covers that (`null`),
    // the emitter serializes it (`"style":null`), and a re-parse
    // reconstructs `None` → the recomputed hash matches → `Verified` on the
    // default path. No emit flag: the emitter serializes exactly what the
    // graph holds. (The provisional Block D workaround — freezing WITH
    // `--include-style-info` because the default emit didn't self-verify —
    // is exactly the bug CR-86 fixes; it is gone.)
    emit_markdown(&graph, &provenance)
}

/// `blazegraph-io` git HEAD sha — the codebase_sha binding recorded in the
/// `PRODUCED_BY` sidecar. Not part of any serialized artifact.
fn git_head_sha() -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(env!("CARGO_MANIFEST_DIR"))
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("git rev-parse HEAD runs");
    assert!(out.status.success(), "git rev-parse HEAD failed");
    String::from_utf8(out.stdout)
        .expect("git sha is valid UTF-8")
        .trim()
        .to_string()
}

fn bless_enabled() -> bool {
    std::env::var_os("BLESS_GOLDEN").is_some()
}

// =========================================================================
// Test A — reproduction (freeze).
// =========================================================================

#[test]
fn golden_freeze_attention_reproduces_bgraph_md() {
    let regenerated = regenerate_bgraph_md();
    let path = golden_md_path();

    if bless_enabled() {
        std::fs::write(&path, &regenerated).expect("write frozen golden bgraph.md");
        let sha = git_head_sha();
        std::fs::write(golden_dir().join("PRODUCED_BY"), format!("{sha}\n"))
            .expect("write PRODUCED_BY sidecar");
        eprintln!(
            "✅ BLESS_GOLDEN: re-froze {} ({} bytes) + PRODUCED_BY {}",
            path.display(),
            regenerated.len(),
            sha
        );
        return;
    }

    let frozen = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "Missing frozen golden bgraph.md at {}.\n\
             Generate it JVM-free from the committed C2 cache with:\n  \
             BLESS_GOLDEN=1 cargo test -p blazegraph-io-core --test golden_freeze_tests\n\
             or rebuild the whole family (needs the JVM) with `make golden-generate`.",
            path.display()
        )
    });

    if regenerated != frozen {
        let max = regenerated.len().min(frozen.len());
        let mut first_diff = max;
        for i in 0..max {
            if regenerated.as_bytes()[i] != frozen.as_bytes()[i] {
                first_diff = i;
                break;
            }
        }
        let start = first_diff.saturating_sub(80);
        let end_r = (first_diff + 120).min(regenerated.len());
        let end_f = (first_diff + 120).min(frozen.len());
        panic!(
            "Golden freeze mismatch: HEAD no longer reproduces attention's 1.0.0 bgraph.md.\n\
             First divergence at byte {first_diff} (regenerated={} bytes, frozen={} bytes).\n\
             --- frozen window ---\n{}\n\
             --- regenerated window ---\n{}\n\
             \n\
             If this change *legitimately* moves the output (e.g. a crate-version bump or an\n\
             intended pipeline change), re-freeze intentionally:\n  \
             BLESS_GOLDEN=1 cargo test -p blazegraph-io-core --test golden_freeze_tests\n\
             and commit the updated document.bgraph.md + PRODUCED_BY. Otherwise this is a\n\
             reproduction regression — investigate before blessing.",
            regenerated.len(),
            frozen.len(),
            &frozen[start..end_f],
            &regenerated[start..end_r],
        );
    }
}

// =========================================================================
// Test B — roundtrip (read-side agreement).
// =========================================================================

#[test]
fn golden_freeze_attention_roundtrips_verified() {
    let path = golden_md_path();

    let md = match std::fs::read_to_string(&path) {
        Ok(md) => md,
        Err(_) if bless_enabled() => {
            // Bless bootstrap: Test A (running in parallel) writes the md.
            // On a first-ever bless with no frozen md yet, there is nothing
            // to read back — skip rather than racing the writer.
            eprintln!(
                "⏭  BLESS_GOLDEN: {} not present yet — roundtrip check skipped \
                 (Test A writes it).",
                path.display()
            );
            return;
        }
        Err(e) => panic!(
            "Missing frozen golden bgraph.md at {}: {e}.\n\
             Generate it with `BLESS_GOLDEN=1 cargo test -p blazegraph-io-core \
             --test golden_freeze_tests` or `make golden-generate`.",
            path.display()
        ),
    };

    let result = parse_markdown(&md, ParseOptions::default())
        .expect("frozen golden bgraph.md parses cleanly");

    assert!(
        matches!(result.identity, ParseIdentity::Verified),
        "frozen golden bgraph.md must self-verify (doc-level graph_sha256); got {:?}",
        result.identity
    );
}
