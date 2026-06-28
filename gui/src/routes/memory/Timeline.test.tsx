import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen, fireEvent, waitFor, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import * as api from '$lib/api/memory';
import type { SearchResult } from '$lib/types';
import { Timeline } from './Timeline';

afterEach(() => vi.restoreAllMocks());

// Wrap the component under test in the required React Query context.
function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

// Clicking a year card drills into that year's months.
it('drills from year into months', async () => {
  vi.spyOn(api, 'getCalendar').mockImplementation(async (g) =>
    g === 'year' ? [{ bucket: '2026', count: 12 }] : [{ bucket: '03', count: 4 }]
  );
  wrap(<Timeline />);
  const year = await screen.findByText('2026');
  fireEvent.click(year);
  await waitFor(() => expect(screen.getByText('Mar')).toBeInTheDocument());
});

// Clicking the year breadcrumb from within a month clears the month crumb
// while keeping the year crumb -- regression guard for breadcrumb reset logic.
it('year breadcrumb clears month crumb without clearing year', async () => {
  vi.spyOn(api, 'getCalendar').mockImplementation(async (g) =>
    g === 'year' ? [{ bucket: '2026', count: 12 }] : [{ bucket: '03', count: 4 }]
  );
  wrap(<Timeline />);

  // Drill: year view -> month overview.
  fireEvent.click(await screen.findByText('2026'));
  // Drill: month overview -> day overview (March is non-empty so click fires).
  await waitFor(() => expect(screen.getByText('Mar')).toBeInTheDocument());
  fireEvent.click(screen.getByText('Mar'));

  // At day overview the breadcrumb now shows "Timeline / 2026 / Mar".
  const nav = screen.getByRole('navigation', { name: 'Timeline position' });
  await waitFor(() => expect(within(nav).getByText('Mar')).toBeInTheDocument());

  // Click the year breadcrumb -- should reset month without resetting year.
  fireEvent.click(within(nav).getByText('2026'));

  // Month crumb is gone; year crumb remains.
  await waitFor(() => expect(within(nav).queryByText('Mar')).not.toBeInTheDocument());
  expect(within(nav).getByText('2026')).toBeInTheDocument();
});

// Build a minimal SearchResult for day-level memory rendering in tests.
function mem(id: number, content: string): SearchResult {
  return {
    id,
    content,
    category: 'general',
    source: 'agent',
    importance: 5,
    created_at: '2026-03-14 10:00:00',
    score: 0,
    tags: [],
    search_type: 'fts'
  };
}

// Mock the calendar so a single drill path (2026 / Mar / 14) reaches a day that
// holds the supplied memories.
function mockDrill(memories: ReturnType<typeof mem>[]) {
  vi.spyOn(api, 'getCalendar').mockImplementation(async (g) =>
    g === 'year'
      ? [{ bucket: '2026', count: memories.length }]
      : g === 'month'
        ? [{ bucket: '03', count: memories.length }]
        : [{ bucket: '14', count: memories.length }]
  );
  vi.spyOn(api, 'listMemoriesByDay').mockResolvedValue(memories);
}

// Drill the rendered timeline down to the seeded day (2026 / Mar / 14).
async function drillToDay() {
  fireEvent.click(await screen.findByText('2026'));
  fireEvent.click(await screen.findByText('Mar'));
  fireEvent.click(await screen.findByText('14'));
}

// The New-memory panel creates a memory via storeMemory.
it('creates a memory from the timeline', async () => {
  vi.spyOn(api, 'getCalendar').mockResolvedValue([]);
  vi.spyOn(api, 'listMemoriesByDay').mockResolvedValue([]);
  const store = vi.spyOn(api, 'storeMemory').mockResolvedValue({ id: 1, duplicate: false });

  wrap(<Timeline />);
  fireEvent.click(await screen.findByRole('button', { name: /new memory/i }));
  fireEvent.change(screen.getByLabelText('Memory content'), { target: { value: 'a brand new fact' } });
  const panel = screen.getByRole('region', { name: 'New memory' });
  fireEvent.click(within(panel).getByRole('button', { name: 'Save' }));

  await waitFor(() => expect(store).toHaveBeenCalled());
  expect(store.mock.calls[0][0]).toMatchObject({ content: 'a brand new fact' });
});

// Editing a day-level card sends the patch to updateMemory keyed by id.
it('edits a memory from the day view', async () => {
  mockDrill([mem(42, 'old body')]);
  const update = vi.spyOn(api, 'updateMemory').mockResolvedValue();

  wrap(<Timeline />);
  await drillToDay();
  fireEvent.click(await screen.findByRole('button', { name: 'Edit' }));
  fireEvent.change(screen.getByLabelText('Memory content'), { target: { value: 'new body' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save' }));

  await waitFor(() => expect(update).toHaveBeenCalled());
  expect(update.mock.calls[0][0]).toBe(42);
  expect(update.mock.calls[0][1]).toMatchObject({ content: 'new body' });
});

// Deleting a card requires a confirm step before deleteMemory fires.
it('deletes a memory after confirm', async () => {
  mockDrill([mem(42, 'doomed memory')]);
  const del = vi.spyOn(api, 'deleteMemory').mockResolvedValue();

  wrap(<Timeline />);
  await drillToDay();
  // First click arms the confirmation; the memory is not yet deleted.
  fireEvent.click(await screen.findByRole('button', { name: 'Delete' }));
  expect(del).not.toHaveBeenCalled();
  // Second Delete (now the confirm button) commits the soft-delete.
  fireEvent.click(await screen.findByRole('button', { name: 'Delete' }));

  await waitFor(() => expect(del).toHaveBeenCalledWith(42));
});
