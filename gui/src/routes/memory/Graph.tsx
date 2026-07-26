// Memory Galaxy renders the live Kleos knowledge graph as an interactive cosmic instrument.
import { useEffect, useRef, useState, type FormEvent } from 'react';
import {
  getCommunities,
  getMemoryDetail,
  getMemoryGraph,
  getStats,
  searchGraph,
  // CategoryCount describes the category ledger returned by graph statistics.
  type CategoryCount,
  // GraphSearchResult describes a memory returned by galaxy search.
  type GraphSearchResult,
  // MemoryDetail describes the selected memory inspector payload.
  type MemoryDetail
} from '$lib/api/graph';
import { selectRenderEdges } from '$lib/graph/selectRenderEdges';
import {
  buildGalaxyTargets,
  seedGalaxyPositions,
  // GalaxyTarget carries the stable guide position consumed by the live force.
  type GalaxyTarget
} from '$lib/graph/galaxyLayout';
import './graph.css';

// ── Working types ──────────────────────────────────────────
// The graph mutates nodes in place (neighbors/links/positions), so these are
// looser than the API GraphNode/GraphEdge and own the runtime-only fields.

// One memory as the force simulation sees it, including runtime-only fields.
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

// GLink carries the mutable source and target references used by the force simulation.
interface GLink {
  source: string | GNode;
  target: string | GNode;
  type: string;
  weight: number;
}

// ── Constants ──────────────────────────────────────────────

// Palette cycled by community id so neighbouring clusters get distinct hues.
const COMMUNITY_COLORS = [
  '#00d7ff', '#6d7cff', '#22e87a', '#ff7a1a',
  '#1463ff', '#b46cff', '#ffd166', '#00f0c8',
  '#ff5e7a', '#7aa2ff', '#a6ff6a', '#ff9f43'
];

// Fixed colour per known category, used before falling back to community hue.
const CATEGORY_FALLBACK: Record<string, string> = {
  general: '#00d7ff', decision: '#b46cff', task: '#22e87a',
  state: '#ff7a1a', discovery: '#1463ff', reference: '#ff5e9f',
  issue: '#ff5e7a', preference: '#ffd166', credential: '#7aa2ff',
  infrastructure: '#1463ff', incident: '#ff7a1a', directive: '#22e87a'
};

// Resolve the group identity used to distinguish dense local links from orbital bridges.
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

// Draw the soft cell-like glow sprite used for a node, varied by seed.
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

// createRingTexture builds the halo used to mark static memories.
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

// Pull live nodes toward stable spiral targets without pinning or replacing force physics.
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

// The guide is strong enough to preserve arms but weak enough for links and drag
// to deform them. At production scale the link forces overwhelmed the weaker
// original value and pulled the disc back into a ball, erasing the layout, so
// the guide has to hold its own against ~14k edges of contrary pull.
const GALAXY_GUIDE_STRENGTH = 0.38;

// Fraction of edges hidden by default. Every edge drawn at once additively
// blends into a single saturated mass that hides node colour entirely, so the
// default view shows the strongest connections and the slider reveals the rest.
const DEFAULT_EDGE_FLOOR_QUANTILE = 0.2;

// Extra room left around the graph when framing it, as a multiple of the
// bounding radius, so the outermost nodes are not flush against the viewport.
const FIT_PADDING_FACTOR = 1.06;

// Describe the range of edge weights actually present, plus the default floor.
// Weights sit in a narrow band near 1.0, so the slider is calibrated to the
// observed spread instead of a nominal 0..1 that would be mostly inert.
function describeEdgeWeights(edges: readonly GLink[]): { min: number; max: number; defaultFloor: number } {
  const weights = edges
    .map((edge) => edge.weight)
    .filter((weight): weight is number => Number.isFinite(weight))
    .sort((left, right) => left - right);

  if (!weights.length) return { min: 0, max: 1, defaultFloor: 0 };

  const min = weights[0];
  const max = weights[weights.length - 1];
  // A degenerate range (every edge sharing one weight) must not collapse the
  // slider to zero width, which would make it impossible to grab.
  if (!(max > min)) return { min, max: min + 1, defaultFloor: min };

  const quantile = weights[Math.min(weights.length - 1, Math.floor(weights.length * DEFAULT_EDGE_FLOOR_QUANTILE))];
  return { min, max, defaultFloor: quantile };
}

// ── Cosmic scene ───────────────────────────────────────────

// Create the soft circular point texture shared by both fixed-cost backdrop clouds.
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

// Create the luminous central core texture that anchors the visual hierarchy.
function createGalaxyCoreTexture(THREE: any) {
  const canvas = document.createElement('canvas');
  canvas.width = 256;
  canvas.height = 256;
  const context = canvas.getContext('2d')!;
  const glow = context.createRadialGradient(128, 128, 0, 128, 128, 128);
  glow.addColorStop(0, 'rgba(240,255,255,1)');
  glow.addColorStop(0.08, 'rgba(120,245,255,0.98)');
  glow.addColorStop(0.22, 'rgba(0,215,255,0.72)');
  glow.addColorStop(0.46, 'rgba(20,99,255,0.26)');
  glow.addColorStop(0.72, 'rgba(124,77,255,0.08)');
  glow.addColorStop(1, 'rgba(0,0,0,0)');
  context.fillStyle = glow;
  context.fillRect(0, 0, 256, 256);
  return new THREE.CanvasTexture(canvas);
}

// Build one star cloud, one spiral cloud, and one core sprite behind the live graph.
function addGalaxyBackdrop(THREE: any, scene: any) {
  let seed = 0x4b4c454f;
  // nextRandom advances a stable linear congruential generator for repeatable frames.
  const nextRandom = () => {
    seed = (seed * 1664525 + 1013904223) >>> 0;
    return seed / 0x100000000;
  };

  const pointTexture = createGalaxyPointTexture(THREE);
  const coreTexture = createGalaxyCoreTexture(THREE);
  const starCount = 720;
  const starPositions = new Float32Array(starCount * 3);
  const starColors = new Float32Array(starCount * 3);
  for (let i = 0; i < starCount; i++) {
    starPositions[i * 3] = (nextRandom() - 0.5) * 2400;
    starPositions[i * 3 + 1] = (nextRandom() - 0.5) * 1500;
    starPositions[i * 3 + 2] = -240 - nextRandom() * 1250;
    const brightness = 0.24 + nextRandom() * 0.76;
    starColors[i * 3] = brightness * 0.72;
    starColors[i * 3 + 1] = brightness * 0.9;
    starColors[i * 3 + 2] = brightness;
  }
  const starGeometry = new THREE.BufferGeometry();
  starGeometry.setAttribute('position', new THREE.BufferAttribute(starPositions, 3));
  starGeometry.setAttribute('color', new THREE.BufferAttribute(starColors, 3));
  const starMaterial = new THREE.PointsMaterial({
    size: 4.2,
    map: pointTexture,
    vertexColors: true,
    transparent: true,
    opacity: 0.66,
    sizeAttenuation: true,
    depthWrite: false,
    depthTest: false,
    blending: THREE.AdditiveBlending
  });
  const starPoints = new THREE.Points(starGeometry, starMaterial);
  starPoints.renderOrder = -30;
  scene.add(starPoints);

  const nebulaCount = 4800;
  const nebulaPositions = new Float32Array(nebulaCount * 3);
  const nebulaColors = new Float32Array(nebulaCount * 3);
  const palette = [
    new THREE.Color('#00d7ff'),
    new THREE.Color('#1463ff'),
    new THREE.Color('#7c4dff'),
    new THREE.Color('#ff7a1a')
  ];
  for (let i = 0; i < nebulaCount; i++) {
    const progress = Math.pow(nextRandom(), 0.68);
    const arm = i % 2;
    const radius = 24 + progress * 365;
    const angle = 0.55 + progress * Math.PI * 4.72 + arm * Math.PI + (nextRandom() - 0.5) * (0.3 + progress * 0.48);
    const scatter = (nextRandom() - 0.5) * (24 + radius * 0.18);
    nebulaPositions[i * 3] = Math.cos(angle) * radius + Math.cos(angle + Math.PI / 2) * scatter;
    nebulaPositions[i * 3 + 1] = Math.sin(angle) * radius * 0.64 + Math.sin(angle + Math.PI / 2) * scatter * 0.7;
    nebulaPositions[i * 3 + 2] = -175 + (nextRandom() - 0.5) * (24 + radius * 0.16);
    const colorIndex = i % 43 === 0 ? 3 : (arm + Math.floor(progress * 2)) % 3;
    const color = palette[colorIndex];
    const intensity = (0.48 + nextRandom() * 0.72) * (1.12 - progress * 0.24);
    nebulaColors[i * 3] = color.r * intensity;
    nebulaColors[i * 3 + 1] = color.g * intensity;
    nebulaColors[i * 3 + 2] = color.b * intensity;
  }
  const nebulaGeometry = new THREE.BufferGeometry();
  nebulaGeometry.setAttribute('position', new THREE.BufferAttribute(nebulaPositions, 3));
  nebulaGeometry.setAttribute('color', new THREE.BufferAttribute(nebulaColors, 3));
  const nebulaMaterial = new THREE.PointsMaterial({
    size: 8.8,
    map: pointTexture,
    vertexColors: true,
    transparent: true,
    opacity: 0.76,
    sizeAttenuation: true,
    depthWrite: false,
    depthTest: false,
    blending: THREE.AdditiveBlending
  });
  const nebulaPoints = new THREE.Points(nebulaGeometry, nebulaMaterial);
  nebulaPoints.renderOrder = -20;
  scene.add(nebulaPoints);

  const coreMaterial = new THREE.SpriteMaterial({
    map:coreTexture,
    color:new THREE.Color('#c8fbff'),
    transparent:true,
    opacity:0.92,
    depthWrite:false,
    depthTest:false,
    blending:THREE.AdditiveBlending
  });
  const core = new THREE.Sprite(coreMaterial);
  core.position.set(0, 0, -155);
  core.scale.set(132, 132, 1);
  core.renderOrder = -10;
  scene.add(core);

  // The returned disposer releases every GPU resource created for the backdrop.
  return () => {
    scene.remove(starPoints, nebulaPoints, core);
    starGeometry.dispose();
    starMaterial.dispose();
    nebulaGeometry.dispose();
    nebulaMaterial.dispose();
    coreMaterial.dispose();
    pointTexture.dispose();
    coreTexture.dispose();
  };
}

// ── Component ──────────────────────────────────────────────

// Full-screen 3D memory galaxy: loads the graph, builds the WebGL scene, and
// wires the search, inspector, and signal controls around it.
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
  const [dbSizeMb, setDbSizeMb] = useState<number | undefined>(undefined);
  const [categories, setCategories] = useState<CategoryCount[]>([]);
  const [searchQuery, setSearchQuery] = useState('');
  const [searchResults, setSearchResults] = useState<GraphSearchResult[]>([]);
  const [showSearchResults, setShowSearchResults] = useState(false);
  const [selectedMemory, setSelectedMemory] = useState<MemoryDetail | null>(null);
  const [sidePanelOpen, setSidePanelOpen] = useState(false);
  const [showLabels, setShowLabels] = useState(false);
  const [weightThreshold, setWeightThreshold] = useState(0);
  // Observed edge-weight range, used to calibrate the Edge Floor slider. Real
  // similarity scores occupy a narrow band near the top of 0..1 (production
  // spans roughly 0.68..1.00), so a fixed 0..1 slider leaves most of its travel
  // filtering nothing at all. Defaults to the full range until data arrives.
  const [weightBounds, setWeightBounds] = useState<{ min: number; max: number }>({ min: 0, max: 1 });
  const [clusterEnabled, setClusterEnabled] = useState(true);

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
    let disposeCosmicScene: (() => void) | null = null;
    const motionReduced = window.matchMedia('(prefers-reduced-motion: reduce)').matches;

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
    // Ids of nodes drawn as background dust, filled in once the galaxy layout
    // is built. Link styling consults this to keep dust edges from hazing over
    // the structure they pass through.
    const dustNodeIds = new Set<string>();
    let galaxyTargets = new Map<string, GalaxyTarget>();

    // ── Color helpers ──────────────────────────────────────
    const getNodeColor = (node: GNode): string => {
      if (searchHighlights.has(node.id)) return '#ffd700';
      if (node.category && CATEGORY_FALLBACK[node.category]) return CATEGORY_FALLBACK[node.category];
      if (node.community_id != null) return COMMUNITY_COLORS[node.community_id % COMMUNITY_COLORS.length];
      return '#00d7ff';
    };
    const getNodeOpacity = (node: GNode): number => {
      if (highlightNodes.has(node) || searchHighlights.has(node.id)) return 1.0;
      const decay = node.decay_score ?? 5;
      return Math.max(0.5, Math.min(1.0, decay / 6));
    };
    const getLinkColor = (link: GLink): string => {
      const src = typeof link.source === 'object' ? (link.source as GNode) : null;
      return src ? getNodeColor(src) : '#00d7ff';
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
    // True when either endpoint is background dust. Such edges are the bulk of
    // the graph and criss-cross the whole disc, so at equal weight they blur
    // the arms and clusters into a uniform haze.
    const linkTouchesDust = (link: GLink): boolean => {
      const source = typeof link.source === 'object' ? (link.source as GNode).id : (link.source as string);
      const target = typeof link.target === 'object' ? (link.target as GNode).id : (link.target as string);
      return dustNodeIds.has(source) || dustNodeIds.has(target);
    };

    // How far diffuse-thread edges recede relative to compact-cluster edges.
    // Topology placement has made these local rather than cross-disc noise, so
    // they can remain visible enough to reveal the filaments they now encode.
    const DUST_EDGE_DIM = 0.48;

    // Resolve one mutable force-edge endpoint back to its stable node id.
    const linkEndpointId = (endpoint: GLink['source']): string =>
      typeof endpoint === 'object' ? endpoint.id : endpoint;

    // Attenuate long resting bridges because their screen coverage compounds
    // far faster than local links. Selection still restores full emphasis.
    const linkSpanAttenuation = (link: GLink): number => {
      const source = galaxyTargets.get(linkEndpointId(link.source));
      const target = galaxyTargets.get(linkEndpointId(link.target));
      if (!source || !target) return 1;
      const span = Math.hypot(source.x - target.x, source.y - target.y, source.z - target.z);
      const fullStrengthSpan = 96;
      const falloff = Math.pow(fullStrengthSpan / Math.max(fullStrengthSpan, span), 1.7);
      return Math.max(0.018, Math.min(1, falloff));
    };

    // Return resting or highlighted alpha for one visible semantic relationship.
    const getLinkAlpha = (link: GLink): number => {
      if (highlightLinks.has(link)) return Math.max(0.56, (link.weight ?? 0.5) * 0.98);
      if (hoverNode && !highlightLinks.has(link)) return 0.04 * linkSpanAttenuation(link);
      // Edges blend additively, so thousands of overlapping strands compound
      // into flat white-cyan and erase the node colours underneath. Resting
      // alpha stays low; highlighted edges above still get full presence.
      if ((link.weight ?? 0) >= weightThresholdLocal) {
        const base = 0.09 + (link.weight ?? 0) * 0.2;
        const dustDim = linkTouchesDust(link) ? DUST_EDGE_DIM : 1;
        return base * dustDim * linkSpanAttenuation(link);
      }
      return 0;
    };
    // Return a width that preserves local threads and de-emphasizes long bridges.
    const getLinkWidth = (link: GLink): number => {
      if (highlightLinks.has(link)) return Math.max(0.5, (link.weight ?? 0.5) * 2);
      if ((link.weight ?? 0) < weightThresholdLocal) return 0;
      const base = linkTouchesDust(link) ? 0.16 : 0.42;
      return base * Math.max(0.08, Math.sqrt(linkSpanAttenuation(link)));
    };

    // Return one edge color with the current interaction alpha applied.
    const getVisibleLinkColor = (link: GLink): string => {
      const alpha = getLinkAlpha(link);
      if (alpha <= 0) return 'rgba(0,0,0,0)';
      return withAlpha(getLinkColor(link), alpha);
    };
    const refreshLinkVisuals = () => {
      if (!graphInstance) return;
      graphInstance
        .linkOpacity(1)
        .linkWidth((link: any) => getLinkWidth(link as GLink))
        .linkColor((link: any) => getVisibleLinkColor(link as GLink))
        .linkVisibility((link: any) => {
          if (highlightLinks.has(link)) return true;
          return (link.weight ?? 0) >= weightThresholdLocal;
        });
    };

    const updateNodeVisuals = () => {
      if (!threeRef) return;
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

    const zoomToNode = (memId: number | string) => {
      const id = typeof memId === 'number' ? 'm' + memId : memId;
      const node = nodeMap.get(id);
      if (node && graphInstance && node.x != null) {
        const dist = 120;
        const hyp = Math.hypot(node.x!, node.y!, node.z!);
        const ratio = hyp > 0 ? 1 + dist / hyp : 1;
        graphInstance.cameraPosition(
          { x: node.x! * ratio, y: node.y! * ratio, z: node.z! * ratio },
          { x: node.x, y: node.y, z: node.z },
          1500
        );
        void handleNodeClick(node);
      }
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

    // init loads graph data, creates the WebGL scene, and publishes control hooks to the interface.
    async function init() {
      try {
        const [FG3D, THREE] = await Promise.all([
          import('3d-force-graph') as Promise<any>,
          import('three') as Promise<any>
        ]);
        const ForceGraph3D = FG3D.default;
        threeRef = THREE;

        const [graphData, commData, statsData] = await Promise.all([
          // min_component=2 prunes singleton "dust" (unlinked memories, mostly
          // session auto-captures) so the view shows connected structure rather
          // than a starfield of disconnected points.
          getMemoryGraph(3, 50000, 2),
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

        if (!nodes.length) {
          setLoadError('No memories found. Store some memories first.');
          setLoading(false);
          return;
        }

        // Performance: a browser can't draw tens of thousands of edges (or
        // animate that many sprites) smoothly. Past a threshold we render only a
        // bounded subset and turn off the per-frame extras (breathing); nodes
        // collapse to a single GPU point cloud (see below). The Edge Floor slider
        // filters within the rendered set. All thresholds are constants, so this
        // scales on its own.
        //
        // The subset is chosen to PRESERVE CONNECTIVITY (see selectRenderEdges):
        // a plain top-N-by-weight slice silently drops every edge of any node
        // whose links all sit below the global cutoff, fragmenting clusters into
        // disconnected dust. selectRenderEdges keeps each node's strongest edge
        // first, then fills the budget by weight -- so the structure survives the
        // cap. The cap is higher than the old flat 9k because the skeleton pass
        // spends most of its budget on the connective edges that actually matter.
        const big = nodes.length > 2500;
        const MAX_RENDER_EDGES = 14000;
        const edges: GLink[] = big ? selectRenderEdges(allEdges, MAX_RENDER_EDGES) : allEdges;
        // Report what's actually drawn so the header isn't misleading.
        setEdgeCount(edges.length);

        // Calibrate the Edge Floor control to the weights this graph actually
        // contains, and open on a floor that keeps the strongest connections
        // legible instead of drawing every edge into one saturated wash.
        const weightProfile = describeEdgeWeights(edges);
        setWeightBounds({ min: weightProfile.min, max: weightProfile.max });
        weightThresholdLocal = weightProfile.defaultFloor;
        setWeightThreshold(weightProfile.defaultFloor);

        // Map community IDs onto nodes
        const commMap = new Map<string, number>();
        (commData.communities || []).forEach((c) => {
          (c.top_memories || []).forEach((mid) => commMap.set('m' + mid, c.id));
        });

        // Build neighbor/link lookups
        nodes.forEach((node) => {
          node.neighbors = [];
          node.links = [];
          node.community_id = commMap.get(node.id);
          nodeMap.set(node.id, node);
        });
        edges.forEach((link) => {
          const src = nodeMap.get(link.source as string);
          const tgt = nodeMap.get(link.target as string);
          if (src && tgt) {
            src.neighbors!.push(tgt);
            src.links!.push(link);
            tgt.neighbors!.push(src);
            tgt.links!.push(link);
          }
        });

        // Seed the first visible frame in the final semantic shape. The guide
        // force can still deform this structure through links, drag, and charge.
        galaxyTargets = buildGalaxyTargets(nodes, edges);
        seedGalaxyPositions(nodes, galaxyTargets);

        // Pin the topology-driven layout within the spiral.
        //
        // Dust is the overwhelming majority of nodes, but its links still carry
        // structure. The layout grows those motes from compact semantic anchors
        // along a maximum-affinity forest, so every endpoint must share the same
        // fixed coordinate model. Moving only the anchors tears the forest into
        // long edge beams. The Clusters toggle releases every pin for users who
        // prefer a pure force-directed view.
        const pinGalaxyLayout = (pinned: boolean) => {
          for (const node of nodes) {
            const target = galaxyTargets.get(node.id);
            if (!target) continue;
            if (pinned) {
              (node as any).fx = target.x;
              (node as any).fy = target.y;
              (node as any).fz = target.z;
            } else {
              (node as any).fx = undefined;
              (node as any).fy = undefined;
              (node as any).fz = undefined;
            }
          }
        };
        pinGalaxyLayout(true);

        // Record which nodes are dust so their edges can be pushed back.
        for (const node of nodes) {
          if (galaxyTargets.get(node.id)?.diffuse) dustNodeIds.add(node.id);
        }

        const ringTexture = createRingTexture(THREE);
        // Pool of 8 organism textures, reused across nodes
        const organismTextures = Array.from({ length: 8 }, (_, i) => createOrganismTexture(THREE, i * 137));
        const breathPhases = new Map<string, number>();

        // Big-graph node rendering: ONE GPU point cloud (single draw call) with
        // per-point color + size, using the organism glow as the point sprite.
        // Positions are synced from the simulation each tick (see onEngineTick).
        // Small graphs keep the richer per-node sprites with hover/click/breathing.
        let pointGeom: any = null;
        let pointMat: any = null;
        let nodeCloud: any = null;
        if (big) {
          const count = nodes.length;
          const positions = new Float32Array(count * 3);
          const colors = new Float32Array(count * 3);
          const sizes = new Float32Array(count);
          const phases = new Float32Array(count);
          const col = new THREE.Color();
          // Background dust is drawn smaller and dimmer than structural nodes.
          // Bookkeeping categories are the majority of any real instance, so at
          // equal weight they wash out the communities that carry the meaning;
          // pushing them back is what lets the clusters read as clusters.
          // Dust points carry the spiral arms, so they stay bright enough to
          // draw them. It is the dust EDGES that had to recede, not the motes:
          // dimming the points instead simply erased the galaxy's shape.
          const DUST_BRIGHTNESS = 0.95;
          const DUST_SIZE = 0.9;
          nodes.forEach((node, i) => {
            col.set(getNodeColor(node));
            const isDust = galaxyTargets.get(node.id)?.diffuse === true;
            const brightness = isDust ? DUST_BRIGHTNESS : 1;
            colors[i * 3] = col.r * brightness;
            colors[i * 3 + 1] = col.g * brightness;
            colors[i * 3 + 2] = col.b * brightness;
            const baseSize = Math.max(8, ((node.importance || 5) * 1.8 + (node.size || 0) * 0.4) * 2.4);
            sizes[i] = isDust ? baseSize * DUST_SIZE : baseSize;
            phases[i] = (i * 0.7) % (Math.PI * 2);
          });
          pointGeom = new THREE.BufferGeometry();
          pointGeom.setAttribute('position', new THREE.BufferAttribute(positions, 3));
          pointGeom.setAttribute('aColor', new THREE.BufferAttribute(colors, 3));
          pointGeom.setAttribute('size', new THREE.BufferAttribute(sizes, 1));
          pointGeom.setAttribute('aPhase', new THREE.BufferAttribute(phases, 1));
          pointMat = new THREE.ShaderMaterial({
            uniforms: { uTex: { value: organismTextures[0] }, uTime: { value: 0 } },
            transparent: true,
            depthWrite: false,
            blending: THREE.AdditiveBlending,
            vertexShader:
              'attribute float size;\n' +
              'attribute float aPhase;\n' +
              'attribute vec3 aColor;\n' +
              'uniform float uTime;\n' +
              'varying vec3 vColor;\n' +
              'void main() {\n' +
              '  vColor = aColor;\n' +
              // Gentle per-point breathing pulse, computed on the GPU.
              '  float breathe = 1.0 + sin(uTime * 0.8 + aPhase) * 0.11;\n' +
              '  vec4 mv = modelViewMatrix * vec4(position, 1.0);\n' +
              '  gl_PointSize = size * breathe * (440.0 / max(1.0, -mv.z));\n' +
              '  gl_Position = projectionMatrix * mv;\n' +
              '}',
            // Brighter than 1:1 -- additive blending plus a color boost makes the
            // points read as glowing cells rather than dim specks.
            fragmentShader:
              'uniform sampler2D uTex;\n' +
              'varying vec3 vColor;\n' +
              'void main() {\n' +
              '  vec4 tex = texture2D(uTex, gl_PointCoord);\n' +
              '  if (tex.a < 0.02) discard;\n' +
              '  gl_FragColor = vec4(vColor * 2.3, 1.0) * tex;\n' +
              '}'
          });
          nodeCloud = new THREE.Points(pointGeom, pointMat);
          nodeCloud.frustumCulled = false;
        }

        const graph = new ForceGraph3D(container)
          .graphData({ nodes, links: edges })
          .backgroundColor('#05060d')
          .showNavInfo(false)
          .nodeLabel(() => '')
          .nodeVal((n: any) => (n as GNode).importance || 5)
          .linkSource('source')
          .linkTarget('target')
          // Living organism nodes with optional text labels
          .nodeThreeObject((node: any) => {
            // Big graphs draw nodes via the single point cloud; give the lib an
            // empty object so it tracks position for the sim without a draw call.
            if (big) return new THREE.Object3D();
            const n = node as GNode;
            const baseSize = Math.max(10, (n.importance || 5) * 3.1 + (n.size || 0) * 0.65);
            const idNum = Number.parseInt(n.id.replace(/\D/g, '') || '0', 10);
            const tex = organismTextures[idNum % organismTextures.length];
            breathPhases.set(n.id, (idNum * 0.7) % (Math.PI * 2));

            const material = new THREE.SpriteMaterial({
              map: tex,
              color: new THREE.Color(getNodeColor(n)),
              transparent: true,
              opacity: getNodeOpacity(n),
              depthWrite: false,
              blending: THREE.AdditiveBlending
            });
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
                depthWrite: false,
                blending: THREE.AdditiveBlending
              });
              const ring = new THREE.Sprite(ringMat);
              ring.scale.set(baseSize * 1.15, baseSize * 1.15, baseSize * 1.15);
              group.add(ring);

              // Text label (hidden by default, toggled via showLabels)
              const canvas = document.createElement('canvas');
              const ctx = canvas.getContext('2d')!;
              const text = n.label || n.content?.slice(0, 30) || n.id;
              canvas.width = 256;
              canvas.height = 40;
              ctx.font = '20px Inter, sans-serif';
              ctx.fillStyle = 'white';
              ctx.textAlign = 'center';
              ctx.fillText(text.length > 28 ? text.slice(0, 28) + '...' : text, 128, 28);
              const labelTex = new THREE.CanvasTexture(canvas);
              const labelMat = new THREE.SpriteMaterial({ map: labelTex, transparent: true, opacity: 0.7, depthWrite: false });
              const label = new THREE.Sprite(labelMat);
              label.scale.set(baseSize * 2.5, baseSize * 0.4, 1);
              label.position.set(0, baseSize * 0.8, 0);
              label.visible = false;
              group.add(label);
              nodeLabels.set(n.id, label);
              return group;
            }
            return sprite;
          })
          // Breathing animation -- nodes gently pulse like living cells. Skipped
          // on big graphs: scaling thousands of sprites per tick is too costly
          // and the pulse is imperceptible at that zoom anyway.
          .onEngineTick(() => {
            if (big) {
              // Drive the point cloud from the live simulation positions.
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
              return;
            }
            const t = motionReduced ? 0 : performance.now() * 0.001;
            nodeSprites.forEach((entry, id) => {
              const phase = breathPhases.get(id) ?? 0;
              const breathScale = motionReduced ? 1 : 1 + Math.sin(t * 0.8 + phase) * 0.08;
              const sizeVal = entry.baseSize * breathScale;
              const isHovered = highlightNodes.has(nodeMap.get(id)!);
              const scale = isHovered ? sizeVal * 1.3 : sizeVal;
              entry.sprite.scale.set(scale, scale, scale);
            });
          })
          // Layer 1: faint static edges
          .linkWidth((link: any) => getLinkWidth(link as GLink))
          .linkOpacity(1)
          .linkColor((link: any) => getVisibleLinkColor(link as GLink))
          // Flow-trail particles were removed: they only ever rendered on the
          // small-graph path (big graphs disable them), so they never appeared
          // in production and read as an unstyled default. Hover/selection
          // feedback comes from link colour + opacity (see getVisibleLinkColor).
          // Interactions
          .onNodeHover((node: any) => handleNodeHover(node as GNode | null))
          .onNodeClick((node: any) => void handleNodeClick(node as GNode))
          .onBackgroundClick(() => {
            if (!showSearchResultsRef.current) closePanel();
          })
          // Seeded positions make blocking pre-warm unnecessary. Both paths
          // paint immediately and settle within a bounded amount of work.
          .warmupTicks(0)
          .cooldownTicks(big ? 36 : 120)
          .d3AlphaDecay(big ? 0.075 : 0.036)
          .d3VelocityDecay(big ? 0.58 : 0.48);

        graphInstance = graph;

        // Force canvas background to the same deep-space black as the interface shell.
        const canvas = graph.renderer().domElement;
        canvas.style.backgroundColor = '#05060d';

        disposeCosmicScene = addGalaxyBackdrop(THREE, graph.scene());

        // Add the big-graph node point cloud to the live scene, and drive its
        // breathing pulse from a lightweight rAF (just advances a time uniform;
        // the GPU does the per-point work, so it stays alive even after settle).
        if (nodeCloud) {
          graph.scene().add(nodeCloud);
          // animateCloud advances one shader uniform while the GPU handles every point.
          const animateCloud = () => {
            if (destroyed) return;
            if (pointMat) pointMat.uniforms.uTime.value = performance.now() * 0.001;
            cloudRaf = requestAnimationFrame(animateCloud);
          };
          if (!motionReduced) cloudRaf = requestAnimationFrame(animateCloud);
        }

        // ── Guided, scale-invariant force model ───────────────
        // The O(n) guide keeps semantic groups on two spiral arms while link,
        // charge, drag, and orbit preserve the graph's live three-dimensional behavior.
        graph.d3Force('galaxy', makeGalaxyGuideForce(galaxyTargets, GALAXY_GUIDE_STRENGTH));

        // Repulsion: bigger (more important) memories push a little harder, so
        // hubs get room while leaves pack in. distanceMax keeps it O(n) friendly
        // and stops far clusters from blasting each other apart.
        graph
          .d3Force('charge')
          ?.strength((node: any) => -(34 + ((node as GNode).importance || 5) * 6))
          .distanceMax(700)
          .theta(0.9);

        // Attraction: EVERY link pulls (no zeroed weak links), so the graph
        // stays one connected organism. Stronger similarity -> shorter, firmer
        // edge; weak bridges -> longer, softer -- structure emerges from this.
        graph
          .d3Force('link')
          ?.distance((link: any) => {
            const weight = Math.min(1, link.weight ?? 0.3);
            return linkStaysWithinGroup(link as GLink) ? 18 + (1 - weight) * 42 : 118 + (1 - weight) * 96;
          })
          .strength((link: any) => {
            const weight = Math.min(1, link.weight ?? 0.3);
            return linkStaysWithinGroup(link as GLink) ? 0.16 + weight * 0.32 : 0.025 + weight * 0.075;
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

        // Frame the whole galaxy from the live simulation coordinates.
        //
        // graph.zoomToFit() cannot be used here: the library derives its bounding
        // box from node object GEOMETRY, and this view draws nodes as sprites or
        // as a single GPU point cloud whose per-point positions are invisible to
        // that walk, so getGraphBbox() returns null and the call silently does
        // nothing. Measuring the node coordinates directly is both correct and
        // independent of how the nodes happen to be rendered.
        const fitGalaxyView = (durationMs = 800) => {
          if (destroyed || !graphInstance) return;
          const positioned = (graphInstance.graphData()?.nodes ?? []).filter(
            (node: any) => Number.isFinite(node.x) && Number.isFinite(node.y) && Number.isFinite(node.z)
          );
          // Nothing to frame yet -- the simulation has not produced coordinates.
          if (!positioned.length) return;

          let minX = Infinity, minY = Infinity, minZ = Infinity;
          let maxX = -Infinity, maxY = -Infinity, maxZ = -Infinity;
          for (const node of positioned) {
            if (node.x < minX) minX = node.x;
            if (node.y < minY) minY = node.y;
            if (node.z < minZ) minZ = node.z;
            if (node.x > maxX) maxX = node.x;
            if (node.y > maxY) maxY = node.y;
            if (node.z > maxZ) maxZ = node.z;
          }
          const centre = { x: (minX + maxX) / 2, y: (minY + maxY) / 2, z: (minZ + maxZ) / 2 };
          // True bounding sphere: the furthest node from the centre. The box
          // diagonal would badly overstate the radius of a wide flat disc --
          // the galaxy's usual shape -- and leave it marooned in dead space.
          let furthest = 0;
          for (const node of positioned) {
            const away = Math.hypot(node.x - centre.x, node.y - centre.y, node.z - centre.z);
            if (away > furthest) furthest = away;
          }
          // Floored so a single node still yields a sane camera distance
          // rather than collapsing the camera onto the node itself.
          const radius = Math.max(40, furthest);

          const camera = graphInstance.camera?.();
          const verticalFov = ((camera?.fov ?? 50) * Math.PI) / 180;
          const aspect = camera?.aspect || 1;
          // Fit the bounding sphere on whichever axis is tighter: for a wide
          // viewport that is the vertical one, for a tall one the horizontal.
          const horizontalFov = 2 * Math.atan(Math.tan(verticalFov / 2) * aspect);
          const distance =
            (radius / Math.sin(Math.min(verticalFov, horizontalFov) / 2)) * FIT_PADDING_FACTOR;

          // Preserve the direction the viewer is already looking from, so
          // fitting reframes the galaxy without also spinning it.
          const position = camera?.position;
          let dirX = (position?.x ?? 0) - centre.x;
          let dirY = (position?.y ?? 0) - centre.y;
          let dirZ = (position?.z ?? 0) - centre.z;
          const length = Math.hypot(dirX, dirY, dirZ);
          if (!(length > 1e-6)) {
            dirX = 0;
            dirY = 0;
            dirZ = 1;
          } else {
            dirX /= length;
            dirY /= length;
            dirZ /= length;
          }

          graphInstance.cameraPosition(
            {
              x: centre.x + dirX * distance,
              y: centre.y + dirY * distance,
              z: centre.z + dirZ * distance
            },
            centre,
            durationMs
          );
        };

        // Fit after settling
        setTimeout(() => {
          fitGalaxyView();
        }, 900);

        // Publish the imperative handle for the UI controls.
        apiRef.current = {
          setWeight: (v: number) => {
            weightThresholdLocal = v;
            refreshLinkVisuals();
          },
          setLabels: (v: boolean) => {
            nodeLabels.forEach((label) => {
              label.visible = v;
            });
          },
          setClusters: (v: boolean) => {
            if (!graphInstance) return;
            pinGalaxyLayout(v);
            graphInstance.d3Force('galaxy', v ? makeGalaxyGuideForce(galaxyTargets, GALAXY_GUIDE_STRENGTH) : null);
            graphInstance.d3ReheatSimulation();
          },
          fitView: () => fitGalaxyView(),
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
      if (resizeHandler) window.removeEventListener('resize', resizeHandler);
      disposeCosmicScene?.();
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

  // ── Interface shell ─────────────────────────────────────
  return (
    <div className="memgraph-root fixed inset-0 z-40 overflow-hidden">
      <div
        ref={containerRef}
        className="memgraph-canvas w-full h-full"
        role="img"
        aria-label={`Interactive memory galaxy with ${nodeCount.toLocaleString()} memories and ${edgeCount.toLocaleString()} links. Use search to select a memory without a pointer.`}
      />

      {loading && (
        <div className="memgraph-state absolute inset-0 flex items-center justify-center z-50">
          <div className="memgraph-state__card text-center">
            <div className="memgraph-loader w-12 h-12 rounded-full mx-auto mb-4" />
            <p className="memgraph-kicker">KLEOS // MEMORY GALAXY</p>
            <p className="text-gray-500 text-sm">Mapping live memory topology...</p>
          </div>
        </div>
      )}

      {loadError && (
        <div className="memgraph-state absolute inset-0 flex items-center justify-center z-50">
          <div className="memgraph-state__card memgraph-state__card--error p-6 max-w-md text-center">
            <p className="text-red-400 text-sm mb-2">Failed to load graph</p>
            <p className="text-red-300/60 text-xs font-mono">{loadError}</p>
            <a
              href="/"
              className="memgraph-return inline-block mt-4 px-4 py-2 text-sm transition-colors"
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
            <a href="/" className="memgraph-back flex items-center gap-2 transition-colors shrink-0" aria-label="Back to dashboard">
              <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M15 19l-7-7 7-7" />
              </svg>
            </a>

            <div className="memgraph-brand shrink-0">
              <span className="memgraph-brand__name">KLEOS</span>
              <span className="memgraph-brand__mode">MEMORY GALAXY</span>
            </div>

            <span className="memgraph-live shrink-0"><i /> LIVE</span>

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
              {dbSizeMb != null && <span><strong>{dbSizeMb.toFixed(1)}</strong> MB</span>}
            </div>
          </header>

          {/* Graph controls */}
          <section className="memgraph-instruments absolute z-50 flex flex-col gap-3 p-4 memgraph-glass-panel" aria-label="Galaxy controls">
            <div className="memgraph-panel-heading">SIGNAL CONTROLS</div>
            <div>
              <div className="memgraph-control-label text-[10px] uppercase tracking-wider mb-1.5">Edge floor</div>
              <div className="flex items-center gap-2">
                <input
                  type="range"
                  min={weightBounds.min}
                  max={weightBounds.max}
                  step={(weightBounds.max - weightBounds.min) / 100}
                  value={weightThreshold}
                  onChange={(e) => setWeightThreshold(Number.parseFloat(e.target.value))}
                  aria-label="Minimum edge weight"
                  className="memgraph-range-slider w-28"
                />
                <span className="memgraph-control-value text-[10px] w-7 text-right">{weightThreshold.toFixed(2)}</span>
              </div>
            </div>

            <button onClick={() => setShowLabels((v) => !v)} aria-pressed={showLabels} className="memgraph-toggle flex items-center gap-2 group">
              <div className={`memgraph-switch w-7 h-4 rounded-full relative transition-colors ${showLabels ? 'is-on' : ''}`}>
                <div className="memgraph-switch__thumb absolute left-0.5 top-0.5 w-3 h-3 rounded-full transition-all" />
              </div>
              <span className="text-[10px] transition-colors">Labels</span>
            </button>

            <button onClick={() => setClusterEnabled((v) => !v)} aria-pressed={clusterEnabled} className="memgraph-toggle flex items-center gap-2 group">
              <div className={`memgraph-switch w-7 h-4 rounded-full relative transition-colors ${clusterEnabled ? 'is-on' : ''}`}>
                <div className="memgraph-switch__thumb absolute left-0.5 top-0.5 w-3 h-3 rounded-full transition-all" />
              </div>
              <span className="text-[10px] transition-colors">Clusters</span>
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
            <aside className="memgraph-inspector absolute top-0 right-0 bottom-0 w-[380px] z-50 overflow-y-auto memgraph-side-panel memgraph-glass-panel-solid">
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
                                  background: `${CATEGORY_FALLBACK[result.category] || '#00d7ff'}20`,
                                  color: CATEGORY_FALLBACK[result.category] || '#00d7ff'
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
                          background: `${CATEGORY_FALLBACK[selectedMemory.category] || '#00d7ff'}20`,
                          color: CATEGORY_FALLBACK[selectedMemory.category] || '#00d7ff'
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
                              background: CATEGORY_FALLBACK[selectedMemory.category] || '#00d7ff'
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

          {/* Live category ledger */}
          {categories.length > 0 && !sidePanelOpen && (
            <section className="memgraph-category-ledger absolute z-40 p-3 memgraph-glass-panel-light" aria-label="Memory categories">
              <div className="memgraph-panel-heading mb-2">MEMORY LEDGER</div>
              <div className="space-y-1">
                {categories.slice(0, 8).map((cat) => (
                  <div key={cat.category} className="memgraph-ledger-row flex items-center gap-2">
                    <div className="memgraph-ledger-dot w-2 h-2 rounded-full" style={{ background: CATEGORY_FALLBACK[cat.category] || '#00d7ff' }} />
                    <span className="text-[10px]">{cat.category}</span>
                    <span className="text-[10px] ml-auto">{cat.count.toLocaleString()}</span>
                  </div>
                ))}
              </div>
            </section>
          )}

          <div className="memgraph-gesture-hint absolute z-40" aria-hidden="true">
            DRAG TO ORBIT <span>·</span> SCROLL TO ZOOM <span>·</span> SELECT A MEMORY
          </div>
        </>
      )}
    </div>
  );
}
