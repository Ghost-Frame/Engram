// Regression coverage for deterministic, lossless semantic-region aggregation.

import { describe, expect, it } from 'vitest';
import type { GalaxyTarget } from './galaxyLayout';
import {
  buildSemanticRegions,
  selectSemanticRegionEdges,
  // SemanticRegionNode describes one deterministic fixture memory.
  type SemanticRegionNode
} from './semanticRegions';

// Build one complete target fixture with explicit component and layout metadata.
function target(
  overrides: Partial<GalaxyTarget> & Pick<GalaxyTarget, 'groupKey' | 'component'>
): GalaxyTarget {
  return {
    x: 0,
    y: 0,
    z: 0,
    clusterX: 0,
    clusterY: 0,
    clusterZ: 0,
    arm: 0,
    diffuse: false,
    componentSize: 1,
    inMainComponent: overrides.component === 0,
    depth: 0,
    ...overrides
  };
}

describe('buildSemanticRegions', () => {
  it('retains every memory in exactly one overview region', () => {
    const nodes: SemanticRegionNode[] = [
      { id: 'm1', category: 'decision', importance: 8 },
      { id: 'm2', category: 'decision', importance: 6 },
      { id: 'm3', category: 'task', importance: 4 }
    ];
    const targets = new Map<string, GalaxyTarget>([
      ['m1', target({ groupKey: 'community:7', component: 0, x: 10 })],
      ['m2', target({ groupKey: 'community:7', component: 0, x: 20 })],
      ['m3', target({ groupKey: 'category:task', component: 0, x: 80 })]
    ]);

    const atlas = buildSemanticRegions(nodes, [], targets);

    expect(atlas.nodeRegion.size).toBe(nodes.length);
    expect(atlas.regions.flatMap((region) => region.memberIds).sort()).toEqual(
      nodes.map((node) => node.id).sort()
    );
    expect(atlas.regions.reduce((total, region) => total + region.memberCount, 0)).toBe(3);
  });

  it('keeps disconnected components separate even when their semantic labels match', () => {
    const nodes: SemanticRegionNode[] = [
      { id: 'main', category: 'session' },
      { id: 'island', category: 'session' }
    ];
    const targets = new Map<string, GalaxyTarget>([
      ['main', target({ groupKey: 'category:session', component: 0 })],
      ['island', target({ groupKey: 'category:session', component: 4 })]
    ]);

    const atlas = buildSemanticRegions(nodes, [], targets);

    expect(atlas.regions).toHaveLength(2);
    expect(atlas.nodeRegion.get('main')).not.toBe(atlas.nodeRegion.get('island'));
    expect(atlas.regions.find((region) => region.component === 4)?.inMainComponent).toBe(false);
  });

  it('aggregates only stored cross-region links and counts internal links separately', () => {
    const nodes: SemanticRegionNode[] = [
      { id: 'a1', category: 'decision' },
      { id: 'a2', category: 'decision' },
      { id: 'b1', category: 'task' }
    ];
    const targets = new Map<string, GalaxyTarget>([
      ['a1', target({ groupKey: 'category:decision', component: 0 })],
      ['a2', target({ groupKey: 'category:decision', component: 0 })],
      ['b1', target({ groupKey: 'category:task', component: 0, x: 40 })]
    ]);
    const atlas = buildSemanticRegions(nodes, [
      { source: 'a1', target: 'a2', weight: 0.7 },
      { source: 'a1', target: 'b1', weight: 0.8 },
      { source: { id: 'a2' }, target: { id: 'b1' }, weight: 0.6 }
    ], targets);

    expect(atlas.edges).toHaveLength(1);
    expect(atlas.edges[0]).toMatchObject({ count: 2, maxWeight: 0.8, weight: 0.7 });
    expect(atlas.regions.find((region) => region.category === 'decision')?.internalEdgeCount).toBe(1);
  });

  it('splits the main component into bounded radial arm and depth sectors', () => {
    const nodes: SemanticRegionNode[] = Array.from({ length: 4 }, (_, index) => ({
      id: `dust-${index}`,
      category: 'session'
    }));
    const targets = new Map<string, GalaxyTarget>([
      ['dust-0', target({ groupKey: 'category:session', component: 0, diffuse: true, arm: 0, x: 10 })],
      ['dust-1', target({ groupKey: 'category:session', component: 0, diffuse: true, arm: 0, x: 90 })],
      ['dust-2', target({ groupKey: 'category:session', component: 0, diffuse: true, arm: 1, x: 90 })],
      ['dust-3', target({ groupKey: 'category:session', component: 0, diffuse: true, arm: 1, x: 180 })]
    ]);

    const atlas = buildSemanticRegions(nodes, [], targets);

    expect(atlas.regions.length).toBeGreaterThan(1);
    expect(atlas.regions.length).toBeLessThanOrEqual(32);
    expect(atlas.regions.reduce((total, region) => total + region.memberCount, 0)).toBe(4);
  });

  it('coalesces compact semantic groups that occupy the same spatial sector', () => {
    const nodes: SemanticRegionNode[] = [
      { id: 'decision', category: 'decision' },
      { id: 'task', category: 'task' }
    ];
    const targets = new Map<string, GalaxyTarget>([
      ['decision', target({ groupKey: 'community:7', component: 0, arm: 2, x: 50, z: -4 })],
      ['task', target({ groupKey: 'category:task', component: 0, arm: 2, x: 52, z: -2 })]
    ]);

    const atlas = buildSemanticRegions(nodes, [], targets);

    expect(atlas.regions).toHaveLength(1);
    expect(atlas.regions[0].memberCount).toBe(2);
  });

  it('is deterministic across node, link, and endpoint order', () => {
    const nodes: SemanticRegionNode[] = [
      { id: 'a', category: 'decision' },
      { id: 'b', category: 'task' }
    ];
    const targets = new Map<string, GalaxyTarget>([
      ['a', target({ groupKey: 'category:decision', component: 0, x: -20 })],
      ['b', target({ groupKey: 'category:task', component: 0, arm: 1, x: 20 })]
    ]);
    const forward = buildSemanticRegions(
      nodes,
      [{ source: 'a', target: 'b', weight: 0.9 }],
      targets
    );
    const reverse = buildSemanticRegions(
      [...nodes].reverse(),
      [{ source: { id: 'b' }, target: { id: 'a' }, weight: 0.9 }],
      targets
    );

    expect(reverse.regions).toEqual(forward.regions);
    expect(reverse.edges).toEqual(forward.edges);
    expect([...reverse.nodeRegion]).toEqual([...forward.nodeRegion]);
  });

  it('keeps a real connected backbone while bounding overview edge density', () => {
    const nodes: SemanticRegionNode[] = Array.from({ length: 8 }, (_, index) => ({
      id: `node-${index}`,
      category: `category-${index}`
    }));
    const targets = new Map<string, GalaxyTarget>(
      nodes.map((node, index) => [
        node.id,
        target({
          groupKey: `category:category-${index}`,
          component: 0,
          arm: index % 4,
          x: 40,
          z: index < 4 ? -10 : 10
        })
      ])
    );
    const links = nodes.flatMap((source, sourceIndex) =>
      nodes.slice(sourceIndex + 1).map((destination, offset) => ({
        source: source.id,
        target: destination.id,
        weight: 1 - (sourceIndex + offset) / 100
      }))
    );
    const atlas = buildSemanticRegions(nodes, links, targets);
    const selected = selectSemanticRegionEdges(atlas, 10);
    const incident = new Set(selected.flatMap((edge) => [edge.source, edge.target]));

    expect(selected).toHaveLength(10);
    expect(incident.size).toBe(atlas.regions.length);
    expect(selected.every((edge) => atlas.edges.includes(edge))).toBe(true);
  });
});
