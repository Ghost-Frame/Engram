import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import App from './App';
import type { ReactNode } from 'react';

const liveStats = vi.hoisted(() => ({
  axon: { by_channel: [], channels: 5, sources: 3, total_events: 88 },
  broca: { agents: 7, by_action: [], by_agent: [], by_service: [], services: 6, total_actions: 120 },
  chiasm: { by_status: { active: 3, completed: 39 }, total: 42 },
  loom: { active_runs: 2, runs: 9, runs_by_status: [], steps: 30, workflows: 4 },
  soma: { by_status: [], by_type: [], online_agents: 5, total_agents: 8, types: 3 },
  thymus: { agent_count: 4, by_rubric: [], evaluations: 11, metrics: 2, rubrics: 6 }
}));

// Task fixture used by the decision-oriented Mission Control queue.
const activeTask = {
  agent: 'codex',
  assigned: true,
  created_at: '2026-07-25T12:00:00Z',
  guardrail_retries: 0,
  heartbeat_interval: 30,
  id: 17,
  last_heartbeat: '2026-07-25T12:01:00Z',
  project: 'Kleos',
  status: 'active',
  title: 'Rebuild operator GUI',
  updated_at: '2026-07-25T12:01:00Z',
  user_id: 1
};

// AppShell resolves the caller's scopes via getMe; stub it as a non-admin so
// no real request is made and the admin nav stays hidden.
vi.mock('$lib/api/admin', () => ({
  getMe: () => Promise.resolve({ is_admin: false, scopes: ['read'], user_id: 1, username: 'root' })
}));

// Provide a real QueryClient (AppShell now uses useQuery) without the live SSE
// stream the real RealtimeProvider would open in jsdom.
vi.mock('$lib/realtime', async () => {
  const rq = await import('@tanstack/react-query');
  const client = new rq.QueryClient({ defaultOptions: { queries: { retry: false } } });
  return {
    RealtimeProvider: ({ children }: { children: ReactNode }) => (
      <rq.QueryClientProvider client={client}>{children}</rq.QueryClientProvider>
    ),
    useLive: (key: unknown, _fetcher: unknown, channel: keyof typeof liveStats) => {
      const parts = key as string[];
      const data = parts[1] === 'tasks'
        ? [activeTask]
        : parts[1] === 'actions'
          ? []
          : parts[1] === 'agents'
            ? []
            : liveStats[channel];
      return { data, isError: false, isLoading: false };
    },
    useStreamStatus: () => 'live'
  };
});

describe('App shell', () => {
  beforeEach(() => {
    vi.unstubAllGlobals();
    localStorage.clear();
    window.history.pushState({}, '', '/');
  });

  it('renders mission control with intent-based navigation and live stats', async () => {
    render(<App />);

    expect(screen.getByRole('heading', { name: 'Mission Control' })).toBeInTheDocument();
    expect(screen.getByRole('link', { name: /Work Coordinated tasks/ })).toHaveAttribute('href', '/chiasm');
    expect(screen.getByRole('link', { name: /Stream Actions and events/ })).toHaveAttribute('href', '/stream');
    expect(screen.getByRole('link', { name: /Memory Recall and curation/ })).toHaveAttribute('href', '/memory');
    expect(screen.getAllByText('live').length).toBeGreaterThan(0);
    expect(screen.getByText('Rebuild operator GUI')).toBeInTheDocument();
    expect(screen.getByText('120')).toBeInTheDocument();
    expect(screen.getByText('88')).toBeInTheDocument();
  });

  it('points the Graph nav at the real similarity graph under Memory', () => {
    // Top-level /graph collides with the server's API-reserved /graph path, so
    // the nav links to the working graph under Memory instead.
    render(<App />);

    expect(screen.getByRole('link', { name: /Graph Relationship atlas/ })).toHaveAttribute('href', '/memory/graph');
  });

  it('logs in via the cookie endpoint and never persists the raw key', async () => {
    const fetchSpy = vi.fn(
      async () =>
        new Response('{"ok":true,"user_id":1}', {
          headers: { 'content-type': 'application/json' },
          status: 200
        })
    );
    vi.stubGlobal('fetch', fetchSpy);

    render(<App />);

    fireEvent.change(screen.getByLabelText('API key'), { target: { value: 'abc123' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));

    // The key is exchanged for a cookie session, not written to localStorage.
    await waitFor(() => {
      const calls = fetchSpy.mock.calls as unknown as Array<[RequestInfo | URL, RequestInit]>;
      const loginCall = calls.find((c) => String(c[0]).includes('/gui/auth'));
      expect(loginCall).toBeDefined();
      expect(String(loginCall![1].body)).toContain('api_key=abc123');
    });
    expect(localStorage.getItem('kleos_api_key')).toBeNull();
    await waitFor(() =>
      expect(screen.queryByRole('dialog', { name: 'API Key' })).not.toBeInTheDocument()
    );
  });

  it('keeps a required login open with the attempted key and an actionable error', async () => {
    const fetchSpy = vi.fn(
      async () =>
        new Response('invalid api key', {
          headers: { 'content-type': 'text/plain' },
          status: 401
        })
    );
    vi.stubGlobal('fetch', fetchSpy);

    render(<App />);

    const input = screen.getByLabelText('API key');
    fireEvent.change(input, { target: { value: 'wrong-key' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('That API key was rejected.');
    expect(input).toHaveValue('wrong-key');
    expect(screen.queryByRole('button', { name: 'Cancel' })).not.toBeInTheDocument();
    expect(screen.getByRole('dialog', { name: 'API Key' })).toBeInTheDocument();
    expect(localStorage.getItem('kleos_api_key')).toBeNull();
  });
});
