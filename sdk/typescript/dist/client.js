/**
 * Engram Client
 *
 * TypeScript client for the Engram memory server API.
 */
import { EngramError } from './types.js';
const DEFAULT_TIMEOUT = 30000;
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
export class EngramClient {
    baseUrl;
    apiKey;
    timeout;
    constructor(config) {
        this.baseUrl = config.url.replace(/\/$/, ''); // Remove trailing slash
        this.apiKey = config.apiKey;
        this.timeout = config.timeout ?? DEFAULT_TIMEOUT;
    }
    /**
     * Make an authenticated request to the API.
     */
    async request(method, path, body) {
        const url = `${this.baseUrl}${path}`;
        const controller = new AbortController();
        const timeoutId = setTimeout(() => controller.abort(), this.timeout);
        try {
            const response = await fetch(url, {
                method,
                headers: {
                    'Authorization': `Bearer ${this.apiKey}`,
                    'Content-Type': 'application/json',
                    'Accept': 'application/json',
                },
                body: body ? JSON.stringify(body) : undefined,
                signal: controller.signal,
            });
            clearTimeout(timeoutId);
            if (!response.ok) {
                let errorBody;
                try {
                    errorBody = await response.json();
                }
                catch {
                    // Response body wasn't JSON
                }
                throw new EngramError(errorBody?.error ?? `HTTP ${response.status}`, response.status, errorBody);
            }
            return await response.json();
        }
        catch (error) {
            clearTimeout(timeoutId);
            if (error instanceof EngramError) {
                throw error;
            }
            if (error instanceof Error) {
                if (error.name === 'AbortError') {
                    throw new EngramError('Request timeout', 408);
                }
                throw new EngramError(error.message, 0);
            }
            throw new EngramError('Unknown error', 0);
        }
    }
    // -------------------------------------------------------------------------
    // Memory Operations
    // -------------------------------------------------------------------------
    /**
     * Store a new memory.
     *
     * @param req - The memory to store
     * @returns The stored memory ID and whether it was created or deduplicated
     */
    async store(req) {
        return this.request('POST', '/memories', req);
    }
    /**
     * Get a memory by ID.
     *
     * @param id - The memory ID
     * @returns The memory or null if not found
     */
    async get(id) {
        try {
            return await this.request('GET', `/memories/${id}`);
        }
        catch (error) {
            if (error instanceof EngramError && error.statusCode === 404) {
                return null;
            }
            throw error;
        }
    }
    /**
     * List memories with optional filtering.
     *
     * @param options - Filtering and pagination options
     * @returns Array of memories
     */
    async list(options = {}) {
        const params = new URLSearchParams();
        if (options.limit !== undefined)
            params.set('limit', String(options.limit));
        if (options.offset !== undefined)
            params.set('offset', String(options.offset));
        if (options.category)
            params.set('category', options.category);
        if (options.source)
            params.set('source', options.source);
        if (options.space_id !== undefined)
            params.set('space_id', String(options.space_id));
        if (options.include_forgotten)
            params.set('include_forgotten', 'true');
        if (options.include_archived)
            params.set('include_archived', 'true');
        const queryString = params.toString();
        const path = queryString ? `/memories?${queryString}` : '/memories';
        return this.request('GET', path);
    }
    /**
     * Search memories using hybrid search (vector + full-text).
     *
     * @param req - Search parameters
     * @returns Array of search results with scores
     */
    async search(req) {
        return this.request('POST', '/memories/search', req);
    }
    /**
     * Update an existing memory.
     *
     * @param id - The memory ID to update
     * @param req - Fields to update
     * @returns The updated memory
     */
    async update(id, req) {
        return this.request('PATCH', `/memories/${id}`, req);
    }
    /**
     * Delete a memory.
     *
     * @param id - The memory ID to delete
     */
    async delete(id) {
        await this.request('DELETE', `/memories/${id}`);
    }
    /**
     * Mark a memory as forgotten (soft delete).
     *
     * @param id - The memory ID to forget
     * @param reason - Optional reason for forgetting
     */
    async forget(id, reason) {
        await this.request('POST', `/memories/${id}/forget`, {
            reason,
        });
    }
    // -------------------------------------------------------------------------
    // Context Assembly
    // -------------------------------------------------------------------------
    /**
     * Assemble context from relevant memories for a prompt.
     *
     * @param req - Context assembly parameters
     * @returns Assembled context blocks
     */
    async assembleContext(req) {
        return this.request('POST', '/context', req);
    }
    // -------------------------------------------------------------------------
    // Low-level API Access
    // -------------------------------------------------------------------------
    /**
     * Raw API access for endpoints not covered by high-level methods.
     */
    api = {
        get: (path) => this.request('GET', path),
        post: (path, body) => this.request('POST', path, body),
        patch: (path, body) => this.request('PATCH', path, body),
        delete: (path) => this.request('DELETE', path),
    };
}
//# sourceMappingURL=client.js.map