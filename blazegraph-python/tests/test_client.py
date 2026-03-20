"""Test HTTP client with mocked httpx responses."""

from __future__ import annotations

import json
from pathlib import Path
from unittest.mock import MagicMock, patch

import pytest

from blazegraphio._config import _Config
from blazegraphio.client import _handle_response, _sync_parse_pdf, _async_parse_pdf
from blazegraphio.errors import (
    BlazeGraphAuthError,
    BlazeGraphCreditsError,
    BlazeGraphError,
    BlazeGraphProcessingError,
)
from blazegraphio.types import BlazeGraph

_FIXTURES_DIR = Path(__file__).parent / "fixtures"


def _make_response(status_code: int, body: dict) -> MagicMock:
    """Create a mock httpx.Response."""
    resp = MagicMock()
    resp.status_code = status_code
    resp.json.return_value = body
    resp.text = json.dumps(body)
    return resp


def _success_body() -> dict:
    """Load a success response body from the fixture."""
    raw = json.loads((_FIXTURES_DIR / "shannon_graph.json").read_text())
    return {
        "success": True,
        "graph": raw,
        "pipeline_diagnostics": {
            "processing_time_ms": 964,
            "node_count": 95,
        },
        "billing": {
            "credits_consumed": 55,
            "credits_remaining": 445,
        },
        "error": None,
    }


class TestHandleResponse:
    """Test _handle_response maps HTTP status codes to exceptions."""

    def test_success_200(self) -> None:
        body = _success_body()
        resp = _make_response(200, body)
        graph = _handle_response(resp)
        assert isinstance(graph, BlazeGraph)
        assert graph.schema_version == "0.2.0"
        assert len(graph.nodes) == 95

    def test_401_raises_auth_error(self) -> None:
        body = {
            "success": False,
            "error": {"code": "unauthorized", "message": "Invalid API key"},
        }
        resp = _make_response(401, body)
        with pytest.raises(BlazeGraphAuthError, match="Invalid API key"):
            _handle_response(resp)

    def test_402_raises_credits_error(self) -> None:
        body = {
            "success": False,
            "error": {"code": "payment_required", "message": "Insufficient credits"},
        }
        resp = _make_response(402, body)
        with pytest.raises(BlazeGraphCreditsError, match="Insufficient credits"):
            _handle_response(resp)

    def test_500_raises_processing_error(self) -> None:
        body = {
            "success": False,
            "error": {"code": "processing_error", "message": "PDF corrupt"},
        }
        resp = _make_response(500, body)
        with pytest.raises(BlazeGraphProcessingError, match="PDF corrupt"):
            _handle_response(resp)

    def test_500_non_json(self) -> None:
        resp = MagicMock()
        resp.status_code = 500
        resp.json.side_effect = Exception("not json")
        with pytest.raises(BlazeGraphProcessingError, match="Server error"):
            _handle_response(resp)

    def test_success_false_raises(self) -> None:
        body = {
            "success": False,
            "error": {"code": "bad_request", "message": "Empty body"},
        }
        resp = _make_response(200, body)
        with pytest.raises(BlazeGraphProcessingError, match="Empty body"):
            _handle_response(resp)

    def test_unexpected_status(self) -> None:
        resp = _make_response(418, {"detail": "I'm a teapot"})
        with pytest.raises(BlazeGraphError, match="Unexpected HTTP 418"):
            _handle_response(resp)


class TestSyncParsePdf:
    """Test _sync_parse_pdf with mocked httpx.Client."""

    def test_file_not_found(self) -> None:
        cfg = _Config(api_key="blaze_prod_test", url="https://api.blazegraph.io")
        with pytest.raises(FileNotFoundError, match="PDF not found"):
            _sync_parse_pdf("/nonexistent/file.pdf", cfg)

    @patch("blazegraphio.client.httpx.Client")
    def test_sends_request(self, mock_client_cls: MagicMock, tmp_path: Path) -> None:
        # Create a dummy PDF
        pdf = tmp_path / "test.pdf"
        pdf.write_bytes(b"%PDF-1.4 dummy")

        body = _success_body()
        mock_response = _make_response(200, body)

        mock_client = MagicMock()
        mock_client.__enter__ = MagicMock(return_value=mock_client)
        mock_client.__exit__ = MagicMock(return_value=False)
        mock_client.post.return_value = mock_response
        mock_client_cls.return_value = mock_client

        cfg = _Config(api_key="blaze_prod_test", url="https://api.blazegraph.io")
        graph = _sync_parse_pdf(str(pdf), cfg)

        assert isinstance(graph, BlazeGraph)
        mock_client.post.assert_called_once()
        call_kwargs = mock_client.post.call_args
        assert "/v1/process/pdf" in call_kwargs[0][0] or "/v1/process/pdf" in str(call_kwargs)


class TestAsyncParsePdf:
    """Test _async_parse_pdf with mocked httpx.AsyncClient."""

    def test_file_not_found(self) -> None:
        import asyncio
        cfg = _Config(api_key="blaze_prod_test", url="https://api.blazegraph.io")
        with pytest.raises(FileNotFoundError, match="PDF not found"):
            asyncio.get_event_loop().run_until_complete(
                _async_parse_pdf("/nonexistent/file.pdf", cfg)
            )
