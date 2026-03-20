"""Test CLI subprocess wrapper with mocked subprocess calls."""

from __future__ import annotations

import json
from pathlib import Path
from unittest.mock import MagicMock, patch

import pytest

from blazegraphio.errors import BlazeGraphProcessingError, BlazeGraphNotFoundError
from blazegraphio.local import _local_parse_pdf
from blazegraphio.types import BlazeGraph

_FIXTURES_DIR = Path(__file__).parent / "fixtures"


class TestLocalParsePdf:
    """Test _local_parse_pdf with mocked subprocess."""

    def test_file_not_found(self) -> None:
        with pytest.raises(FileNotFoundError, match="PDF not found"):
            _local_parse_pdf("/nonexistent/file.pdf")

    @patch("blazegraphio.local.find_or_download_cli")
    @patch("blazegraphio.local.get_jre_dir")
    @patch("blazegraphio.local.subprocess.run")
    def test_successful_parse(
        self,
        mock_run: MagicMock,
        mock_jre: MagicMock,
        mock_cli: MagicMock,
        tmp_path: Path,
    ) -> None:
        # Set up mock CLI path
        mock_cli.return_value = tmp_path / "blazegraph-cli"
        mock_jre.return_value = tmp_path / "jre"

        # Create a dummy PDF
        pdf = tmp_path / "test.pdf"
        pdf.write_bytes(b"%PDF-1.4 dummy")

        # Load the real fixture as the CLI output
        fixture = json.loads(
            (_FIXTURES_DIR / "shannon_graph.json").read_text(encoding="utf-8")
        )

        def fake_run(cmd, **kwargs):
            # The CLI writes to the -o path, which is the last argument
            output_path = None
            for i, arg in enumerate(cmd):
                if arg == "-o" and i + 1 < len(cmd):
                    output_path = cmd[i + 1]
                    break
            if output_path:
                Path(output_path).write_text(json.dumps(fixture), encoding="utf-8")
            result = MagicMock()
            result.returncode = 0
            result.stderr = b""
            return result

        mock_run.side_effect = fake_run

        graph = _local_parse_pdf(str(pdf))

        assert isinstance(graph, BlazeGraph)
        assert graph.schema_version == "0.2.0"
        assert len(graph.nodes) == 95
        mock_run.assert_called_once()

    @patch("blazegraphio.local.find_or_download_cli")
    @patch("blazegraphio.local.get_jre_dir")
    @patch("blazegraphio.local.subprocess.run")
    def test_cli_failure(
        self,
        mock_run: MagicMock,
        mock_jre: MagicMock,
        mock_cli: MagicMock,
        tmp_path: Path,
    ) -> None:
        mock_cli.return_value = tmp_path / "blazegraph-cli"
        mock_jre.return_value = tmp_path / "jre"

        pdf = tmp_path / "test.pdf"
        pdf.write_bytes(b"%PDF-1.4 dummy")

        result = MagicMock()
        result.returncode = 1
        result.stderr = b"Error: corrupt PDF"
        mock_run.return_value = result

        with pytest.raises(BlazeGraphProcessingError, match="corrupt PDF"):
            _local_parse_pdf(str(pdf))

    @patch("blazegraphio.local.find_or_download_cli")
    @patch("blazegraphio.local.get_jre_dir")
    @patch("blazegraphio.local.subprocess.run")
    def test_no_output_file(
        self,
        mock_run: MagicMock,
        mock_jre: MagicMock,
        mock_cli: MagicMock,
        tmp_path: Path,
    ) -> None:
        mock_cli.return_value = tmp_path / "blazegraph-cli"
        mock_jre.return_value = tmp_path / "jre"

        pdf = tmp_path / "test.pdf"
        pdf.write_bytes(b"%PDF-1.4 dummy")

        # CLI succeeds but doesn't write output
        result = MagicMock()
        result.returncode = 0
        result.stderr = b""
        mock_run.return_value = result

        with pytest.raises(BlazeGraphProcessingError, match="did not produce output"):
            _local_parse_pdf(str(pdf))

    @patch("blazegraphio.local.find_or_download_cli")
    @patch("blazegraphio.local.get_jre_dir")
    @patch("blazegraphio.local.subprocess.run")
    def test_config_path_passed(
        self,
        mock_run: MagicMock,
        mock_jre: MagicMock,
        mock_cli: MagicMock,
        tmp_path: Path,
    ) -> None:
        mock_cli.return_value = tmp_path / "blazegraph-cli"
        mock_jre.return_value = tmp_path / "jre"

        pdf = tmp_path / "test.pdf"
        pdf.write_bytes(b"%PDF-1.4 dummy")

        fixture = json.loads(
            (_FIXTURES_DIR / "shannon_graph.json").read_text(encoding="utf-8")
        )

        def fake_run(cmd, **kwargs):
            output_path = None
            for i, arg in enumerate(cmd):
                if arg == "-o" and i + 1 < len(cmd):
                    output_path = cmd[i + 1]
                    break
            if output_path:
                Path(output_path).write_text(json.dumps(fixture), encoding="utf-8")
            result = MagicMock()
            result.returncode = 0
            result.stderr = b""
            return result

        mock_run.side_effect = fake_run

        _local_parse_pdf(str(pdf), config_path="/path/to/config.yaml")

        call_args = mock_run.call_args[0][0]
        assert "--config" in call_args
        assert "/path/to/config.yaml" in call_args
