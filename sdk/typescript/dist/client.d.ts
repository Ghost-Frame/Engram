/**
 * Engram Client
 *
 * TypeScript client for the Engram memory server API.
 */
import type { EngramClientConfig, StoreRequest, StoreResult, SearchRequest, SearchResult, ListOptions, Memory, UpdateRequest, ContextRequest, ContextResult } from './types.js';
/**
 * Client for interacting with the Engram memory server.
 *
 * @example
 * ```typescript
 * const engram = new EngramClient({
 *   url: 'http://localhost:4200',
 *   apiKey: process.env.ENGRAM_API_KEY!,
 * });
 *
 * // Store a memory
 * const result = await engram.store({
 *   content: 'Important fact to remember',
 *   category: 'general',
 *   importance: 7,
 * });
 *
 * // Search memories
 * const results = await engram.search({
 *   query: 'important facts',
 *   limit: 10,
 * });
 * ```
 */
export declare class EngramClient {
    private readonly baseUrl;
    private readonly apiKey;
    private readonly timeout;
    constructor(config: EngramClientConfig);
    /**
     * Make an authenticated request to the API.
     */
    private request;
    /**
     * Store a new memory.
     *
     * @param req - The memory to store
     * @returns The stored memory ID and whether it was created or deduplicated
     */
    store(req: StoreRequest): Promise<StoreResult>;
    /**
     * Get a memory by ID.
     *
     * @param id - The memory ID
     * @returns The memory or null if not found
     */
    get(id: number): Promise<Memory | null>;
    /**
     * List memories with optional filtering.
     *
     * @param options - Filtering and pagination options
     * @returns Array of memories
     */
    list(options?: ListOptions): Promise<Memory[]>;
    /**
     * Search memories using hybrid search (vector + full-text).
     *
     * @param req - Search parameters
     * @returns Array of search results with scores
     */
    search(req: SearchRequest): Promise<SearchResult[]>;
    /**
     * Update an existing memory.
     *
     * @param id - The memory ID to update
     * @param req - Fields to update
     * @returns The updated memory
     */
    update(id: number, req: UpdateRequest): Promise<Memory>;
    /**
     * Delete a memory.
     *
     * @param id - The memory ID to delete
     */
    delete(id: number): Promise<void>;
    /**
     * Mark a memory as forgotten (soft delete).
     *
     * @param id - The memory ID to forget
     * @param reason - Optional reason for forgetting
     */
    forget(id: number, reason?: string): Promise<void>;
    /**
     * Assemble context from relevant memories for a prompt.
     *
     * @param req - Context assembly parameters
     * @returns Assembled context blocks
     */
    assembleContext(req: ContextRequest): Promise<ContextResult>;
    /**
     * Raw API access for endpoints not covered by high-level methods.
     */
    readonly api: {
        get: <T>(path: string) => Promise<T>;
        post: <T>(path: string, body?: unknown) => Promise<T>;
        patch: <T>(path: string, body?: unknown) => Promise<T>;
        delete: <T>(path: string) => Promise<T>;
    };
}
//# sourceMappingURL=client.d.ts.map