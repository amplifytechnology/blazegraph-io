# blazegraph-io

Python SDK for [Blazegraph](https://blazegraph.io) — parse PDFs into typed semantic document graphs.

## Install

```bash
pip install blazegraph-io
```

## Quick Start

```python
import blazegraphio as bg

# Local mode (no account needed — uses blazegraph-cli binary)
graph = bg.parse_pdf("document.pdf")

# API mode
bg.configure(api_key="blaze_prod_XXX...")
graph = bg.parse_pdf("document.pdf")

# Typed access
for node in graph.sections:
    print(node.content.text)
    print(node.render(graph))
```

## Documentation

See the [SDK Guide](https://blazegraph.io/docs/guides/python-sdk) for full usage.
