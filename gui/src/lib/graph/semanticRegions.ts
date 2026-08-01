// Semantic-region derivation for the memory galaxy's overview zoom level.

import type { GalaxyTarget } from './galaxyLayout';

// Four radial bands retain useful drilldown granularity without overview confetti.
const MAIN_REGION_RADIAL_BANDS = 4;

// Two depth planes preserve the landing constellation's three-dimensional orbit.
const MAIN_REGION_DEPTH_BANDS = 2;

// SemanticRegionNode is the immutable node subset required to build overview regions.
export interface SemanticRegionNode {
  id: string;
  category?: string;
  community_id?: number;
  importance?: number;
}

// SemanticRegionLink is one real stored relationship used for region aggregation.
export interface SemanticRegionLink {
  source: string | { id: string };
  target: string | { id: string };
  weight?: number;
}

// SemanticRegion is one selectable overview landmark backed by original memories.
export interface SemanticRegion {
  id: string;
  label: string;
  category: string;
  communityId?: number;
  component: number;
  inMainComponent: boolean;
  memberIds: string[];
  memberCount: number;
  internalEdgeCount: number;
  importance: number;
  x: number;
  y: number;
  z: number;
  extent: number;
}

// SemanticRegionEdge aggregates only real stored links that cross two overview regions.
export interface SemanticRegionEdge {
  source: string;
  target: string;
  count: number;
  weight: number;
  maxWeight: number;
}

// SemanticRegionAtlas contains the overview plus the lossless memory-to-region index.
export interface SemanticRegionAtlas {
  regions: SemanticRegion[];
  edges: SemanticRegionEdge[];
  nodeRegion: Map<string, string>;
}

// SemanticRegionPosition supplies one overview coordinate for edge selection.
interface SemanticRegionPosition {
  x: number;
  y: number;
  z: number;
}

// RegionAccumulator collects deterministic statistics before centroids are finalized.
interface RegionAccumulator {
  id: string;
  groupKey: string;
  categoryCounts: Map<string, number>;
  component: number;
  inMainComponent: boolean;
  memberIds: string[];
  importanceTotal: number;
  xTotal: number;
  yTotal: number;
  zTotal: number;
}

// EdgeAccumulator combines parallel stored links without changing their endpoints.
interface EdgeAccumulator {
  source: string;
  target: string;
  count: number;
  weightTotal: number;
  maxWeight: number;
}

// Resolve an endpoint before or after graph libraries replace an id with an object.
function endpointId(endpoint: SemanticRegionLink['source']): string | null {
  if (typeof endpoint === 'string') return endpoint;
  return typeof endpoint?.id === 'string' ? endpoint.id : null;
}

// Resolve one stable overview id while collapsing the main component into spatial sectors.
function regionIdForTarget(target: GalaxyTarget, atlasRadius: number): string {
  const componentKey = `component:${target.component}`;
  if (!target.inMainComponent) {
    return componentKey;
  }
  const normalizedRadius = Math.hypot(target.x, target.y) / Math.max(1, atlasRadius);
  const radialBand = Math.min(
    MAIN_REGION_RADIAL_BANDS - 1,
    Math.floor(normalizedRadius * MAIN_REGION_RADIAL_BANDS)
  );
  const depthBand = Math.min(MAIN_REGION_DEPTH_BANDS - 1, target.z >= 0 ? 1 : 0);
  return `sector:arm:${target.arm}|band:${radialBand}|depth:${depthBand}`;
}

// Return the most common category with stable lexical tie-breaking.
function dominantCategory(counts: ReadonlyMap<string, number>): string {
  return [...counts]
    .sort((left, right) => right[1] - left[1] || left[0].localeCompare(right[0]))[0]?.[0]
    ?? 'general';
}

// Turn an internal region identity into a compact human-readable overview label.
function regionLabel(id: string, groupKey: string, category: string, component: number): string {
  if (component !== 0) return `${category} island ${component}`;
  if (id.startsWith('sector:')) {
    const coordinates = id
      .replace('sector:', '')
      .replaceAll('|', ' ')
      .replaceAll(':', ' ');
    const name = category.replace(/(^|[-_])\w/g, (match) => match.replace(/[-_]/, ' ').toUpperCase());
    return `${name} ${coordinates}`;
  }
  const base = groupKey.startsWith('community:')
    ? `Community ${groupKey.slice('community:'.length)}`
    : category.replace(/(^|[-_])\w/g, (match) => match.replace(/[-_]/, ' ').toUpperCase());
  return base;
}

// Build a deterministic semantic overview without removing any original node or edge.
export function buildSemanticRegions(
  nodes: readonly SemanticRegionNode[],
  links: readonly SemanticRegionLink[],
  targets: ReadonlyMap<string, GalaxyTarget>
): SemanticRegionAtlas {
  const mainRadii = [...targets.values()]
    .filter((target) => target.inMainComponent)
    .map((target) => Math.hypot(target.x, target.y));
  const atlasRadius = Math.max(1, ...mainRadii);
  const nodeById = new Map(nodes.map((node) => [node.id, node]));
  const nodeRegion = new Map<string, string>();
  const accumulators = new Map<string, RegionAccumulator>();

  for (const node of [...nodes].sort((left, right) => left.id.localeCompare(right.id))) {
    const target = targets.get(node.id);
    if (!target) continue;
    const id = regionIdForTarget(target, atlasRadius);
    const category = node.category || 'general';
    const accumulator = accumulators.get(id) ?? {
      id,
      groupKey: target.groupKey,
      categoryCounts: new Map<string, number>(),
      component: target.component,
      inMainComponent: target.inMainComponent,
      memberIds: [],
      importanceTotal: 0,
      xTotal: 0,
      yTotal: 0,
      zTotal: 0
    };
    accumulator.memberIds.push(node.id);
    accumulator.categoryCounts.set(
      category,
      (accumulator.categoryCounts.get(category) ?? 0) + 1
    );
    accumulator.importanceTotal += Number.isFinite(node.importance) ? Number(node.importance) : 5;
    accumulator.xTotal += target.x;
    accumulator.yTotal += target.y;
    accumulator.zTotal += target.z;
    accumulators.set(id, accumulator);
    nodeRegion.set(node.id, id);
  }

  const regions = [...accumulators.values()]
    .map((accumulator): SemanticRegion => {
      const memberCount = accumulator.memberIds.length;
      const category = dominantCategory(accumulator.categoryCounts);
      return {
        id: accumulator.id,
        label: regionLabel(accumulator.id, accumulator.groupKey, category, accumulator.component),
        category,
        communityId: !accumulator.id.startsWith('sector:') && accumulator.groupKey.startsWith('community:')
          ? Number.parseInt(accumulator.groupKey.slice('community:'.length), 10)
          : undefined,
        component: accumulator.component,
        inMainComponent: accumulator.inMainComponent,
        memberIds: accumulator.memberIds,
        memberCount,
        internalEdgeCount: 0,
        importance: accumulator.importanceTotal / Math.max(1, memberCount),
        x: accumulator.xTotal / Math.max(1, memberCount),
        y: accumulator.yTotal / Math.max(1, memberCount),
        z: accumulator.zTotal / Math.max(1, memberCount),
        extent: 0
      };
    })
    .sort((left, right) => left.id.localeCompare(right.id));
  const regionById = new Map(regions.map((region) => [region.id, region]));

  for (const node of nodes) {
    const target = targets.get(node.id);
    const region = regionById.get(nodeRegion.get(node.id) ?? '');
    if (!target || !region) continue;
    region.extent = Math.max(
      region.extent,
      Math.hypot(target.x - region.x, target.y - region.y, target.z - region.z)
    );
  }

  const edgeAccumulators = new Map<string, EdgeAccumulator>();
  for (const link of links) {
    const sourceId = endpointId(link.source);
    const targetId = endpointId(link.target);
    if (!sourceId || !targetId || !nodeById.has(sourceId) || !nodeById.has(targetId)) continue;
    const sourceRegion = nodeRegion.get(sourceId);
    const targetRegion = nodeRegion.get(targetId);
    if (!sourceRegion || !targetRegion) continue;
    if (sourceRegion === targetRegion) {
      const region = regionById.get(sourceRegion);
      if (region) region.internalEdgeCount += 1;
      continue;
    }
    const source = sourceRegion < targetRegion ? sourceRegion : targetRegion;
    const target = sourceRegion < targetRegion ? targetRegion : sourceRegion;
    const key = JSON.stringify([source, target]);
    const weight = Number.isFinite(link.weight) ? Number(link.weight) : 0;
    const accumulator = edgeAccumulators.get(key) ?? {
      source,
      target,
      count: 0,
      weightTotal: 0,
      maxWeight: 0
    };
    accumulator.count += 1;
    accumulator.weightTotal += weight;
    accumulator.maxWeight = Math.max(accumulator.maxWeight, weight);
    edgeAccumulators.set(key, accumulator);
  }

  const edges = [...edgeAccumulators.values()]
    .map((edge): SemanticRegionEdge => ({
      source: edge.source,
      target: edge.target,
      count: edge.count,
      weight: edge.weightTotal / Math.max(1, edge.count),
      maxWeight: edge.maxWeight
    }))
    .sort((left, right) =>
      right.count - left.count
      || left.source.localeCompare(right.source)
      || left.target.localeCompare(right.target)
    );

  return { regions, edges, nodeRegion };
}

// Select a connected real-edge backbone, then spend the remaining budget on local links.
export function selectSemanticRegionEdges(
  atlas: SemanticRegionAtlas,
  maxEdges: number
): SemanticRegionEdge[] {
  if (maxEdges <= 0 || atlas.edges.length === 0) return [];
  if (atlas.edges.length <= maxEdges) return [...atlas.edges];
  const positions = new Map<string, SemanticRegionPosition>(
    atlas.regions.map((region) => [
      region.id,
      { x: region.x, y: region.y, z: region.z }
    ])
  );
  const parents = new Map(atlas.regions.map((region) => [region.id, region.id]));

  // Find one disjoint-set root while compressing the visited path.
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

  const affinityRanked = [...atlas.edges].sort((left, right) =>
    right.count - left.count
    || right.maxWeight - left.maxWeight
    || left.source.localeCompare(right.source)
    || left.target.localeCompare(right.target)
  );
  const selected: SemanticRegionEdge[] = [];
  const selectedKeys = new Set<string>();
  for (const edge of affinityRanked) {
    const sourceRoot = findRoot(edge.source);
    const targetRoot = findRoot(edge.target);
    if (sourceRoot === targetRoot) continue;
    parents.set(targetRoot, sourceRoot);
    selected.push(edge);
    selectedKeys.add(JSON.stringify([edge.source, edge.target]));
  }
  if (selected.length >= maxEdges) return selected;

  const localRanked = atlas.edges
    .filter((edge) => !selectedKeys.has(JSON.stringify([edge.source, edge.target])))
    .sort((left, right) => {
      const leftSource = positions.get(left.source)!;
      const leftTarget = positions.get(left.target)!;
      const rightSource = positions.get(right.source)!;
      const rightTarget = positions.get(right.target)!;
      const leftDistance = Math.hypot(
        leftSource.x - leftTarget.x,
        leftSource.y - leftTarget.y,
        leftSource.z - leftTarget.z
      );
      const rightDistance = Math.hypot(
        rightSource.x - rightTarget.x,
        rightSource.y - rightTarget.y,
        rightSource.z - rightTarget.z
      );
      return leftDistance - rightDistance
        || right.count - left.count
        || left.source.localeCompare(right.source)
        || left.target.localeCompare(right.target);
    });
  selected.push(...localRanked.slice(0, maxEdges - selected.length));
  return selected;
}
