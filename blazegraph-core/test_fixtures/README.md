# Test Fixtures

Pipeline boundary tests for blazegraph-core. Tests load pre-generated snapshots and assert stability at the pipeline edges — no JVM required.

## Structure

```
test_fixtures/
├── pdfs/                          ← Fixture PDFs (committed to git)
│   ├── claude_shannon_paper.pdf      Small academic paper (~358KB)
│   └── elements_of_euclid.pdf       Large book (~1.8MB)
├── snapshots/                     ← Generated pipeline outputs (committed to git)
│   ├── claude_shannon_paper/         Stage snapshots (stage-fixture tests)
│   │   ├── stage1a_xhtml.html        Tika XHTML output (boundary 1)
│   │   ├── stage1b_text_elements.json
│   │   ├── stage2_parsed_elements.json
│   │   ├── stage3_graph.json         Final graph output (boundary 2)
│   │   └── summary.json
│   ├── elements_of_euclid/           Stage snapshots
│   │   └── ...
│   ├── c1-xhtml/<sha256>.xhtml      ← Golden-family cache tier C1 (Block D)
│   └── c2-preprocessor/<sha256>.json ← Golden-family cache tier C2 (Block D)
├── golden/                         ← Block D reconstruction anchors (committed)
│   └── 1.0.0/attention/
│       ├── attention.pdf             The source document
│       ├── config.yaml               The exact config the family binds to
│       ├── document.bgraph.md         The frozen 1.0.0 emit (the anchor)
│       └── PRODUCED_BY               git HEAD sha at freeze time (codebase_sha binding)
└── README.md
```

## The Sandwich Model

Tests stabilize the boundaries, not the middle:

```
Boundary 1 (stable):  PDF → Tika → XHTML → TextElements
                      Only changes if Tika version changes.

Middle (flexible):    TextElements → Rules → ParsedElements
                      Where we iterate. NOT snapshot-tested.

Boundary 2 (stable):  ParsedElements → Graph → graph.json
                      Schema contract for API customers.
```

## Workflow

### Run existing tests

```bash
cargo test -p blazegraph-core
```

No JVM needed — tests load from saved snapshots.

### Add a new fixture PDF

1. Drop the PDF into `test_fixtures/pdfs/`
2. Regenerate snapshots:
   ```bash
   make test-generate-fixtures
   ```
3. Add assertions for the new fixture in `tests/pipeline_tests.rs`
4. Commit both the PDF and its snapshots

### Regenerate all snapshots

After pipeline changes that intentionally alter output:

```bash
make test-clean-fixtures
make test-generate-fixtures
cargo test -p blazegraph-core    # verify tests pass with new snapshots
```

Review the diff carefully before committing — the snapshot change IS the behavioral change.

### Check fixture status

```bash
make test-list-fixtures
```

## Config

Snapshots are generated using the standard processing config:

```
blazegraph-io/blazegraph-cli/configs/processing/config.yaml
```

This enables spatial clustering and paragraph merging — the same pipeline configuration used in production. Without it, text element counts are ~30x higher (raw Tika output without merging).

## What the tests cover

| Module | Tests | What it guards |
|--------|-------|----------------|
| `tika_boundary` | 4 | XHTML byte counts, text element counts per fixture |
| `schema_contract` | 5 | Schema version, required fields, document_info shape |
| `graph_structure` | 7 | Node counts, Document root, sections, node types, sort order |
| `breadcrumbs` | 4 | Title in root, section propagation, depth sanity |

**Total: 20 tests, ~0.2s, no JVM**

## Golden freeze family (Block D — the cold-tier reconstruction anchor)

Separate from the stage snapshots above. `golden/1.0.0/attention/` freezes one
real document's emitted **bgraph.md** at schema `1.0.0` as a *reconstruction
anchor*: the durable artifact a future version-pinned binary can use to prove it
still reproduces this edition. Tests live in `tests/golden_freeze_tests.rs`.

The family:

| Artifact | Role |
|----------|------|
| `golden/1.0.0/attention/attention.pdf` | The source document. |
| `golden/1.0.0/attention/config.yaml` | The exact config the family binds to (`config_hash` is stamped in the md). `dump_analytics: false` so the replay writes no sidecars. |
| `golden/1.0.0/attention/document.bgraph.md` | The frozen `1.0.0` emit — style-bearing (`--include-style-info`) so it self-verifies. |
| `golden/1.0.0/attention/PRODUCED_BY` | `blazegraph-io` git HEAD sha at freeze time — the `codebase_sha` binding. A sidecar, **not** a serialized-artifact field. |
| `snapshots/c1-xhtml/<sha>.xhtml`, `snapshots/c2-preprocessor/<sha>.json` | The committed cache tiers. `<sha>` is the SHA-256 of `attention.pdf`. |

### How the freeze test works (JVM-free)

`golden_freeze_tests.rs` replays the deterministic identity path
(`process_document_with_cache` → `build_graph_deterministic`, a pure function of
`PreprocessorOutput` + config) from the committed **C2 preprocessor cache**. A C2
cache hit skips Tika *and* the XHTML parse entirely — the test's stub
preprocessor panics if either is reached, so a JVM invocation is a loud failure,
not a silent slow pass.

- **Test A — reproduction:** regenerate the md from C2 (JVM-free) and assert it
  is **byte-identical** to the frozen `document.bgraph.md`.
- **Test B — roundtrip:** `parse_markdown` the frozen md and assert `Verified`.

```bash
cargo test -p blazegraph-io-core --test golden_freeze_tests   # or: make golden-test
```

### Re-freeze intentionally (a change legitimately moved the output)

JVM-free — regenerates the md + refreshes `PRODUCED_BY` from the committed C2:

```bash
BLESS_GOLDEN=1 cargo test -p blazegraph-io-core --test golden_freeze_tests
```

### Rebuild the whole family from the PDF (needs the JVM)

Runs a **clean, fresh Tika parse** (`--fresh-from c0`) — the family is never
seeded from a stale cache — rebuilds C1→C2, emits the style-bearing md, and
records the sha:

```bash
make golden-generate   # submodule Makefile; needs JRE + the Tika JAR
```

> **C3 note:** C3 is the cached **output** — the config-keyed `DocumentGraph`,
> from which `bgraph.md`/`.json` are serialized on demand (it is the first
> config-dependent tier; C0–C2 are config-independent intermediates). The
> pipeline does not yet *write* C3 (`store_graph_output` has no caller in
> `process_document_with_cache`); CR-89 corrected the read-gate, and wiring the
> writer + API delivery is the **C3 output-cache feature CR**. The golden family
> needs only C2 — the freeze replays via `FreshFrom::C3`, which skips the C3
> read and always rebuilds — so it commits no `c3-graph/` tier. When the writer
> lands, `golden-generate` can populate the slot.

## Git notes

Both `pdfs/` and `snapshots/` are committed to git. The `blazegraph-io/.gitignore` has a `*.json` rule with an exception for `test_fixtures/**/*.json`.

If the snapshot directory grows past ~100MB, consider Git LFS for the larger files.
