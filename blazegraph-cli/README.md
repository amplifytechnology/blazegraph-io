# blazegraph-io

Parse PDFs into semantic document graphs with bounding boxes. Built for GraphRAG.

```
55 pages  →  3,022 text elements  →  94 nodes  →  1.1s
```

## Install

```bash
cargo install blazegraph-io
```

No account needed. No API key. Runs entirely on your machine.

> On first run, the CLI downloads a Java Runtime (~60MB) for PDF text extraction. It's cached for future use.

## Usage

```bash
# Parse a PDF, output to stdout
blazegraph-io parse document.pdf

# Write to a file
blazegraph-io parse document.pdf -o bgraph.json

# Use a custom config
blazegraph-io parse contract.pdf -c my-config.yaml -o bgraph.json
```

## What You Get

Every node in the output graph has:

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

The default config works well for most documents. For specific document types, create a YAML config file and reuse it across similar documents.

See the [Configuration Reference](https://github.com/AmplifyTechnology/blazegraph-io/blob/main/docs/reference/03-config-reference.md) for all tuning parameters.

## As a library

If you want to embed the parser in your own Rust application, use [`blazegraph-io-core`](https://crates.io/crates/blazegraph-io-core) instead.

## Python SDK

A typed Python SDK is also available:

```bash
pip install blazegraph-io
```

See the [Python SDK Guide](https://github.com/AmplifyTechnology/blazegraph-io/blob/main/docs/guides/02-python-sdk.md).

## Hosted API

Same parser, no infrastructure. Available at [blazegraph.io](https://blazegraph.io). 500 free credits on signup.

## License

Licensed under either of:

- Apache License, Version 2.0
- MIT license

at your option.
