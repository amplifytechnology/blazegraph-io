"""Blazegraph Python SDK — parse PDFs into typed semantic document graphs.

Usage::

    import blazegraphio as bg

    # Local mode (no account needed)
    graph = bg.parse_pdf("document.pdf")

    # API mode
    bg.configure(api_key="blaze_prod_XXX...")
    graph = bg.parse_pdf("document.pdf")

    # Async mode (self-hosted or API)
    graph = await bg.parse_pdf_async("document.pdf")
"""

from __future__ import annotations

from blazegraphio._config import configure, get_config
from blazegraphio.errors import (
    BlazeGraphAuthError,
    BlazeGraphCreditsError,
    BlazeGraphError,
    BlazeGraphNotFoundError,
    BlazeGraphProcessingError,
)
from blazegraphio.types import (
    BlazeGraph,
    BoundingBox,
    DepthDistribution,
    DocumentAnalysis,
    DocumentInfo,
    DocumentMetadata,
    DocumentNode,
    HistogramBin,
    NodeContent,
    NodeLocation,
    NodeTypeDistribution,
    PhysicalLocation,
    SemanticLocation,
    StructuralProfile,
    TokenDistribution,
    TokenHistogram,
)

__all__ = [
    # Public API functions
    "configure",
    "parse_pdf",
    "parse_pdf_async",
    # Top-level type
    "BlazeGraph",
    # Node types
    "DocumentNode",
    "NodeLocation",
    "SemanticLocation",
    "PhysicalLocation",
    "BoundingBox",
    "NodeContent",
    # Document info types
    "DocumentInfo",
    "DocumentMetadata",
    "DocumentAnalysis",
    # Structural profile types
    "StructuralProfile",
    "TokenDistribution",
    "TokenHistogram",
    "HistogramBin",
    "NodeTypeDistribution",
    "DepthDistribution",
    # Errors
    "BlazeGraphError",
    "BlazeGraphAuthError",
    "BlazeGraphCreditsError",
    "BlazeGraphProcessingError",
    "BlazeGraphNotFoundError",
]

__version__ = "0.1.1"


def parse_pdf(
    path: str,
    *,
    config: str | None = None,
) -> BlazeGraph:
    """Parse a PDF and return a typed document graph.

    Uses the configured mode:
    - If ``api_key`` is set via :func:`configure`: sends the PDF to the API.
    - Otherwise: runs the ``blazegraph-cli`` binary locally.

    Args:
        path: Path to a PDF file.
        config: Path to a config YAML file (local mode only).

    Returns:
        A :class:`BlazeGraph` with fully typed nodes.

    Raises:
        BlazeGraphAuthError: If the API key is invalid (API mode).
        BlazeGraphCreditsError: If credits are exhausted (API mode).
        BlazeGraphProcessingError: If processing fails.
        BlazeGraphNotFoundError: If the CLI binary is not found (local mode).
    """
    cfg = get_config()
    if cfg.is_http_mode:
        from blazegraphio.client import _sync_parse_pdf

        return _sync_parse_pdf(path, cfg)
    else:
        from blazegraphio.local import _local_parse_pdf

        return _local_parse_pdf(path, config_path=config)


async def parse_pdf_async(
    path: str,
) -> BlazeGraph:
    """Parse a PDF asynchronously via the API.

    Requires an API key to be configured via :func:`configure`.

    Args:
        path: Path to a PDF file.

    Returns:
        A :class:`BlazeGraph` with fully typed nodes.

    Raises:
        BlazeGraphAuthError: If the API key is invalid.
        BlazeGraphCreditsError: If credits are exhausted.
        BlazeGraphProcessingError: If processing fails.
        BlazeGraphError: If no API key is configured.
    """
    cfg = get_config()
    if not cfg.is_http_mode:
        raise BlazeGraphError(
            "parse_pdf_async requires a host or API key. Call bg.configure(host=...) or bg.configure(api_key=...) first."
        )
    from blazegraphio.client import _async_parse_pdf

    return await _async_parse_pdf(path, cfg)
