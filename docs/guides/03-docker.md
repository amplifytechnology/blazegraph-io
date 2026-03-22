# Docker Guide

Run the Blazegraph processing server as a container. No Rust toolchain, no Java install — the container bundles everything.

---

## Quick Start

### With docker-compose (recommended)

```bash
docker-compose up -d
```

The server starts on `http://localhost:8080`. Verify it's running:

```bash
curl http://localhost:8080/health
# {"status": "ok"}
```

### With docker run

```bash
docker build -t blazegraph-io .
docker run -d -p 8080:8080 blazegraph-io
```

---

## Parse via the Python SDK

Once the server is running, point the SDK at it:

```python
import blazegraphio as bg

bg.configure(host="localhost:8080")
graph = await bg.parse_pdf_async("document.pdf")

for section in graph.sections:
    print(section.content.text)
```

This is the self-hosted tier — async processing without needing a hosted API key. Same output as local mode, same `BlazeGraph` object.

---

## Parse via curl

```bash
curl -X POST http://localhost:8080/v1/process/pdf \
  -F "file=@document.pdf" \
  -o bgraph.json
```

### Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `file` | multipart | required | The PDF to parse |
| `config` | string | none | Path to a config YAML (inside the container) |
| `output_format` | string | `"graph"` | One of: `graph`, `sequential`, `flat` |

---

## One-off CLI parses

You can also use the container for one-off CLI parses without starting the server:

```bash
docker run --rm -v $(pwd):/data blazegraph-io \
  blazegraph-io parse /data/document.pdf -o /data/bgraph.json
```

---

## What's in the container

The Docker image bundles:

- **blazegraph-cli** — the compiled Rust binary
- **JRE** (BellSoft Liberica OpenJDK 21) — for Apache Tika PDF extraction
- **Apache Tika** — PDF text extraction via JNI
- **FastAPI + uvicorn** — the processing server
- **Fonts** (URW Base35, Liberation, DejaVu) — critical for accurate bounding box calculations on PDFs with non-embedded fonts

The image is built in two stages: Rust compilation, then a slim runtime image.

---

## Custom config

To use a custom config file, mount it into the container:

```bash
docker run -d -p 8080:8080 \
  -v $(pwd)/my-config.yaml:/config/my-config.yaml \
  blazegraph-io
```

Then reference it in your API call:

```bash
curl -X POST http://localhost:8080/v1/process/pdf \
  -F "file=@document.pdf" \
  -F "config=/config/my-config.yaml"
```

---

## Environment variables

| Variable | Default | Description |
|----------|---------|-------------|
| `RUST_LOG` | `info` | Log level for the CLI (`debug`, `info`, `warn`, `error`) |
| `BLAZEGRAPH_CLI_PATH` | `/usr/local/bin/blazegraph-io` | Path to the CLI binary |
| `BLAZEGRAPH_JAR_PATH` | `/app/tika-jni.jar` | Path to the Tika JAR |
| `BLAZEGRAPH_CONFIG_PATH` | `/app/default_config.yaml` | Default config file |

---

## Health check

The container includes a built-in health check on the `/health` endpoint. Docker will mark the container as healthy once the server is ready to accept requests.

```bash
docker inspect --format='{{.State.Health.Status}}' <container_id>
```
