import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { getMemoryGraph } from '$lib/api/graph';
import { Graph } from './Graph';

// Captured ForceGraph instances expose configuration calls for regression assertions.
const graphRuntime = vi.hoisted(() => ({
  instances: [] as Array<{
    calls: Array<{ args: unknown[]; name: string }>;
    data: { links: unknown[]; nodes: unknown[] };
  }>
}));

vi.mock('3d-force-graph', () => {
  // TestForceGraph models the fluent surface used by the 3D memory graph without creating WebGL.
  class TestForceGraph {
    calls: Array<{ args: unknown[]; name: string }> = [];
    data = { links: [] as unknown[], nodes: [] as unknown[] };
    private readonly canvas = document.createElement('canvas');
    private readonly sceneValue = {
      add: vi.fn(),
      remove: vi.fn()
    };

    // Build a proxy that records fluent configuration while preserving explicit runtime methods.
    constructor(_container: HTMLElement) {
      const proxy = new Proxy(this, {
        get: (target, property, receiver) => {
          if (Reflect.has(target, property)) return Reflect.get(target, property, receiver);
          return (...args: unknown[]) => {
            target.calls.push({ args, name: String(property) });
            return receiver;
          };
        }
      });
      graphRuntime.instances.push(proxy);
      return proxy;
    }

    // Store graph data on setter calls and return it on getter calls.
    graphData(value?: { links: unknown[]; nodes: unknown[] }) {
      if (value) {
        this.data = value;
        this.calls.push({ args: [value], name: 'graphData' });
        return this;
      }
      return this.data;
    }

    // Expose the canvas expected by the graph initialization path.
    renderer() {
      return { domElement: this.canvas };
    }

    // Expose a minimal scene that accepts and removes graph objects.
    scene() {
      return this.sceneValue;
    }

    // Return a fluent force stub for lookups and the graph instance for force assignments.
    d3Force(_name: string, value?: unknown) {
      if (arguments.length > 1) {
        this.calls.push({ args: [_name, value], name: 'd3Force' });
        return this;
      }
      const force = new Proxy({}, {
        get: (_target, property, receiver) => (...args: unknown[]) => {
          this.calls.push({ args, name: `force.${String(property)}` });
          return receiver;
        }
      });
      return force;
    }

    // Return stable camera values so fit controls can calculate a position.
    camera() {
      return { aspect: 16 / 9, fov: 50, position: { x: 0, y: 0, z: 1000 } };
    }

    // Record camera movement requested by fit and search controls.
    cameraPosition(...args: unknown[]) {
      this.calls.push({ args, name: 'cameraPosition' });
      return this;
    }

    // Record simulation reheats requested when cluster pinning changes.
    d3ReheatSimulation() {
      this.calls.push({ args: [], name: 'd3ReheatSimulation' });
      return this;
    }

    // Record disposal so tests cover cleanup of the imperative renderer.
    _destructor() {
      this.calls.push({ args: [], name: '_destructor' });
    }
  }

  return { default: TestForceGraph };
});

// Deterministic graph fixture used to exercise the 3D memory graph interface.
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
  getCommunities: vi.fn(async () => ({ communities: [] })),
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
  getStats: vi.fn(async () => ({ db_size_mb: 12.5 })),
  searchGraph: vi.fn(async () => ({
    results: [{ category: 'decision', content: 'Keep the operator surface bounded.', id: 1, score: 0.98 }]
  }))
}));

describe('3D memory graph', () => {
  beforeEach(() => {
    graphRuntime.instances.length = 0;
    const gradient = { addColorStop: vi.fn() };
    const context = {
      arc: vi.fn(),
      beginPath: vi.fn(),
      clearRect: vi.fn(),
      createRadialGradient: vi.fn(() => gradient),
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

  it('restores the direct force graph and its spatial controls', async () => {
    render(<Graph />);

    expect(await screen.findByText('KLEOS')).toBeInTheDocument();
    expect(screen.getByText((_content, element) => element?.textContent === '2 nodes')).toBeInTheDocument();
    expect(screen.getByText((_content, element) => element?.textContent === '1 edges')).toBeInTheDocument();
    expect(screen.getByText('Edge Floor')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Fit View' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Labels' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Clusters' })).toBeInTheDocument();

    await waitFor(() => expect(graphRuntime.instances).toHaveLength(1));
    const instance = graphRuntime.instances[0];
    expect(instance.data.nodes).toHaveLength(2);
    expect(instance.data.links).toHaveLength(1);
    expect(instance.calls).toContainEqual({ args: [150], name: 'warmupTicks' });
    expect(instance.calls).toContainEqual({ args: [400], name: 'cooldownTicks' });
  });

  it('returns loaded search results without replacing the spatial controls', async () => {
    render(<Graph />);
    await screen.findByText('KLEOS');

    const searchInput = screen.getByPlaceholderText('Search memories...');
    fireEvent.change(searchInput, { target: { value: 'bounded' } });
    fireEvent.submit(searchInput.closest('form')!);

    expect(await screen.findByRole('heading', { name: 'Search Results' })).toBeInTheDocument();
    expect(screen.getByText('Keep the operator surface bounded.')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Fit View' })).toBeInTheDocument();
  });

  it('uses the point-cloud path and bounded edge budget for large graphs', async () => {
    const largeNodes = Array.from({ length: 2501 }, (_, index) => ({
      category: index % 2 === 0 ? 'decision' : 'task',
      content: `Memory ${index + 1}`,
      created_at: '2026-07-25T12:00:00Z',
      id: `m${index + 1}`,
      importance: 5,
      is_static: false,
      label: `Memory ${index + 1}`,
      size: 2,
      source: 'test'
    }));
    const largeEdges = Array.from({ length: 15000 }, (_, index) => ({
      source: `m${(index % 2501) + 1}`,
      target: `m${((index * 17 + 1) % 2501) + 1}`,
      type: 'association' as const,
      weight: 0.5 + (index % 50) / 100
    }));
    vi.mocked(getMemoryGraph).mockResolvedValueOnce({
      edge_count: largeEdges.length,
      edges: largeEdges,
      node_count: largeNodes.length,
      nodes: largeNodes
    });

    render(<Graph />);

    await screen.findByText((_content, element) => element?.textContent === '2501 nodes');
    const instance = graphRuntime.instances[0];
    expect(instance.data.nodes).toHaveLength(2501);
    expect(instance.data.links).toHaveLength(14000);
    expect(instance.calls).toContainEqual({ args: [0], name: 'warmupTicks' });
    expect(instance.calls).toContainEqual({ args: [45], name: 'cooldownTicks' });
  });
});
