import { afterEach, expect, it, vi } from 'vitest';
import { getCalendar, listMemoriesByDay } from './memory';

afterEach(() => vi.restoreAllMocks());

// getCalendar unwraps the buckets array from the calendar endpoint.
it('getCalendar returns buckets', async () => {
  vi.spyOn(globalThis, 'fetch').mockResolvedValue(
    new Response(JSON.stringify({ buckets: [{ bucket: '2026', count: 5 }], granularity: 'year' }), {
      status: 200,
      headers: { 'Content-Type': 'application/json' }
    })
  );
  const buckets = await getCalendar('year');
  expect(buckets).toEqual([{ bucket: '2026', count: 5 }]);
});

// listMemoriesByDay builds an inclusive-from / exclusive-to one-day window.
it('listMemoriesByDay requests a one-day window', async () => {
  const spy = vi.spyOn(globalThis, 'fetch').mockResolvedValue(
    new Response(JSON.stringify({ results: [] }), {
      status: 200,
      headers: { 'Content-Type': 'application/json' }
    })
  );
  await listMemoriesByDay(2026, 3, 14, 100);
  const url = String(spy.mock.calls[0][0]);
  expect(url).toContain('from=2026-03-14');
  expect(url).toContain('to=2026-03-15');
});
