import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import * as api from '$lib/api/memory';
import { Projects } from './Projects';

afterEach(() => vi.restoreAllMocks());

// Wrap the component under test in the required React Query context.
function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

// Empty projects are hidden until the toggle is clicked.
it('hides empty projects by default', async () => {
  vi.spyOn(api, 'listProjects').mockResolvedValue([
    { id: 1, name: 'Real', status: 'active', memory_count: 4, created_at: '2026-01-01' },
    { id: 2, name: 'Test Project 1773', status: 'active', memory_count: 0, created_at: '2026-01-01' }
  ]);
  wrap(<Projects />);
  await screen.findByText('Real');
  expect(screen.queryByText('Test Project 1773')).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: /show empty/i }));
  await waitFor(() => expect(screen.getByText('Test Project 1773')).toBeInTheDocument());
});
