import { request } from '$lib/http';
import type { GraphData } from '$lib/types';

// Fetch memory graph data with a node cap (legacy simple form, kept for callers
// that only need a flat node list).
export const getGraph = (max = 1500) => request<GraphData>(`/graph?max=${max}`);

// Fetch a bounded memory graph without server-side depth truncation.
//
// The backend's `depth` option keeps only a neighborhood around one seed in each
// component. That is useful for targeted traversal, but it amputates the paths
// that make the full force layout coherent. `max` bounds browser work while
// preserving the real connectivity among the highest-ranked memories.
//
// `minComponent` asks the backend to drop connected components smaller than N
// nodes. The default of 2 prunes singleton "dust" without inventing links.
export const getMemoryGraph = (max = 1500, minComponent = 2) =>
  request<GraphData>(`/graph?max=${max}&min_component=${minComponent}`);

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

// Fetch community assignments used to group related nodes.
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

// Fetch instance stats with database size and per-category counts.
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
