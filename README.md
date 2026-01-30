# blazegraph-io

Semantic document graph extraction. Transform PDFs into structured, queryable graphs for RAG, search, and document understanding.

## Features

- **Semantic graph output** — Preserves document structure (sections, paragraphs, lists, tables)
- **Bounding boxes** — Every node maps to exact PDF coordinates
- **Hierarchical structure** — Parent-child relationships preserved
- **Fast** — Native Rust with embedded Tika for PDF parsing
- **Local-first** — No API key required, runs entirely on your machine

## Installation

### Build from source

```bash
git clone https://github.com/AmplifyTechnology/blazegraph-io.git
cd blazegraph-io
cargo build --release -p blazegraph-cli
```

### Run

```bash
# Parse a PDF to JSON graph
./target/release/blazegraph-cli -i document.pdf -o graph.json

# With custom config
./target/release/blazegraph-cli -i document.pdf -c config.yaml -o graph.json

# See all options
./target/release/blazegraph-cli --help
```

> **Note:** On first run, the CLI will automatically download a Java Runtime (~60MB) for PDF processing. It's cached for future use:
> - macOS/Linux: `~/.local/share/blazegraph/jre`
> - Windows: `%LOCALAPPDATA%\blazegraph\jre` 

## Output Formats

| Format | Description |
|--------|-------------|
| `graph` | Full graph structure with nodes and edges (default) |
| `sequential` | Ordered segments with hierarchy info (good for RAG) |

```bash
# Sequential format for RAG pipelines
./target/release/blazegraph-cli -i document.pdf -f sequential -o chunks.json
```

## Configuration

See `blazegraph-cli/configs/processing/` for example configuration files. These control:

- Section detection thresholds
- List detection patterns
- Spatial clustering parameters
- Size enforcement (max chunk size)

## Project Structure

```
blazegraph-io/
├── blazegraph-core/     # Core parsing library
├── blazegraph-cli/      # Command-line interface
└── Cargo.toml           # Workspace definition
```

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

## Roadmap

This project is actively developed. Here's what's planned:

### Distribution
- [ ] Publish CLI to crates.io (`blazegraph-io`)
- [ ] Publish core library to crates.io (`blazegraph-io-core`)
- [ ] Publish Python wrapper to PyPI (`blazegraph-io`)

### File Format Support
- [x] PDF
- [ ] Markdown (`.md`)
- [ ] Word documents (`.docx`)

### Schema & Output
- [ ] Stable v1 schema specification
- [ ] Schema documentation with examples
- [ ] Migration guide for schema changes

### Documentation
- [ ] Getting started guide
- [ ] Configuration reference
- [ ] Integration examples (LangChain, LlamaIndex, etc.)
- [ ] Output schema reference

Contributions and feedback welcome! Open an issue to discuss.
