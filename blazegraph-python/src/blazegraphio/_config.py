"""Module-level configuration state for Blazegraph SDK."""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Optional


@dataclass
class _Config:
    """Internal configuration holder."""

    api_key: Optional[str] = None
    url: str = "https://api.blazegraph.io"

    @property
    def is_api_mode(self) -> bool:
        """True if an API key is configured (use HTTP mode)."""
        return self.api_key is not None

    def reset(self) -> None:
        """Reset configuration to defaults."""
        self.api_key = None
        self.url = "https://api.blazegraph.io"


# Module-level singleton
_global_config = _Config()


def configure(
    *,
    api_key: Optional[str] = None,
    url: Optional[str] = None,
) -> None:
    """Configure module-level defaults for all subsequent calls.

    Args:
        api_key: API key for HTTP mode. If set, enables API mode.
        url: API base URL. Defaults to ``https://api.blazegraph.io``.
    """
    if api_key is not None:
        _global_config.api_key = api_key
    if url is not None:
        _global_config.url = url


def get_config() -> _Config:
    """Return the current global configuration."""
    return _global_config
