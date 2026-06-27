// Connectivity-preserving edge selection for the 3D memory graph.
//
// A browser can't draw tens of thousands of edges smoothly, so past a threshold
// the graph renders only a subset. The naive subset -- "keep the top-N edges by
// weight" -- silently severs whole clusters: every edge a node has can be just
// below the global cutoff, so the node drops out of the drawn structure entirely
// and the graph reads as a disconnected starfield even though the data is linked.
//
// This selection instead guarantees a skeleton: in the first pass every node
// keeps its single strongest incident edge (so any node with at least one link
// stays attached to the structure), then the remaining budget is filled with the
// globally strongest leftover edges to thicken the dense cores. The result is a
// connected graph at a bounded edge count.

// Minimal edge shape this operates on. Endpoints are typed as `unknown` because
// the algorithm only needs identity equality between them: at selection time the
// graph's edges carry string endpoint ids (the backend ships "m123"), which is
// exactly when this runs -- before the force engine swaps the ids for node object
// references. `weight` is cosine similarity. Typing endpoints as `unknown` lets
// the caller pass its richer GLink ({ source: string | GNode }) without a cast.
export interface SelectableEdge {
  source: unknown;
  target: unknown;
  weight?: number;
}

// Treat a missing/NaN weight as the weakest possible edge so it sorts last.
function edgeWeight(edge: SelectableEdge): number {
  const w = edge.weight;
  return typeof w === 'number' && Number.isFinite(w) ? w : 0;
}

/**
 * Pick at most `cap` edges that preserve graph connectivity.
 *
 * - If `allEdges.length <= cap`, returns the input unchanged (no work to do).
 * - Otherwise returns a subset of exactly `cap` edges (a strict subset of the
 *   input, no duplicates) in which every node that has any incident edge keeps
 *   at least its strongest one, budget permitting.
 *
 * Self-loops and duplicate edges are handled without double counting: each input
 * edge is considered once and chosen at most once.
 */
export function selectRenderEdges<E extends SelectableEdge>(allEdges: E[], cap: number): E[] {
  if (cap <= 0) return [];
  if (allEdges.length <= cap) return allEdges;

  // Strongest first; index breaks ties so the sort is stable across engines.
  const ranked = allEdges
    .map((edge, index) => ({ edge, index }))
    .sort((a, b) => edgeWeight(b.edge) - edgeWeight(a.edge) || a.index - b.index);

  const chosen = new Array<boolean>(allEdges.length).fill(false);
  const attached = new Set<unknown>();
  const result: E[] = [];

  // Pass 1: every node's strongest incident edge. Because `ranked` is sorted by
  // weight, the first edge we encounter touching a still-unattached node is that
  // node's strongest, so adding it attaches the node to the skeleton.
  for (const { edge, index } of ranked) {
    if (result.length >= cap) break;
    if (chosen[index]) continue;
    if (!attached.has(edge.source) || !attached.has(edge.target)) {
      chosen[index] = true;
      attached.add(edge.source);
      attached.add(edge.target);
      result.push(edge);
    }
  }

  // Pass 2: spend the remaining budget on the strongest edges not yet taken,
  // thickening dense cores once the skeleton is in place.
  if (result.length < cap) {
    for (const { edge, index } of ranked) {
      if (result.length >= cap) break;
      if (chosen[index]) continue;
      chosen[index] = true;
      result.push(edge);
    }
  }

  return result;
}
