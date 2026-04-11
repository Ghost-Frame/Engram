/**
 * Example: Store and Search Memories
 *
 * This example demonstrates the basic workflow of storing memories
 * and searching for them using the Engram SDK.
 *
 * Run with:
 *   npx tsx examples/store-and-search.ts
 *
 * Requires:
 *   - ENGRAM_URL (defaults to http://localhost:4200)
 *   - ENGRAM_API_KEY (required)
 */

import { EngramClient, EngramError } from '../src/index.js';

async function main() {
  // Create client from environment
  const client = new EngramClient({
    url: process.env.ENGRAM_URL || 'http://localhost:4200',
    apiKey: process.env.ENGRAM_API_KEY!,
  });

  if (!process.env.ENGRAM_API_KEY) {
    console.error('Error: ENGRAM_API_KEY environment variable required');
    process.exit(1);
  }

  console.log('Connected to Engram server');

  // Store some memories
  console.log('\n--- Storing memories ---');

  const memories = [
    {
      content: 'User prefers dark mode for all applications',
      category: 'preference' as const,
      importance: 7,
      tags: ['preferences', 'ui'],
    },
    {
      content: 'Project deadline is next Friday',
      category: 'task' as const,
      importance: 8,
      tags: ['project', 'deadline'],
    },
    {
      content: 'Decided to use TypeScript for the SDK',
      category: 'decision' as const,
      importance: 6,
      tags: ['sdk', 'technology'],
    },
  ];

  const storedIds: number[] = [];

  for (const mem of memories) {
    try {
      const result = await client.store(mem);
      console.log(`Stored: "${mem.content.substring(0, 40)}..." -> ID ${result.id}`);
      storedIds.push(result.id);
    } catch (error) {
      if (error instanceof EngramError) {
        console.error(`Failed to store: ${error.message}`);
      }
      throw error;
    }
  }

  // Search for memories
  console.log('\n--- Searching memories ---');

  const searchQueries = [
    'user preferences',
    'project deadlines',
    'technology decisions',
  ];

  for (const query of searchQueries) {
    console.log(`\nQuery: "${query}"`);
    const results = await client.search({
      query,
      limit: 5,
      mode: 'hybrid',
    });

    if (results.length === 0) {
      console.log('  No results found');
    } else {
      for (const r of results) {
        console.log(`  [${r.score.toFixed(3)}] ${r.memory.content.substring(0, 50)}...`);
      }
    }
  }

  // Assemble context
  console.log('\n--- Assembling context ---');

  const context = await client.assembleContext({
    query: 'What do I know about the user?',
    strategy: 'semantic',
    max_tokens: 2000,
  });

  console.log(`Total tokens: ${context.total_tokens}`);
  console.log(`Blocks: ${context.blocks.length}`);
  for (const block of context.blocks.slice(0, 3)) {
    console.log(`  - ${block.content.substring(0, 60)}...`);
  }

  // Get a specific memory
  console.log('\n--- Getting specific memory ---');

  if (storedIds.length > 0) {
    const memory = await client.get(storedIds[0]);
    if (memory) {
      console.log(`ID: ${memory.id}`);
      console.log(`Content: ${memory.content}`);
      console.log(`Category: ${memory.category}`);
      console.log(`Created: ${memory.created_at}`);
    }
  }

  // List memories
  console.log('\n--- Listing memories ---');

  const list = await client.list({ limit: 5 });
  console.log(`Found ${list.length} memories`);
  for (const mem of list) {
    console.log(`  [${mem.id}] ${mem.content.substring(0, 50)}...`);
  }

  console.log('\nDone!');
}

main().catch((error) => {
  console.error('Error:', error);
  process.exit(1);
});
