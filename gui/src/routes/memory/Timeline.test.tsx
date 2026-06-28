import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen, fireEvent, waitFor, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import * as api from '$lib/api/memory';
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
