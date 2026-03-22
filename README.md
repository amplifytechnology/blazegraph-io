# blazegraph-io

Parse PDFs into semantic document graphs with bounding boxes. Built for GraphRAG.

**Input:** A PDF file.
**Output:** A `bgraph.json` — a tree of sections, paragraphs, and content with semantic paths, physical coordinates, and token counts.

```
55 pages  →  3,022 text elements  →  94 nodes  →  1.1s
```

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

> On first run, the CLI downloads a Java Runtime (~60MB) for PDF text extraction. It's cached for future use.

## Usage

### CLI

```bash
blazegraph-io parse document.pdf -o bgraph.json
```

Output goes to stdout by default. Use `-o` to write to a file.

### Python

```python
import blazegraphio as bg

graph = bg.parse_pdf("document.pdf")

for section in graph.sections:
    print(section.content.text)
    print(section.location.physical.page)
    print(section.location.physical.bounding_box)
```

### Rust library

```rust
use blazegraph_io_core::{DocumentProcessor, ParsingConfig};

let config = ParsingConfig::default();
let processor = DocumentProcessor::new(config);
let graph = processor.process_pdf("document.pdf")?;
```

## What You Get

Every node in the output has:

- **Semantic location** — tree position (`path: "2.3"`), depth, breadcrumbs
- **Physical location** — page number, bounding box (`x`, `y`, `width`, `height` in PDF points)
- **Content** — the node's text with pre-calculated token count
- **Relationships** — parent and children UUIDs for tree navigation

```json
{
  "node_type": "Paragraph",
  "location": {
    "semantic": { "path": "2.2", "depth": 2, "breadcrumbs": ["paper.pdf", "Introduction"] },
    "physical": { "page": 1, "bounding_box": { "x": 91.9, "y": 585.9, "width": 427.5, "height": 164.2 } }
  },
  "content": { "text": "The fundamental problem of communication..." },
  "token_count": 206
}
```

This dual location model is what makes the output GraphRAG-ready: ground LLM outputs to specific physical locations in the original PDF.

## Configuration

The default config works well for most documents. For specific document types, create a YAML config file:

```bash
blazegraph-io parse contract.pdf -c my-config.yaml -o bgraph.json
```

Build one config per document category (e.g., legal contracts, academic papers) and reuse it across similar documents. See the [Configuration Reference](docs/reference/03-config-reference.md) for all tuning parameters.

## Docker

The Docker container runs the Blazegraph processing server — use it for async processing in your pipeline:

```bash
# Start the processing server
make serve

# Parse via the Python SDK
import blazegraphio as bg
bg.configure(host="localhost:8080")
graph = await bg.parse_pdf_async("document.pdf")
```

The container bundles the CLI, JRE, Tika, and the FastAPI server. No Rust toolchain or Java install needed. See the [Docker Guide](docs/guides/03-docker.md) for setup.

You can also run one-off CLI parses:

```bash
docker run --rm -v $(pwd):/data blazegraph/blazegraph-io \
  parse /data/document.pdf -o /data/bgraph.json
```

## Hosted API

Same parser, no infrastructure. Available at [blazegraph.io](https://blazegraph.io):

```python
bg.configure(api_key="blaze_prod_...")
graph = bg.parse_pdf("document.pdf")  # same code, cloud processing
```

The Python SDK supports three tiers — same interface at every level:

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

500 free credits on signup. See [blazegraph.io](https://blazegraph.io) for details.

## Documentation

- [Quickstart](docs/guides/01-quickstart.md) — Parse your first PDF in 60 seconds
- [Python SDK Guide](docs/guides/02-python-sdk.md) — Typed access, tree navigation, rendering
- [Schema Reference](docs/reference/02-schema-reference.md) — Full `bgraph.json` field documentation
- [Configuration Reference](docs/reference/03-config-reference.md) — Tuning for your document type

## Project Structure

```
blazegraph-io/
├── blazegraph-core/     # Core parsing library (blazegraph-io-core on crates.io)
├── blazegraph-cli/      # Command-line interface (blazegraph-io on crates.io)
├── blazegraph-python/   # Python SDK (blazegraph-io on PyPI)
└── docs/                # Documentation
```

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.
