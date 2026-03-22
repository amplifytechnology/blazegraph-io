# Python SDK Guide

The `blazegraph-io` Python package lets you parse PDFs into typed semantic document graphs.

```bash
pip install blazegraph-io
```

**Requirements:** Python 3.9+. The only runtime dependency is `httpx`.

---

## Quick Start

```python
import blazegraphio as bg

graph = bg.parse_pdf("document.pdf")

print(f"Nodes: {len(graph.nodes)}")
print(f"Sections: {len(graph.sections)}")

for section in graph.sections:
    print(section.content.text)
```

On first run, the SDK automatically downloads the `blazegraph-cli` binary and a JRE. Subsequent runs are instant.

---

## Modes

The SDK works at three tiers. Your graph-processing code stays the same regardless of mode.

### Local mode (default)

Runs the `blazegraph-cli` binary via subprocess. No account needed. Synchronous — blocks until the PDF is parsed.

```python
graph = bg.parse_pdf("document.pdf")
```

### Self-hosted mode

Run the Docker container for async processing in your pipeline. No Rust toolchain or Java install needed — the container bundles everything.

```bash
make serve  # starts the Blazegraph processing server
```

```python
bg.configure(host="localhost:8080")
graph = await bg.parse_pdf_async("document.pdf")
```

See the [Docker Guide](./03-docker.md) for setup.

### API mode

Send PDFs to the hosted Blazegraph API at [blazegraph.io](https://blazegraph.io). Same interface, cloud-scale processing.

```python
bg.configure(api_key="blaze_prod_XXX...")
graph = await bg.parse_pdf_async("document.pdf")
```

See the [API documentation](https://blazegraph.io/docs) for API key setup, credit system, and endpoint details.

### The three tiers

```python
# 1. Local — scripts, notebooks, no setup
graph = bg.parse_pdf("paper.pdf")

# 2. Self-hosted — async processing via Docker
bg.configure(host="localhost:8080")
graph = await bg.parse_pdf_async("paper.pdf")

# 3. Hosted API — scale without infrastructure
bg.configure(api_key="blaze_prod_...")
graph = await bg.parse_pdf_async("paper.pdf")
```

---

## The BlazeGraph Object

Every `parse_pdf` call returns a `BlazeGraph` — the top-level wrapper for the document graph.

```python
graph = bg.parse_pdf("document.pdf")

graph.nodes                    # list[DocumentNode] — all nodes
graph.sections                 # list[DocumentNode] — Section nodes only
graph.paragraphs               # list[DocumentNode] — Paragraph nodes only
graph.root                     # DocumentNode — the Document root
graph.document_info            # DocumentInfo — metadata about the document
graph.structural_profile       # StructuralProfile — graph statistics
graph.schema_version           # str — schema version (e.g., "0.2.0")
```

### Lookup helpers

```python
node = graph.get_node("uuid-here")       # By ID
page_nodes = graph.nodes_by_page(3)      # All nodes on page 3
```

### Serialization

```python
graph.to_dict()    # Raw dict (the original JSON)
graph.to_json()    # JSON string
```

---

## Working with Nodes

Each node in the graph is a `DocumentNode` with typed fields:

```python
node = graph.nodes[0]

node.id                        # str (UUID)
node.node_type                 # str — "Document", "Section", "Paragraph", etc.
node.content.text              # str — the node's text
node.token_count               # int — pre-calculated token count

# Semantic location (tree position)
node.location.semantic.path          # str — e.g., "2.3"
node.location.semantic.depth         # int — tree depth
node.location.semantic.breadcrumbs   # list[str] — human-readable trail

# Physical location (page position, PDF only)
node.location.physical.page          # int — page number (1-indexed)
node.location.physical.bounding_box  # BoundingBox (x, y, width, height)

# Tree relationships
node.parent                    # str | None — parent node UUID
node.children                  # list[str] — child node UUIDs
```

### Tree navigation

```python
parent = node.get_parent(graph)      # DocumentNode | None
children = node.get_children(graph)  # list[DocumentNode]
```

---

## Rendering Text

The `render()` method recursively collects text from a node and all its descendants.

### Plain render

```python
section = graph.sections[0]
print(section.render(graph))
```

Output:
```
A Mathematical Theory of Communication

The fundamental problem of communication is that of reproducing at one point
either exactly or approximately a message selected at another point.

Frequently the messages have meaning; that is they refer to or are correlated
according to some system with certain physical or conceptual entities.
```

### With breadcrumbs

```python
print(section.render(graph, breadcrumbs=True))
```

Output:
```
[shannon1948.dvi > A Mathematical Theory of Communication]

The fundamental problem of communication...
```

### With node types

```python
print(section.render(graph, node_types=True))
```

Output:
```
[Section] A Mathematical Theory of Communication

[Paragraph] The fundamental problem of communication...
```

### Full document

```python
print(graph.render())
```

---

## Error Handling

The SDK raises typed exceptions:

```python
from blazegraphio.errors import (
    BlazeGraphError,          # Base exception
    BlazeGraphAuthError,      # 401 — bad/missing API key (API mode)
    BlazeGraphCreditsError,   # 402 — insufficient credits (API mode)
    BlazeGraphProcessingError,# 500 — processing failure
    BlazeGraphNotFoundError,  # CLI binary not found (local mode)
)

try:
    graph = bg.parse_pdf("document.pdf")
except BlazeGraphNotFoundError:
    print("blazegraph-cli not found — it should auto-download on first run")
except BlazeGraphProcessingError as e:
    print(f"Processing failed: {e}")
```

---

## Local Mode Details

### Runtime management

All runtime artifacts live inside the package directory (`site-packages/blazegraphio/_runtime/`). Nothing touches your home directory. `pip uninstall` is a clean removal.

### Binary resolution order

1. `BLAZEGRAPH_CLI_PATH` environment variable (user override)
2. `_runtime/bin/blazegraph-cli` (package-local, from previous download)
3. System PATH / `~/.cargo/bin/blazegraph-cli` (user-installed via `cargo install`)
4. Auto-download from GitHub Releases

### First run

```
>>> bg.parse_pdf("document.pdf")
Downloading blazegraph-cli v0.1.0 (aarch64-apple-darwin)... done.
Downloading JRE (Eclipse Temurin 21)... done.
Processing document.pdf... done.
<BlazeGraph: 94 nodes, schema v0.2.0>
```

### Config file (local mode only)

```python
graph = bg.parse_pdf("document.pdf", config="path/to/config.yaml")
```

See the [Configuration Reference](../reference/03-config-reference.md) for config file options.

---

## Node Types

| Type | Description | Typical depth |
|------|-------------|---------------|
| `Document` | Root node (one per graph) | 0 |
| `Section` | Heading or structural division | 1+ |
| `Paragraph` | Merged text block | 2+ |
| `List` | List container | 2+ |
| `ListItem` | Individual list entry | 3+ |
| `Table` | Detected table | 2+ |
| `Figure` | Detected figure | 2+ |
| `Header` | Page header | 2+ |
| `Footer` | Page footer | 2+ |

Currently, PDF processing produces primarily `Document`, `Section`, and `Paragraph` nodes.

---

## Type Reference

All types are plain Python dataclasses with full IDE autocomplete.

| Type | Description |
|------|-------------|
| `BlazeGraph` | Top-level graph wrapper |
| `DocumentNode` | A single node |
| `NodeLocation` | Combined semantic + physical location |
| `SemanticLocation` | Tree position (path, depth, breadcrumbs) |
| `PhysicalLocation` | Page position (page, bounding box) |
| `BoundingBox` | Position rectangle (x, y, width, height) |
| `NodeContent` | Node text (`text` field) |
| `DocumentInfo` | Document-level metadata |
| `DocumentMetadata` | PDF metadata (title, author, etc.) |
| `DocumentAnalysis` | Typographic analysis (font sizes, etc.) |
| `StructuralProfile` | Graph statistics |
