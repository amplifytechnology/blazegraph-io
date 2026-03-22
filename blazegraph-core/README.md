# blazegraph-io-core

Core library for semantic document graph processing. Parses PDFs into structured, queryable graphs with bounding boxes.

This is the library crate that powers [`blazegraph-io`](https://crates.io/crates/blazegraph-io) (the CLI) and the [Blazegraph API](https://blazegraph.io).

## Usage

```rust
use blazegraph_io_core::{DocumentProcessor, ParsingConfig};

let config = ParsingConfig::default();
let processor = DocumentProcessor::new(config);
let graph = processor.process_pdf("document.pdf")?;

for node in &graph.nodes {
    println!("{}: {}", node.node_type, node.content.text);
    if let Some(physical) = &node.location.physical {
        println!("  Page {}, bbox: {:?}", physical.page, physical.bounding_box);
    }
}
```

## Features

- `jni-backend` (default) — Uses JNI to call Apache Tika for PDF text extraction. Requires a JRE (the CLI auto-downloads one; if using the library directly, provide your own).

## When to use this vs the CLI

Use **blazegraph-io-core** when you want to embed the parser in your own Rust application. Use **blazegraph-io** (the CLI crate) for command-line usage.

## Documentation

- [Schema Reference](https://github.com/AmplifyTechnology/blazegraph-io/blob/main/docs/reference/02-schema-reference.md) — Full `bgraph.json` field documentation
- [Configuration Reference](https://github.com/AmplifyTechnology/blazegraph-io/blob/main/docs/reference/03-config-reference.md) — Tuning for your document type

## License

Licensed under either of:

- Apache License, Version 2.0
- MIT license

at your option.
