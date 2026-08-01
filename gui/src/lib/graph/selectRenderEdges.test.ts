import { describe, expect, it } from 'vitest';
import { selectRenderEdges, type SelectableEdge } from './selectRenderEdges';

// Collect the set of node ids touched by a list of edges.
function coveredNodes(edges: SelectableEdge[]): Set<unknown> {
  const s = new Set<unknown>();
  for (const e of edges) {
    s.add(e.source);
    s.add(e.target);
  }
  return s;
}

describe('selectRenderEdges', () => {
  it('returns the input unchanged when at or under the cap', () => {
    const edges = [
      { source: 'a', target: 'b', weight: 0.5 },
      { source: 'b', target: 'c', weight: 0.4 }
    ];
    expect(selectRenderEdges(edges, 5)).toBe(edges);
    expect(selectRenderEdges(edges, 2)).toBe(edges);
  });

  it('returns at most cap edges, all from the input, with no duplicates', () => {
    const edges = Array.from({ length: 100 }, (_, i) => ({
      source: `n${i}`,
      target: `n${i + 1}`,
      weight: i / 100
    }));
    const out = selectRenderEdges(edges, 30);
    expect(out.length).toBe(30);
    // strict subset, every element is an input element
    expect(new Set(out).size).toBe(30);
    out.forEach((e) => expect(edges).toContain(e));
  });

  it('preserves connectivity that a naive top-N-by-weight slice would sever', () => {
    // Two triangles: a strong one (weights 0.9) and a weak one (weights 0.3).
    const edges = [
      { source: 'A', target: 'B', weight: 0.9 },
      { source: 'A', target: 'C', weight: 0.9 },
      { source: 'B', target: 'C', weight: 0.9 },
      { source: 'D', target: 'E', weight: 0.3 },
      { source: 'D', target: 'F', weight: 0.3 },
      { source: 'E', target: 'F', weight: 0.3 }
    ];

    // Naive top-4 would take all three strong edges + one weak edge, leaving one
    // node of the weak triangle (F) with no incident edge -- disconnected.
    const naiveTop4 = [...edges].sort((a, b) => b.weight - a.weight).slice(0, 4);
    expect(coveredNodes(naiveTop4).has('F')).toBe(false);

    // selectRenderEdges spends its skeleton budget attaching every node first,
    // so all six nodes stay connected within the same cap of 4.
    const out = selectRenderEdges(edges, 4);
    expect(out.length).toBe(4);
    const covered = coveredNodes(out);
    ['A', 'B', 'C', 'D', 'E', 'F'].forEach((n) => expect(covered.has(n)).toBe(true));
  });

  it('keeps every node attached when the cap allows a full skeleton', () => {
    // A hub-and-spoke graph: without the skeleton pass, redundant high-weight
    // edges could crowd out a low-weight leaf.
    const edges = [
      { source: 'hub', target: 'a', weight: 0.99 },
      { source: 'hub', target: 'b', weight: 0.98 },
      { source: 'a', target: 'b', weight: 0.97 }, // redundant: a,b already attached
      { source: 'hub', target: 'leaf', weight: 0.1 } // weakest, but the only link for leaf
    ];
    const out = selectRenderEdges(edges, 3);
    expect(coveredNodes(out).has('leaf')).toBe(true);
  });

  it('uses local real forest segments when the overview budget is smaller than the skeleton', () => {
    const edges = [
      { source: 'a', target: 'b', weight: 0.8 },
      { source: 'b', target: 'c', weight: 0.8 },
      { source: 'c', target: 'd', weight: 0.8 },
      { source: 'd', target: 'e', weight: 0.8 }
    ];
    const positions = new Map([
      ['a', { x: 0, y: 0, z: 0 }],
      ['b', { x: 1, y: 0, z: 0 }],
      ['c', { x: 100, y: 0, z: 0 }],
      ['d', { x: 101, y: 0, z: 0 }],
      ['e', { x: 102, y: 0, z: 0 }]
    ]);

    const out = selectRenderEdges(edges, 2, positions);

    expect(out).toHaveLength(2);
    expect(out).not.toContain(edges[1]);
    for (const edge of out) {
      const source = positions.get(String(edge.source))!;
      const target = positions.get(String(edge.target))!;
      expect(Math.hypot(source.x - target.x, source.y - target.y, source.z - target.z)).toBe(1);
    }
  });

  it('keeps a weak real bridge before spending budget on redundant strong edges', () => {
    const edges = [
      { source: 'A', target: 'B', weight: 0.99 },
      { source: 'A', target: 'C', weight: 0.98 },
      { source: 'B', target: 'C', weight: 0.97 },
      { source: 'D', target: 'E', weight: 0.96 },
      { source: 'D', target: 'F', weight: 0.95 },
      { source: 'E', target: 'F', weight: 0.94 },
      { source: 'C', target: 'D', weight: 0.1 }
    ];

    const out = selectRenderEdges(edges, 5);
    expect(out).toContain(edges[6]);
    expect(coveredNodes(out)).toEqual(new Set(['A', 'B', 'C', 'D', 'E', 'F']));
  });

  it('treats missing/NaN weight as the weakest edge', () => {
    const edges: SelectableEdge[] = [
      { source: 'a', target: 'b' }, // undefined weight -> 0
      { source: 'c', target: 'd', weight: NaN }, // NaN -> 0
      { source: 'e', target: 'f', weight: 0.5 }
    ];
    const out = selectRenderEdges(edges, 2);
    // Pass 1 still attaches by encounter order; the strong edge ranks first.
    expect(out[0]).toEqual({ source: 'e', target: 'f', weight: 0.5 });
    expect(out.length).toBe(2);
  });

  it('handles a self-loop without double-attaching or crashing', () => {
    const edges = [
      { source: 'a', target: 'a', weight: 0.9 }, // self loop
      { source: 'b', target: 'c', weight: 0.8 },
      { source: 'c', target: 'd', weight: 0.7 }
    ];
    const out = selectRenderEdges(edges, 2);
    expect(out.length).toBe(2);
    expect(new Set(out).size).toBe(2);
  });

  it('returns empty for a non-positive cap', () => {
    expect(selectRenderEdges([{ source: 'a', target: 'b', weight: 1 }], 0)).toEqual([]);
    expect(selectRenderEdges([{ source: 'a', target: 'b', weight: 1 }], -3)).toEqual([]);
  });
});
