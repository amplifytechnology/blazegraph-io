"""CLI subprocess wrapper for local-mode PDF processing."""

from __future__ import annotations

import json
import subprocess
import tempfile
from pathlib import Path

from blazegraphio._download import find_or_download_cli, get_jre_dir
from blazegraphio.errors import BlazeGraphProcessingError
from blazegraphio.types import BlazeGraph


def _local_parse_pdf(
    path: str,
    *,
    config_path: str | None = None,
) -> BlazeGraph:
    """Parse a PDF using the blazegraph-cli binary.

    Args:
        path: Path to the PDF file.
        config_path: Optional path to a config YAML file.

    Returns:
        A :class:`BlazeGraph` with fully typed nodes.

    Raises:
        BlazeGraphProcessingError: If the CLI exits with an error.
        BlazeGraphNotFoundError: If the CLI binary cannot be found.
        FileNotFoundError: If the PDF file does not exist.
    """
    pdf_path = Path(path)
    if not pdf_path.exists():
        raise FileNotFoundError(f"PDF not found: {path}")

    cli_path = find_or_download_cli()
    jre_dir = get_jre_dir()

    with tempfile.TemporaryDirectory() as tmpdir:
        output_path = Path(tmpdir) / "graph.json"

        cmd = [
            str(cli_path),
            "-i", str(pdf_path),
            "--jre-path", str(jre_dir),
            "-f", "graph",
            "-o", str(output_path),
        ]

        if config_path:
            cmd.extend(["--config", str(config_path)])

        print(f"Processing {pdf_path.name}... ", end="", flush=True)

        result = subprocess.run(
            cmd,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            check=False,
        )

        if result.returncode != 0:
            stderr_text = result.stderr.decode("utf-8", errors="replace").strip()
            print("failed.")
            raise BlazeGraphProcessingError(
                f"blazegraph-cli exited with code {result.returncode}: {stderr_text}"
            )

        if not output_path.exists():
            print("failed.")
            raise BlazeGraphProcessingError(
                "blazegraph-cli completed but did not produce output."
            )

        print("done.")

        raw = json.loads(output_path.read_text(encoding="utf-8"))
        return BlazeGraph.from_dict(raw)
