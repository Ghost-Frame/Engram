import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { ReactNode } from 'react';
import { Graph } from './Graph';

// Deterministic graph fixture used to exercise the Atlas interface.
const graphFixture = {
  edge_count: 1,
  edges: [{ source: 'm1', target: 'm2', type: 'association' as const, weight: 0.84 }],
  node_count: 2,
  nodes: [
    {
      category: 'decision',
      content: 'Keep the operator surface bounded.',
      created_at: '2026-07-25T12:00:00Z',
      id: 'm1',
      importance: 9,
      is_static: true,
      label: 'Bound the atlas',
      size: 4,
      source: 'test'
    },
    {
      category: 'task',
      content: 'Replace the perpetual renderer.',
      created_at: '2026-07-25T12:01:00Z',
      id: 'm2',
      importance: 8,
      is_static: false,
      label: 'Replace renderer',
      size: 3,
      source: 'test'
    }
  ]
};

vi.mock('$lib/api/graph', () => ({
  getMemoryDetail: vi.fn(async (id: number) => ({
    access_count: 0,
    category: id === 1 ? 'decision' : 'task',
    content: id === 1 ? 'Keep the operator surface bounded.' : 'Replace the perpetual renderer.',
    created_at: '2026-07-25T12:00:00Z',
    decay_score: 1,
    id,
    importance: 9,
    is_latest: true,
    is_static: false,
    last_accessed_at: '',
    links: [],
    source: 'test',
    tags: [],
    updated_at: '2026-07-25T12:00:00Z',
    version: 1
  })),
  getMemoryGraph: vi.fn(async () => graphFixture),
  searchGraph: vi.fn(async () => ({
    results: [{ category: 'decision', content: 'Keep the operator surface bounded.', id: 1, score: 0.98 }]
  }))
}));

// ResizeObserver stub keeps the component contract without a browser layout engine.
class TestResizeObserver {
  // Accept the production callback even though the test uses initial dimensions.
  constructor(_callback: ResizeObserverCallback) {}

  // Observation is inert because jsdom has no layout box.
  observe() {}

  // Disconnect is inert because the observer never registered resources.
  disconnect() {}
}

// Render children under the query provider required by the graph route.
function QueryHarness({ children }: { children: ReactNode }) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
}

describe('Memory Atlas', () => {
  beforeEach(() => {
    vi.stubGlobal('ResizeObserver', TestResizeObserver);
    const context = {
      arc: vi.fn(),
      beginPath: vi.fn(),
      clearRect: vi.fn(),
      fill: vi.fn(),
      fillRect: vi.fn(),
      fillText: vi.fn(),
      lineTo: vi.fn(),
      moveTo: vi.fn(),
      setTransform: vi.fn(),
      stroke: vi.fn()
    };
    HTMLCanvasElement.prototype.getContext = vi.fn(
      () => context as unknown as CanvasRenderingContext2D
    ) as unknown as typeof HTMLCanvasElement.prototype.getContext;
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('reports bounded connectivity and exposes the essential navigation controls', async () => {
    render(<Graph />, { wrapper: QueryHarness });

    expect(screen.getByRole('heading', { name: 'Memory Atlas' })).toBeInTheDocument();
    const connectivityReport = await screen.findByLabelText('Graph connectivity report');
    await waitFor(() => expect(connectivityReport).toHaveTextContent('2 loaded nodes'));
    expect(connectivityReport).toHaveTextContent('1 loaded links');
    expect(screen.getByRole('button', { name: 'Fit all nodes' })).toBeInTheDocument();
    expect(screen.getByRole('combobox', { name: 'Density' })).toHaveValue('800');
    expect(screen.getByRole('checkbox', { name: 'Show relationships' })).toBeChecked();
  });

  it('returns loaded search results to the inspector without changing density', async () => {
    render(<Graph />, { wrapper: QueryHarness });
    await screen.findByLabelText('Graph connectivity report');

    fireEvent.change(screen.getByLabelText('Find a memory'), { target: { value: 'bounded' } });
    fireEvent.click(screen.getByRole('button', { name: 'Search' }));

    expect(await screen.findByText('Open in atlas')).toBeInTheDocument();
    expect(screen.getByRole('combobox', { name: 'Density' })).toHaveValue('800');
  });
});
