import type { GraphEdge, GraphNode } from '$lib/types';

// Golden-angle spacing avoids radial bands while remaining deterministic.
const GOLDEN_ANGLE = Math.PI * (3 - Math.sqrt(5));

// Represents a graph node with its stable atlas position and connectivity.
export interface AtlasNode extends GraphNode {
  degree: number;
  x: number;
  y: number;
}

// Represents the rectangular world-space extent of an atlas layout.
export interface AtlasBounds {
  maxX: number;
  maxY: number;
  minX: number;
  minY: number;
}

// Contains deterministic positions and validated graph connectivity.
export interface AtlasLayout {
  bounds: AtlasBounds;
  edges: GraphEdge[];
  neighbors: Map<string, Set<string>>;
  nodeById: Map<string, AtlasNode>;
  nodes: AtlasNode[];
}

// Represents the canvas transform required to fit world bounds in a viewport.
export interface AtlasView {
  offsetX: number;
  offsetY: number;
  scale: number;
}

// Build a stable category/community atlas without a live force simulation.
export function buildAtlasLayout(nodes: GraphNode[], edges: GraphEdge[]): AtlasLayout {
  const knownIds = new Set(nodes.map((node) => node.id));
  const validEdges = edges.filter((edge) => knownIds.has(edge.source) && knownIds.has(edge.target));
  const neighbors = buildNeighborIndex(nodes, validEdges);
  const groups = new Map<string, GraphNode[]>();

  for (const node of nodes) {
    const group = node.community_id == null ? `category:${node.category || 'general'}` : `community:${node.community_id}`;
    const groupNodes = groups.get(group) ?? [];
    groupNodes.push(node);
    groups.set(group, groupNodes);
  }

  const orderedGroups = [...groups.entries()].sort(
    ([leftName, left], [rightName, right]) => right.length - left.length || leftName.localeCompare(rightName)
  );
  const positioned: AtlasNode[] = [];
  const groupOrbit = Math.max(280, 105 * Math.sqrt(Math.max(1, orderedGroups.length)));

  orderedGroups.forEach(([groupName, groupNodes], groupIndex) => {
    const groupAngle = groupIndex * GOLDEN_ANGLE;
    const groupDistance = orderedGroups.length === 1 ? 0 : groupOrbit * (0.55 + 0.45 * Math.sqrt(groupIndex / orderedGroups.length));
    const centerX = Math.cos(groupAngle) * groupDistance;
    const centerY = Math.sin(groupAngle) * groupDistance;
    const orderedNodes = [...groupNodes].sort(
      (left, right) => right.importance - left.importance || left.id.localeCompare(right.id)
    );

    orderedNodes.forEach((node, nodeIndex) => {
      const localAngle = nodeIndex * GOLDEN_ANGLE + stablePhase(groupName);
      const localDistance = nodeIndex === 0 ? 0 : 15 * Math.sqrt(nodeIndex);
      positioned.push({
        ...node,
        degree: neighbors.get(node.id)?.size ?? 0,
        x: centerX + Math.cos(localAngle) * localDistance,
        y: centerY + Math.sin(localAngle) * localDistance
      });
    });
  });

  const bounds = measureBounds(positioned);
  return {
    bounds,
    edges: validEdges,
    neighbors,
    nodeById: new Map(positioned.map((node) => [node.id, node])),
    nodes: positioned
  };
}

// Compute a padded view transform that reveals every positioned node.
export function fitAtlasView(bounds: AtlasBounds, width: number, height: number, padding = 48): AtlasView {
  const worldWidth = Math.max(1, bounds.maxX - bounds.minX);
  const worldHeight = Math.max(1, bounds.maxY - bounds.minY);
  const usableWidth = Math.max(1, width - padding * 2);
  const usableHeight = Math.max(1, height - padding * 2);
  const scale = Math.min(2.4, Math.max(0.08, Math.min(usableWidth / worldWidth, usableHeight / worldHeight)));
  const centerX = (bounds.minX + bounds.maxX) / 2;
  const centerY = (bounds.minY + bounds.maxY) / 2;
  return {
    offsetX: width / 2 - centerX * scale,
    offsetY: height / 2 - centerY * scale,
    scale
  };
}

// Find the closest node within a screen-space selection radius.
export function hitTestAtlas(
  nodes: AtlasNode[],
  view: AtlasView,
  screenX: number,
  screenY: number,
  radius = 12
): AtlasNode | null {
  let closest: AtlasNode | null = null;
  let closestDistance = radius * radius;
  for (const node of nodes) {
    const dx = node.x * view.scale + view.offsetX - screenX;
    const dy = node.y * view.scale + view.offsetY - screenY;
    const distance = dx * dx + dy * dy;
    if (distance <= closestDistance) {
      closest = node;
      closestDistance = distance;
    }
  }
  return closest;
}

// Build symmetric adjacency sets for every node in the loaded graph.
function buildNeighborIndex(nodes: GraphNode[], edges: GraphEdge[]): Map<string, Set<string>> {
  const neighbors = new Map(nodes.map((node) => [node.id, new Set<string>()]));
  for (const edge of edges) {
    neighbors.get(edge.source)?.add(edge.target);
    neighbors.get(edge.target)?.add(edge.source);
  }
  return neighbors;
}

// Produce a stable angular phase from a group name.
function stablePhase(value: string): number {
  let hash = 0;
  for (let index = 0; index < value.length; index += 1) {
    hash = (hash * 31 + value.charCodeAt(index)) | 0;
  }
  return (Math.abs(hash) % 6283) / 1000;
}

// Measure positioned nodes while preserving a useful empty-state extent.
function measureBounds(nodes: AtlasNode[]): AtlasBounds {
  if (nodes.length === 0) {
    return { maxX: 1, maxY: 1, minX: -1, minY: -1 };
  }
  return nodes.reduce<AtlasBounds>(
    (bounds, node) => ({
      maxX: Math.max(bounds.maxX, node.x),
      maxY: Math.max(bounds.maxY, node.y),
      minX: Math.min(bounds.minX, node.x),
      minY: Math.min(bounds.minY, node.y)
    }),
    { maxX: -Infinity, maxY: -Infinity, minX: Infinity, minY: Infinity }
  );
}
