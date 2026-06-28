import { buildUrl, request } from '$lib/http';
import type { CalendarBucket, Entity, Health, InboxItem, Project, SearchResult, Track } from '$lib/types';

// Fetch server health for memory overview panels.
export const getHealth = () => request<Health>('/health');

// Fetch global memory statistics.
export const getMemStats = () =>
  request<{ categories: Record<string, number>; entities: number; episodes: number; memories: number }>('/stats');

// Search memories by query text.
export async function search(query: string, limit = 20): Promise<SearchResult[]> {
  return (await request<{ results: SearchResult[] }>('/search', { body: { limit, query }, method: 'POST' })).results ?? [];
}

// Fetch latest memories for timeline-style views.
export async function listMemories(limit = 50): Promise<SearchResult[]> {
  return (await request<{ results: SearchResult[] }>(`/list?limit=${limit}`)).results ?? [];
}

// Fetch pending memory inbox items.
export async function getInbox(limit = 30): Promise<InboxItem[]> {
  return (await request<{ pending: InboxItem[] }>(`/inbox?limit=${limit}`)).pending ?? [];
}

// Fetch extracted memory entities.
export async function listEntities(limit = 50): Promise<Entity[]> {
  return (await request<{ entities: Entity[] }>(`/entities?limit=${limit}`)).entities ?? [];
}

// Fetch memory projects.
export async function listProjects(): Promise<Project[]> {
  return (await request<{ projects: Project[] }>('/projects')).projects ?? [];
}

// Fetch bucketed memory counts for the timeline drill-down.
export async function getCalendar(
  granularity: 'year' | 'month' | 'day',
  year?: number,
  month?: number
): Promise<CalendarBucket[]> {
  const params = new URLSearchParams({ granularity });
  if (year !== undefined) params.set('year', String(year));
  if (month !== undefined) params.set('month', String(month));
  return (await request<{ buckets: CalendarBucket[] }>(`/memories/calendar?${params}`)).buckets ?? [];
}

// Zero-pad a 1-2 digit number to two digits for date strings.
function pad2(n: number): string {
  return String(n).padStart(2, '0');
}

// Fetch all memories created on a specific day (half-open [day, day+1) window).
export async function listMemoriesByDay(
  year: number,
  month: number,
  day: number,
  limit = 500
): Promise<SearchResult[]> {
  const from = `${year}-${pad2(month)}-${pad2(day)}`;
  const next = new Date(Date.UTC(year, month - 1, day + 1));
  const to = `${next.getUTCFullYear()}-${pad2(next.getUTCMonth() + 1)}-${pad2(next.getUTCDate())}`;
  const qs = new URLSearchParams({ from, to, limit: String(limit) });
  return (await request<{ results: SearchResult[] }>(`/list?${qs}`)).results ?? [];
}

// Approve a pending inbox memory.
export async function approveInbox(id: number): Promise<void> {
  await request(`/inbox/${id}/approve`, { method: 'POST' });
}

// Reject a pending inbox memory, optionally recording a reason.
export async function rejectInbox(id: number, reason?: string): Promise<void> {
  await request(`/inbox/${id}/reject`, { body: reason ? { reason } : {}, method: 'POST' });
}

// Edit and approve a pending inbox memory in a single round-trip.
export async function editInbox(id: number, content: string): Promise<void> {
  await request(`/inbox/${id}/edit`, { body: { content }, method: 'POST' });
}

// Fetch the music manifest (empty when no music dir is configured).
export async function getMusicManifest(): Promise<Track[]> {
  try {
    return await request<Track[]>('/media/music/manifest.json');
  } catch {
    return [];
  }
}

// Resolve a same-origin URL for a track filename (honors the dev /api base).
export function musicSrc(file: string): string {
  return buildUrl(`/media/music/${file}`);
}
