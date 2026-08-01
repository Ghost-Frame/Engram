// The memory graph renders Kleos relationships as a direct interactive 3D force layout.
import { useEffect, useRef, useState, type FormEvent } from 'react';
import {
  getCommunities,
  getMemoryDetail,
  getMemoryGraph,
  getStats,
  searchGraph,
  // CategoryCount describes one category total displayed in the graph legend.
  type CategoryCount,
  // GraphSearchResult describes one memory returned by graph search.
  type GraphSearchResult,
  // MemoryDetail describes the selected memory inspector payload.
  type MemoryDetail
} from '$lib/api/graph';
import { selectRenderEdges } from '$lib/graph/selectRenderEdges';
import {
  buildGalaxyTargets,
  // GalaxyTarget carries the stable guide position consumed by the live force.
  type GalaxyTarget
} from '$lib/graph/galaxyLayout';
import {
  buildSemanticRegions,
  selectSemanticRegionEdges,
  // SemanticRegion is one selectable overview landmark backed by real memories.
  type SemanticRegion,
  // SemanticRegionAtlas retains the lossless memory-to-region lookup.
  type SemanticRegionAtlas
} from '$lib/graph/semanticRegions';
import './graph.css';

// ── Working types ──────────────────────────────────────────
// The graph mutates nodes in place (neighbors/links/positions), so these are
// looser than the API GraphNode/GraphEdge and own the runtime-only fields.

interface GNode {
  id: string;
  label: string;
  type: string;
  category: string;
  importance: number;
  group?: string;
  size: number;
  source: string;
  created_at: string;
  is_static: boolean;
  content: string;
  source_count?: number;
  community_id?: number;
  decay_score?: number;
  x?: number;
  y?: number;
  z?: number;
  vx?: number;
  vy?: number;
  vz?: number;
  neighbors?: GNode[];
  links?: GLink[];
}

// GLink carries the mutable node references used by the force simulation.
interface GLink {
  source: string | GNode;
  target: string | GNode;
  type: string;
  weight: number;
}

// RegionSummary is the lightweight drilldown context surfaced in the control panel.
interface RegionSummary {
  label: string;
  memberCount: number;
  internalEdgeCount: number;
}

// ── Constants ──────────────────────────────────────────────

const COMMUNITY_COLORS = [
  '#00d7ff', '#34d8ff', '#00c5ef', '#7aa2ff',
  '#00f0c8', '#b46cff', '#5e91ff', '#25d9c1'
];

const CATEGORY_FALLBACK: Record<string, string> = {
  session: '#00d7ff', general: '#00d7ff', decision: '#b46cff', task: '#00f0c8',
  state: '#7aa2ff', discovery: '#00c5ef', reference: '#7aa2ff',
  issue: '#34d8ff', preference: '#b46cff', credential: '#7aa2ff',
  infrastructure: '#00c5ef', incident: '#34d8ff', directive: '#00f0c8'
};

// Resolve the group identity used to distinguish local links from arm bridges.
function galaxyGroupId(node: GNode): string {
  if (node.community_id != null) return `community:${node.community_id}`;
  return `category:${node.category || 'general'}`;
}

// Report whether a simulated link joins nodes inside the same semantic cluster.
function linkStaysWithinGroup(link: GLink): boolean {
  if (typeof link.source !== 'object' || typeof link.target !== 'object') return false;
  return galaxyGroupId(link.source as GNode) === galaxyGroupId(link.target as GNode);
}

// ── Textures (verbatim from the old graph) ─────────────────

function createOrganismTexture(THREE: any, seed: number) {
  const size = 128;
  const c = document.createElement('canvas');
  c.width = size;
  c.height = size;
  const ctx = c.getContext('2d')!;
  const cx = size / 2;
  const cy = size / 2;

  // Outer corona / atmosphere
  const corona = ctx.createRadialGradient(cx, cy, 0, cx, cy, cx);
  corona.addColorStop(0, 'rgba(255,255,255,0)');
  corona.addColorStop(0.55, 'rgba(255,255,255,0)');
  corona.addColorStop(0.7, 'rgba(255,255,255,0.06)');
  corona.addColorStop(0.85, 'rgba(255,255,255,0.03)');
  corona.addColorStop(1, 'rgba(255,255,255,0)');
  ctx.fillStyle = corona;
  ctx.fillRect(0, 0, size, size);

  // Membrane - soft outer ring
  ctx.beginPath();
  ctx.arc(cx, cy, 28, 0, Math.PI * 2);
  ctx.strokeStyle = 'rgba(255,255,255,0.12)';
  ctx.lineWidth = 1.5;
  ctx.stroke();

  // Inner organelles - tiny bright dots scattered inside
  const rng = (n: number) => {
    let s = seed + n;
    s = (s * 1103515245 + 12345) & 0x7fffffff;
    return (s % 1000) / 1000;
  };
  const organelleCount = 4 + Math.floor(rng(0) * 6);
  for (let i = 0; i < organelleCount; i++) {
    const angle = rng(i * 3 + 1) * Math.PI * 2;
    const dist = 6 + rng(i * 3 + 2) * 16;
    const ox = cx + Math.cos(angle) * dist;
    const oy = cy + Math.sin(angle) * dist;
    const r = 1 + rng(i * 3 + 3) * 2.5;
    const og = ctx.createRadialGradient(ox, oy, 0, ox, oy, r);
    og.addColorStop(0, `rgba(255,255,255,${0.6 + rng(i * 5) * 0.4})`);
    og.addColorStop(1, 'rgba(255,255,255,0)');
    ctx.fillStyle = og;
    ctx.beginPath();
    ctx.arc(ox, oy, r, 0, Math.PI * 2);
    ctx.fill();
  }

  // Nucleus - bright core with strong glow
  const core = ctx.createRadialGradient(cx, cy, 0, cx, cy, 18);
  core.addColorStop(0, 'rgba(255,255,255,1)');
  core.addColorStop(0.15, 'rgba(255,255,255,0.95)');
  core.addColorStop(0.35, 'rgba(255,255,255,0.6)');
  core.addColorStop(0.6, 'rgba(255,255,255,0.25)');
  core.addColorStop(0.8, 'rgba(255,255,255,0.1)');
  core.addColorStop(1, 'rgba(255,255,255,0)');
  ctx.fillStyle = core;
  ctx.fillRect(0, 0, size, size);

  // Inner filaments - curved lines like internal structure
  ctx.globalAlpha = 0.2;
  for (let i = 0; i < 3; i++) {
    const startAngle = rng(i * 7 + 10) * Math.PI * 2;
    const arcLen = 0.5 + rng(i * 7 + 11) * 1.5;
    const arcDist = 10 + rng(i * 7 + 12) * 14;
    ctx.beginPath();
    ctx.arc(cx, cy, arcDist, startAngle, startAngle + arcLen);
    ctx.strokeStyle = 'white';
    ctx.lineWidth = 0.8;
    ctx.stroke();
  }
  ctx.globalAlpha = 1;

  // Clip to circle -- eliminates square sprite boundary artifacts.
  ctx.globalCompositeOperation = 'destination-in';
  const mask = ctx.createRadialGradient(cx, cy, 0, cx, cy, cx);
  mask.addColorStop(0, 'rgba(255,255,255,1)');
  mask.addColorStop(0.85, 'rgba(255,255,255,1)');
  mask.addColorStop(1, 'rgba(255,255,255,0)');
  ctx.fillStyle = mask;
  ctx.fillRect(0, 0, size, size);
  ctx.globalCompositeOperation = 'source-over';

  return new THREE.CanvasTexture(c);
}

// Create the halo texture used to mark static memories.
function createRingTexture(THREE: any) {
  const c = document.createElement('canvas');
  c.width = 64;
  c.height = 64;
  const ctx = c.getContext('2d')!;
  const g = ctx.createRadialGradient(32, 32, 18, 32, 32, 32);
  g.addColorStop(0, 'rgba(255,255,255,0)');
  g.addColorStop(0.6, 'rgba(255,255,255,0)');
  g.addColorStop(0.78, 'rgba(255,215,0,0.15)');
  g.addColorStop(0.88, 'rgba(255,215,0,0.06)');
  g.addColorStop(1, 'rgba(255,215,0,0)');
  ctx.fillStyle = g;
  ctx.fillRect(0, 0, 64, 64);
  return new THREE.CanvasTexture(c);
}

// ── Galactic guide force ──────────────────────────────────

// Pull live nodes toward stable spiral targets without pinning force physics.
function makeGalaxyGuideForce(targets: ReadonlyMap<string, GalaxyTarget>, strength: number) {
  let nodes: GNode[] = [];
  const force: any = (alpha: number) => {
    for (const node of nodes) {
      const target = targets.get(node.id);
      if (!target) continue;
      node.vx = (node.vx ?? 0) + (target.x - (node.x ?? 0)) * strength * alpha;
      node.vy = (node.vy ?? 0) + (target.y - (node.y ?? 0)) * strength * alpha;
      node.vz = (node.vz ?? 0) + (target.z - (node.z ?? 0)) * strength * alpha;
    }
  };
  force.initialize = (n: GNode[]) => {
    nodes = n;
  };
  return force;
}

// Preserve readable arms while allowing stored links to deform the structure.
const GALAXY_GUIDE_STRENGTH = 1.15;

// Switch to fixed-cost GPU batches before the live dataset reaches sprite lag.
const GPU_BATCH_THRESHOLD = 600;

// Leave enough negative space for the graph to read as a galaxy instead of a chart.
const FIT_PADDING_FACTOR = 1.3;

// FitPoint is the finite coordinate contract used by projected camera fitting.
export interface FitPoint {
  x: number;
  y: number;
  z: number;
}

// ProjectedCameraFit describes a camera move that tightly frames a flat atlas.
export interface ProjectedCameraFit {
  center: FitPoint;
  distance: number;
  position: FitPoint;
}

// Fit a point cloud in the camera's current orientation without treating a
// flattened galaxy as a sphere and wasting most of a widescreen viewport.
export function calculateProjectedCameraFit(
  points: readonly FitPoint[],
  cameraPosition: FitPoint,
  verticalFovDegrees: number,
  aspect: number,
  padding = FIT_PADDING_FACTOR
): ProjectedCameraFit | null {
  if (!points.length) return null;
  let minX = Number.POSITIVE_INFINITY;
  let maxX = Number.NEGATIVE_INFINITY;
  let minY = Number.POSITIVE_INFINITY;
  let maxY = Number.NEGATIVE_INFINITY;
  let minZ = Number.POSITIVE_INFINITY;
  let maxZ = Number.NEGATIVE_INFINITY;
  for (const point of points) {
    minX = Math.min(minX, point.x);
    maxX = Math.max(maxX, point.x);
    minY = Math.min(minY, point.y);
    maxY = Math.max(maxY, point.y);
    minZ = Math.min(minZ, point.z);
    maxZ = Math.max(maxZ, point.z);
  }
  const center = {
    x: (minX + maxX) / 2,
    y: (minY + maxY) / 2,
    z: (minZ + maxZ) / 2
  };

  let outwardX = cameraPosition.x - center.x;
  let outwardY = cameraPosition.y - center.y;
  let outwardZ = cameraPosition.z - center.z;
  const outwardLength = Math.hypot(outwardX, outwardY, outwardZ) || 1;
  outwardX /= outwardLength;
  outwardY /= outwardLength;
  outwardZ /= outwardLength;

  // Use Z as the fallback up axis when the camera looks almost vertically.
  const upX = 0;
  const upY = Math.abs(outwardY) < 0.98 ? 1 : 0;
  const upZ = Math.abs(outwardY) < 0.98 ? 0 : 1;
  let rightX = upY * outwardZ - upZ * outwardY;
  let rightY = upZ * outwardX - upX * outwardZ;
  let rightZ = upX * outwardY - upY * outwardX;
  const rightLength = Math.hypot(rightX, rightY, rightZ) || 1;
  rightX /= rightLength;
  rightY /= rightLength;
  rightZ /= rightLength;
  const screenUpX = outwardY * rightZ - outwardZ * rightY;
  const screenUpY = outwardZ * rightX - outwardX * rightZ;
  const screenUpZ = outwardX * rightY - outwardY * rightX;

  let horizontalExtent = 0;
  let verticalExtent = 0;
  let nearDepth = 0;
  for (const point of points) {
    const x = point.x - center.x;
    const y = point.y - center.y;
    const z = point.z - center.z;
    horizontalExtent = Math.max(horizontalExtent, Math.abs(x * rightX + y * rightY + z * rightZ));
    verticalExtent = Math.max(
      verticalExtent,
      Math.abs(x * screenUpX + y * screenUpY + z * screenUpZ)
    );
    nearDepth = Math.max(nearDepth, x * outwardX + y * outwardY + z * outwardZ);
  }

  const halfFov = (Math.max(1, verticalFovDegrees) * Math.PI) / 360;
  const tangent = Math.max(0.1, Math.tan(halfFov));
  const safeAspect = Math.max(0.5, aspect);
  const projectedDistance = Math.max(
    verticalExtent / tangent,
    horizontalExtent / (tangent * safeAspect)
  );
  const distance = Math.max(120, (projectedDistance + nearDepth) * padding);
  return {
    center,
    distance,
    position: {
      x: center.x + outwardX * distance,
      y: center.y + outwardY * distance,
      z: center.z + outwardZ * distance
    }
  };
}

// ── Cosmic scene ───────────────────────────────────────────

// GalaxyBackdropControls adjusts decorative depth between overview and memory detail.
interface GalaxyBackdropControls {
  setDetail: (active: boolean) => void;
  dispose: () => void;
}

// Create the soft point texture shared by the fixed-cost backdrop clouds.
function createGalaxyPointTexture(THREE: any) {
  const canvas = document.createElement('canvas');
  canvas.width = 64;
  canvas.height = 64;
  const context = canvas.getContext('2d')!;
  const glow = context.createRadialGradient(32, 32, 0, 32, 32, 32);
  glow.addColorStop(0, 'rgba(255,255,255,1)');
  glow.addColorStop(0.16, 'rgba(255,255,255,0.92)');
  glow.addColorStop(0.48, 'rgba(255,255,255,0.24)');
  glow.addColorStop(1, 'rgba(255,255,255,0)');
  context.fillStyle = glow;
  context.fillRect(0, 0, 64, 64);
  return new THREE.CanvasTexture(canvas);
}

// Build a fixed-cost stellar underlay from real atlas density plus a quiet star field.
function addGalaxyBackdrop(
  THREE: any,
  scene: any,
  targets: ReadonlyMap<string, GalaxyTarget>
): GalaxyBackdropControls {
  let seed = 0x4b4c454f;
  // Advance a stable generator so repeated visits keep the same composition.
  const nextRandom = () => {
    seed = (seed * 1664525 + 1013904223) >>> 0;
    return seed / 0x100000000;
  };

  const pointTexture = createGalaxyPointTexture(THREE);
  const mainTargets = [...targets.values()].filter((target) => target.inMainComponent);
  const atlasRadius = Math.max(
    260,
    ...mainTargets.map((target) => Math.hypot(target.x, target.y))
  );
  const starCount = 720;
  const starPositions = new Float32Array(starCount * 3);
  const starColors = new Float32Array(starCount * 3);
  for (let index = 0; index < starCount; index++) {
    starPositions[index * 3] = (nextRandom() - 0.5) * atlasRadius * 3.8;
    starPositions[index * 3 + 1] = (nextRandom() - 0.5) * atlasRadius * 2.4;
    starPositions[index * 3 + 2] = -180 - nextRandom() * atlasRadius * 1.8;
    const brightness = 0.24 + nextRandom() * 0.76;
    starColors[index * 3] = brightness * 0.72;
    starColors[index * 3 + 1] = brightness * 0.9;
    starColors[index * 3 + 2] = brightness;
  }
  const starGeometry = new THREE.BufferGeometry();
  starGeometry.setAttribute('position', new THREE.BufferAttribute(starPositions, 3));
  starGeometry.setAttribute('color', new THREE.BufferAttribute(starColors, 3));
  const starMaterial = new THREE.PointsMaterial({
    size: 1.45,
    map: pointTexture,
    vertexColors: true,
    transparent: true,
    opacity: 0.54,
    sizeAttenuation: true,
    depthWrite: false,
    depthTest: false,
    blending: THREE.AdditiveBlending
  });
  const starPoints = new THREE.Points(starGeometry, starMaterial);
  starPoints.renderOrder = -30;
  scene.add(starPoints);

  // Derive the haze from live node coordinates so it reinforces actual density.
  const nebulaCount = Math.min(3600, mainTargets.length);
  const nebulaPositions = new Float32Array(nebulaCount * 3);
  const nebulaColors = new Float32Array(nebulaCount * 3);
  const nebulaPalette = [
    new THREE.Color('#00d7ff'),
    new THREE.Color('#00c5ef'),
    new THREE.Color('#7aa2ff'),
    new THREE.Color('#00f0c8')
  ];
  for (let index = 0; index < nebulaCount; index++) {
    const sourceIndex = Math.floor((index * mainTargets.length) / nebulaCount);
    const source = mainTargets[sourceIndex];
    const spread = 5 + nextRandom() * 18;
    const angle = nextRandom() * Math.PI * 2;
    nebulaPositions[index * 3] = source.x + Math.cos(angle) * spread;
    nebulaPositions[index * 3 + 1] = source.y + Math.sin(angle) * spread * 0.68;
    nebulaPositions[index * 3 + 2] = source.z - 54 - nextRandom() * 54;
    const color =
      nebulaPalette[(source.arm + (index % 29 === 0 ? 3 : 0)) % nebulaPalette.length];
    const intensity = source.diffuse
      ? 0.2 + nextRandom() * 0.24
      : 0.32 + nextRandom() * 0.34;
    nebulaColors[index * 3] = color.r * intensity;
    nebulaColors[index * 3 + 1] = color.g * intensity;
    nebulaColors[index * 3 + 2] = color.b * intensity;
  }
  const nebulaGeometry = new THREE.BufferGeometry();
  nebulaGeometry.setAttribute('position', new THREE.BufferAttribute(nebulaPositions, 3));
  nebulaGeometry.setAttribute('color', new THREE.BufferAttribute(nebulaColors, 3));
  const nebulaMaterial = new THREE.PointsMaterial({
    size: 3.8,
    map: pointTexture,
    vertexColors: true,
    transparent: true,
    opacity: 0.09,
    sizeAttenuation: true,
    depthWrite: false,
    depthTest: false,
    blending: THREE.AdditiveBlending
  });
  const nebulaPoints = new THREE.Points(nebulaGeometry, nebulaMaterial);
  nebulaPoints.renderOrder = -20;
  scene.add(nebulaPoints);

  return {
    // Recede decorative points so exact memories own the detail layer.
    setDetail: (active: boolean) => {
      starMaterial.opacity = active ? 0.16 : 0.54;
      nebulaMaterial.opacity = active ? 0.025 : 0.09;
    },
    // Release every GPU resource created for the backdrop.
    dispose: () => {
      scene.remove(starPoints, nebulaPoints);
      starGeometry.dispose();
      starMaterial.dispose();
      nebulaGeometry.dispose();
      nebulaMaterial.dispose();
      pointTexture.dispose();
    }
  };
}

// ── Component ──────────────────────────────────────────────

export function Graph() {
  const containerRef = useRef<HTMLDivElement>(null);
  const startedRef = useRef(false);
  // Mirror of showSearchResults read by the graph's onBackgroundClick closure.
  const showSearchResultsRef = useRef(false);
  // Imperative handle: UI controls call into the live graph through this.
  const apiRef = useRef<{
    setWeight: (v: number) => void;
    setLabels: (v: boolean) => void;
    setClusters: (v: boolean) => void;
    fitView: () => void;
    zoomToNode: (id: number | string) => void;
    runSearch: (q: string) => Promise<GraphSearchResult[]>;
    closePanel: () => void;
  } | null>(null);

  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState('');
  const [nodeCount, setNodeCount] = useState(0);
  const [edgeCount, setEdgeCount] = useState(0);
  const [drawnEdgeCount, setDrawnEdgeCount] = useState(0);
  const [regionCount, setRegionCount] = useState(0);
  const [regionEdgeCount, setRegionEdgeCount] = useState(0);
  const [componentCount, setComponentCount] = useState(0);
  const [mainComponentCount, setMainComponentCount] = useState(0);
  const [dbSizeMb, setDbSizeMb] = useState<number | undefined>(undefined);
  const [categories, setCategories] = useState<CategoryCount[]>([]);
  const [searchQuery, setSearchQuery] = useState('');
  const [searchResults, setSearchResults] = useState<GraphSearchResult[]>([]);
  const [showSearchResults, setShowSearchResults] = useState(false);
  const [selectedMemory, setSelectedMemory] = useState<MemoryDetail | null>(null);
  const [sidePanelOpen, setSidePanelOpen] = useState(false);
  const [showLabels, setShowLabels] = useState(false);
  const [weightThreshold, setWeightThreshold] = useState(0);
  const [clusterEnabled, setClusterEnabled] = useState(true);
  const [viewMode, setViewMode] = useState<'overview' | 'memories'>('overview');
  const [regionSummary, setRegionSummary] = useState<RegionSummary | null>(null);

  // ── Graph lifecycle (init once, imperative) ──────────────
  useEffect(() => {
    // StrictMode mounts effects twice in dev; build the WebGL graph only once.
    if (startedRef.current) {
      return;
    }
    startedRef.current = true;
    const container = containerRef.current;
    if (!container) {
      return;
    }

    let destroyed = false;
    let graphInstance: any = null;
    let threeRef: any = null;
    let resizeHandler: (() => void) | null = null;
    let cloudRaf: number | undefined;
    let removeBatchInteractions: (() => void) | null = null;
    let galaxyBackdrop: GalaxyBackdropControls | null = null;
    // Track custom GPU resources because removing Three.js objects does not
    // release their textures or sprite materials automatically on remount.
    const ownedTextures = new Set<any>();
    const ownedSpriteMaterials = new Set<any>();

    // Effect-local mutable graph state (mirrors the old component scope).
    const highlightNodes = new Set<GNode>();
    const highlightLinks = new Set<GLink>();
    const searchHighlights = new Set<string>();
    let hoverNode: GNode | null = null;
    let pinnedNode: GNode | null = null;
    let weightThresholdLocal = 0;
    const nodeSprites = new Map<string, { material: any; baseSize: number; sprite: any }>();
    const nodeLabels = new Map<string, any>();
    const nodeMap = new Map<string, GNode>();
    let graphNodes: GNode[] = [];
    let renderEdges: GLink[] = [];
    let batchedRendering = false;
    let pointGeom: any = null;
    let pointMat: any = null;
    let nodeCloud: any = null;
    let pointBaseSizes = new Float32Array();
    let linkGeom: any = null;
    let linkMat: any = null;
    let linkLines: any = null;
    let highlightLinkGeom: any = null;
    let highlightLinkMat: any = null;
    let highlightLinkLines: any = null;
    let regionPointGeom: any = null;
    let regionPointMat: any = null;
    let regionPoints: any = null;
    let regionLineGeom: any = null;
    let regionLineMat: any = null;
    let regionLines: any = null;
    let detailLinkGeom: any = null;
    let detailLinkMat: any = null;
    let detailLinkLines: any = null;
    let refreshRegionDetailLinks: ((regionId: string | null) => void) | null = null;
    let semanticAtlas: SemanticRegionAtlas | null = null;
    let overviewActive = false;
    let activeRegionId: string | null = null;
    let galaxyTargets = new Map<string, GalaxyTarget>();
    let semanticColorEnabled = clusterEnabled;

    // ── Color helpers ──────────────────────────────────────
    const getNodeColor = (node: GNode): string => {
      if (searchHighlights.has(node.id)) return '#ffd700';
      if (!semanticColorEnabled) {
        const target = galaxyTargets.get(node.id);
        if (batchedRendering && target && !target.inMainComponent) {
          return target.depth === 0 ? '#607d91' : '#31495c';
        }
        return '#19aee8';
      }
      if (node.category && CATEGORY_FALLBACK[node.category]) return CATEGORY_FALLBACK[node.category];
      if (node.community_id != null) return COMMUNITY_COLORS[node.community_id % COMMUNITY_COLORS.length];
      return '#4fc3f7';
    };
    const getNodeOpacity = (node: GNode): number => {
      if (highlightNodes.has(node) || searchHighlights.has(node.id)) return 1.0;
      const decay = node.decay_score ?? 5;
      const baseOpacity = Math.max(0.5, Math.min(1.0, decay / 6));
      const target = galaxyTargets.get(node.id);
      if (
        activeRegionId
        && semanticAtlas
        && semanticAtlas.nodeRegion.get(node.id) !== activeRegionId
      ) {
        return 0;
      }
      if (activeRegionId && semanticAtlas) {
        return Math.min(1, 0.58 + baseOpacity * 0.4);
      }
      if (hoverNode || pinnedNode) {
        return target?.inMainComponent === false ? 0.025 : 0.06;
      }
      // Disconnected components remain visible and selectable as muted islands.
      return batchedRendering && target && !target.inMainComponent
        ? target.depth === 0
          ? Math.min(0.52, baseOpacity)
          : Math.min(0.18, baseOpacity)
        : batchedRendering
          ? target?.diffuse
            ? Math.min(0.44, 0.11 + baseOpacity * 0.34)
            : Math.min(0.74, 0.2 + baseOpacity * 0.54)
          : baseOpacity;
    };
    const getLinkColor = (link: GLink): string => {
      const src =
        typeof link.source === 'object'
          ? (link.source as GNode)
          : nodeMap.get(link.source);
      return src ? getNodeColor(src) : '#4fc3f7';
    };
    const withAlpha = (color: string, alpha: number): string => {
      const clamped = Math.max(0, Math.min(1, alpha));
      const hex = color.startsWith('#') ? color.slice(1) : color;
      if (hex.length !== 6) return color;
      const value = Number.parseInt(hex, 16);
      if (Number.isNaN(value)) return color;
      const r = (value >> 16) & 255;
      const g = (value >> 8) & 255;
      const b = value & 255;
      return `rgba(${r},${g},${b},${clamped})`;
    };
    const getLinkAlpha = (link: GLink): number => {
      if (highlightLinks.has(link)) return Math.max(0.3, (link.weight ?? 0.5) * 0.8);
      if (activeRegionId && semanticAtlas) {
        const source = resolveEndpoint(link.source);
        const target = resolveEndpoint(link.target);
        if (
          !source
          || !target
          || semanticAtlas.nodeRegion.get(source.id) !== activeRegionId
          || semanticAtlas.nodeRegion.get(target.id) !== activeRegionId
        ) {
          return 0;
        }
      }
      if (hoverNode || pinnedNode) return 0.0002;
      if ((link.weight ?? 0) >= weightThresholdLocal) {
        return batchedRendering
          ? 0.025 + (link.weight ?? 0) * 0.045
          : 0.012 + (link.weight ?? 0) * 0.035;
      }
      return 0;
    };
    const getVisibleLinkColor = (link: GLink): string => {
      const alpha = getLinkAlpha(link);
      if (alpha <= 0) return 'rgba(0,0,0,0)';
      return withAlpha(getLinkColor(link), alpha);
    };
    // Resolve an endpoint before or after d3-force replaces its string ID.
    const resolveEndpoint = (endpoint: string | GNode): GNode | undefined => {
      return typeof endpoint === 'object' ? endpoint : nodeMap.get(endpoint);
    };
    // Rebuild the small interaction layer from every real edge touching the active memory.
    const refreshHighlightGeometry = () => {
      if (!batchedRendering || !highlightLinkGeom || !threeRef) return;
      const activeLinks = [...highlightLinks]
        .filter((link) => (link.weight ?? 0) >= weightThresholdLocal)
        .sort((left, right) =>
          (right.weight ?? 0) - (left.weight ?? 0)
          || String(left.source).localeCompare(String(right.source))
          || String(left.target).localeCompare(String(right.target))
        )
        .slice(0, 128);
      const positions = new Float32Array(activeLinks.length * 6);
      activeLinks.forEach((link, index) => {
        const source = resolveEndpoint(link.source);
        const target = resolveEndpoint(link.target);
        const offset = index * 6;
        positions[offset] = source?.x ?? 0;
        positions[offset + 1] = source?.y ?? 0;
        positions[offset + 2] = source?.z ?? 0;
        positions[offset + 3] = target?.x ?? 0;
        positions[offset + 4] = target?.y ?? 0;
        positions[offset + 5] = target?.z ?? 0;
      });
      highlightLinkGeom.setAttribute(
        'position',
        new threeRef.BufferAttribute(positions, 3)
      );
      highlightLinkGeom.setDrawRange(0, activeLinks.length * 2);
      highlightLinkGeom.computeBoundingSphere();
      if (highlightLinkLines) highlightLinkLines.visible = activeLinks.length > 0;
    };
    // Refresh the fixed-cost edge buffers or the richer small-graph objects.
    const refreshLinkVisuals = () => {
      if (!graphInstance) return;
      if (batchedRendering && linkGeom && threeRef) {
        const colorAttribute = linkGeom.getAttribute('aColor');
        const alphaAttribute = linkGeom.getAttribute('aAlpha');
        const color = new threeRef.Color();
        renderEdges.forEach((link, index) => {
          color.set('#2385a3');
          const alpha = getLinkAlpha(link);
          for (const endpointOffset of [0, 1]) {
            const vertex = index * 2 + endpointOffset;
            colorAttribute.setXYZ(vertex, color.r, color.g, color.b);
            alphaAttribute.setX(vertex, alpha);
          }
        });
        colorAttribute.needsUpdate = true;
        alphaAttribute.needsUpdate = true;
        refreshHighlightGeometry();
        return;
      }
      graphInstance
        .linkOpacity(1)
        .linkWidth((link: any) => {
          if (highlightLinks.has(link)) return Math.max(0.5, (link.weight ?? 0.5) * 2);
          if ((link.weight ?? 0) >= weightThresholdLocal) return 0.15;
          return 0;
        })
        .linkColor((link: any) => getVisibleLinkColor(link as GLink))
        .linkVisibility((link: any) => {
          if (highlightLinks.has(link)) return true;
          return (link.weight ?? 0) >= weightThresholdLocal;
        });
    };

    // Refresh the fixed-cost node buffers or the richer small-graph sprites.
    const updateNodeVisuals = () => {
      if (!threeRef) return;
      if (batchedRendering && pointGeom) {
        const colorAttribute = pointGeom.getAttribute('aColor');
        const opacityAttribute = pointGeom.getAttribute('aOpacity');
        const sizeAttribute = pointGeom.getAttribute('size');
        const color = new threeRef.Color();
        graphNodes.forEach((node, index) => {
          color.set(getNodeColor(node));
          colorAttribute.setXYZ(index, color.r, color.g, color.b);
          opacityAttribute.setX(index, getNodeOpacity(node));
          sizeAttribute.setX(
            index,
            pointBaseSizes[index] * (highlightNodes.has(node) || searchHighlights.has(node.id) ? 1.34 : 1)
          );
        });
        colorAttribute.needsUpdate = true;
        opacityAttribute.needsUpdate = true;
        sizeAttribute.needsUpdate = true;
        return;
      }
      nodeSprites.forEach((entry, id) => {
        const node = nodeMap.get(id);
        if (!node) return;
        entry.material.color.set(getNodeColor(node));
        entry.material.opacity = getNodeOpacity(node);
        const isHovered = highlightNodes.has(node);
        const scale = isHovered ? entry.baseSize * 1.3 : entry.baseSize;
        entry.sprite.scale.set(scale, scale, scale);
      });
    };

    const handleNodeHover = (node: GNode | null) => {
      highlightNodes.clear();
      highlightLinks.clear();
      if (node) {
        highlightNodes.add(node);
        node.neighbors?.forEach((n) => {
          if (n) highlightNodes.add(n);
        });
        node.links?.forEach((l) => {
          if ((l.weight ?? 0) >= weightThresholdLocal) highlightLinks.add(l);
        });
      }
      if (pinnedNode && pinnedNode !== node) {
        highlightNodes.add(pinnedNode);
        pinnedNode.neighbors?.forEach((n) => {
          if (n) highlightNodes.add(n);
        });
        pinnedNode.links?.forEach((l) => {
          if ((l.weight ?? 0) >= weightThresholdLocal) highlightLinks.add(l);
        });
      }
      hoverNode = node;
      updateNodeVisuals();
      refreshLinkVisuals();
    };

    const handleNodeClick = async (node: GNode) => {
      if (!node) return;
      pinnedNode = node;
      const memId = node.id.startsWith('m') ? node.id.slice(1) : node.id;
      try {
        const detail = await getMemoryDetail(Number.parseInt(memId, 10));
        if (destroyed) return;
        setSelectedMemory(detail);
        setSidePanelOpen(true);
        setShowSearchResults(false);
      } catch (e) {
        console.error('Failed to fetch memory:', e);
      }
    };

    const closePanel = () => {
      pinnedNode = null;
      highlightNodes.clear();
      highlightLinks.clear();
      searchHighlights.clear();
      updateNodeVisuals();
      refreshLinkVisuals();
      setSidePanelOpen(false);
      setSelectedMemory(null);
      setShowSearchResults(false);
    };

    // Switch fixed-cost GPU layers between region overview and exact-memory detail.
    const applySemanticZoom = (overview: boolean) => {
      if (!batchedRendering || !semanticAtlas) return;
      overviewActive = overview;
      if (overview) {
        activeRegionId = null;
        setRegionSummary(null);
      }
      if (regionPoints) regionPoints.visible = overview;
      if (regionLines) regionLines.visible = overview;
      if (nodeCloud) nodeCloud.visible = !overview;
      if (linkLines) linkLines.visible = false;
      if (overview) refreshRegionDetailLinks?.(null);
      if (highlightLinkLines) highlightLinkLines.visible = false;
      galaxyBackdrop?.setDetail(!overview);
      if (overview) handleNodeHover(null);
      updateNodeVisuals();
      setViewMode(overview ? 'overview' : 'memories');
    };

    // Enter one overview region and frame the original memories it represents.
    const focusRegion = (region: SemanticRegion) => {
      if (!graphInstance) return;
      handleNodeHover(null);
      activeRegionId = region.id;
      refreshRegionDetailLinks?.(region.id);
      setRegionSummary({
        label: region.label,
        memberCount: region.memberCount,
        internalEdgeCount: region.internalEdgeCount
      });
      applySemanticZoom(false);
      const distance = Math.max(105, Math.min(420, region.extent * 1.8 + 90));
      graphInstance.cameraPosition(
        { x: region.x, y: region.y, z: region.z + distance },
        { x: region.x, y: region.y, z: region.z },
        900
      );
    };

    // Move the camera toward one point without changing inspector state.
    const focusLoadedNode = (node: GNode) => {
      if (graphInstance && node.x != null) {
        activeRegionId = semanticAtlas?.nodeRegion.get(node.id) ?? null;
        refreshRegionDetailLinks?.(activeRegionId);
        const region = semanticAtlas?.regions.find(
          (candidate) => candidate.id === activeRegionId
        );
        setRegionSummary(region ? {
          label: region.label,
          memberCount: region.memberCount,
          internalEdgeCount: region.internalEdgeCount
        } : null);
        applySemanticZoom(false);
        const dist = 120;
        const hyp = Math.hypot(node.x!, node.y!, node.z!);
        const ratio = hyp > 0 ? 1 + dist / hyp : 1;
        graphInstance.cameraPosition(
          { x: node.x! * ratio, y: node.y! * ratio, z: node.z! * ratio },
          { x: node.x, y: node.y, z: node.z },
          1500
        );
      }
    };

    // Focus a loaded point, or open an off-screen search result directly.
    const zoomToNode = async (memId: number | string) => {
      const id = typeof memId === 'number' ? 'm' + memId : memId;
      const node = nodeMap.get(id);
      if (node && graphInstance && node.x != null) {
        focusLoadedNode(node);
        await handleNodeClick(node);
        return;
      }
      const numericId = Number.parseInt(id.startsWith('m') ? id.slice(1) : id, 10);
      if (Number.isNaN(numericId)) return;
      try {
        const detail = await getMemoryDetail(numericId);
        if (destroyed) return;
        setSelectedMemory(detail);
        setSidePanelOpen(true);
        setShowSearchResults(false);
      } catch (error) {
        console.error('Failed to fetch memory:', error);
      }
    };

    // Frame the custom point batch because ForceGraph cannot measure its bounds.
    const fitGalaxyView = () => {
      if (!graphInstance || graphNodes.length === 0) return;
      applySemanticZoom(true);
      const positioned = graphNodes.filter(
        (node) => Number.isFinite(node.x) && Number.isFinite(node.y) && Number.isFinite(node.z)
      );
      if (positioned.length === 0) return;
      const camera = graphInstance.camera();
      const fit = calculateProjectedCameraFit(
        positioned as FitPoint[],
        camera.position ?? { x: 0, y: 0, z: 1000 },
        camera.fov ?? 60,
        camera.aspect ?? 1
      );
      if (fit) graphInstance.cameraPosition(fit.position, fit.center, 900);
    };

    const runSearch = async (query: string): Promise<GraphSearchResult[]> => {
      if (!query.trim()) {
        searchHighlights.clear();
        updateNodeVisuals();
        return [];
      }
      try {
        const data = await searchGraph(query, 20);
        const results = data.results || [];
        searchHighlights.clear();
        results.forEach((r) => searchHighlights.add('m' + r.id));
        updateNodeVisuals();
        return results;
      } catch (e) {
        console.error('Search failed:', e);
        return [];
      }
    };

    // Initialize the Three.js force graph and publish its imperative control surface.
    async function init() {
      try {
        const [FG3D, THREE] = await Promise.all([
          import('3d-force-graph') as Promise<any>,
          import('three') as Promise<any>
        ]);
        const ForceGraph3D = FG3D.default;
        threeRef = THREE;

        const [graphData, commData, statsData] = await Promise.all([
          // Fetch the complete caller-scoped graph. The GPU overview performs
          // edge level-of-detail without dropping memories from the atlas.
          getMemoryGraph(),
          getCommunities(),
          getStats()
        ]);
        if (destroyed) return;

        setDbSizeMb(statsData?.db_size_mb);
        const nodes: GNode[] = (graphData.nodes as unknown as GNode[]) ?? [];
        // Color legend ("ledger") built from the nodes actually shown -- it maps
        // each node color to its category (task, state, ...) and always matches
        // what's drawn, rather than depending on a /stats category breakdown.
        const catCounts = new Map<string, number>();
        nodes.forEach((n) => {
          const c = n.category || 'general';
          catCounts.set(c, (catCounts.get(c) ?? 0) + 1);
        });
        setCategories(
          [...catCounts.entries()]
            .map(([category, count]) => ({ category, count }))
            .sort((a, b) => b.count - a.count)
        );
        const allEdges: GLink[] = (graphData.edges as unknown as GLink[]) ?? [];
        setNodeCount(graphData.node_count || nodes.length || 0);
        setEdgeCount(graphData.edge_count || allEdges.length || 0);

        if (!nodes.length) {
          setLoadError('No memories found. Store some memories first.');
          setLoading(false);
          return;
        }

        // Map community IDs onto nodes
        const commMap = new Map<string, number>();
        (commData.communities || []).forEach((c) => {
          (c.top_memories || []).forEach((mid) => commMap.set('m' + mid, c.id));
        });

        // Initialize every node before topology and interaction indexes are built.
        nodes.forEach((node) => {
          node.neighbors = [];
          node.links = [];
          node.community_id = commMap.get(node.id) ?? node.community_id;
          nodeMap.set(node.id, node);
        });

        // The stable overview is derived from every stored relationship. Edge
        // level-of-detail then keeps the maximum-affinity spanning forest and
        // spends the remainder on short local detail, so it cannot manufacture
        // detached islands or long decorative beams.
        galaxyTargets = buildGalaxyTargets(nodes, allEdges);
        setComponentCount(new Set([...galaxyTargets.values()].map((target) => target.component)).size);
        setMainComponentCount(
          [...galaxyTargets.values()].filter((target) => target.inMainComponent).length
        );
        const overviewEdgeCap = Math.min(
          allEdges.length,
          Math.max(4000, Math.ceil(Math.sqrt(nodes.length) * 32))
        );
        const edges: GLink[] = selectRenderEdges(allEdges, overviewEdgeCap, galaxyTargets);
        graphNodes = nodes;
        renderEdges = edges;
        batchedRendering = nodes.length >= GPU_BATCH_THRESHOLD;
        setDrawnEdgeCount(edges.length);

        // Full neighbor/link lookups preserve selection behavior even when only
        // a level-of-detail subset of relationships is visible at overview zoom.
        allEdges.forEach((link) => {
          const src = nodeMap.get(link.source as string);
          const tgt = nodeMap.get(link.target as string);
          if (src && tgt) {
            src.neighbors!.push(tgt);
            src.links!.push(link);
            tgt.neighbors!.push(src);
            tgt.links!.push(link);
          }
        });

        // Seed the complete topology once; large graphs remain static GPU data
        // while small graphs may continue into the richer force simulation.
        for (const node of nodes) {
          const target = galaxyTargets.get(node.id);
          if (!target) continue;
          if (!Number.isFinite(node.x)) node.x = target.x;
          if (!Number.isFinite(node.y)) node.y = target.y;
          if (!Number.isFinite(node.z)) node.z = target.z;
        }
        semanticAtlas = batchedRendering
          ? buildSemanticRegions(nodes, allEdges, galaxyTargets)
          : null;
        setRegionCount(semanticAtlas?.regions.length ?? 0);

        const ringTexture = createRingTexture(THREE);
        // Rich organism textures are only legible on the small-graph sprite path.
        const organismTextures = batchedRendering
          ? []
          : Array.from({ length: 8 }, (_, i) => createOrganismTexture(THREE, i * 137));
        // Production atlases use one compact signal texture instead of
        // downsampling organism detail into bright butterfly-shaped artifacts.
        const batchPointTexture = batchedRendering ? createGalaxyPointTexture(THREE) : null;
        ownedTextures.add(ringTexture);
        organismTextures.forEach((texture) => ownedTextures.add(texture));
        if (batchPointTexture) ownedTextures.add(batchPointTexture);
        const breathPhases = new Map<string, number>();
        const motionReduced =
          window.matchMedia?.('(prefers-reduced-motion: reduce)').matches ?? false;

        // Production-sized graphs use one node draw and one edge draw. Small
        // graphs keep richer per-node sprites because their fixed cost is modest.
        if (batchedRendering) {
          const count = nodes.length;
          const atlasDensityScale = Math.max(
            0.34,
            Math.min(1, Math.sqrt(GPU_BATCH_THRESHOLD / count))
          );
          const positions = new Float32Array(count * 3);
          const colors = new Float32Array(count * 3);
          const opacities = new Float32Array(count);
          const sizes = new Float32Array(count);
          const phases = new Float32Array(count);
          const col = new THREE.Color();
          nodes.forEach((node, i) => {
            positions[i * 3] = node.x ?? 0;
            positions[i * 3 + 1] = node.y ?? 0;
            positions[i * 3 + 2] = node.z ?? 0;
            col.set(getNodeColor(node));
            colors[i * 3] = col.r;
            colors[i * 3 + 1] = col.g;
            colors[i * 3 + 2] = col.b;
            opacities[i] = getNodeOpacity(node);
            const target = galaxyTargets.get(node.id);
            const componentScale =
              target?.inMainComponent === false
                ? target.depth === 0
                  ? 1.28
                  : 0.72
                : 1;
            sizes[i] =
              Math.max(8, ((node.importance || 5) * 1.8 + (node.size || 0) * 0.4) * 2.4) *
              (node.is_static ? 1.18 : 1) *
              componentScale *
              atlasDensityScale;
            phases[i] = (i * 0.7) % (Math.PI * 2);
          });
          pointBaseSizes = sizes.slice();
          pointGeom = new THREE.BufferGeometry();
          pointGeom.setAttribute('position', new THREE.BufferAttribute(positions, 3));
          pointGeom.setAttribute('aColor', new THREE.BufferAttribute(colors, 3));
          pointGeom.setAttribute('aOpacity', new THREE.BufferAttribute(opacities, 1));
          pointGeom.setAttribute('size', new THREE.BufferAttribute(sizes, 1));
          pointGeom.setAttribute('aPhase', new THREE.BufferAttribute(phases, 1));
          pointMat = new THREE.ShaderMaterial({
            uniforms: {
              uTex: { value: batchPointTexture },
              uTime: { value: 0 },
              uMotion: { value: motionReduced ? 0 : 1 }
            },
            transparent: true,
            depthWrite: false,
            blending: THREE.AdditiveBlending,
            vertexShader:
              'attribute float size;\n' +
              'attribute float aPhase;\n' +
              'attribute float aOpacity;\n' +
              'attribute vec3 aColor;\n' +
              'uniform float uTime;\n' +
              'uniform float uMotion;\n' +
              'varying vec3 vColor;\n' +
              'varying float vOpacity;\n' +
              'void main() {\n' +
              '  vColor = aColor;\n' +
              '  vOpacity = aOpacity;\n' +
              // Gentle per-point breathing pulse, computed on the GPU.
              '  float breathe = 1.0 + sin(uTime * 0.8 + aPhase) * 0.11 * uMotion;\n' +
              '  vec4 mv = modelViewMatrix * vec4(position, 1.0);\n' +
              '  gl_PointSize = size * breathe * (440.0 / max(1.0, -mv.z));\n' +
              '  gl_Position = projectionMatrix * mv;\n' +
              '}',
            // Brighter than 1:1 -- additive blending plus a color boost makes the
            // points read as glowing cells rather than dim specks.
            fragmentShader:
              'uniform sampler2D uTex;\n' +
              'varying vec3 vColor;\n' +
              'varying float vOpacity;\n' +
              'void main() {\n' +
              '  vec4 tex = texture2D(uTex, gl_PointCoord);\n' +
              '  if (tex.a < 0.02) discard;\n' +
              '  gl_FragColor = vec4(vColor * 1.15, vOpacity) * tex;\n' +
              '}'
          });
          nodeCloud = new THREE.Points(pointGeom, pointMat);
          nodeCloud.frustumCulled = false;

          const linkPositions = new Float32Array(edges.length * 6);
          const linkColors = new Float32Array(edges.length * 6);
          const linkAlphas = new Float32Array(edges.length * 2);
          edges.forEach((link, index) => {
            const source = resolveEndpoint(link.source);
            const target = resolveEndpoint(link.target);
            const offset = index * 6;
            linkPositions[offset] = source?.x ?? 0;
            linkPositions[offset + 1] = source?.y ?? 0;
            linkPositions[offset + 2] = source?.z ?? 0;
            linkPositions[offset + 3] = target?.x ?? 0;
            linkPositions[offset + 4] = target?.y ?? 0;
            linkPositions[offset + 5] = target?.z ?? 0;
          });
          linkGeom = new THREE.BufferGeometry();
          linkGeom.setAttribute('position', new THREE.BufferAttribute(linkPositions, 3));
          linkGeom.setAttribute('aColor', new THREE.BufferAttribute(linkColors, 3));
          linkGeom.setAttribute('aAlpha', new THREE.BufferAttribute(linkAlphas, 1));
          linkMat = new THREE.ShaderMaterial({
            transparent: true,
            depthWrite: false,
            blending: THREE.NormalBlending,
            vertexShader:
              'attribute vec3 aColor;\n' +
              'attribute float aAlpha;\n' +
              'varying vec3 vColor;\n' +
              'varying float vAlpha;\n' +
              'void main() {\n' +
              '  vColor = aColor;\n' +
              '  vAlpha = aAlpha;\n' +
              '  gl_Position = projectionMatrix * modelViewMatrix * vec4(position, 1.0);\n' +
              '}',
            fragmentShader:
              'varying vec3 vColor;\n' +
              'varying float vAlpha;\n' +
              'void main() {\n' +
              '  if (vAlpha <= 0.001) discard;\n' +
              '  gl_FragColor = vec4(vColor, vAlpha);\n' +
              '}'
          });
          linkLines = new THREE.LineSegments(linkGeom, linkMat);
          linkLines.frustumCulled = false;

          highlightLinkGeom = new THREE.BufferGeometry();
          highlightLinkGeom.setAttribute(
            'position',
            new THREE.BufferAttribute(new Float32Array(), 3)
          );
          highlightLinkMat = new THREE.LineBasicMaterial({
            color: new THREE.Color('#72ecff'),
            transparent: true,
            opacity: 0.72,
            depthWrite: false,
            blending: THREE.NormalBlending
          });
          highlightLinkLines = new THREE.LineSegments(highlightLinkGeom, highlightLinkMat);
          highlightLinkLines.frustumCulled = false;
          highlightLinkLines.visible = false;

          const regionList = semanticAtlas?.regions ?? [];
          const regionById = new Map(regionList.map((region) => [region.id, region]));
          const regionPositions = new Float32Array(regionList.length * 3);
          const regionColors = new Float32Array(regionList.length * 3);
          const regionSizes = new Float32Array(regionList.length);
          const regionOpacities = new Float32Array(regionList.length);
          const regionColor = new THREE.Color();
          regionList.forEach((region, index) => {
            regionPositions[index * 3] = region.x;
            regionPositions[index * 3 + 1] = region.y;
            regionPositions[index * 3 + 2] = region.z;
            regionColor.set(
              region.communityId != null
                ? COMMUNITY_COLORS[region.communityId % COMMUNITY_COLORS.length]
                : CATEGORY_FALLBACK[region.category] || '#4fc3f7'
            );
            regionColors[index * 3] = regionColor.r;
            regionColors[index * 3 + 1] = regionColor.g;
            regionColors[index * 3 + 2] = regionColor.b;
            regionSizes[index] = region.inMainComponent
              ? 28
                + Math.min(34, Math.log2(region.memberCount + 1) * 3.4)
                + Math.min(9, region.importance * 0.9)
              : 11
                + Math.min(12, Math.log2(region.memberCount + 1) * 1.8)
                + Math.min(4, region.importance * 0.4);
            regionOpacities[index] = region.inMainComponent ? 0.94 : 0.42;
          });
          regionPointGeom = new THREE.BufferGeometry();
          regionPointGeom.setAttribute(
            'position',
            new THREE.BufferAttribute(regionPositions, 3)
          );
          regionPointGeom.setAttribute(
            'aColor',
            new THREE.BufferAttribute(regionColors, 3)
          );
          regionPointGeom.setAttribute(
            'aOpacity',
            new THREE.BufferAttribute(regionOpacities, 1)
          );
          regionPointGeom.setAttribute(
            'size',
            new THREE.BufferAttribute(regionSizes, 1)
          );
          regionPointMat = new THREE.ShaderMaterial({
            uniforms: { uTex: { value: batchPointTexture } },
            transparent: true,
            depthWrite: false,
            blending: THREE.AdditiveBlending,
            vertexShader:
              'attribute float size;\n' +
              'attribute float aOpacity;\n' +
              'attribute vec3 aColor;\n' +
              'varying vec3 vColor;\n' +
              'varying float vOpacity;\n' +
              'void main() {\n' +
              '  vColor = aColor;\n' +
              '  vOpacity = aOpacity;\n' +
              '  vec4 mv = modelViewMatrix * vec4(position, 1.0);\n' +
              '  gl_PointSize = size * (440.0 / max(1.0, -mv.z));\n' +
              '  gl_Position = projectionMatrix * mv;\n' +
              '}',
            fragmentShader:
              'uniform sampler2D uTex;\n' +
              'varying vec3 vColor;\n' +
              'varying float vOpacity;\n' +
              'void main() {\n' +
              '  vec4 tex = texture2D(uTex, gl_PointCoord);\n' +
              '  if (tex.a < 0.02) discard;\n' +
              '  gl_FragColor = vec4(vColor * 1.12, vOpacity) * tex;\n' +
              '}'
          });
          regionPoints = new THREE.Points(regionPointGeom, regionPointMat);
          regionPoints.frustumCulled = false;

          const overviewEdges = semanticAtlas
            ? selectSemanticRegionEdges(
                semanticAtlas,
                Math.max(48, Math.min(96, Math.ceil(semanticAtlas.regions.length * 0.7)))
              )
            : [];
          setRegionEdgeCount(overviewEdges.length);
          const regionLinePositions = new Float32Array(overviewEdges.length * 6);
          const regionLineAlphas = new Float32Array(overviewEdges.length * 2);
          overviewEdges.forEach((edge, index) => {
            const source = regionById.get(edge.source);
            const target = regionById.get(edge.target);
            const offset = index * 6;
            regionLinePositions[offset] = source?.x ?? 0;
            regionLinePositions[offset + 1] = source?.y ?? 0;
            regionLinePositions[offset + 2] = source?.z ?? 0;
            regionLinePositions[offset + 3] = target?.x ?? 0;
            regionLinePositions[offset + 4] = target?.y ?? 0;
            regionLinePositions[offset + 5] = target?.z ?? 0;
            const alpha = 0.08 + Math.min(0.16, Math.log2(edge.count + 1) * 0.022);
            regionLineAlphas[index * 2] = alpha;
            regionLineAlphas[index * 2 + 1] = alpha;
          });
          regionLineGeom = new THREE.BufferGeometry();
          regionLineGeom.setAttribute(
            'position',
            new THREE.BufferAttribute(regionLinePositions, 3)
          );
          regionLineGeom.setAttribute(
            'aAlpha',
            new THREE.BufferAttribute(regionLineAlphas, 1)
          );
          regionLineMat = new THREE.ShaderMaterial({
            transparent: true,
            depthWrite: false,
            blending: THREE.NormalBlending,
            vertexShader:
              'attribute float aAlpha;\n' +
              'varying float vAlpha;\n' +
              'void main() {\n' +
              '  vAlpha = aAlpha;\n' +
              '  gl_Position = projectionMatrix * modelViewMatrix * vec4(position, 1.0);\n' +
              '}',
            fragmentShader:
              'varying float vAlpha;\n' +
              'void main() {\n' +
              '  gl_FragColor = vec4(0.0, 0.71, 1.0, vAlpha);\n' +
              '}'
          });
          regionLines = new THREE.LineSegments(regionLineGeom, regionLineMat);
          regionLines.frustumCulled = false;

          detailLinkGeom = new THREE.BufferGeometry();
          detailLinkGeom.setAttribute(
            'position',
            new THREE.BufferAttribute(new Float32Array(), 3)
          );
          detailLinkMat = new THREE.LineBasicMaterial({
            color: new THREE.Color('#38d7f2'),
            transparent: true,
            opacity: 0.34,
            depthWrite: false,
            blending: THREE.NormalBlending
          });
          detailLinkLines = new THREE.LineSegments(detailLinkGeom, detailLinkMat);
          detailLinkLines.frustumCulled = false;
          detailLinkLines.visible = false;

          // Rebuild one bounded real-edge batch for the currently expanded region.
          refreshRegionDetailLinks = (regionId: string | null) => {
            if (!regionId || !semanticAtlas || !detailLinkGeom || !detailLinkLines) {
              if (detailLinkLines) detailLinkLines.visible = false;
              return;
            }
            const region = semanticAtlas.regions.find((candidate) => candidate.id === regionId);
            const regionEdges = allEdges.filter((edge) => {
              const source = resolveEndpoint(edge.source);
              const target = resolveEndpoint(edge.target);
              return Boolean(
                source
                && target
                && (edge.weight ?? 0) >= weightThresholdLocal
                && semanticAtlas?.nodeRegion.get(source.id) === regionId
                && semanticAtlas?.nodeRegion.get(target.id) === regionId
              );
            });
            const detailEdgeCap = Math.min(
              regionEdges.length,
              Math.max(240, Math.ceil(Math.sqrt(region?.memberCount ?? 1) * 36))
            );
            const selected = selectRenderEdges(
              regionEdges,
              detailEdgeCap,
              galaxyTargets
            );
            const positions = new Float32Array(selected.length * 6);
            selected.forEach((edge, index) => {
              const source = resolveEndpoint(edge.source);
              const target = resolveEndpoint(edge.target);
              const offset = index * 6;
              positions[offset] = source?.x ?? 0;
              positions[offset + 1] = source?.y ?? 0;
              positions[offset + 2] = source?.z ?? 0;
              positions[offset + 3] = target?.x ?? 0;
              positions[offset + 4] = target?.y ?? 0;
              positions[offset + 5] = target?.z ?? 0;
            });
            detailLinkGeom.setAttribute(
              'position',
              new THREE.BufferAttribute(positions, 3)
            );
            detailLinkGeom.setDrawRange(0, selected.length * 2);
            detailLinkGeom.computeBoundingSphere();
            detailLinkLines.visible = selected.length > 0;
          };
        }

        // Create one opt-in label sprite for a static memory.
        const createNodeLabel = (node: GNode, baseSize: number) => {
          const canvas = document.createElement('canvas');
          const context = canvas.getContext('2d')!;
          const text = node.label || node.content?.slice(0, 30) || node.id;
          canvas.width = 256;
          canvas.height = 40;
          context.font = '20px Inter, sans-serif';
          context.fillStyle = 'white';
          context.textAlign = 'center';
          context.fillText(text.length > 28 ? text.slice(0, 28) + '...' : text, 128, 28);
          const texture = new THREE.CanvasTexture(canvas);
          const material = new THREE.SpriteMaterial({
            map: texture,
            transparent: true,
            opacity: 0.7,
            depthWrite: false
          });
          ownedTextures.add(texture);
          ownedSpriteMaterials.add(material);
          const label = new THREE.Sprite(material);
          label.scale.set(baseSize * 2.5, baseSize * 0.4, 1);
          label.position.set(0, baseSize * 0.8, 0);
          label.visible = false;
          nodeLabels.set(node.id, label);
          return label;
        };

        // Production-sized atlases are already owned by the fixed-cost Points
        // and LineSegments batches above. Feeding the same data into ForceGraph
        // would create a second per-node scene graph and run a duplicate force
        // simulation even though its native objects and links are hidden.
        const forceGraphData = batchedRendering
          ? { nodes: [] as GNode[], links: [] as GLink[] }
          : { nodes, links: edges };
        const graph = new ForceGraph3D(container)
          .graphData(forceGraphData)
          .backgroundColor('#05060d')
          .showNavInfo(false)
          .nodeLabel(() => '')
          .nodeVal((n: any) => (n as GNode).importance || 5)
          .linkSource('source')
          .linkTarget('target')
          // Living organism nodes with optional text labels
          .nodeThreeObject((node: any) => {
            const n = node as GNode;
            const baseSize = Math.max(4, (n.importance || 5) * 1.8 + (n.size || 0) * 0.4);
            // The point batch draws live nodes; retain only hidden static labels
            // as scene objects so the Labels control remains useful.
            if (batchedRendering) {
              const group = new THREE.Group();
              if (n.is_static) group.add(createNodeLabel(n, baseSize));
              return group;
            }
            const idNum = Number.parseInt(n.id.replace(/\D/g, '') || '0', 10);
            const tex = organismTextures[idNum % organismTextures.length];
            breathPhases.set(n.id, (idNum * 0.7) % (Math.PI * 2));

            const material = new THREE.SpriteMaterial({
              map: tex,
              color: new THREE.Color(getNodeColor(n)),
              transparent: true,
              opacity: getNodeOpacity(n),
              depthWrite: false
            });
            ownedSpriteMaterials.add(material);
            const sprite = new THREE.Sprite(material);
            sprite.scale.set(baseSize, baseSize, baseSize);
            nodeSprites.set(n.id, { material, baseSize, sprite });

            if (n.is_static) {
              const group = new THREE.Group();
              group.add(sprite);
              const ringMat = new THREE.SpriteMaterial({
                map: ringTexture,
                transparent: true,
                opacity: 0.15,
                depthWrite: false
              });
              ownedSpriteMaterials.add(ringMat);
              const ring = new THREE.Sprite(ringMat);
              ring.scale.set(baseSize * 1.15, baseSize * 1.15, baseSize * 1.15);
              group.add(ring);

              const label = createNodeLabel(n, baseSize);
              group.add(label);
              return group;
            }
            return sprite;
          })
          // Keep both fixed-cost GPU layers aligned with the force simulation.
          .onEngineTick(() => {
            if (batchedRendering) {
              if (pointGeom) {
                const arr = pointGeom.attributes.position.array as Float32Array;
                for (let i = 0; i < nodes.length; i++) {
                  const nd = nodes[i];
                  arr[i * 3] = nd.x ?? 0;
                  arr[i * 3 + 1] = nd.y ?? 0;
                  arr[i * 3 + 2] = nd.z ?? 0;
                }
                pointGeom.attributes.position.needsUpdate = true;
              }
              if (linkGeom) {
                const arr = linkGeom.attributes.position.array as Float32Array;
                renderEdges.forEach((link, index) => {
                  const source = resolveEndpoint(link.source);
                  const target = resolveEndpoint(link.target);
                  const offset = index * 6;
                  arr[offset] = source?.x ?? 0;
                  arr[offset + 1] = source?.y ?? 0;
                  arr[offset + 2] = source?.z ?? 0;
                  arr[offset + 3] = target?.x ?? 0;
                  arr[offset + 4] = target?.y ?? 0;
                  arr[offset + 5] = target?.z ?? 0;
                });
                linkGeom.attributes.position.needsUpdate = true;
              }
              return;
            }
            const t = motionReduced ? 0 : performance.now() * 0.001;
            nodeSprites.forEach((entry, id) => {
              const phase = breathPhases.get(id) ?? 0;
              const breathScale = 1 + Math.sin(t * 0.8 + phase) * 0.08;
              const sizeVal = entry.baseSize * breathScale;
              const isHovered = highlightNodes.has(nodeMap.get(id)!);
              const scale = isHovered ? sizeVal * 1.3 : sizeVal;
              entry.sprite.scale.set(scale, scale, scale);
            });
          })
          // Layer 1: faint static edges
          .linkWidth((link: any) => {
            if (batchedRendering) return 0;
            if (highlightLinks.has(link)) return Math.max(0.5, (link.weight ?? 0.5) * 2);
            if ((link.weight ?? 0) >= weightThresholdLocal) return 0.15;
            return 0;
          })
          .linkOpacity(1)
          .linkColor((link: any) => getVisibleLinkColor(link as GLink))
          .linkVisibility((link: any) => {
            if (batchedRendering) return false;
            if (highlightLinks.has(link)) return true;
            return (link.weight ?? 0) >= weightThresholdLocal;
          })
          // Flow-trail particles were removed: they only ever rendered on the
          // small-graph path (big graphs disable them), so they never appeared
          // in production and read as an unstyled default. Hover/selection
          // feedback comes from link colour + opacity (see getVisibleLinkColor).
          // Interactions
          .onNodeHover((node: any) => {
            if (!batchedRendering) handleNodeHover(node as GNode | null);
          })
          .onNodeClick((node: any) => {
            if (!batchedRendering) {
              focusLoadedNode(node as GNode);
              void handleNodeClick(node as GNode);
            }
          })
          .onBackgroundClick(() => {
            if (!showSearchResultsRef.current) closePanel();
          })
          // Deterministic starting coordinates remove the blocking pre-warm.
          .warmupTicks(0)
          .cooldownTicks(batchedRendering ? 0 : 120)
          .d3AlphaDecay(batchedRendering ? 0.06 : 0.036)
          .d3VelocityDecay(batchedRendering ? 0.55 : 0.48);

        graphInstance = graph;

        // Prepare the canvas as both a visual surface and a keyboard target.
        const canvas = graph.renderer().domElement;
        canvas.style.backgroundColor = '#05060d';
        canvas.tabIndex = 0;
        canvas.setAttribute('role', 'application');
        canvas.setAttribute(
          'aria-label',
          'Interactive memory galaxy. Use the pointer or arrow keys to select memories.'
        );

        galaxyBackdrop = addGalaxyBackdrop(THREE, graph.scene(), galaxyTargets);

        // Add both fixed-cost live layers after the backdrop.
        if (regionLines) graph.scene().add(regionLines);
        if (regionPoints) graph.scene().add(regionPoints);
        if (detailLinkLines) graph.scene().add(detailLinkLines);
        if (linkLines) graph.scene().add(linkLines);
        if (highlightLinkLines) graph.scene().add(highlightLinkLines);
        if (nodeCloud) {
          graph.scene().add(nodeCloud);
          const animateCloud = () => {
            if (destroyed) return;
            if (pointMat) pointMat.uniforms.uTime.value = performance.now() * 0.001;
            cloudRaf = requestAnimationFrame(animateCloud);
          };
          cloudRaf = requestAnimationFrame(animateCloud);
        }
        applySemanticZoom(batchedRendering);
        refreshLinkVisuals();
        updateNodeVisuals();

        // Restore point-level hover, click, and keyboard access for the batched
        // layer because its nodes are no longer individual Three.js objects.
        if (batchedRendering && nodeCloud) {
          const raycaster = new THREE.Raycaster();
          raycaster.params.Points.threshold = 12;
          const regionRaycaster = new THREE.Raycaster();
          regionRaycaster.params.Points.threshold = 28;
          const pointer = new THREE.Vector2();
          let pointerFrame: number | undefined;
          let keyboardIndex = -1;
          // Return the point under a canvas-space pointer event.
          const pickNode = (event: PointerEvent | MouseEvent): GNode | null => {
            const rect = canvas.getBoundingClientRect();
            pointer.x = ((event.clientX - rect.left) / Math.max(1, rect.width)) * 2 - 1;
            pointer.y = -((event.clientY - rect.top) / Math.max(1, rect.height)) * 2 + 1;
            raycaster.setFromCamera(pointer, graph.camera());
            const hit = raycaster.intersectObject(nodeCloud, false)[0];
            return hit?.index == null ? null : nodes[hit.index] ?? null;
          };
          // Return the semantic region under a canvas-space pointer event.
          const pickRegion = (event: PointerEvent | MouseEvent): SemanticRegion | null => {
            if (!regionPoints || !semanticAtlas) return null;
            const rect = canvas.getBoundingClientRect();
            pointer.x = ((event.clientX - rect.left) / Math.max(1, rect.width)) * 2 - 1;
            pointer.y = -((event.clientY - rect.top) / Math.max(1, rect.height)) * 2 + 1;
            regionRaycaster.setFromCamera(pointer, graph.camera());
            const hit = regionRaycaster.intersectObject(regionPoints, false)[0];
            return hit?.index == null ? null : semanticAtlas.regions[hit.index] ?? null;
          };
          // Throttle hover picking to one raycast per animation frame.
          const onPointerMove = (event: PointerEvent) => {
            if (pointerFrame !== undefined) cancelAnimationFrame(pointerFrame);
            pointerFrame = requestAnimationFrame(() => {
              pointerFrame = undefined;
              if (overviewActive) {
                const region = pickRegion(event);
                canvas.style.cursor = region ? 'pointer' : 'grab';
                return;
              }
              const node = pickNode(event);
              handleNodeHover(node);
              canvas.style.cursor = node ? 'pointer' : 'grab';
            });
          };
          // Clear transient emphasis when the pointer exits the graph.
          const onPointerLeave = () => {
            handleNodeHover(null);
            canvas.style.cursor = 'grab';
          };
          // Open a hit memory while preserving background-close behavior.
          const onCanvasClick = (event: MouseEvent) => {
            if (overviewActive) {
              const region = pickRegion(event);
              if (region) focusRegion(region);
              return;
            }
            const node = pickNode(event);
            if (node) {
              focusLoadedNode(node);
              void handleNodeClick(node);
            } else if (!showSearchResultsRef.current) {
              closePanel();
            }
          };
          // Cycle and open memories without requiring a precise pointer.
          const onCanvasKeyDown = (event: KeyboardEvent) => {
            if (overviewActive && semanticAtlas) {
              const regions = semanticAtlas.regions;
              if (event.key === 'ArrowRight' || event.key === 'ArrowDown') {
                event.preventDefault();
                keyboardIndex = (keyboardIndex + 1 + regions.length) % regions.length;
              } else if (event.key === 'ArrowLeft' || event.key === 'ArrowUp') {
                event.preventDefault();
                keyboardIndex = (keyboardIndex - 1 + regions.length) % regions.length;
              } else if (event.key === 'Enter' && keyboardIndex >= 0) {
                event.preventDefault();
                focusRegion(regions[keyboardIndex]);
              }
              return;
            }
            if (event.key === 'ArrowRight' || event.key === 'ArrowDown') {
              event.preventDefault();
              keyboardIndex = (keyboardIndex + 1 + nodes.length) % nodes.length;
            } else if (event.key === 'ArrowLeft' || event.key === 'ArrowUp') {
              event.preventDefault();
              keyboardIndex = (keyboardIndex - 1 + nodes.length) % nodes.length;
            } else if (event.key === 'Enter' && keyboardIndex >= 0) {
              event.preventDefault();
              focusLoadedNode(nodes[keyboardIndex]);
              void handleNodeClick(nodes[keyboardIndex]);
              return;
            } else {
              return;
            }
            const node = nodes[keyboardIndex];
            handleNodeHover(node);
            focusLoadedNode(node);
          };
          canvas.addEventListener('pointermove', onPointerMove);
          canvas.addEventListener('pointerleave', onPointerLeave);
          canvas.addEventListener('click', onCanvasClick);
          canvas.addEventListener('keydown', onCanvasKeyDown);
          removeBatchInteractions = () => {
            if (pointerFrame !== undefined) cancelAnimationFrame(pointerFrame);
            canvas.removeEventListener('pointermove', onPointerMove);
            canvas.removeEventListener('pointerleave', onPointerLeave);
            canvas.removeEventListener('click', onCanvasClick);
            canvas.removeEventListener('keydown', onCanvasKeyDown);
          };
        }

        // The guide supplies the readable spiral; real relationships still
        // determine local shape and may bend or bridge its arms.
        graph.d3Force('galaxy', makeGalaxyGuideForce(galaxyTargets, GALAXY_GUIDE_STRENGTH));

        // Repulsion: bigger (more important) memories push a little harder, so
        // hubs get room while leaves pack in. distanceMax keeps it O(n) friendly
        // and stops far clusters from blasting each other apart.
        graph
          .d3Force('charge')
          ?.strength((node: any) => -(34 + ((node as GNode).importance || 5) * 6))
          .distanceMax(700)
          .theta(0.9);

        // Keep related memories close inside a group and give real cross-group
        // bridges enough length to remain legible instead of crushing the arms.
        graph
          .d3Force('link')
          ?.distance((link: any) => {
            const weight = Math.min(1, link.weight ?? 0.3);
            return linkStaysWithinGroup(link as GLink)
              ? 18 + (1 - weight) * 42
              : 118 + (1 - weight) * 96;
          })
          .strength((link: any) => {
            const weight = Math.min(1, link.weight ?? 0.3);
            return linkStaysWithinGroup(link as GLink)
              ? 0.16 + weight * 0.32
              : 0.025 + weight * 0.075;
          });

        // Light centering so the whole organism stays framed, not drifting.
        graph.d3Force('center')?.strength(0.02);

        // Size canvas to its container (not the whole window -- this lives in
        // a full-screen overlay, so the container already fills the viewport).
        const sizeToContainer = () => {
          // container is guaranteed non-null by the guard at the effect top;
          // TS just doesn't carry that narrowing into this nested closure.
          const rect = container!.getBoundingClientRect();
          graph.width(rect.width || window.innerWidth).height(rect.height || window.innerHeight);
        };
        sizeToContainer();
        resizeHandler = sizeToContainer;
        window.addEventListener('resize', resizeHandler);

        // Fit after the short live settle without depending on hidden objects.
        setTimeout(() => {
          if (!destroyed) fitGalaxyView();
        }, 900);

        // Publish the imperative handle for the UI controls.
        apiRef.current = {
          setWeight: (v: number) => {
            weightThresholdLocal = v;
            refreshLinkVisuals();
            refreshRegionDetailLinks?.(activeRegionId);
          },
          setLabels: (v: boolean) => {
            nodeLabels.forEach((label) => {
              label.visible = v;
            });
          },
          setClusters: (v: boolean) => {
            semanticColorEnabled = v;
            updateNodeVisuals();
            refreshLinkVisuals();
          },
          fitView: fitGalaxyView,
          zoomToNode,
          runSearch,
          closePanel
        };

        setLoading(false);
      } catch (e: any) {
        setLoadError(e?.message || 'Unknown error');
        setLoading(false);
        console.error('Graph init failed:', e);
      }
    }
    void init();

    return () => {
      destroyed = true;
      if (cloudRaf !== undefined) cancelAnimationFrame(cloudRaf);
      removeBatchInteractions?.();
      galaxyBackdrop?.dispose();
      if (resizeHandler) window.removeEventListener('resize', resizeHandler);
      if (graphInstance && regionLines) graphInstance.scene().remove(regionLines);
      if (graphInstance && regionPoints) graphInstance.scene().remove(regionPoints);
      if (graphInstance && detailLinkLines) graphInstance.scene().remove(detailLinkLines);
      if (graphInstance && linkLines) graphInstance.scene().remove(linkLines);
      if (graphInstance && highlightLinkLines) graphInstance.scene().remove(highlightLinkLines);
      if (graphInstance && nodeCloud) graphInstance.scene().remove(nodeCloud);
      linkGeom?.dispose?.();
      linkMat?.dispose?.();
      highlightLinkGeom?.dispose?.();
      highlightLinkMat?.dispose?.();
      pointGeom?.dispose?.();
      pointMat?.dispose?.();
      regionPointGeom?.dispose?.();
      regionPointMat?.dispose?.();
      regionLineGeom?.dispose?.();
      regionLineMat?.dispose?.();
      detailLinkGeom?.dispose?.();
      detailLinkMat?.dispose?.();
      ownedSpriteMaterials.forEach((material) => material.dispose?.());
      ownedTextures.forEach((texture) => texture.dispose?.());
      graphInstance?._destructor?.();
      apiRef.current = null;
      // Allow a genuine remount (incl. StrictMode's dev double-mount) to rebuild.
      startedRef.current = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Keep onBackgroundClick aware of whether the search panel is open.
  useEffect(() => {
    showSearchResultsRef.current = showSearchResults;
  }, [showSearchResults]);

  // Sync UI controls into the imperative graph.
  useEffect(() => {
    apiRef.current?.setLabels(showLabels);
  }, [showLabels]);
  useEffect(() => {
    apiRef.current?.setWeight(weightThreshold);
  }, [weightThreshold]);
  useEffect(() => {
    apiRef.current?.setClusters(clusterEnabled);
  }, [clusterEnabled]);

  // Escape closes the side panel.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') apiRef.current?.closePanel();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  const onSearchSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    const results = (await apiRef.current?.runSearch(searchQuery)) ?? [];
    if (!searchQuery.trim()) {
      setSearchResults([]);
      setShowSearchResults(false);
      return;
    }
    setSearchResults(results);
    setShowSearchResults(true);
    setSidePanelOpen(true);
    setSelectedMemory(null);
  };

  // ── Cosmic instrument shell ──────────────────────────────
  return (
    <div className="memgraph-root fixed inset-0 z-40 overflow-hidden">
      <div
        ref={containerRef}
        className="memgraph-canvas w-full h-full"
        role="img"
        aria-label={viewMode === 'overview'
          ? `Interactive memory galaxy with ${nodeCount.toLocaleString()} memories represented by ${regionCount.toLocaleString()} selectable regions and ${regionEdgeCount.toLocaleString()} stored-link paths.`
          : `Interactive memory detail with original memories and stored links from the selected region.`}
      />

      {loading && (
        <div className="absolute inset-0 flex items-center justify-center z-50 bg-[#0a0a0a]">
          <div className="text-center">
            <div className="w-12 h-12 border-2 border-teal-500/30 border-t-teal-400 rounded-full animate-spin mx-auto mb-4" />
            <p className="text-gray-500 text-sm">Loading memory graph...</p>
          </div>
        </div>
      )}

      {loadError && (
        <div className="absolute inset-0 flex items-center justify-center z-50 bg-[#0a0a0a]">
          <div className="bg-red-900/20 border border-red-800/50 rounded-xl p-6 max-w-md text-center">
            <p className="text-red-400 text-sm mb-2">Failed to load graph</p>
            <p className="text-red-300/60 text-xs font-mono">{loadError}</p>
            <a
              href="/"
              className="inline-block mt-4 px-4 py-2 bg-gray-800 hover:bg-gray-700 rounded-lg text-sm text-gray-300 transition-colors"
            >
              Back to Dashboard
            </a>
          </div>
        </div>
      )}

      {!loading && !loadError && (
        <>
          {/* Top instrument bar */}
          <header className="memgraph-topbar absolute top-0 left-0 right-0 z-50 flex items-center gap-4">
            <a
              href="/"
              className="memgraph-back flex items-center gap-2 transition-colors shrink-0"
              aria-label="Back to dashboard"
            >
              <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M15 19l-7-7 7-7" />
              </svg>
            </a>

            <div className="memgraph-brand shrink-0">
              <span className="memgraph-brand__name">KLEOS</span>
              <span className="memgraph-brand__mode">MEMORY GALAXY</span>
            </div>

            <span className="memgraph-live shrink-0"><i /> LIVE</span>
            <span className="memgraph-view-mode shrink-0" aria-live="polite">
              {viewMode === 'overview' ? 'REGION MAP' : 'MEMORY DETAIL'}
            </span>

            <form className="memgraph-search flex-1 max-w-md" onSubmit={onSearchSubmit} role="search">
              <div className="relative">
                <input
                  type="text"
                  value={searchQuery}
                  onChange={(e) => setSearchQuery(e.target.value)}
                  placeholder="Search memories..."
                  aria-label="Search memories"
                  className="memgraph-search__input w-full px-4 py-2 pl-9 text-sm focus:outline-none transition-all"
                />
                <svg
                  className="memgraph-search__icon absolute left-3 top-1/2 -translate-y-1/2 w-3.5 h-3.5"
                  fill="none"
                  stroke="currentColor"
                  viewBox="0 0 24 24"
                >
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z" />
                </svg>
              </div>
            </form>

            <div className="memgraph-metrics flex items-center gap-4 shrink-0" aria-label="Graph statistics">
              <span>
                <strong>{nodeCount.toLocaleString()}</strong> memories
              </span>
              <span>
                <strong>{edgeCount.toLocaleString()}</strong> links
              </span>
              {regionCount > 0 && viewMode === 'overview' && (
                <span>
                  <strong>{regionCount.toLocaleString()}</strong> regions
                </span>
              )}
              {viewMode === 'overview' && regionEdgeCount > 0 && (
                <span>
                  <strong>{regionEdgeCount.toLocaleString()}</strong> paths
                </span>
              )}
              {viewMode === 'memories' && drawnEdgeCount !== edgeCount && (
                <span>
                  <strong>{drawnEdgeCount.toLocaleString()}</strong> drawn
                </span>
              )}
              {dbSizeMb != null && <span><strong>{dbSizeMb.toFixed(1)}</strong> MB</span>}
            </div>
          </header>

          {/* Graph controls */}
          <section
            className="memgraph-instruments absolute z-50 flex flex-col gap-3 p-4 memgraph-glass-panel"
            aria-label="Galaxy controls"
          >
            <div className="memgraph-panel-heading">SIGNAL CONTROLS</div>
            <p className="memgraph-zoom-note">
              {viewMode === 'overview'
                ? 'Select a region to reveal its memories.'
                : regionSummary
                  ? `${regionSummary.label}: ${regionSummary.memberCount.toLocaleString()} memories, ${regionSummary.internalEdgeCount.toLocaleString()} internal links.`
                  : 'Exact memories and stored links. Fit Galaxy returns to regions.'}
            </p>
            <div className="memgraph-topology-note">
              <strong>{componentCount.toLocaleString()}</strong>{' '}
              {componentCount === 1 ? 'stored component' : 'stored components'}
              {componentCount > 1 && (
                <small>
                  {(nodeCount - mainComponentCount).toLocaleString()} memories have no stored path
                  to the main component
                </small>
              )}
            </div>
            <div>
              <div className="memgraph-control-label text-[10px] uppercase tracking-wider mb-1.5">Edge floor</div>
              <div className="flex items-center gap-2">
                <input
                  type="range"
                  min={0}
                  max={1}
                  step={0.05}
                  value={weightThreshold}
                  onChange={(e) => setWeightThreshold(Number.parseFloat(e.target.value))}
                  aria-label="Minimum edge weight"
                  className="memgraph-range-slider w-28"
                />
                <span className="memgraph-control-value text-[10px] w-7 text-right">{weightThreshold.toFixed(2)}</span>
              </div>
            </div>

            <button
              onClick={() => setShowLabels((v) => !v)}
              aria-pressed={showLabels}
              className="memgraph-toggle flex items-center gap-2 group"
            >
              <div className={`memgraph-switch w-7 h-4 rounded-full relative transition-colors ${showLabels ? 'is-on' : ''}`}>
                <div className="memgraph-switch__thumb absolute left-0.5 top-0.5 w-3 h-3 rounded-full transition-all" />
              </div>
              <span className="text-[10px] transition-colors">Labels</span>
            </button>

            <button
              onClick={() => setClusterEnabled((v) => !v)}
              aria-pressed={clusterEnabled}
              className="memgraph-toggle flex items-center gap-2 group"
            >
              <div className={`memgraph-switch w-7 h-4 rounded-full relative transition-colors ${clusterEnabled ? 'is-on' : ''}`}>
                <div className="memgraph-switch__thumb absolute left-0.5 top-0.5 w-3 h-3 rounded-full transition-all" />
              </div>
                <span className="text-[10px] transition-colors">Color groups</span>
            </button>

            <button
              onClick={() => apiRef.current?.fitView()}
              className="memgraph-fit px-3 py-1.5 text-[10px] transition-all"
            >
              FIT GALAXY
            </button>
          </section>

          {/* Side Panel */}
          {sidePanelOpen && (
            <aside className="absolute top-0 right-0 bottom-0 w-[380px] z-50 overflow-y-auto memgraph-side-panel memgraph-glass-panel-solid">
              <button
                onClick={() => apiRef.current?.closePanel()}
                aria-label="Close panel"
                className="absolute top-4 right-4 w-7 h-7 flex items-center justify-center rounded-lg bg-white/5 hover:bg-white/10 text-gray-500 hover:text-gray-300 transition-all z-10"
              >
                <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
                </svg>
              </button>

              <div className="p-5 pt-6">
                {showSearchResults ? (
                  <>
                    <h3 className="text-xs font-semibold text-gray-500 uppercase tracking-wider mb-3">Search Results</h3>
                    {searchResults.length === 0 ? (
                      <p className="text-sm text-gray-600">No results found</p>
                    ) : (
                      <div className="space-y-2">
                        {searchResults.map((result) => (
                          <button
                            key={result.id}
                            onClick={() => apiRef.current?.zoomToNode(result.id)}
                            className="w-full text-left p-3 bg-white/[0.03] hover:bg-white/[0.06] border border-white/[0.05] rounded-lg transition-all group"
                          >
                            <div className="flex items-center gap-2 mb-1">
                              <span className="text-[10px] font-mono text-gray-600">#{result.id}</span>
                              <span
                                className="px-1.5 py-0.5 rounded text-[9px] font-medium"
                                style={{
                                  background: `${CATEGORY_FALLBACK[result.category] || '#4fc3f7'}20`,
                                  color: CATEGORY_FALLBACK[result.category] || '#4fc3f7'
                                }}
                              >
                                {result.category}
                              </span>
                              {result.score != null && (
                                <span className="text-[10px] text-gray-600 ml-auto">{(result.score * 100).toFixed(0)}%</span>
                              )}
                            </div>
                            <p className="text-xs text-gray-400 line-clamp-2 group-hover:text-gray-300 transition-colors">{result.content}</p>
                          </button>
                        ))}
                      </div>
                    )}
                  </>
                ) : selectedMemory ? (
                  <div className="space-y-5">
                    <p className="text-sm text-gray-300 leading-relaxed whitespace-pre-wrap">{selectedMemory.content}</p>

                    <div className="flex flex-wrap gap-1.5">
                      <span
                        className="px-2 py-0.5 rounded-full text-[10px] font-medium"
                        style={{
                          background: `${CATEGORY_FALLBACK[selectedMemory.category] || '#4fc3f7'}20`,
                          color: CATEGORY_FALLBACK[selectedMemory.category] || '#4fc3f7'
                        }}
                      >
                        {selectedMemory.category}
                      </span>
                      <span className="px-2 py-0.5 rounded-full text-[10px] bg-gray-800 text-gray-500">{selectedMemory.source}</span>
                      {selectedMemory.is_static && (
                        <span className="px-2 py-0.5 rounded-full text-[10px] bg-amber-900/30 text-amber-400">static</span>
                      )}
                      <span className="px-2 py-0.5 rounded-full text-[10px] bg-gray-800 text-gray-500">v{selectedMemory.version}</span>
                    </div>

                    <div className="grid grid-cols-2 gap-3">
                      <div>
                        <div className="text-[10px] text-gray-600 mb-1">Importance</div>
                        <div className="h-1.5 bg-gray-800 rounded-full overflow-hidden">
                          <div
                            className="h-full rounded-full transition-all"
                            style={{
                              width: `${selectedMemory.importance * 10}%`,
                              background: CATEGORY_FALLBACK[selectedMemory.category] || '#4fc3f7'
                            }}
                          />
                        </div>
                        <div className="text-[10px] text-gray-500 mt-0.5">{selectedMemory.importance}/10</div>
                      </div>
                      <div>
                        <div className="text-[10px] text-gray-600 mb-1">Decay</div>
                        <div className="h-1.5 bg-gray-800 rounded-full overflow-hidden">
                          <div
                            className="h-full bg-teal-500/60 rounded-full transition-all"
                            style={{
                              width: `${Math.min(100, ((selectedMemory.decay_score ?? 0) / Math.max(1, selectedMemory.importance)) * 100)}%`
                            }}
                          />
                        </div>
                        <div className="text-[10px] text-gray-500 mt-0.5">{selectedMemory.decay_score?.toFixed(2) ?? 'N/A'}</div>
                      </div>
                    </div>

                    <div className="space-y-1.5 text-[11px]">
                      <div className="flex justify-between">
                        <span className="text-gray-600">Created</span>
                        <span className="text-gray-400">
                          {new Date(selectedMemory.created_at).toLocaleDateString()}{' '}
                          {new Date(selectedMemory.created_at).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}
                        </span>
                      </div>
                      <div className="flex justify-between">
                        <span className="text-gray-600">Accessed</span>
                        <span className="text-gray-400">{selectedMemory.access_count ?? 0}x</span>
                      </div>
                      {selectedMemory.last_accessed_at && (
                        <div className="flex justify-between">
                          <span className="text-gray-600">Last accessed</span>
                          <span className="text-gray-400">{new Date(selectedMemory.last_accessed_at).toLocaleDateString()}</span>
                        </div>
                      )}
                      {selectedMemory.episode && (
                        <div className="flex justify-between">
                          <span className="text-gray-600">Episode</span>
                          <span className="text-gray-400">{selectedMemory.episode.title}</span>
                        </div>
                      )}
                    </div>

                    {selectedMemory.tags?.length ? (
                      <div>
                        <h4 className="text-[10px] text-gray-600 uppercase tracking-wider mb-2">Tags</h4>
                        <div className="flex flex-wrap gap-1.5">
                          {selectedMemory.tags.map((tag) => (
                            <span key={tag} className="px-2 py-0.5 rounded-md text-[10px] bg-teal-500/10 text-teal-400/80 border border-teal-500/10">
                              {tag}
                            </span>
                          ))}
                        </div>
                      </div>
                    ) : null}

                    {selectedMemory.links?.length ? (
                      <div>
                        <h4 className="text-[10px] text-gray-600 uppercase tracking-wider mb-2">
                          Linked Memories ({selectedMemory.links.length})
                        </h4>
                        <div className="space-y-1.5">
                          {selectedMemory.links.map((link) => (
                            <button
                              key={link.id}
                              onClick={() => apiRef.current?.zoomToNode(link.id)}
                              className="w-full text-left p-2.5 bg-white/[0.02] hover:bg-white/[0.05] border border-white/[0.04] rounded-lg transition-all group"
                            >
                              <div className="flex items-center gap-2 mb-0.5">
                                <span className="text-[9px] px-1.5 py-0.5 rounded bg-gray-800 text-gray-500">{link.type}</span>
                                <span className="text-[9px] text-gray-600 ml-auto">{(link.similarity * 100).toFixed(0)}%</span>
                              </div>
                              <p className="text-[11px] text-gray-500 line-clamp-1 group-hover:text-gray-400 transition-colors">{link.content}</p>
                            </button>
                          ))}
                        </div>
                      </div>
                    ) : null}

                    {selectedMemory.version_chain && selectedMemory.version_chain.length > 1 ? (
                      <div>
                        <h4 className="text-[10px] text-gray-600 uppercase tracking-wider mb-2">Version History</h4>
                        <div className="relative ml-2 pl-4 border-l border-gray-800 space-y-3">
                          {selectedMemory.version_chain.map((ver) => (
                            <div key={ver.id} className="relative">
                              <div
                                className={`absolute -left-[21px] top-1 w-2.5 h-2.5 rounded-full border-2 ${
                                  ver.is_latest ? 'bg-teal-400 border-teal-400' : 'bg-gray-800 border-gray-700'
                                }`}
                              />
                              <div className="text-[10px] text-gray-600">
                                v{ver.version} {ver.is_latest ? '(latest)' : ''}
                              </div>
                              <p className="text-[11px] text-gray-500 line-clamp-2 mt-0.5">{ver.content}</p>
                            </div>
                          ))}
                        </div>
                      </div>
                    ) : null}
                  </div>
                ) : null}
              </div>
            </aside>
          )}

          {/* Category Legend (bottom-right, hidden when panel open) */}
          {categories.length > 0 && !sidePanelOpen && (
            <div className="memgraph-category-ledger absolute z-40 p-3 memgraph-glass-panel-light">
              <div className="memgraph-panel-heading mb-2">MEMORY LEDGER</div>
              <div className="space-y-1">
                {categories.slice(0, 8).map((cat) => (
                  <div key={cat.category} className="flex items-center gap-2">
                    <div className="w-2 h-2 rounded-full" style={{ background: CATEGORY_FALLBACK[cat.category] || '#4fc3f7' }} />
                    <span className="text-[10px] text-gray-500">{cat.category}</span>
                    <span className="text-[10px] text-gray-700 ml-auto">{cat.count}</span>
                  </div>
                ))}
              </div>
            </div>
          )}
          <p className="memgraph-interaction-hint" aria-hidden="true">
            DRAG TO ORBIT · SCROLL TO ZOOM · SELECT A REGION · SEARCH ANY MEMORY
          </p>
        </>
      )}
    </div>
  );
}
