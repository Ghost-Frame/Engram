/**
 * Engram SDK Types
 *
 * Type definitions matching the engram-server API.
 */
export type MemoryCategory = 'task' | 'discovery' | 'decision' | 'state' | 'issue' | 'general' | 'reference';
export type QuestionType = 'fact_recall' | 'preference' | 'reasoning' | 'generalization' | 'temporal';
export type SearchMode = 'hybrid' | 'vector' | 'fts';
export type MemoryStatus = 'approved' | 'pending';
/**
 * A stored memory record.
 */
export interface Memory {
    id: number;
    content: string;
    category: string;
    source: string;
    session_id?: string;
    importance: number;
    embedding?: number[];
    version: number;
    is_latest: boolean;
    parent_memory_id?: number;
    root_memory_id?: number;
    source_count: number;
    is_static: boolean;
    is_forgotten: boolean;
    is_archived: boolean;
    is_fact: boolean;
    is_decomposed: boolean;
    forget_after?: string;
    forget_reason?: string;
    model?: string;
    recall_hits: number;
    recall_misses: number;
    adaptive_score?: number;
    pagerank_score?: number;
    last_accessed_at?: string;
    access_count: number;
    tags?: string;
    episode_id?: number;
    decay_score?: number;
    confidence: number;
    sync_id?: string;
    status: string;
    user_id: number;
    space_id?: number;
    valence?: number;
    arousal?: number;
    dominant_emotion?: string;
    created_at: string;
    updated_at: string;
    is_superseded: boolean;
    is_consolidated: boolean;
}
/**
 * Request to store a new memory.
 */
export interface StoreRequest {
    content: string;
    category?: MemoryCategory | string;
    source?: string;
    importance?: number;
    tags?: string[];
    embedding?: number[];
    session_id?: string;
    is_static?: boolean;
    space_id?: number;
    parent_memory_id?: number;
}
/**
 * Result of storing a memory.
 */
export interface StoreResult {
    id: number;
    created: boolean;
    duplicate_of?: number;
}
/**
 * Request to search memories.
 */
export interface SearchRequest {
    query: string;
    embedding?: number[];
    limit?: number;
    category?: MemoryCategory | string;
    source?: string;
    tags?: string[];
    threshold?: number;
    space_id?: number;
    include_forgotten?: boolean;
    mode?: SearchMode;
    question_type?: QuestionType;
    expand_relationships?: boolean;
    include_links?: boolean;
    latest_only?: boolean;
    source_filter?: string;
}
/**
 * A linked memory reference.
 */
export interface LinkedMemory {
    id: number;
    content: string;
    category: string;
    similarity: number;
    type: string;
}
/**
 * A version chain entry for memory history.
 */
export interface VersionChainEntry {
    id: number;
    content: string;
    version: number;
    is_latest: boolean;
}
/**
 * A search result with scoring details.
 */
export interface SearchResult {
    memory: Memory;
    score: number;
    search_type: string;
    decay_score?: number;
    combined_score?: number;
    semantic_score?: number;
    fts_score?: number;
    graph_score?: number;
    personality_signal_score?: number;
    temporal_boost?: number;
    channels?: string[];
    question_type?: QuestionType;
    reranked?: boolean;
    reranker_ms?: number;
    candidate_count?: number;
    linked?: LinkedMemory[];
    version_chain?: VersionChainEntry[];
}
/**
 * Options for listing memories.
 */
export interface ListOptions {
    limit?: number;
    offset?: number;
    category?: MemoryCategory | string;
    source?: string;
    space_id?: number;
    include_forgotten?: boolean;
    include_archived?: boolean;
}
/**
 * Request to update a memory.
 */
export interface UpdateRequest {
    content?: string;
    category?: MemoryCategory | string;
    importance?: number;
    tags?: string[];
    is_static?: boolean;
    is_fact?: boolean;
}
export type ContextStrategy = 'semantic' | 'temporal' | 'importance' | 'mixed';
export type ContextMode = 'default' | 'focused' | 'broad';
/**
 * Request to assemble context for a prompt.
 */
export interface ContextRequest {
    query: string;
    strategy?: ContextStrategy;
    mode?: ContextMode;
    max_tokens?: number;
    categories?: string[];
    question_type?: QuestionType;
    include_personality?: boolean;
}
/**
 * A block of context content.
 */
export interface ContextBlock {
    content: string;
    source: string;
    memory_id?: number;
    score?: number;
    category?: string;
}
/**
 * Result of context assembly.
 */
export interface ContextResult {
    blocks: ContextBlock[];
    total_tokens: number;
    strategy: string;
    mode: string;
}
/**
 * Configuration for the Engram client.
 */
export interface EngramClientConfig {
    url: string;
    apiKey: string;
    timeout?: number;
}
/**
 * Error response from the API.
 */
export interface ApiError {
    error: string;
}
/**
 * Custom error class for Engram API errors.
 */
export declare class EngramError extends Error {
    statusCode: number;
    response?: ApiError | undefined;
    constructor(message: string, statusCode: number, response?: ApiError | undefined);
}
//# sourceMappingURL=types.d.ts.map