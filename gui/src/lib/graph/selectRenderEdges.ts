// Connectivity-preserving edge selection for the memory relationship atlas.
//
// A browser can't draw tens of thousands of edges smoothly, so past a threshold
// the graph renders only a subset. The naive subset -- "keep the top-N edges by
// weight" -- silently severs whole clusters: every edge a node has can be just
// below the global cutoff, so the node drops out of the drawn structure entirely
// and the graph reads as a disconnected starfield even though the data is linked.
//
// This selection instead builds a maximum-affinity spanning forest before it
// spends any budget on redundant edges. A weak bridge is therefore retained
// whenever it is the only real route between two otherwise strong regions.

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

// RenderPosition is the finite coordinate subset used to prefer local detail edges.
export interface RenderPosition {
  x: number;
  y: number;
  z: number;
}

// Treat a missing/NaN weight as the weakest possible edge so it sorts last.
function edgeWeight(edge: SelectableEdge): number {
  const w = edge.weight;
  return typeof w === 'number' && Number.isFinite(w) ? w : 0;
}

// Resolve the stable graph identity carried by an endpoint before or after d3 mutation.
function endpointKey(endpoint: unknown): string {
  if (typeof endpoint === 'string') return endpoint;
  if (typeof endpoint === 'number' || typeof endpoint === 'bigint') return String(endpoint);
  if (endpoint && typeof endpoint === 'object' && 'id' in endpoint) {
    const id = (endpoint as { id?: unknown }).id;
    if (typeof id === 'string' || typeof id === 'number' || typeof id === 'bigint') return String(id);
  }
  return `${typeof endpoint}:${String(endpoint)}`;
}

// Measure how expensive an optional detail edge is in the current atlas geometry.
function localityCost(
  sourceId: string,
  targetId: string,
  weight: number,
  positions: ReadonlyMap<string, RenderPosition> | undefined
): number {
  if (!positions) return -weight;
  const source = positions.get(sourceId);
  const target = positions.get(targetId);
  if (!source || !target) return Number.POSITIVE_INFINITY;
  const distance = Math.hypot(source.x - target.x, source.y - target.y, source.z - target.z);
  return distance / (0.2 + Math.max(0, weight));
}

/**
 * Pick at most `cap` edges that preserve graph connectivity.
 *
 * - If `allEdges.length <= cap`, returns the input unchanged (no work to do).
 * - Otherwise returns a subset of exactly `cap` edges (a strict subset of the
 *   input, no duplicates) whose first pass is a maximum-affinity spanning
 *   forest, budget permitting.
 *
 * Self-loops and duplicate edges are handled without double counting: each input
 * edge is considered once and chosen at most once.
 */
export function selectRenderEdges<E extends SelectableEdge>(
  allEdges: E[],
  cap: number,
  positions?: ReadonlyMap<string, RenderPosition>
): E[] {
  if (cap <= 0) return [];
  if (allEdges.length <= cap) return allEdges;

  // Strongest first; index breaks ties so the sort is stable across engines.
  const ranked = allEdges
    .map((edge, index) => {
      const left = endpointKey(edge.source);
      const right = endpointKey(edge.target);
      return {
        edge,
        index,
        source: left < right ? left : right,
        target: left < right ? right : left,
        weight: edgeWeight(edge)
      };
    })
    .sort((left, right) =>
      right.weight - left.weight
      || left.source.localeCompare(right.source)
      || left.target.localeCompare(right.target)
      || left.index - right.index
    );

  const chosen = new Array<boolean>(allEdges.length).fill(false);
  const parents = new Map<string, string>();
  const result: E[] = [];

  // Find one disjoint-set root while compressing the traversed path.
  const findRoot = (id: string): string => {
    if (!parents.has(id)) parents.set(id, id);
    let root = id;
    while (parents.get(root) !== root) root = parents.get(root)!;
    let cursor = id;
    while (cursor !== root) {
      const next = parents.get(cursor)!;
      parents.set(cursor, root);
      cursor = next;
    }
    return root;
  };

  // Pass 1: Kruskal's algorithm retains every necessary bridge before any
  // cycle edge, while strongest-first ordering keeps the forest semantically
  // meaningful among the many valid spanning trees.
  for (const { edge, index, source, target } of ranked) {
    if (source === target) continue;
    const sourceRoot = findRoot(source);
    const targetRoot = findRoot(target);
    if (sourceRoot === targetRoot) continue;
    parents.set(targetRoot, sourceRoot);
    chosen[index] = true;
    result.push(edge);
  }

  // A production overview can deliberately budget fewer edges than a complete
  // forest. In that case retain the shortest real forest segments across the
  // atlas instead of whichever high-weight branches Kruskal happened to visit
  // first; active-memory interaction still derives its ranked neighborhood from
  // the complete relationship index.
  if (result.length > cap) {
    return result
      .map((edge, index) => {
        const source = endpointKey(edge.source);
        const target = endpointKey(edge.target);
        const weight = edgeWeight(edge);
        return {
          edge,
          index,
          cost: localityCost(source, target, weight, positions),
          weight
        };
      })
      .sort((left, right) =>
        left.cost - right.cost
        || right.weight - left.weight
        || left.index - right.index
      )
      .slice(0, cap)
      .map(({ edge }) => edge);
  }

  // Pass 2: thicken the atlas with short, strong local edges after connectivity
  // is safe. Without positions this preserves the legacy strongest-first order.
  if (result.length < cap) {
    const detail = ranked
      .filter(({ index }) => !chosen[index])
      .map((entry) => ({
        ...entry,
        cost: localityCost(entry.source, entry.target, entry.weight, positions)
      }))
      .sort((left, right) =>
        left.cost - right.cost
        || right.weight - left.weight
        || left.index - right.index
      );
    for (const { edge, index } of detail) {
      if (result.length >= cap) break;
      chosen[index] = true;
      result.push(edge);
    }
  }

  return result;
}
