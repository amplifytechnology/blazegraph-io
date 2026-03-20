"""Exception hierarchy for Blazegraph SDK."""


class BlazeGraphError(Exception):
    """Base exception for all Blazegraph errors."""

    pass


class BlazeGraphAuthError(BlazeGraphError):
    """Raised on 401 responses (bad or missing API key)."""

    pass


class BlazeGraphCreditsError(BlazeGraphError):
    """Raised on 402 responses (insufficient credits)."""

    pass


class BlazeGraphProcessingError(BlazeGraphError):
    """Raised on 500 responses or CLI processing failures."""

    pass


class BlazeGraphNotFoundError(BlazeGraphError):
    """Raised when the blazegraph-cli binary is not found (local mode only)."""

    pass
