/**
 * @engram/sdk - TypeScript SDK for Engram memory server
 *
 * @example
 * ```typescript
 * import { EngramClient } from '@engram/sdk';
 *
 * const engram = new EngramClient({
 *   url: 'http://localhost:4200',
 *   apiKey: process.env.ENGRAM_API_KEY!,
 * });
 *
 * // Store a memory
 * await engram.store({
 *   content: 'User prefers dark mode',
 *   category: 'preference',
 *   importance: 6,
 * });
 *
 * // Search memories
 * const results = await engram.search({
 *   query: 'user preferences',
 *   limit: 10,
 * });
 *
 * // Assemble context
 * const context = await engram.assembleContext({
 *   query: 'What are the user preferences?',
 *   strategy: 'semantic',
 *   max_tokens: 4000,
 * });
 * ```
 */
export { EngramClient } from './client.js';
export { 
// Error
EngramError, } from './types.js';
//# sourceMappingURL=index.js.map