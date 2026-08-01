import { describe, expect, it } from 'vitest';
import { buildGalaxyTargets, seedGalaxyPositions } from './galaxyLayout';

// Describe link fixtures accepted by the topology-aware galaxy target builder.
import type { GalaxyLayoutLink } from './galaxyLayout';

// Describe node fixtures accepted by the galaxy layout helpers.
import type { GalaxyLayoutNode } from './galaxyLayout';

// Return one compact fixture spanning community and category fallback groups.
function fixtureNodes(): GalaxyLayoutNode[] {
  return [
    { id: 'm1', category: 'decision', community_id: 7, importance: 10 },
    { id: 'm2', category: 'decision', community_id: 7, importance: 6 },
    { id: 'm3', category: 'decision', community_id: 7, importance: 2 },
    { id: 'm4', category: 'incident', community_id: 11, importance: 8 },
    { id: 'm5', category: 'incident', community_id: 11, importance: 4 },
    { id: 'm6', category: 'general', importance: 5 },
    { id: 'm7', category: 'general', importance: 3 }
  ];
}

// Calculate the distance between two three-dimensional guide positions.
function distance(a: { x: number; y: number; z: number }, b: { x: number; y: number; z: number }): number {
  return Math.hypot(a.x - b.x, a.y - b.y, a.z - b.z);
}

// Return a production-shaped graph with one oversized diffuse group and one compact anchor.
function threadedFixture(): { nodes: GalaxyLayoutNode[]; links: GalaxyLayoutLink[] } {
  const anchorNodes: GalaxyLayoutNode[] = Array.from({ length: 20 }, (_, index) => ({
    id: `anchor-${index}`,
    category: 'decision',
    importance: 8
  }));
  const dustNodes: GalaxyLayoutNode[] = Array.from({ length: 420 }, (_, index) => ({
    id: `dust-${index}`,
    category: 'session',
    importance: 3
  }));
  const links: GalaxyLayoutLink[] = [
    { source: 'anchor-0', target: 'dust-0', weight: 1 },
    ...dustNodes.slice(1).map((node, index) => ({
      source: `dust-${index}`,
      target: node.id,
      weight: 0.99 - index / 10000
    }))
  ];
  return { nodes: [...anchorNodes, ...dustNodes], links };
}

describe('buildGalaxyTargets', () => {
  it('prevents graph fetch order from changing the visible galaxy', () => {
    const nodes = fixtureNodes();
    const forward = buildGalaxyTargets(nodes);
    const reverse = buildGalaxyTargets([...nodes].reverse());

    for (const node of nodes) {
      expect(reverse.get(node.id)).toEqual(forward.get(node.id));
    }
  });

  it('uses real links to keep semantic groups in one local spiral neighbourhood', () => {
    const nodes: GalaxyLayoutNode[] = Array.from({ length: 40 }, (_, index) => ({
      id: `linked-${index}`,
      category: index < 20 ? 'decision' : 'incident',
      community_id: index < 20 ? 7 : 11,
      importance: index === 0 ? 10 : 5
    }));
    const links: GalaxyLayoutLink[] = nodes.slice(1).map((node, index) => ({
      source: nodes[index].id,
      target: node.id,
      weight: 0.9
    }));
    const targets = buildGalaxyTargets(nodes, links);

    const lengths = links.map((link) => distance(
      targets.get(typeof link.source === 'string' ? link.source : link.source.id)!,
      targets.get(typeof link.target === 'string' ? link.target : link.target.id)!
    ));
    expect(Math.max(...lengths)).toBeLessThan(60);
    expect(targets.get('linked-0')?.component).toBe(0);
    expect(targets.get('linked-39')?.inMainComponent).toBe(true);
  });

  it('returns finite bounded targets for empty, singleton, and large graphs', () => {
    expect(buildGalaxyTargets([]).size).toBe(0);

    const singleton = buildGalaxyTargets([{ id: 'only', category: 'general', importance: 5 }]).get('only')!;
    expect([singleton.x, singleton.y, singleton.z].every(Number.isFinite)).toBe(true);

    const large = Array.from({ length: 5000 }, (_, index) => ({
      id: `m${index}`,
      category: `category-${index % 24}`,
      community_id: index % 96,
      importance: (index % 10) + 1
    }));
    const targets = buildGalaxyTargets(large);
    expect(targets.size).toBe(large.length);
    targets.forEach((target) => {
      expect([target.x, target.y, target.z].every(Number.isFinite)).toBe(true);
      // Arms widen with group count to keep clusters apart, but the derived
      // scale is capped, so the galaxy stays within a bounded radius.
      expect(Math.hypot(target.x, target.y)).toBeLessThan(2300);
      expect(Math.abs(target.z)).toBeLessThan(150);
    });
  });

  it('folds undersized communities into their category instead of giving each an arm', () => {
    // Two nodes sharing a tiny community must not claim their own cluster: at
    // production scale that long tail produced hundreds of overlapping blobs.
    const nodes: GalaxyLayoutNode[] = [
      { id: 'a1', category: 'ops', community_id: 500 },
      { id: 'a2', category: 'ops', community_id: 501 },
      { id: 'a3', category: 'ops', community_id: 502 }
    ];
    const targets = buildGalaxyTargets(nodes);

    const groupKeys = new Set([...targets.values()].map((target) => target.groupKey));
    expect(groupKeys).toEqual(new Set(['category:ops']));
  });

  it('keeps a community that is large enough to read as its own cluster', () => {
    const nodes: GalaxyLayoutNode[] = Array.from({ length: 24 }, (_, index) => ({
      id: `b${index}`,
      category: 'ops',
      community_id: 900
    }));
    const targets = buildGalaxyTargets(nodes);

    const groupKeys = new Set([...targets.values()].map((target) => target.groupKey));
    expect(groupKeys).toEqual(new Set(['community:900']));
  });

  it('marks real disconnected components without inventing bridge links', () => {
    const nodes: GalaxyLayoutNode[] = [
      { id: 'main-a', category: 'decision', importance: 10 },
      { id: 'main-b', category: 'decision', importance: 8 },
      { id: 'orphan-a', category: 'session', importance: 3 },
      { id: 'orphan-b', category: 'session', importance: 2 }
    ];
    const targets = buildGalaxyTargets(nodes, [
      { source: 'main-a', target: 'main-b', weight: 1 },
      { source: 'orphan-a', target: 'orphan-b', weight: 0.5 }
    ]);

    expect(targets.get('main-a')).toMatchObject({ component: 0, componentSize: 2, inMainComponent: true });
    expect(targets.get('orphan-a')).toMatchObject({ component: 1, componentSize: 2, inMainComponent: false });
  });

  it('keeps a high-degree topology together as one selectable neighbourhood', () => {
    const leaves: GalaxyLayoutNode[] = Array.from({ length: 120 }, (_, index) => ({
      id: `leaf-${index}`,
      category: 'session',
      importance: 4
    }));
    const nodes: GalaxyLayoutNode[] = [
      { id: 'hub', category: 'decision', importance: 10 },
      ...leaves
    ];
    const links: GalaxyLayoutLink[] = leaves.map((leaf, index) => ({
      source: 'hub',
      target: leaf.id,
      weight: 1 - index / 1000
    }));
    const targets = buildGalaxyTargets(nodes, links);
    const hub = targets.get('hub')!;
    const lengths = leaves
      .map((leaf) => distance(hub, targets.get(leaf.id)!))
      .sort((left, right) => left - right);

    expect(lengths[Math.floor(lengths.length / 2)]).toBeLessThan(60);
    expect(Math.max(...lengths)).toBeLessThan(75);
  });

  it('rebalances one dominant root branch across the galaxy arms', () => {
    const leaves: GalaxyLayoutNode[] = Array.from({ length: 18 }, (_, index) => ({
      id: `root-leaf-${index}`,
      category: 'decision'
    }));
    const branchNodes: GalaxyLayoutNode[] = Array.from({ length: 540 }, (_, index) => ({
      id: `branch-${index}`,
      category: 'session'
    }));
    const nodes: GalaxyLayoutNode[] = [
      { id: 'root', category: 'decision', importance: 10 },
      { id: 'gateway', category: 'task', importance: 9 },
      ...leaves,
      ...branchNodes
    ];
    const links: GalaxyLayoutLink[] = [
      { source: 'root', target: 'gateway', weight: 1 },
      ...leaves.map((leaf) => ({ source: 'root', target: leaf.id, weight: 0.8 })),
      ...Array.from({ length: 9 }, (_, branch) => ({
        source: 'gateway',
        target: `branch-${branch * 60}`,
        weight: 0.95
      })),
      ...branchNodes
        .filter((_, index) => index % 60 !== 0)
        .map((node, index) => {
          const branch = Math.floor(index / 59);
          const offset = index % 59;
          return {
            source: `branch-${branch * 60 + offset}`,
            target: node.id,
            weight: 0.9
          };
        })
    ];
    const targets = buildGalaxyTargets(nodes, links);
    const occupiedSectors = new Set(
      branchNodes.map((node) => {
        const target = targets.get(node.id)!;
        const angle =
          (Math.atan2(target.y / 0.62, target.x) + Math.PI * 2) % (Math.PI * 2);
        return Math.floor(angle / (Math.PI * 2 / 12));
      })
    );

    expect(occupiedSectors.size).toBeGreaterThanOrEqual(9);
  });

  it('prevents initialization from overwriting restored simulation coordinates', () => {
    const nodes: GalaxyLayoutNode[] = [
      { id: 'existing', category: 'general', x: 1, y: 2, z: 3 },
      { id: 'missing', category: 'decision' }
    ];
    const targets = buildGalaxyTargets(nodes);

    seedGalaxyPositions(nodes, targets);

    expect(nodes[0]).toMatchObject({ x: 1, y: 2, z: 3 });
    expect([nodes[1].x, nodes[1].y, nodes[1].z].every(Number.isFinite)).toBe(true);
  });

  it('keeps a long diffuse path bounded without collapsing the galaxy', () => {
    const { nodes, links } = threadedFixture();
    const targets = buildGalaxyTargets(nodes, links);
    const lengths = links.map((link) => distance(
      targets.get(typeof link.source === 'string' ? link.source : link.source.id)!,
      targets.get(typeof link.target === 'string' ? link.target : link.target.id)!
    ));
    const sorted = [...lengths].sort((left, right) => left - right);

    expect(sorted[Math.floor(sorted.length / 2)]).toBeLessThan(180);
    expect(Math.max(...lengths)).toBeLessThan(460);
  });

  it('keeps topology placement deterministic across node, edge, and endpoint forms', () => {
    const { nodes, links } = threadedFixture();
    const forward = buildGalaxyTargets(nodes, links);
    const reversed = buildGalaxyTargets(
      [...nodes].reverse(),
      [...links].reverse().map((link) => ({
        source: { id: typeof link.target === 'string' ? link.target : link.target.id },
        target: { id: typeof link.source === 'string' ? link.source : link.source.id },
        weight: link.weight
      }))
    );

    nodes.forEach((node) => expect(reversed.get(node.id)).toEqual(forward.get(node.id)));
  });

  it('bounds disconnected cycles and ignores invalid links', () => {
    const nodes: GalaxyLayoutNode[] = Array.from({ length: 420 }, (_, index) => ({
      id: `orphan-${index}`,
      category: 'session'
    }));
    const links: GalaxyLayoutLink[] = nodes.map((node, index) => ({
      source: node.id,
      target: nodes[(index + 1) % nodes.length].id,
      weight: index % 9 === 0 ? Number.NaN : 0.8
    }));
    links.push({ source: 'missing', target: 'orphan-0', weight: 1 });
    links.push({ source: 'orphan-0', target: 'orphan-0', weight: 1 });

    const targets = buildGalaxyTargets(nodes, links);
    expect(targets.size).toBe(nodes.length);
    targets.forEach((target) => {
      expect([target.x, target.y, target.z].every(Number.isFinite)).toBe(true);
      expect(Math.hypot(target.x, target.y)).toBeLessThan(500);
    });
  });
});
