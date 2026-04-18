"""
Pydantic models for Engram API DTOs.

Models are intentionally non-exhaustive -- optional fields use Optional so
that older server versions with missing fields still deserialise cleanly.
The `model_config` uses `extra="allow"` so new server fields don't break
existing code.
"""

from __future__ import annotations

from typing import Any, Literal, Optional

from pydantic import BaseModel, ConfigDict


# ---------------------------------------------------------------------------
# Shared config
# ---------------------------------------------------------------------------

class _Base(BaseModel):
    model_config = ConfigDict(extra="allow", populate_by_name=True)


# ---------------------------------------------------------------------------
# Enumerations (kept as Literal unions rather than Enum for flexibility)
# ---------------------------------------------------------------------------

MemoryCategory = Literal[
    "task", "discovery", "decision", "state", "issue", "general", "reference",
    "preference", "fact",
]
SearchMode = Literal["hybrid", "vector", "fts"]
QuestionType = Literal[
    "fact_recall", "preference", "reasoning", "generalization", "temporal"
]
ContextStrategy = Literal["semantic", "temporal", "importance", "mixed"]
ContextMode = Literal["default", "focused", "broad"]


# ---------------------------------------------------------------------------
# Memory
# ---------------------------------------------------------------------------

class Memory(_Base):
    id: int
    content: str
    category: str
    source: Optional[str] = None
    session_id: Optional[str] = None
    importance: int = 5
    version: int = 1
    is_latest: bool = True
    parent_memory_id: Optional[int] = None
    root_memory_id: Optional[int] = None
    source_count: int = 0
    is_static: bool = False
    is_forgotten: bool = False
    is_archived: bool = False
    is_fact: bool = False
    is_decomposed: bool = False
    forget_after: Optional[str] = None
    forget_reason: Optional[str] = None
    model: Optional[str] = None
    recall_hits: int = 0
    recall_misses: int = 0
    adaptive_score: Optional[float] = None
    pagerank_score: Optional[float] = None
    last_accessed_at: Optional[str] = None
    access_count: int = 0
    tags: Optional[str] = None
    episode_id: Optional[int] = None
    decay_score: Optional[float] = None
    confidence: float = 1.0
    sync_id: Optional[str] = None
    status: str = "approved"
    user_id: int = 0
    space_id: Optional[int] = None
    valence: Optional[float] = None
    arousal: Optional[float] = None
    dominant_emotion: Optional[str] = None
    created_at: str = ""
    updated_at: str = ""
    is_superseded: bool = False
    is_consolidated: bool = False


# ---------------------------------------------------------------------------
# Store
# ---------------------------------------------------------------------------

class StoreRequest(_Base):
    content: str
    category: Optional[str] = None
    source: Optional[str] = None
    importance: Optional[int] = None
    tags: Optional[list[str]] = None
    embedding: Optional[list[float]] = None
    session_id: Optional[str] = None
    is_static: Optional[bool] = None
    space_id: Optional[int] = None
    parent_memory_id: Optional[int] = None
    agent: Optional[str] = None


class StoreResult(_Base):
    id: int
    status: Optional[str] = None
    created: Optional[bool] = None
    duplicate_of: Optional[int] = None


# ---------------------------------------------------------------------------
# Update
# ---------------------------------------------------------------------------

class UpdateRequest(_Base):
    content: Optional[str] = None
    category: Optional[str] = None
    importance: Optional[int] = None
    tags: Optional[list[str]] = None
    is_static: Optional[bool] = None
    is_fact: Optional[bool] = None


# ---------------------------------------------------------------------------
# Search
# ---------------------------------------------------------------------------

class SearchRequest(_Base):
    query: str
    embedding: Optional[list[float]] = None
    limit: Optional[int] = None
    category: Optional[str] = None
    source: Optional[str] = None
    tags: Optional[list[str]] = None
    threshold: Optional[float] = None
    space_id: Optional[int] = None
    include_forgotten: Optional[bool] = None
    mode: Optional[SearchMode] = None
    question_type: Optional[QuestionType] = None
    expand_relationships: Optional[bool] = None
    include_links: Optional[bool] = None
    latest_only: Optional[bool] = None
    source_filter: Optional[str] = None


class LinkedMemory(_Base):
    id: int
    content: str
    category: str
    similarity: float
    type: str


class VersionChainEntry(_Base):
    id: int
    content: str
    version: int
    is_latest: bool


class SearchResult(_Base):
    memory: Memory
    score: float
    search_type: str = ""
    decay_score: Optional[float] = None
    combined_score: Optional[float] = None
    semantic_score: Optional[float] = None
    fts_score: Optional[float] = None
    graph_score: Optional[float] = None
    personality_signal_score: Optional[float] = None
    temporal_boost: Optional[float] = None
    channels: Optional[list[str]] = None
    question_type: Optional[str] = None
    reranked: Optional[bool] = None
    reranker_ms: Optional[float] = None
    candidate_count: Optional[int] = None
    linked: Optional[list[LinkedMemory]] = None
    version_chain: Optional[list[VersionChainEntry]] = None


class SearchResponse(_Base):
    results: Optional[list[SearchResult]] = None
    total: Optional[int] = None


class FacetedSearchRequest(SearchRequest):
    facets: Optional[list[str]] = None
    group_by: Optional[str] = None


# ---------------------------------------------------------------------------
# Context
# ---------------------------------------------------------------------------

class ContextRequest(_Base):
    query: str
    strategy: Optional[ContextStrategy] = None
    mode: Optional[ContextMode] = None
    max_tokens: Optional[int] = None
    categories: Optional[list[str]] = None
    question_type: Optional[QuestionType] = None
    include_personality: Optional[bool] = None


class ContextBlock(_Base):
    content: str
    source: str
    memory_id: Optional[int] = None
    score: Optional[float] = None
    category: Optional[str] = None


class ContextResponse(_Base):
    blocks: list[ContextBlock] = []
    total_tokens: int = 0
    strategy: str = ""
    mode: str = ""


# ---------------------------------------------------------------------------
# Agent
# ---------------------------------------------------------------------------

class Agent(_Base):
    id: int
    name: str
    description: Optional[str] = None
    slug: Optional[str] = None
    is_active: bool = True
    created_at: Optional[str] = None
    updated_at: Optional[str] = None


class CreateAgentRequest(_Base):
    name: str
    description: Optional[str] = None
    slug: Optional[str] = None


class AgentPassport(_Base):
    token: str
    agent_id: int
    issued_at: Optional[str] = None
    expires_at: Optional[str] = None


# ---------------------------------------------------------------------------
# Graph
# ---------------------------------------------------------------------------

class GraphNode(_Base):
    id: int
    label: str
    node_type: Optional[str] = None
    memory_id: Optional[int] = None
    properties: Optional[dict[str, Any]] = None


class GraphEdge(_Base):
    source_id: int
    target_id: int
    edge_type: str
    weight: Optional[float] = None


class NeighborhoodResponse(_Base):
    center_id: int
    nodes: list[GraphNode] = []
    edges: list[GraphEdge] = []


# ---------------------------------------------------------------------------
# Skill
# ---------------------------------------------------------------------------

class SkillRef(_Base):
    id: int
    name: str
    description: Optional[str] = None
    slug: Optional[str] = None
    version: Optional[str] = None
    tags: Optional[list[str]] = None
    created_at: Optional[str] = None


class SkillDetail(SkillRef):
    content: Optional[str] = None
    parameters: Optional[dict[str, Any]] = None
    dependencies: Optional[list[str]] = None
    execution_count: Optional[int] = None
    success_rate: Optional[float] = None


# ---------------------------------------------------------------------------
# Auth / Keys
# ---------------------------------------------------------------------------

class ApiKey(_Base):
    id: int
    name: Optional[str] = None
    prefix: Optional[str] = None
    created_at: Optional[str] = None
    last_used_at: Optional[str] = None
    is_active: bool = True


class CreateKeyRequest(_Base):
    name: Optional[str] = None
    space_id: Optional[int] = None


class CreateKeyResponse(_Base):
    id: int
    key: str
    name: Optional[str] = None


# ---------------------------------------------------------------------------
# Batch
# ---------------------------------------------------------------------------

class BatchOperation(_Base):
    method: str
    path: str
    body: Optional[dict[str, Any]] = None


class BatchRequest(_Base):
    operations: list[BatchOperation]


class BatchResultItem(_Base):
    status: int
    body: Optional[Any] = None
    error: Optional[str] = None


class BatchResponse(_Base):
    results: list[BatchResultItem] = []
