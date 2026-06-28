import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import * as api from '$lib/api/memory';
import { KleosMusic } from './KleosMusic';

afterEach(() => vi.restoreAllMocks());

// Wrap UI in a QueryClientProvider with retries disabled for fast tests.
function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

// With no tracks configured, the player renders nothing.
it('renders nothing when the manifest is empty', async () => {
  vi.spyOn(api, 'getMusicManifest').mockResolvedValue([]);
  const { container } = wrap(<KleosMusic />);
  await waitFor(() => expect(api.getMusicManifest).toHaveBeenCalled());
  expect(container.querySelector('.music-player-fixed')).toBeNull();
});

// With tracks, the player controls render.
it('renders controls when tracks exist', async () => {
  vi.spyOn(api, 'getMusicManifest').mockResolvedValue([{ src: 'a.mp3', name: 'A' }]);
  wrap(<KleosMusic />);
  await waitFor(() => expect(screen.getByLabelText(/play|pause/i)).toBeInTheDocument());
});
