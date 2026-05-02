"""
EngramClient -- sync and async Python client for the Kleos memory server.

Sync usage::

    from kleos_client import EngramClient

    client = EngramClient("http://localhost:4200", api_key="ek-...")
    result = client.store_memory("User prefers dark mode", category="preference")
    hits = client.search("user preferences")

Async usage::

    import asyncio
    from kleos_client import AsyncEngramClient

    async def main():
        client = AsyncEngramClient("http://localhost:4200", api_key="ek-...")
        result = await client.store_memory("User prefers dark mode")

    asyncio.run(main())
"""

from __future__ import annotations

import json
from collections.abc import AsyncIterator, Iterator
from typing import Any, Optional, TypeVar
from urllib.parse import urlencode

import httpx

from .errors import EngramError, raise_for_status
from .types import (
    Agent,
    AgentPassport,
    ApiKey,
    BatchRequest,
    BatchResponse,
    ContextRequest,
    ContextResponse,
    CreateAgentRequest,
    CreateKeyRequest,
    CreateKeyResponse,
    FacetedSearchRequest,
    Memory,
    NeighborhoodResponse,
    SearchRequest,
    SearchResult,
    SkillDetail,
    SkillRef,
    StoreRequest,
    StoreResult,
    UpdateRequest,
)

T = TypeVar("T")

DEFAULT_TIMEOUT = 30.0
_BEARER_PREFIX = "Bearer "


def _auth_headers(api_key: str) -> dict[str, str]:
    return {
        "Authorization": f"{_BEARER_PREFIX}{api_key}",
        "Content-Type": "application/json",
        "Accept": "application/json",
    }


def _check_response(resp: httpx.Response) -> dict[str, Any]:
    if resp.is_success:
        if not resp.content:
            return {}
        return resp.json()  # type: ignore[no-any-return]
    try:
        body: dict[str, Any] = resp.json()
    except Exception:
        body = {}
    raise_for_status(resp.status_code, body)
    return {}  # unreachable, keeps mypy happy


def _qs(**kwargs: Any) -> str:
    """Build a query string from non-None keyword arguments."""
    params = {k: v for k, v in kwargs.items() if v is not None}
    return ("?" + urlencode(params)) if params else ""


# ---------------------------------------------------------------------------
# Sync client
# ---------------------------------------------------------------------------


class EngramClient:
    """Synchronous Engram API client.

    Args:
        base_url: Base URL of the Engram server, e.g. ``http://localhost:4200``.
        api_key: Bearer token used for all requests.
        timeout: Request timeout in seconds (default 30).
    """

    def __init__(
        self,
        base_url: str,
        api_key: str,
        timeout: float = DEFAULT_TIMEOUT,
    ) -> None:
        self._base = base_url.rstrip("/")
        self._key = api_key
        self._http = httpx.Client(
            base_url=self._base,
            headers=_auth_headers(api_key),
            timeout=timeout,
        )

    def close(self) -> None:
        """Close the underlying HTTP connection pool."""
        self._http.close()

    def __enter__(self) -> "EngramClient":
        return self

    def __exit__(self, *_: Any) -> None:
        self.close()

    # -----------------------------------------------------------------------
    # Internal helpers
    # -----------------------------------------------------------------------

    def _get(self, path: str) -> dict[str, Any]:
        return _check_response(self._http.get(path))

    def _post(self, path: str, body: Any = None) -> dict[str, Any]:
        data = json.dumps(body) if body is not None else None
        return _check_response(self._http.post(path, content=data))

    def _patch(self, path: str, body: Any = None) -> dict[str, Any]:
        data = json.dumps(body) if body is not None else None
        return _check_response(self._http.patch(path, content=data))

    def _put(self, path: str, body: Any = None) -> dict[str, Any]:
        data = json.dumps(body) if body is not None else None
        return _check_response(self._http.put(path, content=data))

    def _delete(self, path: str) -> dict[str, Any]:
        return _check_response(self._http.delete(path))

    # -----------------------------------------------------------------------
    # Memory -- POST /memories
    # -----------------------------------------------------------------------

    def store_memory(
        self,
        content: str,
        *,
        category: Optional[str] = None,
        source: Optional[str] = None,
        importance: Optional[int] = None,
        tags: Optional[list[str]] = None,
        session_id: Optional[str] = None,
        is_static: Optional[bool] = None,
        space_id: Optional[int] = None,
        parent_memory_id: Optional[int] = None,
    ) -> StoreResult:
        """Store a new memory.

        Args:
            content: Text body of the memory.
            category: Category label (task, decision, preference, ...).
            source: Identifier for the originating source (e.g. agent slug).
            importance: Priority 1-10.
            tags: List of tag strings.
            session_id: Optional session correlation ID.
            is_static: Prevent the memory from being auto-forgotten.
            space_id: Namespace / space for multi-tenant separation.
            parent_memory_id: Parent memory this one supersedes.

        Returns:
            StoreResult with the assigned ID.
        """
        req = StoreRequest(
            content=content,
            category=category,
            source=source,
            importance=importance,
            tags=tags,
            session_id=session_id,
            is_static=is_static,
            space_id=space_id,
            parent_memory_id=parent_memory_id,
        )
        raw = self._post("/memories", req.model_dump(exclude_none=True))
        return StoreResult.model_validate(raw)

    def get_memory(self, memory_id: int) -> Memory:
        """Fetch a single memory by ID.

        Args:
            memory_id: Numeric memory ID.

        Returns:
            Memory object.

        Raises:
            NotFoundError: If no memory with that ID exists.
        """
        raw = self._get(f"/memory/{memory_id}")
        return Memory.model_validate(raw)

    def list_memories(
        self,
        *,
        limit: Optional[int] = None,
        offset: Optional[int] = None,
        category: Optional[str] = None,
        source: Optional[str] = None,
        space_id: Optional[int] = None,
        include_forgotten: Optional[bool] = None,
        include_archived: Optional[bool] = None,
    ) -> list[Memory]:
        """List memories with optional filters.

        Args:
            limit: Maximum number of results (default: server decides).
            offset: Pagination offset.
            category: Filter by category.
            source: Filter by source identifier.
            space_id: Filter by space.
            include_forgotten: Include soft-deleted memories.
            include_archived: Include archived memories.

        Returns:
            List of Memory objects.
        """
        qs = _qs(
            limit=limit,
            offset=offset,
            category=category,
            source=source,
            space_id=space_id,
            include_forgotten=include_forgotten,
            include_archived=include_archived,
        )
        raw = self._get(f"/list{qs}")
        if isinstance(raw, list):
            return [Memory.model_validate(m) for m in raw]
        items = raw.get("data") or raw.get("memories") or []
        return [Memory.model_validate(m) for m in items]

    def update_memory(self, memory_id: int, req: UpdateRequest) -> Memory:
        """Update fields on an existing memory.

        Args:
            memory_id: ID of the memory to update.
            req: Fields to change.

        Returns:
            Updated Memory.
        """
        raw = self._post(
            f"/memory/{memory_id}/update",
            req.model_dump(exclude_none=True),
        )
        return Memory.model_validate(raw)

    def delete_memory(self, memory_id: int) -> None:
        """Hard-delete a memory by ID."""
        self._delete(f"/memory/{memory_id}")

    def forget_memory(self, memory_id: int, reason: Optional[str] = None) -> None:
        """Soft-delete (forget) a memory.

        Args:
            memory_id: ID of the memory to forget.
            reason: Optional human-readable reason.
        """
        body: dict[str, Any] = {}
        if reason is not None:
            body["reason"] = reason
        self._post(f"/memory/{memory_id}/forget", body or None)

    # -----------------------------------------------------------------------
    # Search -- POST /memories/search
    # -----------------------------------------------------------------------

    def search(
        self,
        query: str,
        *,
        limit: Optional[int] = None,
        category: Optional[str] = None,
        tags: Optional[list[str]] = None,
        threshold: Optional[float] = None,
        mode: Optional[str] = None,
        space_id: Optional[int] = None,
        question_type: Optional[str] = None,
    ) -> list[SearchResult]:
        """Hybrid vector + full-text search.

        Args:
            query: Natural-language search query.
            limit: Maximum results to return.
            category: Filter by memory category.
            tags: Filter by tag intersection.
            threshold: Minimum similarity threshold 0-1.
            mode: Search mode: "hybrid" | "vector" | "fts".
            space_id: Limit search to a specific space.
            question_type: Hint for search optimisation.

        Returns:
            Ordered list of SearchResult objects.
        """
        req = SearchRequest(
            query=query,
            limit=limit,
            category=category,
            tags=tags,
            threshold=threshold,
            mode=mode,  # type: ignore[arg-type]
            space_id=space_id,
            question_type=question_type,  # type: ignore[arg-type]
        )
        raw = self._post("/memories/search", req.model_dump(exclude_none=True))
        if isinstance(raw, list):
            return [SearchResult.model_validate(r) for r in raw]
        items = raw.get("results") or []
        return [SearchResult.model_validate(r) for r in items]

    def search_faceted(self, req: FacetedSearchRequest) -> dict[str, Any]:
        """Faceted search with aggregations.

        Args:
            req: Faceted search parameters.

        Returns:
            Raw response dict (shape depends on server version).
        """
        return self._post(
            "/search/faceted", req.model_dump(exclude_none=True)
        )

    # -----------------------------------------------------------------------
    # Context -- POST /context
    # -----------------------------------------------------------------------

    def build_context(
        self,
        query: str,
        *,
        strategy: Optional[str] = None,
        mode: Optional[str] = None,
        max_tokens: Optional[int] = None,
        categories: Optional[list[str]] = None,
    ) -> ContextResponse:
        """Assemble context blocks for injecting into an LLM prompt.

        Args:
            query: The question or prompt the context should answer.
            strategy: Ranking strategy: "semantic" | "temporal" | "importance" | "mixed".
            mode: Breadth mode: "default" | "focused" | "broad".
            max_tokens: Approximate token budget for the assembled context.
            categories: Restrict retrieval to these categories.

        Returns:
            ContextResponse with ranked ContextBlock list.
        """
        req = ContextRequest(
            query=query,
            strategy=strategy,  # type: ignore[arg-type]
            mode=mode,  # type: ignore[arg-type]
            max_tokens=max_tokens,
            categories=categories,
        )
        raw = self._post("/context", req.model_dump(exclude_none=True))
        return ContextResponse.model_validate(raw)

    def stream_context(self, query: str, **kwargs: Any) -> Iterator[str]:
        """Stream context assembly as server-sent events.

        Yields raw SSE event lines. Parse ``data:`` lines as JSON.

        Args:
            query: Query to assemble context for.
            **kwargs: Additional ContextRequest fields.
        """
        req = ContextRequest(query=query, **kwargs)
        payload = json.dumps(req.model_dump(exclude_none=True)).encode()
        with self._http.stream(
            "POST",
            "/context/stream",
            content=payload,
            headers={"Accept": "text/event-stream"},
        ) as resp:
            if not resp.is_success:
                raise EngramError(
                    f"Stream request failed with HTTP {resp.status_code}",
                    resp.status_code,
                )
            for line in resp.iter_lines():
                yield line

    # -----------------------------------------------------------------------
    # Agents -- /agents
    # -----------------------------------------------------------------------

    def list_agents(self) -> list[Agent]:
        """List all registered agents."""
        raw = self._get("/agents")
        items = raw if isinstance(raw, list) else raw.get("data") or []
        return [Agent.model_validate(a) for a in items]

    def create_agent(self, name: str, *, description: Optional[str] = None, slug: Optional[str] = None) -> Agent:
        """Register a new agent.

        Args:
            name: Human-readable agent name.
            description: Optional description.
            slug: Optional URL-safe identifier.

        Returns:
            Newly created Agent.
        """
        req = CreateAgentRequest(name=name, description=description, slug=slug)
        raw = self._post("/agents", req.model_dump(exclude_none=True))
        return Agent.model_validate(raw)

    def get_agent(self, agent_id: int) -> Agent:
        """Fetch agent by ID."""
        return Agent.model_validate(self._get(f"/agents/{agent_id}"))

    def revoke_agent(self, agent_id: int) -> None:
        """Revoke an agent (deactivate it)."""
        self._post(f"/agents/{agent_id}/revoke")

    def get_agent_passport(self, agent_id: int) -> AgentPassport:
        """Issue a signed passport token for an agent."""
        raw = self._get(f"/agents/{agent_id}/passport")
        return AgentPassport.model_validate(raw)

    # -----------------------------------------------------------------------
    # Graph -- /graph/neighborhood/{id}
    # -----------------------------------------------------------------------

    def graph_neighborhood(self, memory_id: int, *, depth: Optional[int] = None) -> NeighborhoodResponse:
        """Retrieve the graph neighbourhood around a memory.

        Args:
            memory_id: Centre node memory ID.
            depth: Maximum hop depth (default: server decides).

        Returns:
            NeighborhoodResponse with nodes and edges.
        """
        qs = _qs(depth=depth)
        raw = self._get(f"/graph/neighborhood/{memory_id}{qs}")
        return NeighborhoodResponse.model_validate(raw)

    # -----------------------------------------------------------------------
    # Skills -- /skills
    # -----------------------------------------------------------------------

    def list_skills(self, *, limit: Optional[int] = None, offset: Optional[int] = None) -> list[SkillRef]:
        """List available skills."""
        qs = _qs(limit=limit, offset=offset)
        raw = self._get(f"/skills{qs}")
        items = raw if isinstance(raw, list) else raw.get("data") or []
        return [SkillRef.model_validate(s) for s in items]

    def get_skill(self, skill_id: int) -> SkillDetail:
        """Fetch full skill detail by ID."""
        raw = self._get(f"/skills/{skill_id}")
        return SkillDetail.model_validate(raw)

    # -----------------------------------------------------------------------
    # Auth / API keys -- /keys
    # -----------------------------------------------------------------------

    def list_keys(self) -> list[ApiKey]:
        """List API keys for the current user."""
        raw = self._get("/keys")
        items = raw if isinstance(raw, list) else raw.get("data") or []
        return [ApiKey.model_validate(k) for k in items]

    def create_key(self, *, name: Optional[str] = None, space_id: Optional[int] = None) -> CreateKeyResponse:
        """Create a new API key.

        Returns:
            CreateKeyResponse containing the plaintext key (shown once).
        """
        req = CreateKeyRequest(name=name, space_id=space_id)
        raw = self._post("/keys", req.model_dump(exclude_none=True))
        return CreateKeyResponse.model_validate(raw)

    def revoke_key(self, key_id: int) -> None:
        """Permanently revoke an API key."""
        self._delete(f"/keys/{key_id}")

    # -----------------------------------------------------------------------
    # Batch -- POST /batch
    # -----------------------------------------------------------------------

    def batch(self, operations: list[dict[str, Any]]) -> BatchResponse:
        """Execute multiple API operations in a single round-trip.

        Each operation is a dict with ``method``, ``path``, and optional ``body``.

        Args:
            operations: List of operation dicts.

        Returns:
            BatchResponse with per-operation results.

        Example::

            client.batch([
                {"method": "POST", "path": "/memories", "body": {"content": "fact A"}},
                {"method": "POST", "path": "/memories", "body": {"content": "fact B"}},
            ])
        """
        req = BatchRequest(operations=operations)  # type: ignore[arg-type]
        raw = self._post("/batch", req.model_dump(exclude_none=True))
        return BatchResponse.model_validate(raw)

    # -----------------------------------------------------------------------
    # Low-level passthrough
    # -----------------------------------------------------------------------

    def raw_get(self, path: str) -> dict[str, Any]:
        """Issue an authenticated GET to any path."""
        return self._get(path)

    def raw_post(self, path: str, body: Any = None) -> dict[str, Any]:
        """Issue an authenticated POST to any path."""
        return self._post(path, body)

    def raw_delete(self, path: str) -> dict[str, Any]:
        """Issue an authenticated DELETE to any path."""
        return self._delete(path)


# ---------------------------------------------------------------------------
# Async client
# ---------------------------------------------------------------------------


class AsyncEngramClient:
    """Asynchronous Engram API client (mirrors EngramClient).

    Args:
        base_url: Base URL of the Engram server.
        api_key: Bearer token.
        timeout: Request timeout in seconds (default 30).
    """

    def __init__(
        self,
        base_url: str,
        api_key: str,
        timeout: float = DEFAULT_TIMEOUT,
    ) -> None:
        self._base = base_url.rstrip("/")
        self._key = api_key
        self._http = httpx.AsyncClient(
            base_url=self._base,
            headers=_auth_headers(api_key),
            timeout=timeout,
        )

    async def aclose(self) -> None:
        """Close the async HTTP connection pool."""
        await self._http.aclose()

    async def __aenter__(self) -> "AsyncEngramClient":
        return self

    async def __aexit__(self, *_: Any) -> None:
        await self.aclose()

    # -----------------------------------------------------------------------
    # Internal helpers
    # -----------------------------------------------------------------------

    async def _get(self, path: str) -> dict[str, Any]:
        return _check_response(await self._http.get(path))

    async def _post(self, path: str, body: Any = None) -> dict[str, Any]:
        data = json.dumps(body) if body is not None else None
        return _check_response(await self._http.post(path, content=data))

    async def _patch(self, path: str, body: Any = None) -> dict[str, Any]:
        data = json.dumps(body) if body is not None else None
        return _check_response(await self._http.patch(path, content=data))

    async def _delete(self, path: str) -> dict[str, Any]:
        return _check_response(await self._http.delete(path))

    # -----------------------------------------------------------------------
    # Memory
    # -----------------------------------------------------------------------

    async def store_memory(
        self,
        content: str,
        *,
        category: Optional[str] = None,
        source: Optional[str] = None,
        importance: Optional[int] = None,
        tags: Optional[list[str]] = None,
        session_id: Optional[str] = None,
        is_static: Optional[bool] = None,
        space_id: Optional[int] = None,
    ) -> StoreResult:
        """Async version of :meth:`EngramClient.store_memory`."""
        req = StoreRequest(
            content=content,
            category=category,
            source=source,
            importance=importance,
            tags=tags,
            session_id=session_id,
            is_static=is_static,
            space_id=space_id,
        )
        raw = await self._post("/memories", req.model_dump(exclude_none=True))
        return StoreResult.model_validate(raw)

    async def get_memory(self, memory_id: int) -> Memory:
        """Async version of :meth:`EngramClient.get_memory`."""
        return Memory.model_validate(await self._get(f"/memory/{memory_id}"))

    async def list_memories(
        self,
        *,
        limit: Optional[int] = None,
        offset: Optional[int] = None,
        category: Optional[str] = None,
        source: Optional[str] = None,
        space_id: Optional[int] = None,
    ) -> list[Memory]:
        """Async version of :meth:`EngramClient.list_memories`."""
        qs = _qs(limit=limit, offset=offset, category=category, source=source, space_id=space_id)
        raw = await self._get(f"/list{qs}")
        if isinstance(raw, list):
            return [Memory.model_validate(m) for m in raw]
        items = raw.get("data") or raw.get("memories") or []
        return [Memory.model_validate(m) for m in items]

    async def update_memory(self, memory_id: int, req: UpdateRequest) -> Memory:
        """Async version of :meth:`EngramClient.update_memory`."""
        raw = await self._post(
            f"/memory/{memory_id}/update",
            req.model_dump(exclude_none=True),
        )
        return Memory.model_validate(raw)

    async def delete_memory(self, memory_id: int) -> None:
        """Async version of :meth:`EngramClient.delete_memory`."""
        await self._delete(f"/memory/{memory_id}")

    async def forget_memory(self, memory_id: int, reason: Optional[str] = None) -> None:
        """Async version of :meth:`EngramClient.forget_memory`."""
        body: dict[str, Any] = {}
        if reason is not None:
            body["reason"] = reason
        await self._post(f"/memory/{memory_id}/forget", body or None)

    # -----------------------------------------------------------------------
    # Search
    # -----------------------------------------------------------------------

    async def search(
        self,
        query: str,
        *,
        limit: Optional[int] = None,
        category: Optional[str] = None,
        tags: Optional[list[str]] = None,
        threshold: Optional[float] = None,
        mode: Optional[str] = None,
        space_id: Optional[int] = None,
    ) -> list[SearchResult]:
        """Async version of :meth:`EngramClient.search`."""
        req = SearchRequest(
            query=query,
            limit=limit,
            category=category,
            tags=tags,
            threshold=threshold,
            mode=mode,  # type: ignore[arg-type]
            space_id=space_id,
        )
        raw = await self._post("/memories/search", req.model_dump(exclude_none=True))
        if isinstance(raw, list):
            return [SearchResult.model_validate(r) for r in raw]
        items = raw.get("results") or []
        return [SearchResult.model_validate(r) for r in items]

    async def search_faceted(self, req: FacetedSearchRequest) -> dict[str, Any]:
        """Async version of :meth:`EngramClient.search_faceted`."""
        return await self._post("/search/faceted", req.model_dump(exclude_none=True))

    # -----------------------------------------------------------------------
    # Context
    # -----------------------------------------------------------------------

    async def build_context(
        self,
        query: str,
        *,
        strategy: Optional[str] = None,
        mode: Optional[str] = None,
        max_tokens: Optional[int] = None,
        categories: Optional[list[str]] = None,
    ) -> ContextResponse:
        """Async version of :meth:`EngramClient.build_context`."""
        req = ContextRequest(
            query=query,
            strategy=strategy,  # type: ignore[arg-type]
            mode=mode,  # type: ignore[arg-type]
            max_tokens=max_tokens,
            categories=categories,
        )
        raw = await self._post("/context", req.model_dump(exclude_none=True))
        return ContextResponse.model_validate(raw)

    async def stream_context(self, query: str, **kwargs: Any) -> AsyncIterator[str]:
        """Async SSE stream of context assembly events."""
        req = ContextRequest(query=query, **kwargs)
        payload = json.dumps(req.model_dump(exclude_none=True)).encode()
        async with self._http.stream(
            "POST",
            "/context/stream",
            content=payload,
            headers={"Accept": "text/event-stream"},
        ) as resp:
            if not resp.is_success:
                raise EngramError(
                    f"Stream request failed with HTTP {resp.status_code}",
                    resp.status_code,
                )
            async for line in resp.aiter_lines():
                yield line

    # -----------------------------------------------------------------------
    # Agents
    # -----------------------------------------------------------------------

    async def list_agents(self) -> list[Agent]:
        """Async version of :meth:`EngramClient.list_agents`."""
        raw = await self._get("/agents")
        items = raw if isinstance(raw, list) else raw.get("data") or []
        return [Agent.model_validate(a) for a in items]

    async def create_agent(self, name: str, *, description: Optional[str] = None, slug: Optional[str] = None) -> Agent:
        """Async version of :meth:`EngramClient.create_agent`."""
        req = CreateAgentRequest(name=name, description=description, slug=slug)
        raw = await self._post("/agents", req.model_dump(exclude_none=True))
        return Agent.model_validate(raw)

    async def get_agent(self, agent_id: int) -> Agent:
        """Async version of :meth:`EngramClient.get_agent`."""
        return Agent.model_validate(await self._get(f"/agents/{agent_id}"))

    async def revoke_agent(self, agent_id: int) -> None:
        """Async version of :meth:`EngramClient.revoke_agent`."""
        await self._post(f"/agents/{agent_id}/revoke")

    # -----------------------------------------------------------------------
    # Graph
    # -----------------------------------------------------------------------

    async def graph_neighborhood(self, memory_id: int, *, depth: Optional[int] = None) -> NeighborhoodResponse:
        """Async version of :meth:`EngramClient.graph_neighborhood`."""
        qs = _qs(depth=depth)
        raw = await self._get(f"/graph/neighborhood/{memory_id}{qs}")
        return NeighborhoodResponse.model_validate(raw)

    # -----------------------------------------------------------------------
    # Skills
    # -----------------------------------------------------------------------

    async def list_skills(self, *, limit: Optional[int] = None, offset: Optional[int] = None) -> list[SkillRef]:
        """Async version of :meth:`EngramClient.list_skills`."""
        qs = _qs(limit=limit, offset=offset)
        raw = await self._get(f"/skills{qs}")
        items = raw if isinstance(raw, list) else raw.get("data") or []
        return [SkillRef.model_validate(s) for s in items]

    async def get_skill(self, skill_id: int) -> SkillDetail:
        """Async version of :meth:`EngramClient.get_skill`."""
        raw = await self._get(f"/skills/{skill_id}")
        return SkillDetail.model_validate(raw)

    # -----------------------------------------------------------------------
    # Auth / Keys
    # -----------------------------------------------------------------------

    async def list_keys(self) -> list[ApiKey]:
        """Async version of :meth:`EngramClient.list_keys`."""
        raw = await self._get("/keys")
        items = raw if isinstance(raw, list) else raw.get("data") or []
        return [ApiKey.model_validate(k) for k in items]

    async def create_key(self, *, name: Optional[str] = None, space_id: Optional[int] = None) -> CreateKeyResponse:
        """Async version of :meth:`EngramClient.create_key`."""
        req = CreateKeyRequest(name=name, space_id=space_id)
        raw = await self._post("/keys", req.model_dump(exclude_none=True))
        return CreateKeyResponse.model_validate(raw)

    async def revoke_key(self, key_id: int) -> None:
        """Async version of :meth:`EngramClient.revoke_key`."""
        await self._delete(f"/keys/{key_id}")

    # -----------------------------------------------------------------------
    # Batch
    # -----------------------------------------------------------------------

    async def batch(self, operations: list[dict[str, Any]]) -> BatchResponse:
        """Async version of :meth:`EngramClient.batch`."""
        req = BatchRequest(operations=operations)  # type: ignore[arg-type]
        raw = await self._post("/batch", req.model_dump(exclude_none=True))
        return BatchResponse.model_validate(raw)

    # -----------------------------------------------------------------------
    # Low-level passthrough
    # -----------------------------------------------------------------------

    async def raw_get(self, path: str) -> dict[str, Any]:
        """Issue an authenticated GET to any path."""
        return await self._get(path)

    async def raw_post(self, path: str, body: Any = None) -> dict[str, Any]:
        """Issue an authenticated POST to any path."""
        return await self._post(path, body)

    async def raw_delete(self, path: str) -> dict[str, Any]:
        """Issue an authenticated DELETE to any path."""
        return await self._delete(path)
