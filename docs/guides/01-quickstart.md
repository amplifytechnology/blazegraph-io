# Quickstart

Parse your first PDF into a semantic document graph in under 60 seconds.

---

## What Blazegraph Does

Blazegraph transforms PDFs into structured semantic graphs. Instead of flat text extraction, you get a navigable tree of sections, paragraphs, and content — with physical coordinates that map every node to exact locations in the original PDF.

**Input:** A PDF file.
**Output:** A `bgraph.json` — a tree of typed nodes with semantic paths, physical bounding boxes, and token counts.

---

## Install

### Rust CLI

```bash
cargo install blazegraph-io
```

### Python

```bash
pip install blazegraph-io
```

No account needed. No API key. Runs entirely on your machine.

> **First run:** The CLI automatically downloads a Java Runtime (~60MB) for PDF text extraction. It's cached for future use.

---

## Parse a PDF

### CLI

```bash
blazegraph-io parse document.pdf -o bgraph.json
```

### Python

```python
import blazegraphio as bg

graph = bg.parse_pdf("document.pdf")

print(f"Nodes: {len(graph.nodes)}")
print(f"Sections: {len(graph.sections)}")

for section in graph.sections:
    print(section.content.text)
```

Both produce identical `bgraph.json` output.

---

## Understand the Output

Here's what Blazegraph produces for Claude Shannon's *A Mathematical Theory of Communication* (55 pages):

```
55 pages → 3,022 text elements → 94 nodes → 1.1s
```

### The Graph

```json
{
  "schema_version": "0.2.0",
  "nodes": [ ... ],
  "document_info": { ... },
  "structural_profile": { ... }
}
```

| Field | Description |
|-------|-------------|
| `schema_version` | Output format version (currently `"0.2.0"`) |
| `nodes` | Array of all nodes in the document tree |
| `document_info` | Document metadata and font analysis |
| `structural_profile` | Graph analytics — node types, token distributions, depth stats |

### The Document Tree

The graph is a tree. The root is a `Document` node. Its children are `Section` and `Paragraph` nodes. Sections contain further paragraphs and subsections.

### Node Types

**Section** — a detected heading or structural division:

```json
{
  "id": "17113498-be4b-4bb5-88dd-80978ee00266",
  "node_type": "Section",
  "location": {
    "semantic": {
      "path": "2",
      "depth": 1,
      "breadcrumbs": [
        "shannon1948.dvi",
        "A Mathematical Theory of Communication"
      ]
    },
    "physical": {
      "page": 1,
      "bounding_box": { "x": 181.0, "y": 127.9, "width": 265.5, "height": 8.0 }
    }
  },
  "content": { "text": "A Mathematical Theory of Communication" },
  "token_count": 9,
  "parent": "f49c0604-...",
  "children": ["d1ea207e-...", "480f520b-...", "..."]
}
```

**Paragraph** — a merged, semantically coherent block of text:

```json
{
  "id": "480f520b-c205-479c-896f-01d1eeb2efda",
  "node_type": "Paragraph",
  "location": {
    "semantic": {
      "path": "2.2",
      "depth": 2,
      "breadcrumbs": [
        "shannon1948.dvi",
        "A Mathematical Theory of Communication"
      ]
    },
    "physical": {
      "page": 1,
      "bounding_box": { "x": 91.9, "y": 585.9, "width": 427.5, "height": 164.2 }
    }
  },
  "content": {
    "text": "The fundamental problem of communication is that of reproducing at one point either exactly or approximately a message selected at another point..."
  },
  "token_count": 206,
  "parent": "17113498-...",
  "children": []
}
```

### The Location Model

Every node has a **location** with two components:

**Semantic location** (always present) — where the node sits in the document tree:
- `path`: hierarchical position (`"2.2"` = second child of second top-level element)
- `depth`: tree depth (`0` = document root, `1` = section, `2` = content within section)
- `breadcrumbs`: human-readable trail (`["shannon1948.dvi", "A Mathematical Theory of Communication"]`)

**Physical location** (present for PDFs) — where the content appears on the page:
- `page`: page number (1-indexed)
- `bounding_box`: exact coordinates (`x`, `y`, `width`, `height` in PDF points)

This dual location is what makes Blazegraph useful for GraphRAG: a human says "page 1, the paragraph about communication" and a machine says `path: "2.2", page: 1, bbox: {x: 91.9, y: 585.9}` — both pointing at the same content, and the later can be incorperated into any application (highlights, animations, move to content, etc).

---

## Navigating the Tree

Every node has `parent` and `children` fields. To walk the tree:

```python
import json

with open("graph.json") as f:
    graph = json.load(f)

# Index nodes by ID for fast lookup
nodes = {n["id"]: n for n in graph["nodes"]}

# Find the root
root = next(n for n in graph["nodes"] if n["node_type"] == "Document")

# List top-level sections
for child_id in root["children"]:
    child = nodes[child_id]
    if child["node_type"] == "Section":
        print(f"Section: {child['content']['text']}")
        print(f"  Path: {child['location']['semantic']['path']}")
        print(f"  Page: {child['location']['physical']['page']}")
        print(f"  Children: {len(child['children'])} nodes")
```

Or use the Python SDK for typed access:

```python
import blazegraphio as bg

graph = bg.parse_pdf("document.pdf")

for section in graph.sections:
    print(section.content.text)
    print(section.location.semantic.breadcrumbs)
    print(section.render(graph))
```

---

## Configuration

The default config works well for most documents. For specific document types, create a YAML config that tunes section detection thresholds, spatial clustering, and size limits:

```bash
blazegraph-io parse contract.pdf -c my-config.yaml -o bgraph.json
```

The key insight: build one config per document category and reuse it across similar documents in that group. See the [Configuration Reference](../reference/03-config-reference.md) for all tuning parameters.

---

## Docker

The Docker container runs the Blazegraph processing server — use it for async processing in your pipeline:

```bash
make serve
```

Then parse via the Python SDK:

```python
import blazegraphio as bg

bg.configure(host="localhost:8080")
graph = await bg.parse_pdf_async("document.pdf")
```

The container bundles the CLI, JRE, Tika, and the FastAPI server. No Rust toolchain or Java install needed. See the [Docker Guide](./03-docker.md) for setup details.

You can also run one-off CLI parses directly:

```bash
docker run --rm -v $(pwd):/data blazegraph/blazegraph-io \
  parse /data/document.pdf -o /data/bgraph.json
```

---

## What's Next

- **[Schema Reference](../reference/02-schema-reference.md)** — Full field-by-field documentation of `bgraph.json`
- **[Configuration Reference](../reference/03-config-reference.md)** — Tune parsing for your document type
- **[Python SDK Guide](./02-python-sdk.md)** — Typed access, tree navigation, rendering
- **[Hosted API](https://blazegraph.io)** — Scale without infrastructure, pay per page
