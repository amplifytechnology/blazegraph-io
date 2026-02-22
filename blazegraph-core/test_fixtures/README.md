# Test Fixtures

Pipeline boundary tests for blazegraph-core. Tests load pre-generated snapshots and assert stability at the pipeline edges — no JVM required.

## Structure

```
test_fixtures/
├── pdfs/                          ← Fixture PDFs (committed to git)
│   ├── claude_shannon_paper.pdf      Small academic paper (~358KB)
│   └── elements_of_euclid.pdf       Large book (~1.8MB)
├── snapshots/                     ← Generated pipeline outputs (committed to git)
│   ├── claude_shannon_paper/
│   │   ├── stage1a_xhtml.html        Tika XHTML output (boundary 1)
│   │   ├── stage1b_text_elements.json
│   │   ├── stage2_parsed_elements.json
│   │   ├── stage3_graph.json         Final graph output (boundary 2)
│   │   └── summary.json
│   └── elements_of_euclid/
│       └── ...
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

## Git notes

Both `pdfs/` and `snapshots/` are committed to git. The `blazegraph-io/.gitignore` has a `*.json` rule with an exception for `test_fixtures/**/*.json`.

If the snapshot directory grows past ~100MB, consider Git LFS for the larger files.
