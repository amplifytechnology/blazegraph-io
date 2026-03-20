"""FastAPI server wrapping the blazegraph-cli binary.

Provides a self-hosted API compatible with the hosted endpoint structure
at api.blazegraph.io. No auth, no billing — just document parsing.
"""

from __future__ import annotations

import json
import os
import subprocess
import tempfile
from pathlib import Path

from fastapi import FastAPI, File, Query, UploadFile
from fastapi.responses import JSONResponse

app = FastAPI(
    title="Blazegraph IO — Self-Hosted",
    description="Local document parsing API powered by blazegraph-cli",
    version="0.1.0",
)

CLI_PATH = os.environ.get("BLAZEGRAPH_CLI_PATH", "/app/bin/blazegraph-cli")
JAR_PATH = os.environ.get("BLAZEGRAPH_JAR_PATH", "/app/bin/blazing-tika-jni.jar")


@app.get("/health")
async def health() -> dict:
    return {"status": "ok"}


@app.post("/v1/process/pdf")
async def process_pdf(
    file: UploadFile = File(...),
    config: str | None = Query(None, description="Path to config YAML"),
    output_format: str = Query("graph", description="graph, sequential, or flat"),
) -> JSONResponse:
    # Validate file type
    if file.content_type and file.content_type != "application/pdf":
        if not (file.filename and file.filename.lower().endswith(".pdf")):
            return JSONResponse(
                status_code=400,
                content={"error": f"Expected a PDF file, got: {file.content_type}"},
            )

    # Validate output format
    valid_formats = {"graph", "sequential", "flat"}
    if output_format not in valid_formats:
        return JSONResponse(
            status_code=400,
            content={"error": f"Invalid output_format '{output_format}'. Must be one of: {', '.join(sorted(valid_formats))}"},
        )

    tmp_dir = None
    try:
        tmp_dir = tempfile.mkdtemp(prefix="blazegraph_")
        input_path = Path(tmp_dir) / "input.pdf"
        output_path = Path(tmp_dir) / "output.json"

        # Write uploaded PDF to temp file
        content = await file.read()
        input_path.write_bytes(content)

        # Build CLI command
        cmd = [
            CLI_PATH,
            "-i", str(input_path),
            "--jar-path", JAR_PATH,
            "-f", output_format,
            "-o", str(output_path),
        ]
        if config:
            cmd.extend(["--config", config])

        # Run CLI
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=300,
        )

        if result.returncode != 0:
            return JSONResponse(
                status_code=500,
                content={"error": f"CLI processing failed: {result.stderr.strip() or result.stdout.strip()}"},
            )

        # Read output
        if not output_path.exists():
            return JSONResponse(
                status_code=500,
                content={"error": "CLI completed but no output file was produced"},
            )

        output_data = json.loads(output_path.read_text())
        return JSONResponse(content=output_data)

    except subprocess.TimeoutExpired:
        return JSONResponse(
            status_code=500,
            content={"error": "Processing timed out after 300 seconds"},
        )
    except Exception as exc:
        return JSONResponse(
            status_code=500,
            content={"error": f"Unexpected error: {exc}"},
        )
    finally:
        # Clean up temp files
        if tmp_dir:
            import shutil
            shutil.rmtree(tmp_dir, ignore_errors=True)
