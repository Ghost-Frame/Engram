/**
 * @kleos/sdk - TypeScript SDK for Kleos memory server
 *
 * @example
 * ```typescript
 * import { KleosClient } from '@kleos/sdk';
 *
 * const kleos = new KleosClient({
 *   url: 'http://localhost:4200',
 *   apiKey: process.env.KLEOS_API_KEY!,
 * });
 *
 * // Store a memory
 * await kleos.store({
 *   content: 'User prefers dark mode',
 *   category: 'preference',
 *   importance: 6,
 * });
 *
 * // Search memories
 * const results = await kleos.search({
 *   query: 'user preferences',
 *   limit: 10,
 * });
 *
 * // Assemble context
 * const context = await kleos.assembleContext({
 *   query: 'What are the user preferences?',
 *   strategy: 'semantic',
 *   max_tokens: 4000,
 * });
 * ```
 */
export { KleosClient } from './client.js';
export { KleosError, type Memory, type MemoryCategory, type MemoryStatus, type QuestionType, type SearchMode, type StoreRequest, type SearchRequest, type ListOptions, type UpdateRequest, type ContextRequest, type StoreResult, type SearchResult, type LinkedMemory, type VersionChainEntry, type ContextBlock, type ContextResult, type KleosClientConfig, type ApiError, type ContextStrategy, type ContextMode, } from './types.js';
//# sourceMappingURL=index.d.ts.map