# blazegraph-io

Python SDK for [Blazegraph](https://blazegraph.io) — parse PDFs into typed semantic document graphs.

## Install

```bash
pip install blazegraph-io
```

Python 3.9+. Only dependency: `httpx`.

## Quick Start

```python
import blazegraphio as bg

# Local mode — no account needed, runs on your machine
graph = bg.parse_pdf("document.pdf")

print(f"{len(graph.nodes)} nodes, {len(graph.sections)} sections")

for section in graph.sections:
    print(section.content.text)
    print(section.location.semantic.breadcrumbs)
    print(f"  Page {section.location.physical.page}")
```

On first run, the SDK downloads the `blazegraph-cli` binary and a JRE automatically. Subsequent runs are instant.

## API Mode

Switch to the hosted API with one line:

```python
bg.configure(api_key="blaze_prod_...")
graph = bg.parse_pdf("document.pdf")  # same code, cloud processing
```

Async is also supported:

```python
graph = await bg.parse_pdf_async("document.pdf")
```

## What You Get

Every node has typed fields with IDE autocomplete:

```python
node = graph.sections[0]

node.content.text                        # "Introduction"
node.token_count                         # 206
node.location.semantic.path              # "2.3"
node.location.semantic.breadcrumbs       # ["paper.pdf", "Introduction"]
node.location.physical.page              # 1
node.location.physical.bounding_box      # BoundingBox(x=91.9, y=585.9, ...)
```

Navigate the tree:

```python
parent = node.get_parent(graph)
children = node.get_children(graph)
```

Render text:

```python
print(section.render(graph, breadcrumbs=True))
# [paper.pdf > Introduction]
# The fundamental problem of communication...
```

## Documentation

- [Python SDK Guide](https://blazegraph.io/docs/guides/python-sdk) — Full usage, configuration, error handling
- [Schema Reference](https://blazegraph.io/docs/reference/schema) — Complete `graph.json` field documentation
- [Configuration Reference](https://blazegraph.io/docs/reference/config) — Tuning for your document type

## License

MIT
