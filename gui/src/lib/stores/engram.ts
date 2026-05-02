import { writable, derived } from 'svelte/store';

const BASE_URL = typeof window !== 'undefined'
  ? (window.location.port === '4200'
    ? ''
    : '/api')
  : 'http://127.0.0.1:4200';

export const apiKey = writable<string>(
  typeof window !== 'undefined' ? localStorage.getItem('engram_api_key') || '' : ''
);

apiKey.subscribe((v) => {
  if (typeof window !== 'undefined' && v) localStorage.setItem('engram_api_key', v);
});

export const isAuthed = derived(apiKey, ($key) => !!$key);

async function api<T>(path: string, method = 'GET', body?: unknown): Promise<T> {
  let key = '';
  apiKey.subscribe((v) => (key = v))();
  const headers: Record<string, string> = { 'Content-Type': 'application/json' };
  if (key) headers['Authorization'] = `Bearer ${key}`;
  const res = await fetch(`${BASE_URL}${path}`, {
    method,
    headers,
    body: body !== undefined ? JSON.stringify(body) : undefined,
  });
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`${res.status}: ${text}`);
  }
  return res.json() as Promise<T>;
}

export interface Memory {
  id: number;
  content: string;
  category: string;
  source?: string;
  importance: number;
  created_at: string;
  score?: number;
  semantic_score?: number;
  is_static?: boolean;
  source_count?: number;
  version?: number;
  tags?: string[];
  explain?: {
    vector?: number;
    reranker?: number;
    rrf?: number;
    decay?: number;
    static?: boolean;
    corroborated?: number;
    reasons?: string[];
  };
}

export async function search(query: string, mode?: string, limit = 10): Promise<{ results: Memory[]; abstained: boolean }> {
  return api('/search', 'POST', { query, mode, limit });
}

export async function context(query: string, mode?: string): Promise<{ context: string; memories: Memory[] }> {
  return api('/context', 'POST', { query, mode });
}

export async function store(content: string, category: string, importance = 5): Promise<{ id: number }> {
  return api('/store', 'POST', { content, category, importance, source: 'gui' });
}

export async function listMemories(opts: { category?: string; limit?: number } = {}): Promise<Memory[]> {
  const params = new URLSearchParams({ limit: String(opts.limit || 30) });
  if (opts.category) params.set('category', opts.category);
  const result = await api<{ results: Memory[] }>(`/list?${params}`);
  return result.results || [];
}

export async function getInbox(limit = 30): Promise<Memory[]> {
  const result = await api<{ pending: Memory[] }>(`/inbox?limit=${limit}`);
  return result.pending || [];
}

export async function approveMemory(id: number): Promise<void> {
  await api(`/inbox/${id}/approve`, 'POST');
}

export async function rejectMemory(id: number): Promise<void> {
  await api(`/inbox/${id}/reject`, 'POST');
}

export async function deleteMemory(id: number): Promise<void> {
  await api(`/memory/${id}`, 'DELETE');
}

export async function archiveMemory(id: number): Promise<void> {
  await api(`/memory/${id}/archive`, 'POST');
}

export async function getEntities(type?: string): Promise<any[]> {
  const params = type ? `?type=${type}&limit=50` : '?limit=50';
  const result = await api<{ entities: any[] }>(`/entities${params}`);
  return result.entities || [];
}

export async function getProjects(status?: string): Promise<any[]> {
  const params = status ? `?status=${status}&limit=50` : '?limit=50';
  const result = await api<{ projects: any[] }>(`/projects${params}`);
  return result.projects || [];
}

export async function getHealth(): Promise<any> {
  return api('/health');
}

export async function getGraph(depth = 3, max = 5000): Promise<any> {
  return api(`/graph?depth=${depth}&max=${max}`);
}

export async function getCommunities(): Promise<any> {
  return api('/communities');
}

export async function getStats(): Promise<any> {
  return api('/stats');
}

export async function getMemoryDetail(id: number): Promise<any> {
  return api(`/memory/${id}`);
}

export async function searchGraph(query: string, limit = 20): Promise<{ results: Memory[] }> {
  return api('/search', 'POST', { query, limit, expand_relationships: true });
}

export async function guard(action: string): Promise<{ verdict: string; reasons: string[] }> {
  return api('/guard', 'POST', { action });
}
