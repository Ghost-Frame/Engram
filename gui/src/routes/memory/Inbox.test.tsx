import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import * as api from '$lib/api/memory';
import { Inbox } from './Inbox';

afterEach(() => vi.restoreAllMocks());

// Wrap the component under test in the required React Query context.
function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

// Approving a pending item calls the approve endpoint.
it('approves a pending item', async () => {
  vi.spyOn(api, 'getInbox').mockResolvedValue([
    { id: 7, content: 'pending one', category: 'general', source: 'agent', created_at: '2026-03-14 10:00:00' }
  ]);
  const approve = vi.spyOn(api, 'approveInbox').mockResolvedValue();
  wrap(<Inbox />);
  const btn = await screen.findByRole('button', { name: /approve/i });
  fireEvent.click(btn);
  await waitFor(() => expect(approve).toHaveBeenCalledWith(7));
});
