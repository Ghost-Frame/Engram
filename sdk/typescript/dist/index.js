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
export { 
// Error
KleosError, } from './types.js';
//# sourceMappingURL=index.js.map