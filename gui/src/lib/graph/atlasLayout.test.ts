import { describe, expect, it } from 'vitest';
import type { GraphEdge, GraphNode } from '$lib/types';
import { buildAtlasLayout, fitAtlasView, hitTestAtlas } from './atlasLayout';

// Create a minimal graph node for deterministic layout tests.
function node(id: string, category = 'general', importance = 5): GraphNode {
  return {
    category,
    content: id,
    created_at: '2026-01-01T00:00:00Z',
    id,
    importance,
    is_static: false,
    label: id,
    size: 1,
    source: 'test'
  };
}

describe('buildAtlasLayout', () => {
  it('returns stable positions and validates edge endpoints', () => {
    const nodes = [node('m1', 'decision'), node('m2', 'decision'), node('m3', 'task')];
    const edges: GraphEdge[] = [
      { source: 'm1', target: 'm2', type: 'association', weight: 0.8 },
      { source: 'm1', target: 'missing', type: 'association', weight: 0.9 }
    ];
    const first = buildAtlasLayout(nodes, edges);
    const second = buildAtlasLayout(nodes, edges);

    expect(first.nodes.map(({ id, x, y }) => ({ id, x, y }))).toEqual(
      second.nodes.map(({ id, x, y }) => ({ id, x, y }))
    );
    expect(first.edges).toHaveLength(1);
    expect(first.neighbors.get('m1')).toEqual(new Set(['m2']));
    expect(first.nodeById.get('m2')?.degree).toBe(1);
  });

  it('separates categories and prioritizes important nodes near group centers', () => {
    const layout = buildAtlasLayout(
      [node('low', 'decision', 1), node('high', 'decision', 10), node('other', 'task', 5)],
      []
    );
    const high = layout.nodeById.get('high')!;
    const low = layout.nodeById.get('low')!;
    const other = layout.nodeById.get('other')!;

    expect(Math.hypot(high.x - low.x, high.y - low.y)).toBeGreaterThan(0);
    expect(Math.hypot(high.x - other.x, high.y - other.y)).toBeGreaterThan(100);
  });
});

describe('atlas viewport helpers', () => {
  it('fits bounds inside the requested viewport and supports hit testing', () => {
    const layout = buildAtlasLayout([node('m1'), node('m2')], []);
    const view = fitAtlasView(layout.bounds, 800, 600);
    const target = layout.nodes[0];
    const hit = hitTestAtlas(
      layout.nodes,
      view,
      target.x * view.scale + view.offsetX,
      target.y * view.scale + view.offsetY
    );

    expect(view.scale).toBeGreaterThan(0);
    expect(hit?.id).toBe(target.id);
  });
});
