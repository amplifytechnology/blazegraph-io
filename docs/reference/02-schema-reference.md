# bgraph.json Schema Reference

Complete field-by-field documentation of the BlazeGraph output format (`bgraph.json`).

**Schema version:** `0.2.0`
**Source of truth:** [`types.rs`](../../../../blazegraph-io/blazegraph-core/src/types.rs)

All examples are from processing Claude Shannon's *A Mathematical Theory of Communication* (55 pages).

---

## Top-Level: SortedDocumentGraph

The root object of the `bgraph.json` output.

```json
{
  "schema_version": "0.2.0",
  "nodes": [ ... ],
  "document_info": { ... },
  "structural_profile": { ... }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `schema_version` | string | Output format version. Currently `"0.2.0"`. Check this to detect schema changes. |
| `nodes` | array | All nodes in the document tree, sorted by `text_order`. |
| `document_info` | object | Document-level metadata and analysis. Not a node — information *about* the document. |
| `structural_profile` | object | Statistical properties of the graph (node counts, token distributions, depth). |

---

## DocumentNode

Every element in the `nodes` array is a `DocumentNode`.

```json
{
  "id": "17113498-be4b-4bb5-88dd-80978ee00266",
  "node_type": "Section",
  "location": {
    "semantic": { "path": "2", "depth": 1, "breadcrumbs": ["shannon1948.dvi", "A Mathematical Theory of Communication"] },
    "physical": { "page": 1, "bounding_box": { "x": 181.0, "y": 127.9, "width": 265.5, "height": 8.0 } }
  },
  "text_order": 1,
  "content": { "text": "A Mathematical Theory of Communication" },
  "token_count": 9,
  "parent": "f49c0604-3794-41aa-a7d3-20e4a194a7eb",
  "children": ["d1ea207e-...", "480f520b-...", "..."]
}
```

| Field | Type | Description |
|-------|------|-------------|
| `id` | string (UUID) | Unique identifier for this node. |
| `node_type` | string | One of: `"Document"`, `"Section"`, `"Paragraph"`, `"List"`, `"ListItem"`, `"Table"`, `"Figure"`, `"Header"`, `"Footer"`. |
| `location` | object | Where this node exists — both in the tree and on the page. See [NodeLocation](#nodelocation). |
| `text_order` | integer? | Sequential reading order (0-indexed). `null` for the Document root. |
| `content` | object | The node's text content. See [NodeContent](#nodecontent). |
| `token_count` | integer | Pre-calculated token count for the node's text. Useful for RAG chunk sizing. |
| `parent` | string? (UUID) | Parent node ID. `null` for the Document root. |
| `children` | array (UUID[]) | Child node IDs, ordered by `text_order`. Empty for leaf nodes. |

### Node Types

| Type | Description | Typical depth | Has children? |
|------|-------------|---------------|---------------|
| `Document` | Root node. One per graph. | 0 | Yes — sections and top-level paragraphs |
| `Section` | Detected heading or structural division. | 1+ | Yes — paragraphs and nested sections |
| `Paragraph` | Merged, semantically coherent text block. | 2+ | No (leaf) |
| `List` | Container for list items. | 2+ | Yes — ListItem children |
| `ListItem` | Individual list entry. | 3+ | No (leaf) |
| `Table` | Detected table structure. | 2+ | Varies |
| `Figure` | Detected figure or image reference. | 2+ | Varies |
| `Header` | Page header (repeated content). | 2+ | No (leaf) |
| `Footer` | Page footer (repeated content). | 2+ | No (leaf) |

Currently, PDF processing primarily produces `Document`, `Section`, and `Paragraph` nodes. Other types are defined in the schema for future format support.

---

## NodeLocation

Every node has a location with two components: where it sits in the document tree (semantic) and where it appears on the physical page (physical).

```json
{
  "semantic": {
    "path": "2.3",
    "depth": 2,
    "breadcrumbs": ["shannon1948.dvi", "A Mathematical Theory of Communication"]
  },
  "physical": {
    "page": 1,
    "bounding_box": { "x": 91.9, "y": 585.9, "width": 427.5, "height": 164.2 }
  }
}
```

### SemanticLocation

Always present. Computed from the tree structure.

| Field | Type | Description |
|-------|------|-------------|
| `path` | string | Hierarchical position. `"2.3"` means 3rd child of 2nd top-level element. Empty string for root. |
| `depth` | integer | Tree depth. `0` = document root, `1` = top-level section, `2` = content within section. |
| `breadcrumbs` | string[] | Human-readable trail from root to this node. |

**Path notation:** The path is a dot-separated string of 1-indexed child positions. `"2.3.1"` means: the 1st child of the 3rd child of the 2nd child of root.

### PhysicalLocation

Present for PDFs. `null` for reflow formats (Markdown, DOCX — future).

| Field | Type | Description |
|-------|------|-------------|
| `page` | integer | Page number (1-indexed). |
| `bounding_box` | object | Position on the page in PDF coordinate space. |

### BoundingBox

Coordinates are in PDF points (1 point = 1/72 inch). Origin is top-left of the page.

| Field | Type | Description |
|-------|------|-------------|
| `x` | float | Horizontal position from left edge. |
| `y` | float | Vertical position from top edge. |
| `width` | float | Width of the bounding region. |
| `height` | float | Height of the bounding region. |

**The human-AI bridge:** Semantic and physical locations together let you map between tree positions and page coordinates. A human says "page 3, top paragraph" and a machine could programmatically use `path: "2.5", page: 3, bbox: {x: 72, y: 89}` — both pointing at the same content.

---

## NodeContent

```json
{
  "text": "The fundamental problem of communication is that of reproducing at one point either exactly or approximately a message selected at another point..."
}
```

| Field | Type | Description |
|-------|------|-------------|
| `text` | string | The node's text content, trimmed of leading/trailing whitespace. |

The `content` object is extensible. Future versions may add type-specific fields (e.g., `heading_level` for sections, `table_data` for tables).

---

## DocumentInfo

Document-level metadata. Not a node in the tree — information *about* the document.

```json
{
  "document_info": {
    "root_id": "f49c0604-3794-41aa-a7d3-20e4a194a7eb",
    "document_metadata": { ... },
    "document_analysis": { ... }
  }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `root_id` | string (UUID) | References the `Document` node in the `nodes` array — the tree root. |
| `document_metadata` | object | Metadata extracted from the source format. |
| `document_analysis` | object | Statistical analysis computed from text elements. |

### DocumentMetadata

Extracted from the PDF's XMP/metadata stream. All fields are pass-through — Blazegraph doesn't infer or modify metadata.

```json
{
  "title": "shannon1948.dvi",
  "author": null,
  "language": null,
  "page_count": 55,
  "publisher": null,
  "creator_tool": "dvipsk 5.58f Copyright 1986, 1994 Radical Eye Software",
  "producer": "Acrobat Distiller Command 3.01 for Solaris 2.3 and later (SPARC)",
  "pdf_version": "1.2",
  "created": "1998-07-16T10:14:40Z",
  "modified": null,
  "description": null,
  "encrypted": false,
  "has_marked_content": false
}
```

| Field | Type | Description |
|-------|------|-------------|
| `title` | string? | Document title from metadata. May be a filename if no title is set. |
| `author` | string? | Document author. |
| `language` | string? | Language tag (e.g., `"en"`, `"de"`). |
| `page_count` | integer | Number of pages. Always present (0 if unknown). |
| `publisher` | string? | From `xmp:dc:publisher`. |
| `creator_tool` | string? | The tool that created the document (from `xmp:CreatorTool`). |
| `producer` | string? | The PDF producer (from `pdf:producer`). |
| `pdf_version` | string? | PDF specification version (e.g., `"1.2"`, `"1.7"`). |
| `created` | string? | Creation timestamp (ISO 8601). |
| `modified` | string? | Last modification timestamp (ISO 8601). |
| `description` | string? | Document description from metadata. |
| `encrypted` | boolean? | Whether the PDF is encrypted. |
| `has_marked_content` | boolean? | Whether the PDF has tagged/marked content (accessibility structure). |

### DocumentAnalysis

Computed from the raw text elements during preprocessing. Useful for understanding the document's typographic structure.

| Field | Type | Description |
|-------|------|-------------|
| `font_size_counts` | object | Histogram of font sizes → element count. Keys are size strings (e.g., `"10.0"`). |
| `font_family_counts` | object | Histogram of font families → element count. |
| `bold_counts` | [int, int] | Tuple: `[bold_elements, non_bold_elements]`. |
| `italic_counts` | [int, int] | Tuple: `[italic_elements, non_italic_elements]`. |
| `most_common_font_size` | float | The dominant font size (likely body text). |
| `most_common_font_family` | string | The dominant font family. |
| `all_font_sizes` | float[] | All font sizes found in the document, sorted. |

---

## StructuralProfile

Statistical properties of the document graph. Deterministic — computed mechanically from the tree structure.

```json
{
  "structural_profile": {
    "created_at": "2026-03-18T19:47:00Z",
    "document_type": "Generic",
    "flow_type": "Fixed",
    "total_nodes": 94,
    "total_tokens": 35647,
    "token_distribution": { ... },
    "node_type_distribution": { ... },
    "depth_distribution": { ... }
  }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `created_at` | string (ISO 8601) | When the graph was generated. |
| `document_type` | string | Currently defaults to `"Generic"` for all documents. |
| `flow_type` | string | `"Fixed"` (PDF — has physical layout) or `"Free"` (Markdown, DOCX — reflows). |
| `total_nodes` | integer | Total nodes in the graph. |
| `total_tokens` | integer | Sum of all node token counts. |
| `token_distribution` | object | Token count histograms. |
| `node_type_distribution` | object | Node type counts and percentages. |
| `depth_distribution` | object | Tree depth statistics. |

### NodeTypeDistribution

```json
{
  "counts": { "Document": 1, "Section": 6, "Paragraph": 87 },
  "percentages": { "Document": 1.06, "Section": 6.38, "Paragraph": 92.55 }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `counts` | object | Node count per type. |
| `percentages` | object | Percentage of total nodes per type. |

### DepthDistribution

```json
{
  "max_depth": 3,
  "depth_counts": { "0": 1, "1": 6, "2": 87 },
  "avg_depth": 2.37
}
```

| Field | Type | Description |
|-------|------|-------------|
| `max_depth` | integer | Deepest level in the tree. |
| `depth_counts` | object | Node count at each depth level. |
| `avg_depth` | float | Average node depth. |

### TokenDistribution

Per-type and overall token count histograms. Useful for understanding chunk size distribution (e.g., for RAG).

```json
{
  "by_node_type": {
    "Paragraph": {
      "bins": [
        { "range_start": 0, "range_end": 100, "count": 30, "token_sum": 1500 },
        { "range_start": 100, "range_end": 200, "count": 25, "token_sum": 3750 },
        ...
      ],
      "total_count": 87,
      "total_tokens": 35000,
      "mean": 402.3,
      "median": 350.0,
      "mode": 100,
      "variance": 45000.0
    }
  },
  "overall": { ... }
}
```

**TokenHistogram:**

| Field | Type | Description |
|-------|------|-------------|
| `bins` | array | Bucketed distribution of token counts. |
| `total_count` | integer | Number of nodes in this histogram. |
| `total_tokens` | integer | Sum of all token counts. |
| `mean` | float | Mean token count per node. |
| `median` | float | Median token count. |
| `mode` | integer? | Most frequent bin's `range_start`. `null` if empty. |
| `variance` | float | Variance of token counts. |

**HistogramBin:**

| Field | Type | Description |
|-------|------|-------------|
| `range_start` | integer | Inclusive lower bound of this bin. |
| `range_end` | integer | Exclusive upper bound of this bin. |
| `count` | integer | Number of nodes with token counts in this range. |
| `token_sum` | integer | Total tokens across all nodes in this bin. |

---

## Navigating the Tree

The graph is a tree rooted at the `Document` node. Every node has `parent` and `children` fields that reference other nodes by ID.

### Index-based lookup

```python
nodes = {n["id"]: n for n in graph["nodes"]}

# Get any node by ID
node = nodes["17113498-be4b-4bb5-88dd-80978ee00266"]

# Walk children
for child_id in node["children"]:
    child = nodes[child_id]
```

### Find the root

```python
root = next(n for n in graph["nodes"] if n["node_type"] == "Document")
# Or use document_info:
root = nodes[graph["document_info"]["root_id"]]
```

### Filter by type

```python
sections = [n for n in graph["nodes"] if n["node_type"] == "Section"]
paragraphs = [n for n in graph["nodes"] if n["node_type"] == "Paragraph"]
```

### Get text by page

```python
page_3_nodes = [
    n for n in graph["nodes"]
    if n["location"]["physical"] and n["location"]["physical"]["page"] == 3
]
```

---

## Schema Versioning

The `schema_version` field (currently `"0.2.0"`) follows semver:

- **Major** (X.0.0): Breaking changes to existing fields
- **Minor** (0.X.0): New fields added (backwards compatible)
- **Patch** (0.0.X): Bug fixes to field values

Always check `schema_version` before parsing to handle schema evolution gracefully.
