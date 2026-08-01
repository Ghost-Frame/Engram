// Deterministic topology-first galaxy targets for the memory relationship atlas.

// One full turn in radians.
const TAU = Math.PI * 2;

// The angle that distributes stable fallback points without radial spokes.
const GOLDEN_ANGLE = Math.PI * (3 - Math.sqrt(5));

// Flatten the galactic plane so topology reads as a disc rather than a sphere.
const DISC_FLATTENING = 0.62;

// Bound depth independently from graph size to retain a thin readable plane.
const DISC_HALF_THICKNESS = 44;

// Keep full-data atlases large enough to separate points without unbounded growth.
const MIN_MAIN_RADIUS = 260;

// Scale the disc sublinearly with node count so every memory remains visible.
const RADIUS_PER_SQRT_NODE = 4.2;

// A semantic community below this size remains category-labelled visual dust.
const MIN_COMMUNITY_MEMBERS = 20;

// Four broad arms create a filled galactic disc without collapsing into an S-curve.
const MAIN_ARM_COUNT = 4;

// GalaxyLayoutNode is the immutable subset required to derive one guide target.
export interface GalaxyLayoutNode {
  id: string;
  category?: string;
  community_id?: number;
  importance?: number;
  x?: number;
  y?: number;
  z?: number;
}

// GalaxyLayoutLink is the real edge subset used to derive topology.
export interface GalaxyLayoutLink {
  source: string | { id: string };
  target: string | { id: string };
  weight?: number;
}

// GalaxyTarget stores one deterministic position and its topology diagnostics.
export interface GalaxyTarget {
  x: number;
  y: number;
  z: number;
  clusterX: number;
  clusterY: number;
  clusterZ: number;
  groupKey: string;
  arm: number;
  diffuse: boolean;
  component: number;
  componentSize: number;
  inMainComponent: boolean;
  depth: number;
}

// ForestNeighbour records one deterministic maximum-affinity forest connection.
interface ForestNeighbour {
  id: string;
  weight: number;
}

// TopologyComponent records one real connected component and its stable root.
interface TopologyComponent {
  ids: string[];
  root: string;
}

// TopologyForest combines adjacency and components so both come from one edge pass.
interface TopologyForest {
  neighbours: Map<string, ForestNeighbour[]>;
  components: TopologyComponent[];
}

// AtlasPoint stores one mutable intermediate position during topology smoothing.
interface AtlasPoint {
  x: number;
  y: number;
  z: number;
}

// AtlasGroup collects one legible semantic neighbourhood in the main component.
interface AtlasGroup {
  key: string;
  ids: string[];
  diffuse: boolean;
}

// Hash a string into a stable unsigned integer without runtime randomness.
function stableHash(value: string): number {
  let hash = 2166136261;
  for (let index = 0; index < value.length; index++) {
    hash ^= value.charCodeAt(index);
    hash = Math.imul(hash, 16777619);
  }
  return hash >>> 0;
}

// Convert a stable hash into a repeatable value between zero and one.
function stableUnit(value: string): number {
  return stableHash(value) / 0xffffffff;
}

// Resolve an edge endpoint before or after a force engine replaces ids with objects.
function endpointId(endpoint: GalaxyLayoutLink['source']): string | null {
  if (typeof endpoint === 'string') return endpoint;
  return typeof endpoint?.id === 'string' ? endpoint.id : null;
}

// Normalize non-finite weights to the weakest valid affinity.
function normalizedWeight(link: GalaxyLayoutLink): number {
  return Number.isFinite(link.weight) ? Number(link.weight) : 0;
}

// Clamp an importance value to the supported one-through-ten range.
function normalizedImportance(node: GalaxyLayoutNode): number {
  const value = Number.isFinite(node.importance) ? Number(node.importance) : 5;
  return Math.max(1, Math.min(10, value));
}

// Tally community membership so tiny communities can retain category labels.
function countCommunityMembers(nodes: readonly GalaxyLayoutNode[]): Map<number, number> {
  const counts = new Map<number, number>();
  for (const node of nodes) {
    if (node.community_id == null) continue;
    counts.set(node.community_id, (counts.get(node.community_id) ?? 0) + 1);
  }
  return counts;
}

// Resolve a semantic label without allowing semantics to dictate spatial islands.
function semanticGroupKey(
  node: GalaxyLayoutNode,
  communitySizes: ReadonlyMap<number, number>
): string {
  if (
    node.community_id != null
    && (communitySizes.get(node.community_id) ?? 0) >= MIN_COMMUNITY_MEMBERS
  ) {
    return `community:${node.community_id}`;
  }
  return `category:${node.category || 'general'}`;
}

// Derive a bounded main-disc radius from the number of linked memories.
function mainDiscRadius(nodeCount: number): number {
  return Math.max(MIN_MAIN_RADIUS, Math.sqrt(Math.max(1, nodeCount)) * RADIUS_PER_SQRT_NODE);
}

// Build a deterministic maximum-affinity forest and enumerate its real components.
function buildTopologyForest(
  nodes: readonly GalaxyLayoutNode[],
  links: readonly GalaxyLayoutLink[]
): TopologyForest {
  const nodeById = new Map(nodes.map((node) => [node.id, node]));
  const neighbours = new Map<string, ForestNeighbour[]>();
  const parents = new Map<string, string>();
  for (const id of nodeById.keys()) {
    neighbours.set(id, []);
    parents.set(id, id);
  }

  // Find one disjoint-set root while compressing the traversed path.
  const findRoot = (id: string): string => {
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

  const ranked = links
    .map((link) => {
      const left = endpointId(link.source);
      const right = endpointId(link.target);
      if (!left || !right || left === right || !nodeById.has(left) || !nodeById.has(right)) return null;
      const source = left < right ? left : right;
      const target = left < right ? right : left;
      return { source, target, weight: normalizedWeight(link) };
    })
    .filter((link): link is { source: string; target: string; weight: number } => link !== null)
    .sort((left, right) =>
      right.weight - left.weight
      || left.source.localeCompare(right.source)
      || left.target.localeCompare(right.target)
    );

  for (const link of ranked) {
    const sourceRoot = findRoot(link.source);
    const targetRoot = findRoot(link.target);
    if (sourceRoot === targetRoot) continue;
    parents.set(targetRoot, sourceRoot);
    neighbours.get(link.source)!.push({ id: link.target, weight: link.weight });
    neighbours.get(link.target)!.push({ id: link.source, weight: link.weight });
  }

  neighbours.forEach((adjacent) => {
    adjacent.sort((left, right) => right.weight - left.weight || left.id.localeCompare(right.id));
  });

  const visited = new Set<string>();
  const components: TopologyComponent[] = [];
  const sortedIds = [...nodeById.keys()].sort((left, right) => left.localeCompare(right));
  for (const seed of sortedIds) {
    if (visited.has(seed)) continue;
    const ids: string[] = [];
    const queue = [seed];
    visited.add(seed);
    for (let cursor = 0; cursor < queue.length; cursor++) {
      const id = queue[cursor];
      ids.push(id);
      for (const adjacent of neighbours.get(id) ?? []) {
        if (visited.has(adjacent.id)) continue;
        visited.add(adjacent.id);
        queue.push(adjacent.id);
      }
    }
    ids.sort((left, right) => left.localeCompare(right));
    const root = [...ids].sort((left, right) => {
      const degreeOrder =
        (neighbours.get(right)?.length ?? 0) - (neighbours.get(left)?.length ?? 0);
      if (degreeOrder) return degreeOrder;
      const importanceOrder =
        normalizedImportance(nodeById.get(right)!) - normalizedImportance(nodeById.get(left)!);
      return importanceOrder || left.localeCompare(right);
    })[0];
    components.push({ ids, root });
  }

  components.sort((left, right) =>
    right.ids.length - left.ids.length || left.root.localeCompare(right.root)
  );
  return { neighbours, components };
}

// Place an entirely edgeless graph as honest two-arm dust instead of a fake component.
function placeUnlinkedDust(
  nodes: readonly GalaxyLayoutNode[],
  radius: number,
  communitySizes: ReadonlyMap<number, number>
): Map<string, GalaxyTarget> {
  const targets = new Map<string, GalaxyTarget>();
  const ordered = [...nodes].sort((left, right) => left.id.localeCompare(right.id));
  ordered.forEach((node, index) => {
    const progress = Math.sqrt((index + 0.5) / ordered.length);
    const arm = stableHash(`${node.id}:dust-arm`) % 2;
    const scatter = (stableUnit(`${node.id}:dust-scatter`) - 0.5) * 0.42;
    const angle = 0.55 + progress * TAU * 1.25 + arm * Math.PI + scatter;
    const nodeRadius = 18 + progress * (radius - 18);
    const x = Math.cos(angle) * nodeRadius;
    const y = Math.sin(angle) * nodeRadius * DISC_FLATTENING;
    const z = (stableUnit(`${node.id}:dust-depth`) - 0.5) * DISC_HALF_THICKNESS * 2;
    targets.set(node.id, {
      x,
      y,
      z,
      clusterX: x,
      clusterY: y,
      clusterZ: z,
      groupKey: semanticGroupKey(node, communitySizes),
      arm,
      diffuse: true,
      component: index,
      componentSize: 1,
      inMainComponent: index === 0,
      depth: 0
    });
  });
  return targets;
}

// Pull individual memories toward their real neighbours while retaining semantic landmarks.
function smoothNodePositions(
  positions: ReadonlyMap<string, AtlasPoint>,
  links: readonly GalaxyLayoutLink[],
  mainIds: ReadonlySet<string>
): Map<string, AtlasPoint> {
  const sums = new Map<string, { x: number; y: number; z: number; weight: number }>();
  for (const link of links) {
    const source = endpointId(link.source);
    const target = endpointId(link.target);
    if (!source || !target || !mainIds.has(source) || !mainIds.has(target)) continue;
    const sourcePoint = positions.get(source);
    const targetPoint = positions.get(target);
    if (!sourcePoint || !targetPoint) continue;
    const weight = 0.25 + normalizedWeight(link);
    const sourceSum = sums.get(source) ?? { x: 0, y: 0, z: 0, weight: 0 };
    const targetSum = sums.get(target) ?? { x: 0, y: 0, z: 0, weight: 0 };
    sourceSum.x += targetPoint.x * weight;
    sourceSum.y += targetPoint.y * weight;
    sourceSum.z += targetPoint.z * weight;
    sourceSum.weight += weight;
    targetSum.x += sourcePoint.x * weight;
    targetSum.y += sourcePoint.y * weight;
    targetSum.z += sourcePoint.z * weight;
    targetSum.weight += weight;
    sums.set(source, sourceSum);
    sums.set(target, targetSum);
  }

  const smoothed = new Map<string, AtlasPoint>();
  for (const [id, point] of positions) {
    const sum = sums.get(id);
    if (!sum?.weight || !mainIds.has(id)) {
      smoothed.set(id, { ...point });
      continue;
    }
    smoothed.set(id, {
      x: point.x * 0.85 + (sum.x / sum.weight) * 0.15,
      y: point.y * 0.85 + (sum.y / sum.weight) * 0.15,
      z: point.z * 0.85 + (sum.z / sum.weight) * 0.15
    });
  }
  return smoothed;
}

// Order semantic groups by their strongest real inter-group paths before placing the spiral.
function orderGroupsByTopology(
  groups: readonly AtlasGroup[],
  nodeGroups: ReadonlyMap<string, string>,
  links: readonly GalaxyLayoutLink[],
  mainIds: ReadonlySet<string>
): AtlasGroup[] {
  const groupByKey = new Map(groups.map((group) => [group.key, group]));
  const adjacent = new Map<string, Map<string, number>>();
  for (const group of groups) adjacent.set(group.key, new Map());
  for (const link of links) {
    const source = endpointId(link.source);
    const target = endpointId(link.target);
    if (!source || !target || !mainIds.has(source) || !mainIds.has(target)) continue;
    const sourceGroup = nodeGroups.get(source);
    const targetGroup = nodeGroups.get(target);
    if (
      !sourceGroup
      || !targetGroup
      || sourceGroup === targetGroup
      || !groupByKey.has(sourceGroup)
      || !groupByKey.has(targetGroup)
    ) {
      continue;
    }
    const weight = 0.25 + normalizedWeight(link);
    adjacent.get(sourceGroup)!.set(
      targetGroup,
      (adjacent.get(sourceGroup)!.get(targetGroup) ?? 0) + weight
    );
    adjacent.get(targetGroup)!.set(
      sourceGroup,
      (adjacent.get(targetGroup)!.get(sourceGroup) ?? 0) + weight
    );
  }

  const ordered: AtlasGroup[] = [];
  const visited = new Set<string>();
  const roots = [...groups].sort((left, right) =>
    right.ids.length - left.ids.length || left.key.localeCompare(right.key)
  );
  for (const root of roots) {
    if (visited.has(root.key)) continue;
    const queue = [root.key];
    visited.add(root.key);
    for (let cursor = 0; cursor < queue.length; cursor++) {
      const key = queue[cursor];
      ordered.push(groupByKey.get(key)!);
      const neighbours = [...(adjacent.get(key) ?? [])]
        .filter(([otherKey]) => !visited.has(otherKey))
        .sort((left, right) =>
          right[1] - left[1]
          || (groupByKey.get(right[0])?.ids.length ?? 0)
            - (groupByKey.get(left[0])?.ids.length ?? 0)
          || left[0].localeCompare(right[0])
        );
      for (const [otherKey] of neighbours) {
        if (visited.has(otherKey)) continue;
        visited.add(otherKey);
        queue.push(otherKey);
      }
    }
  }
  return ordered;
}

// Compose a semantic spiral whose local geometry is corrected by every real graph edge.
function buildTopologySmoothedAtlas(
  nodes: readonly GalaxyLayoutNode[],
  links: readonly GalaxyLayoutLink[],
  topology: TopologyForest,
  communitySizes: ReadonlyMap<number, number>
): Map<string, GalaxyTarget> {
  const nodeById = new Map(nodes.map((node) => [node.id, node]));
  const mainComponent = topology.components[0];
  const mainIds = new Set(mainComponent.ids);
  const radius = mainDiscRadius(mainComponent.ids.length);
  const groupedIds = new Map<string, string[]>();
  const nodeGroups = new Map<string, string>();
  for (const id of mainComponent.ids) {
    const node = nodeById.get(id)!;
    const key = semanticGroupKey(node, communitySizes);
    const members = groupedIds.get(key) ?? [];
    members.push(id);
    groupedIds.set(key, members);
    nodeGroups.set(id, key);
  }

  const diffuseThreshold = Math.max(400, mainComponent.ids.length * 0.05);
  const groups: AtlasGroup[] = [...groupedIds]
    .map(([key, ids]) => ({
      key,
      ids: [...ids].sort((left, right) => left.localeCompare(right)),
      diffuse: ids.length >= diffuseThreshold
    }))
    .sort((left, right) =>
      Number(left.diffuse) - Number(right.diffuse)
      || right.ids.length - left.ids.length
      || left.key.localeCompare(right.key)
    );
  const compactGroups = orderGroupsByTopology(
    groups.filter((group) => !group.diffuse),
    nodeGroups,
    links,
    mainIds
  );
  const rotation = stableUnit(`${mainComponent.root}:atlas-rotation`) * TAU;
  const baseCentres = new Map<string, AtlasPoint>();
  compactGroups.forEach((group, index) => {
    if (compactGroups.length === 1 && groups.length === 1) {
      baseCentres.set(group.key, { x: 0, y: 0, z: 0 });
      return;
    }
    if (compactGroups.length <= MAIN_ARM_COUNT) {
      const angle = rotation + index * 0.22;
      const groupRadius = 18 + index * 18;
      baseCentres.set(group.key, {
        x: Math.cos(angle) * groupRadius,
        y: Math.sin(angle) * groupRadius * DISC_FLATTENING,
        z: (index - compactGroups.length / 2) * 3
      });
      return;
    }
    const laneIndex = Math.floor(index / MAIN_ARM_COUNT);
    const laneCount = Math.ceil(compactGroups.length / MAIN_ARM_COUNT);
    const progress = Math.sqrt((laneIndex + 0.7) / Math.max(1, laneCount));
    const arm = index % MAIN_ARM_COUNT;
    const scatter = (stableUnit(`${group.key}:atlas-lane`) - 0.5) * 0.82;
    const angle =
      rotation
      + arm * (TAU / MAIN_ARM_COUNT)
      + progress * TAU * 1.34
      + scatter;
    const radialJitter = (stableUnit(`${group.key}:atlas-radius-jitter`) - 0.5) * 54;
    const groupRadius = Math.max(12, 34 + progress * (radius - 88) + radialJitter);
    baseCentres.set(group.key, {
      x: Math.cos(angle) * groupRadius,
      y: Math.sin(angle) * groupRadius * DISC_FLATTENING,
      z: Math.sin(angle * 1.35) * DISC_HALF_THICKNESS * 0.22
    });
  });
  const centres = baseCentres;
  const positions = new Map<string, AtlasPoint>();

  compactGroups.forEach((group, groupIndex) => {
    const centre = centres.get(group.key)!;
    const spread = compactGroups.length === 1
      ? radius * 0.82
      : 8 + Math.sqrt(group.ids.length) * 1.9;
    const rotationOffset = stableUnit(`${group.key}:member-rotation`) * TAU;
    group.ids.forEach((id, localIndex) => {
      const node = nodeById.get(id)!;
      const density = group.ids.length === 1
        ? 0
        : Math.sqrt((localIndex + 0.5) / group.ids.length);
      const angle = rotationOffset + localIndex * GOLDEN_ANGLE;
      const importancePull = 1 - ((normalizedImportance(node) - 1) / 9) * 0.24;
      const localRadius = density * spread * importancePull;
      positions.set(id, {
        x: centre.x + Math.cos(angle) * localRadius,
        y: centre.y + Math.sin(angle) * localRadius * 0.68,
        z:
          centre.z
          + (stableUnit(`${id}:atlas-depth`) - 0.5)
            * Math.min(34, spread * 0.75)
      });
    });
  });

  for (const group of groups.filter((candidate) => candidate.diffuse)) {
    group.ids.forEach((id) => {
      const radialUnit = stableUnit(`${id}:atlas-radius`);
      const inCore = stableUnit(`${id}:atlas-core`) < 0.18;
      const progress = inCore
        ? Math.sqrt(stableUnit(`${id}:atlas-core-radius`)) * 0.24
        : Math.sqrt(radialUnit);
      const arm = stableHash(`${id}:atlas-arm`) % MAIN_ARM_COUNT;
      const scatter = inCore
        ? stableUnit(`${id}:atlas-core-angle`) * TAU
        : (stableUnit(`${id}:atlas-scatter`) - 0.5) * (1.0 + progress * 1.7);
      const angle = inCore
        ? scatter
        : rotation
          + arm * (TAU / MAIN_ARM_COUNT)
          + progress * TAU * 1.34
          + scatter;
      const nodeRadius = 4 + progress * (radius - 22);
      positions.set(id, {
        x: Math.cos(angle) * nodeRadius,
        y: Math.sin(angle) * nodeRadius * DISC_FLATTENING,
        z: (stableUnit(`${id}:atlas-dust-depth`) - 0.5) * DISC_HALF_THICKNESS
      });
    });
  }

  const compactIds = new Set(compactGroups.flatMap((group) => group.ids));
  const smoothed = smoothNodePositions(positions, links, compactIds);
  const targets = new Map<string, GalaxyTarget>();
  const mainPointsByGroup = new Map<string, AtlasPoint[]>();
  for (const id of mainComponent.ids) {
    const point = smoothed.get(id)!;
    const group = nodeGroups.get(id)!;
    const centre = centres.get(group) ?? point;
    const groupPoints = mainPointsByGroup.get(group) ?? [];
    groupPoints.push(point);
    mainPointsByGroup.set(group, groupPoints);
    targets.set(id, {
      ...point,
      clusterX: centre.x,
      clusterY: centre.y,
      clusterZ: centre.z,
      groupKey: group,
      arm: stableHash(`${id}:atlas-arm`) % MAIN_ARM_COUNT,
      diffuse: (groupedIds.get(group)?.length ?? 0) >= diffuseThreshold,
      component: 0,
      componentSize: mainComponent.ids.length,
      inMainComponent: true,
      depth: id === mainComponent.root ? 0 : 1
    });
  }

  topology.components.slice(1).forEach((component, offset) => {
    const componentIndex = offset + 1;
    const groupCounts = new Map<string, number>();
    for (const id of component.ids) {
      const key = semanticGroupKey(nodeById.get(id)!, communitySizes);
      groupCounts.set(key, (groupCounts.get(key) ?? 0) + 1);
    }
    const dominantGroup = [...groupCounts].sort((left, right) =>
      right[1] - left[1] || left[0].localeCompare(right[0])
    )[0]?.[0];
    const semanticAnchors = dominantGroup
      ? mainPointsByGroup.get(dominantGroup)
      : undefined;
    const fallbackAnchor = targets.get(mainComponent.root)!;
    const anchor = semanticAnchors?.[
      stableHash(`${component.root}:semantic-anchor`) % semanticAnchors.length
    ] ?? fallbackAnchor;
    const angle = rotation + GOLDEN_ANGLE * componentIndex;
    const spread = Math.max(5, Math.sqrt(component.ids.length) * 2.8);
    const nestOffset = Math.max(8, Math.min(34, spread * 0.72));
    const centre = {
      x: anchor.x + Math.cos(angle) * nestOffset,
      y: anchor.y + Math.sin(angle) * nestOffset * DISC_FLATTENING,
      z: anchor.z
        + (stableUnit(`${component.root}:satellite-depth`) - 0.5)
          * DISC_HALF_THICKNESS
          * 0.42
    };
    component.ids.forEach((id, localIndex) => {
      const localProgress = component.ids.length === 1
        ? 0
        : Math.sqrt((localIndex + 0.5) / component.ids.length);
      const localAngle = stableUnit(`${component.root}:satellite-rotation`) * TAU
        + localIndex * GOLDEN_ANGLE;
      const localRadius = localProgress * spread;
      const point = {
        x: centre.x + Math.cos(localAngle) * localRadius,
        y: centre.y + Math.sin(localAngle) * localRadius * 0.68,
        z: centre.z + (stableUnit(`${id}:satellite-z`) - 0.5) * Math.min(18, spread)
      };
      const node = nodeById.get(id)!;
      targets.set(id, {
        ...point,
        clusterX: centre.x,
        clusterY: centre.y,
        clusterZ: centre.z,
        groupKey: semanticGroupKey(node, communitySizes),
        arm: componentIndex % MAIN_ARM_COUNT,
        diffuse: true,
        component: componentIndex,
        componentSize: component.ids.length,
        inMainComponent: false,
        depth: id === component.root ? 0 : 1
      });
    });
  });
  return targets;
}

// Build stable targets whose main geometry comes only from real stored links.
export function buildGalaxyTargets(
  nodes: readonly GalaxyLayoutNode[],
  links: readonly GalaxyLayoutLink[] = []
): Map<string, GalaxyTarget> {
  if (!nodes.length) return new Map<string, GalaxyTarget>();
  const communitySizes = countCommunityMembers(nodes);
  const nodeById = new Map(nodes.map((node) => [node.id, node]));
  const radius = mainDiscRadius(nodes.length);
  const topology = buildTopologyForest(nodes, links);
  const hasForestEdge = topology.components.some((component) => component.ids.length > 1);
  if (!hasForestEdge) return placeUnlinkedDust(nodes, radius, communitySizes);
  return buildTopologySmoothedAtlas(nodes, links, topology, communitySizes);
}

// Seed only missing simulation coordinates so restored positions survive.
export function seedGalaxyPositions(
  nodes: GalaxyLayoutNode[],
  targets: ReadonlyMap<string, GalaxyTarget>
): void {
  for (const node of nodes) {
    const target = targets.get(node.id);
    if (!target) continue;
    if (!Number.isFinite(node.x)) node.x = target.x;
    if (!Number.isFinite(node.y)) node.y = target.y;
    if (!Number.isFinite(node.z)) node.z = target.z;
  }
}
