"""Shared test fixtures for blazegraphio tests."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from blazegraphio.types import BlazeGraph

_FIXTURES_DIR = Path(__file__).parent / "fixtures"


@pytest.fixture
def shannon_raw() -> dict:
    """Load the raw shannon_graph.json as a dict."""
    path = _FIXTURES_DIR / "shannon_graph.json"
    return json.loads(path.read_text(encoding="utf-8"))


@pytest.fixture
def shannon_graph(shannon_raw: dict) -> BlazeGraph:
    """Load the Shannon graph as a fully typed BlazeGraph."""
    return BlazeGraph.from_dict(shannon_raw)
