import { request } from '$lib/http';
import type { GraphData } from '$lib/types';

// Fetch memory graph data with a node cap (legacy simple form, kept for callers
// that only need a flat node list).
export const getGraph = (max = 1500) => request<GraphData>(`/graph?max=${max}`);

// Fetch the memory graph with relationship depth -- this is the form the
// living-organism 3D graph uses. `depth` controls how far edge traversal walks
// from seed nodes; without it the backend collapses most edges, which is why
// the rebuilt graph rendered 0 edges. Mirrors the old GUI's getGraph(3, 1500).
//
// `minComponent` asks the backend to drop connected components smaller than N
// nodes. The default of 2 prunes singleton "dust" -- unlinked memories (the bulk
// of which are `session` auto-captures) that otherwise scatter across the view
// as disconnected points and bury the real semantic clusters. Pass 1 to keep
// every node, including isolated ones.
export const getMemoryGraph = (depth = 3, max = 1500, minComponent = 2) =>
  request<GraphData>(`/graph?depth=${depth}&max=${max}&min_component=${minComponent}`);

// A detected memory community (cluster). `top_memories` lists representative
// memory ids whose nodes inherit the community color/clustering force.
export interface Community {
  id: number;
  top_memories?: number[];
}

// Response shape of GET /communities.
export interface CommunitiesResponse {
  communities: Community[];
  count: number;
}

// Fetch community/cluster assignments used to color nodes and seed the
// Fibonacci-sphere clustering forces.
export const getCommunities = () =>
  request<CommunitiesResponse>('/communities').catch(
    () => ({ communities: [], count: 0 }) as CommunitiesResponse
  );

// A category tally surfaced in the bottom-right legend.
export interface CategoryCount {
  category: string;
  count: number;
}

// Response shape of GET /stats (only the fields the graph reads).
export interface GraphStats {
  db_size_mb?: number;
  memories?: {
    categories?: CategoryCount[];
  };
}

// Fetch instance stats (db size + per-category counts) for the header/legend.
export const getStats = () => request<GraphStats>('/stats').catch(() => null);

// A single linked-memory entry shown in the detail panel.
export interface MemoryLink {
  id: number;
  similarity: number;
  type: string;
  content: string;
  category: string;
}

// Full memory record returned by GET /memory/:id and rendered in the side panel.
export interface MemoryDetail {
  id: number;
  content: string;
  category: string;
  source: string;
  importance: number;
  version: number;
  is_latest: boolean;
  is_static: boolean;
  created_at: string;
  updated_at: string;
  tags: string[];
  decay_score: number;
  access_count: number;
  last_accessed_at: string;
  episode?: { id: number; title: string };
  links?: MemoryLink[];
  version_chain?: Array<{ id: number; content: string; version: number; is_latest: boolean }>;
}

// Fetch one memory's full detail for the click-through side panel.
export const getMemoryDetail = (id: number) => request<MemoryDetail>(`/memory/${id}`);

// A search hit used to highlight + zoom nodes from the search box.
export interface GraphSearchResult {
  id: number;
  content: string;
  category: string;
  score?: number;
}

// Relationship-expanding search over the graph (POST /search). Mirrors the old
// GUI's searchGraph(query, limit) with expand_relationships enabled.
export const searchGraph = (query: string, limit = 20) =>
  request<{ results: GraphSearchResult[] }>('/search', {
    method: 'POST',
    body: { query, limit, expand_relationships: true }
  });
