"""
kleos-client -- Python SDK for the Kleos memory server.

Quick start::

    from kleos_client import EngramClient

    client = EngramClient("http://localhost:4200", api_key="ek-your-key")

    # Store a memory
    result = client.store_memory("User prefers dark mode", category="preference", importance=7)
    print(result.id)

    # Search
    hits = client.search("user preferences", limit=10)
    for h in hits:
        print(h.score, h.memory.content)

    # Assemble context for an LLM prompt
    ctx = client.build_context("What are the user's UI preferences?", max_tokens=2000)
    for block in ctx.blocks:
        print(block.content)

Async::

    from kleos_client import AsyncEngramClient
    import asyncio

    async def main():
        async with AsyncEngramClient("http://localhost:4200", api_key="ek-your-key") as c:
            result = await c.store_memory("Async fact")
            hits = await c.search("Async fact")

    asyncio.run(main())
"""

from .client import AsyncEngramClient, EngramClient
from .errors import (
    AuthError,
    EngramError,
    ForbiddenError,
    NotFoundError,
    RateLimitError,
    ServerError,
    ValidationError,
)
from .types import (
    Agent,
    AgentPassport,
    ApiKey,
    BatchOperation,
    BatchRequest,
    BatchResponse,
    BatchResultItem,
    ContextBlock,
    ContextRequest,
    ContextResponse,
    CreateAgentRequest,
    CreateKeyRequest,
    CreateKeyResponse,
    FacetedSearchRequest,
    GraphEdge,
    GraphNode,
    LinkedMemory,
    Memory,
    NeighborhoodResponse,
    SearchRequest,
    SearchResult,
    SkillDetail,
    SkillRef,
    StoreRequest,
    StoreResult,
    UpdateRequest,
    VersionChainEntry,
)

__all__ = [
    # Clients
    "EngramClient",
    "AsyncEngramClient",
    # Errors
    "EngramError",
    "AuthError",
    "ForbiddenError",
    "NotFoundError",
    "ValidationError",
    "RateLimitError",
    "ServerError",
    # Memory types
    "Memory",
    "StoreRequest",
    "StoreResult",
    "UpdateRequest",
    # Search types
    "SearchRequest",
    "SearchResult",
    "FacetedSearchRequest",
    "LinkedMemory",
    "VersionChainEntry",
    # Context types
    "ContextRequest",
    "ContextResponse",
    "ContextBlock",
    # Agent types
    "Agent",
    "AgentPassport",
    "CreateAgentRequest",
    # Graph types
    "GraphNode",
    "GraphEdge",
    "NeighborhoodResponse",
    # Skill types
    "SkillRef",
    "SkillDetail",
    # Auth types
    "ApiKey",
    "CreateKeyRequest",
    "CreateKeyResponse",
    # Batch types
    "BatchRequest",
    "BatchResponse",
    "BatchOperation",
    "BatchResultItem",
]

__version__ = "0.1.0"
