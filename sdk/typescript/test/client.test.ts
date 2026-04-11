import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { EngramClient, EngramError } from '../src/index.js';

describe('EngramClient', () => {
  const mockFetch = vi.fn();
  const originalFetch = global.fetch;

  beforeEach(() => {
    global.fetch = mockFetch;
    mockFetch.mockReset();
  });

  afterEach(() => {
    global.fetch = originalFetch;
  });

  const client = new EngramClient({
    url: 'http://localhost:4200',
    apiKey: 'test-api-key',
  });

  describe('store', () => {
    it('should store a memory', async () => {
      mockFetch.mockResolvedValueOnce({
        ok: true,
        json: async () => ({ id: 123, created: true }),
      });

      const result = await client.store({
        content: 'Test memory',
        category: 'general',
        importance: 5,
      });

      expect(result).toEqual({ id: 123, created: true });
      expect(mockFetch).toHaveBeenCalledWith(
        'http://localhost:4200/memories',
        expect.objectContaining({
          method: 'POST',
          headers: expect.objectContaining({
            Authorization: 'Bearer test-api-key',
          }),
        })
      );
    });

    it('should include tags in the request', async () => {
      mockFetch.mockResolvedValueOnce({
        ok: true,
        json: async () => ({ id: 124, created: true }),
      });

      await client.store({
        content: 'Tagged memory',
        tags: ['important', 'project'],
      });

      const callBody = JSON.parse(mockFetch.mock.calls[0][1].body);
      expect(callBody.tags).toEqual(['important', 'project']);
    });
  });

  describe('search', () => {
    it('should search memories', async () => {
      const mockResults = [
        {
          memory: { id: 1, content: 'Test', category: 'general' },
          score: 0.95,
          search_type: 'hybrid',
        },
      ];
      mockFetch.mockResolvedValueOnce({
        ok: true,
        json: async () => mockResults,
      });

      const results = await client.search({
        query: 'test query',
        limit: 10,
      });

      expect(results).toEqual(mockResults);
      expect(mockFetch).toHaveBeenCalledWith(
        'http://localhost:4200/memories/search',
        expect.objectContaining({
          method: 'POST',
        })
      );
    });

    it('should include search options', async () => {
      mockFetch.mockResolvedValueOnce({
        ok: true,
        json: async () => [],
      });

      await client.search({
        query: 'test',
        mode: 'hybrid',
        question_type: 'fact_recall',
        expand_relationships: true,
      });

      const callBody = JSON.parse(mockFetch.mock.calls[0][1].body);
      expect(callBody.mode).toBe('hybrid');
      expect(callBody.question_type).toBe('fact_recall');
      expect(callBody.expand_relationships).toBe(true);
    });
  });

  describe('get', () => {
    it('should get a memory by ID', async () => {
      const mockMemory = { id: 123, content: 'Test', category: 'general' };
      mockFetch.mockResolvedValueOnce({
        ok: true,
        json: async () => mockMemory,
      });

      const result = await client.get(123);

      expect(result).toEqual(mockMemory);
      expect(mockFetch).toHaveBeenCalledWith(
        'http://localhost:4200/memories/123',
        expect.objectContaining({ method: 'GET' })
      );
    });

    it('should return null for not found', async () => {
      mockFetch.mockResolvedValueOnce({
        ok: false,
        status: 404,
        json: async () => ({ error: 'not found' }),
      });

      const result = await client.get(999);

      expect(result).toBeNull();
    });
  });

  describe('list', () => {
    it('should list memories with default options', async () => {
      mockFetch.mockResolvedValueOnce({
        ok: true,
        json: async () => [],
      });

      await client.list();

      expect(mockFetch).toHaveBeenCalledWith(
        'http://localhost:4200/memories',
        expect.any(Object)
      );
    });

    it('should include query parameters', async () => {
      mockFetch.mockResolvedValueOnce({
        ok: true,
        json: async () => [],
      });

      await client.list({
        limit: 20,
        offset: 10,
        category: 'task',
        include_forgotten: true,
      });

      const url = mockFetch.mock.calls[0][0];
      expect(url).toContain('limit=20');
      expect(url).toContain('offset=10');
      expect(url).toContain('category=task');
      expect(url).toContain('include_forgotten=true');
    });
  });

  describe('assembleContext', () => {
    it('should assemble context', async () => {
      const mockContext = {
        blocks: [{ content: 'Context', source: 'memory', memory_id: 1 }],
        total_tokens: 100,
        strategy: 'semantic',
        mode: 'default',
      };
      mockFetch.mockResolvedValueOnce({
        ok: true,
        json: async () => mockContext,
      });

      const result = await client.assembleContext({
        query: 'What is X?',
        strategy: 'semantic',
        max_tokens: 4000,
      });

      expect(result).toEqual(mockContext);
      expect(mockFetch).toHaveBeenCalledWith(
        'http://localhost:4200/context',
        expect.objectContaining({ method: 'POST' })
      );
    });
  });

  describe('error handling', () => {
    it('should throw EngramError on API error', async () => {
      mockFetch.mockResolvedValueOnce({
        ok: false,
        status: 400,
        json: async () => ({ error: 'Invalid input' }),
      });

      await expect(client.store({ content: '' })).rejects.toThrow(EngramError);
    });

    it('should include status code in error', async () => {
      mockFetch.mockResolvedValueOnce({
        ok: false,
        status: 401,
        json: async () => ({ error: 'Unauthorized' }),
      });

      try {
        await client.search({ query: 'test' });
        expect.fail('Should have thrown');
      } catch (error) {
        expect(error).toBeInstanceOf(EngramError);
        expect((error as EngramError).statusCode).toBe(401);
        expect((error as EngramError).message).toBe('Unauthorized');
      }
    });

    it('should handle network errors', async () => {
      mockFetch.mockRejectedValueOnce(new Error('Network error'));

      await expect(client.get(1)).rejects.toThrow(EngramError);
    });
  });

  describe('raw API access', () => {
    it('should allow raw GET requests', async () => {
      mockFetch.mockResolvedValueOnce({
        ok: true,
        json: async () => ({ status: 'healthy' }),
      });

      const result = await client.api.get<{ status: string }>('/health');

      expect(result).toEqual({ status: 'healthy' });
    });

    it('should allow raw POST requests', async () => {
      mockFetch.mockResolvedValueOnce({
        ok: true,
        json: async () => ({ ok: true }),
      });

      await client.api.post('/custom/endpoint', { data: 'value' });

      expect(mockFetch).toHaveBeenCalledWith(
        'http://localhost:4200/custom/endpoint',
        expect.objectContaining({ method: 'POST' })
      );
    });
  });

  describe('URL handling', () => {
    it('should handle URLs with trailing slash', async () => {
      const clientWithSlash = new EngramClient({
        url: 'http://localhost:4200/',
        apiKey: 'test',
      });

      mockFetch.mockResolvedValueOnce({
        ok: true,
        json: async () => [],
      });

      await clientWithSlash.list();

      expect(mockFetch.mock.calls[0][0]).toBe('http://localhost:4200/memories');
    });
  });
});
