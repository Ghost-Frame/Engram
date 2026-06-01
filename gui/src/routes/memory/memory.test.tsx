import { fireEvent, render, screen } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { describe, expect, it, vi } from 'vitest';
import { Memory } from './Memory';

const memoryData = vi.hoisted(() => ({
  entities: [
    {
      confidence: 0.9,
      created_at: '',
      description: 'Primary project',
      entity_type: 'project',
      first_seen_at: '',
      id: 1,
      last_seen_at: '2026-05-30 12:00:00',
      name: 'Kleos',
      occurrence_count: 7
    }
  ],
  inbox: [{ content: 'pending memory candidate' }],
  projects: [
    { created_at: '', description: '', id: 1, memory_count: 12, name: 'gui rebuild', status: 'active', updated_at: '' }
  ],
  results: [
    {
      category: 'progress',
      content: 'stored fact about the GUI',
      created_at: '2026-05-30 12:00:00',
      id: 1,
      importance: 4,
      score: 0.82,
      search_type: 'hybrid',
      source: 'codex',
      tags: ['gui']
    }
  ]
}));

vi.mock('@tanstack/react-query', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@tanstack/react-query')>();
  return {
    ...actual,
    useMutation: () => ({ data: memoryData.results, isPending: false, mutate: vi.fn() }),
    useQuery: ({ queryKey }: { queryKey: readonly unknown[] }) => {
      const joined = queryKey.join(':');
      if (joined === 'mem:list') return { data: memoryData.results, isLoading: false };
      if (joined === 'mem:inbox') return { data: memoryData.inbox, isLoading: false };
      if (joined === 'mem:entities') return { data: memoryData.entities, isLoading: false };
      if (joined === 'mem:projects') return { data: memoryData.projects, isLoading: false };
      return { data: undefined, isLoading: false };
    }
  };
});

// The memory graph is a WebGL / 3d-force-graph component that cannot render
// meaningfully in jsdom (no canvas 2D/WebGL context). The Memory hub test only
// needs to confirm the tab mounts, so the heavy graph is stubbed here. The
// graph itself is verified visually against real data.
vi.mock('./Graph', () => ({
  Graph: () => <div data-testid="graph-stub">memory graph</div>
}));

describe('Memory routes', () => {
  // Mount Memory exactly the way the app does -- under /memory/* -- so the
  // absolute tab links (/memory/<tab>) resolve to the nested routes.
  const renderMemory = (path: string) =>
    render(
      <MemoryRouter future={{ v7_relativeSplatPath: true, v7_startTransition: true }} initialEntries={[path]}>
        <Routes>
          <Route path="/memory/*" element={<Memory />} />
        </Routes>
      </MemoryRouter>
    );

  it('renders the memory hub search tab with absolute tab links', () => {
    renderMemory('/memory/search');

    expect(screen.getByRole('link', { name: 'Timeline' })).toHaveAttribute('href', '/memory/timeline');
    expect(screen.getByText('stored fact about the GUI')).toBeInTheDocument();
  });

  it('navigates across timeline, inbox, entities, projects, and graph tabs', () => {
    renderMemory('/memory/timeline');

    expect(screen.getByText('stored fact about the GUI')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('link', { name: 'Inbox' }));
    expect(screen.getByText(/pending memory candidate/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole('link', { name: 'Entities' }));
    expect(screen.getByText('Kleos')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('link', { name: 'Projects' }));
    expect(screen.getByText('gui rebuild')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('link', { name: 'Graph' }));
    expect(screen.getByTestId('graph-stub')).toBeInTheDocument();
  });
});
