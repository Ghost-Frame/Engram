"""
Kleos client error hierarchy.

HTTP status codes map to typed exceptions so callers can handle specific
failure modes without parsing error strings.
"""

from __future__ import annotations

from typing import Any


class EngramError(Exception):
    """Base class for all Engram client errors."""

    def __init__(
        self,
        message: str,
        status_code: int = 0,
        response_body: dict[str, Any] | None = None,
    ) -> None:
        super().__init__(message)
        self.status_code = status_code
        self.response_body = response_body or {}

    def __repr__(self) -> str:
        return f"{type(self).__name__}(status_code={self.status_code}, message={str(self)!r})"


class AuthError(EngramError):
    """Raised on HTTP 401 -- missing or invalid API key."""


class ForbiddenError(EngramError):
    """Raised on HTTP 403 -- authenticated but not authorised."""


class NotFoundError(EngramError):
    """Raised on HTTP 404 -- resource does not exist."""


class ValidationError(EngramError):
    """Raised on HTTP 422 or 400 -- request failed server-side validation."""


class RateLimitError(EngramError):
    """Raised on HTTP 429 -- rate limit exceeded."""


class ServerError(EngramError):
    """Raised on HTTP 5xx -- unexpected server failure."""


def raise_for_status(status_code: int, body: dict[str, Any]) -> None:
    """Raise the appropriate typed error for a non-2xx HTTP status code.

    Args:
        status_code: The HTTP status code returned by the server.
        body: Parsed JSON response body (may be empty).

    Raises:
        AuthError: 401
        ForbiddenError: 403
        NotFoundError: 404
        ValidationError: 400 or 422
        RateLimitError: 429
        ServerError: 5xx
        EngramError: any other non-2xx code
    """
    message: str = body.get("error") or body.get("message") or f"HTTP {status_code}"
    exc_map: dict[int, type[EngramError]] = {
        400: ValidationError,
        401: AuthError,
        403: ForbiddenError,
        404: NotFoundError,
        422: ValidationError,
        429: RateLimitError,
    }
    if status_code in exc_map:
        raise exc_map[status_code](message, status_code, body)
    if status_code >= 500:
        raise ServerError(message, status_code, body)
    raise EngramError(message, status_code, body)
