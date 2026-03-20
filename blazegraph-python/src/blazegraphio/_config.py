"""Module-level configuration state for Blazegraph SDK."""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Optional


_DEFAULT_URL = "https://api.blazegraph.io"


@dataclass
class _Config:
    """Internal configuration holder."""

    api_key: Optional[str] = None
    url: Optional[str] = None  # None = not configured = use local CLI mode

    @property
    def is_http_mode(self) -> bool:
        """True if a URL or API key has been configured (use HTTP mode).

        A URL alone (no key) is sufficient — useful for self-hosted instances
        that don't require authentication.
        """
        return self.url is not None or self.api_key is not None

    @property
    def resolved_url(self) -> str:
        """The effective base URL to use for HTTP requests."""
        return self.url or _DEFAULT_URL

    def reset(self) -> None:
        """Reset configuration to defaults."""
        self.api_key = None
        self.url = None


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
