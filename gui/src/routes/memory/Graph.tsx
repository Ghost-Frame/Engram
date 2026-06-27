import { useEffect, useRef, useState, type FormEvent } from 'react';
import {
  getCommunities,
  getMemoryDetail,
  getMemoryGraph,
  getStats,
  searchGraph,
  type CategoryCount,
  type GraphSearchResult,
  type MemoryDetail
} from '$lib/api/graph';
import { selectRenderEdges } from '$lib/graph/selectRenderEdges';
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

interface GLink {
  source: string | GNode;
  target: string | GNode;
  type: string;
  weight: number;
}

// ── Constants ──────────────────────────────────────────────

const COMMUNITY_COLORS = [
  '#4fc3f7', '#ba68c8', '#81c784', '#ff8a65',
  '#64b5f6', '#f06292', '#fff176', '#4db6ac',
  '#e57373', '#7986cb', '#aed581', '#ffb74d'
];

const CATEGORY_FALLBACK: Record<string, string> = {
  general: '#4fc3f7', decision: '#ba68c8', task: '#81c784',
  state: '#ff8a65', discovery: '#64b5f6', reference: '#f06292',
  issue: '#e57373', preference: '#fff176', credential: '#7986cb'
};

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

// ── Emergent cluster force ─────────────────────────────────
// The old graph pulled nodes toward PREDETERMINED Fibonacci-sphere positions,
// which forced communities into fixed scattered blobs and had to be re-tuned as
// the graph grew. This instead recomputes each community's centroid from the
// LIVE node positions every tick and applies a gentle pull toward it -- so
// grouping EMERGES from where the graph naturally settles rather than being
// imposed. Strength is a constant (no dependence on node count), so it holds at
// any scale without code changes.
function makeEmergentClusterForce(strength: number) {
  let nodes: GNode[] = [];
  const force: any = (alpha: number) => {
    // Per-community running centroid from current positions.
    const sums = new Map<string, { x: number; y: number; z: number; n: number }>();
    for (const node of nodes) {
      const cid = String(node.community_id ?? node.category ?? 'default');
      let s = sums.get(cid);
      if (!s) {
        s = { x: 0, y: 0, z: 0, n: 0 };
        sums.set(cid, s);
      }
      s.x += node.x ?? 0;
      s.y += node.y ?? 0;
      s.z += node.z ?? 0;
      s.n += 1;
    }
    for (const node of nodes) {
      const cid = String(node.community_id ?? node.category ?? 'default');
      const s = sums.get(cid);
      if (!s || s.n === 0) continue;
      node.vx = (node.vx ?? 0) + (s.x / s.n - (node.x ?? 0)) * strength * alpha;
      node.vy = (node.vy ?? 0) + (s.y / s.n - (node.y ?? 0)) * strength * alpha;
      node.vz = (node.vz ?? 0) + (s.z / s.n - (node.z ?? 0)) * strength * alpha;
    }
  };
  force.initialize = (n: GNode[]) => {
    nodes = n;
  };
  return force;
}

// Gentle same-community cohesion strength (emergent, not positional).
const CLUSTER_STRENGTH = 0.06;

// ── Star field (verbatim) ──────────────────────────────────

function addStarField(THREE: any, scene: any) {
  const count = 400;
  const positions = new Float32Array(count * 3);
  const colors = new Float32Array(count * 3);
  for (let i = 0; i < count; i++) {
    positions[i * 3] = (Math.random() - 0.5) * 5000;
    positions[i * 3 + 1] = (Math.random() - 0.5) * 5000;
    positions[i * 3 + 2] = (Math.random() - 0.5) * 5000;
    const b = 0.3 + Math.random() * 0.7;
    colors[i * 3] = b;
    colors[i * 3 + 1] = b;
    colors[i * 3 + 2] = b + Math.random() * 0.15;
  }
  const geo = new THREE.BufferGeometry();
  geo.setAttribute('position', new THREE.BufferAttribute(positions, 3));
  geo.setAttribute('color', new THREE.BufferAttribute(colors, 3));
  scene.add(
    new THREE.Points(
      geo,
      new THREE.PointsMaterial({
        size: 0.8,
        vertexColors: true,
        transparent: true,
        opacity: 0.7,
        sizeAttenuation: true
      })
    )
  );
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

    // ── Color helpers ──────────────────────────────────────
    const getNodeColor = (node: GNode): string => {
      if (searchHighlights.has(node.id)) return '#ffd700';
      if (node.category && CATEGORY_FALLBACK[node.category]) return CATEGORY_FALLBACK[node.category];
      if (node.community_id != null) return COMMUNITY_COLORS[node.community_id % COMMUNITY_COLORS.length];
      return '#4fc3f7';
    };
    const getNodeOpacity = (node: GNode): number => {
      if (highlightNodes.has(node) || searchHighlights.has(node.id)) return 1.0;
      const decay = node.decay_score ?? 5;
      return Math.max(0.5, Math.min(1.0, decay / 6));
    };
    const getLinkColor = (link: GLink): string => {
      const src = typeof link.source === 'object' ? (link.source as GNode) : null;
      return src ? getNodeColor(src) : '#4fc3f7';
    };
    const getParticleColor = (link: GLink): string => {
      if (link.type === 'corrects' || link.type === 'updates') return '#ff8a65';
      if (link.type === 'contradicts' || link.type === 'conflicts') return '#e57373';
      return getLinkColor(link);
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
      if (hoverNode && !highlightLinks.has(link)) return 0.04;
      if ((link.weight ?? 0) >= weightThresholdLocal) return 0.05 + (link.weight ?? 0) * 0.12;
      return 0;
    };
    const getVisibleLinkColor = (link: GLink): string => {
      const alpha = getLinkAlpha(link);
      if (alpha <= 0) return 'rgba(0,0,0,0)';
      return withAlpha(getLinkColor(link), alpha);
    };
    const refreshLinkVisuals = () => {
      if (!graphInstance) return;
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
        // bounded subset and turn off the per-frame extras (flow particles,
        // breathing); nodes collapse to a single GPU point cloud (see below).
        // The Edge Floor slider filters within the rendered set. All thresholds
        // are constants, so this scales on its own.
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

        // Top strongest edges get persistent flow particles (Layer 2). Particles
        // animate every frame, so big graphs get none.
        const particleEdges = new Set<GLink>(
          big
            ? []
            : [...edges]
                .filter((l) => (l.weight ?? 0) >= 0.5)
                .sort((a, b) => (b.weight ?? 0) - (a.weight ?? 0))
                .slice(0, 200)
        );

        // Clustering is emergent (see makeEmergentClusterForce) -- no
        // predetermined centroids are computed.

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
          nodes.forEach((node, i) => {
            col.set(getNodeColor(node));
            colors[i * 3] = col.r;
            colors[i * 3 + 1] = col.g;
            colors[i * 3 + 2] = col.b;
            sizes[i] = Math.max(8, ((node.importance || 5) * 1.8 + (node.size || 0) * 0.4) * 2.4);
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
          .backgroundColor('#0a0a0a')
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
            const baseSize = Math.max(4, (n.importance || 5) * 1.8 + (n.size || 0) * 0.4);
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
            const t = performance.now() * 0.001;
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
            if (highlightLinks.has(link)) return Math.max(0.5, (link.weight ?? 0.5) * 2);
            if ((link.weight ?? 0) >= weightThresholdLocal) return 0.15;
            return 0;
          })
          .linkOpacity(1)
          .linkColor((link: any) => getVisibleLinkColor(link as GLink))
          // Layer 2: flow trail particles on strong + hovered links
          .linkDirectionalParticles((link: any) => {
            if (highlightLinks.has(link)) return Math.floor((link.weight ?? 0.5) * 6) * 2;
            if (particleEdges.has(link)) return Math.floor((link.weight ?? 0.5) * 6);
            return 0;
          })
          .linkDirectionalParticleWidth((link: any) => {
            if (highlightLinks.has(link)) return 2.5 + (link.weight ?? 0.5) * 2;
            return 1.5 + (link.weight ?? 0.5) * 2;
          })
          .linkDirectionalParticleSpeed((link: any) => 0.002 + (link.weight ?? 0.5) * 0.006)
          .linkDirectionalParticleColor((link: any) => getParticleColor(link))
          // Interactions
          .onNodeHover((node: any) => handleNodeHover(node as GNode | null))
          .onNodeClick((node: any) => void handleNodeClick(node as GNode))
          .onBackgroundClick(() => {
            if (!showSearchResultsRef.current) closePanel();
          })
          // Big graphs paint immediately and settle live (no pre-warm freeze that
          // would block the main thread and hang on "Loading"); smaller graphs
          // pre-warm for a tidy first frame. Big graphs also cool faster (higher
          // alpha/velocity decay + fewer ticks) so the brief settle lag is short.
          .warmupTicks(big ? 0 : 150)
          .cooldownTicks(big ? 45 : 400)
          .d3AlphaDecay(big ? 0.06 : 0.0228)
          .d3VelocityDecay(big ? 0.55 : 0.4);

        graphInstance = graph;

        // Force canvas background to near-black
        const canvas = graph.renderer().domElement;
        canvas.style.backgroundColor = '#0a0a0a';

        addStarField(THREE, graph.scene());

        // Add the big-graph node point cloud to the live scene, and drive its
        // breathing pulse from a lightweight rAF (just advances a time uniform;
        // the GPU does the per-point work, so it stays alive even after settle).
        if (nodeCloud) {
          graph.scene().add(nodeCloud);
          const animateCloud = () => {
            if (destroyed) return;
            if (pointMat) pointMat.uniforms.uTime.value = performance.now() * 0.001;
            cloudRaf = requestAnimationFrame(animateCloud);
          };
          cloudRaf = requestAnimationFrame(animateCloud);
        }

        // ── Organic, scale-invariant force model ──────────────
        // Every constant below is independent of node count, so the layout
        // holds its local character (edge length, node spacing, cohesion) from
        // dozens to thousands of memories without re-tuning. The graph simply
        // grows in extent; zoomToFit handles the camera.

        // Emergent same-community cohesion (centroid from live positions).
        graph.d3Force('cluster', makeEmergentClusterForce(CLUSTER_STRENGTH));

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
          ?.distance((link: any) => 14 + (1 - Math.min(1, link.weight ?? 0.3)) * 46)
          .strength((link: any) => 0.12 + Math.min(1, link.weight ?? 0.3) * 0.5);

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

        // Fit after settling
        setTimeout(() => {
          if (!destroyed) graph.zoomToFit(800, 50);
        }, 3000);

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
            graphInstance.d3Force('cluster', v ? makeEmergentClusterForce(CLUSTER_STRENGTH) : null);
            graphInstance.d3ReheatSimulation();
          },
          fitView: () => graphInstance?.zoomToFit(800, 50),
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

  // ── Template (ported verbatim from the old graph) ────────
  return (
    <div className="fixed inset-0 z-40 bg-[#0a0a0a] overflow-hidden">
      <div ref={containerRef} className="w-full h-full" />

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
          {/* Top Bar */}
          <div className="absolute top-0 left-0 right-0 z-50 flex items-center gap-4 px-5 py-3 memgraph-topbar-gradient">
            <a href="/" className="flex items-center gap-2 text-gray-500 hover:text-gray-300 transition-colors shrink-0">
              <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M15 19l-7-7 7-7" />
              </svg>
              <span className="text-xs">Back</span>
            </a>

            <span className="text-sm font-bold tracking-widest bg-gradient-to-r from-teal-300 to-cyan-400 bg-clip-text text-transparent shrink-0">
              KLEOS
            </span>

            <form className="flex-1 max-w-md" onSubmit={onSearchSubmit}>
              <div className="relative">
                <input
                  type="text"
                  value={searchQuery}
                  onChange={(e) => setSearchQuery(e.target.value)}
                  placeholder="Search memories..."
                  className="w-full px-4 py-2 pl-9 bg-white/5 border border-white/10 rounded-lg text-sm text-gray-200 placeholder-gray-600 focus:outline-none focus:border-teal-500/50 transition-all"
                />
                <svg
                  className="absolute left-3 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-gray-600"
                  fill="none"
                  stroke="currentColor"
                  viewBox="0 0 24 24"
                >
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z" />
                </svg>
              </div>
            </form>

            <div className="flex items-center gap-4 text-[11px] text-gray-500 shrink-0">
              <span>
                <span className="text-gray-400 font-medium">{nodeCount}</span> nodes
              </span>
              <span>
                <span className="text-gray-400 font-medium">{edgeCount}</span> edges
              </span>
              {dbSizeMb != null && <span>{dbSizeMb.toFixed(1)} MB</span>}
            </div>
          </div>

          {/* Controls (bottom-left) */}
          <div className="absolute bottom-5 left-5 z-50 flex flex-col gap-3 p-4 rounded-xl memgraph-glass-panel">
            <div>
              <div className="text-[10px] text-gray-500 uppercase tracking-wider mb-1.5">Edge Floor</div>
              <div className="flex items-center gap-2">
                <input
                  type="range"
                  min={0}
                  max={1}
                  step={0.05}
                  value={weightThreshold}
                  onChange={(e) => setWeightThreshold(Number.parseFloat(e.target.value))}
                  className="memgraph-range-slider w-28"
                />
                <span className="text-[10px] text-gray-600 w-7 text-right">{weightThreshold.toFixed(2)}</span>
              </div>
            </div>

            <button onClick={() => setShowLabels((v) => !v)} className="flex items-center gap-2 group">
              <div className={`w-7 h-4 rounded-full relative transition-colors ${showLabels ? 'bg-teal-500/60' : 'bg-gray-700'}`}>
                <div className={`absolute left-0.5 top-0.5 w-3 h-3 rounded-full transition-all ${showLabels ? 'translate-x-3 bg-teal-300' : 'bg-gray-400'}`} />
              </div>
              <span className="text-[10px] text-gray-500 group-hover:text-gray-400 transition-colors">Labels</span>
            </button>

            <button onClick={() => setClusterEnabled((v) => !v)} className="flex items-center gap-2 group">
              <div className={`w-7 h-4 rounded-full relative transition-colors ${clusterEnabled ? 'bg-teal-500/60' : 'bg-gray-700'}`}>
                <div className={`absolute left-0.5 top-0.5 w-3 h-3 rounded-full transition-all ${clusterEnabled ? 'translate-x-3 bg-teal-300' : 'bg-gray-400'}`} />
              </div>
              <span className="text-[10px] text-gray-500 group-hover:text-gray-400 transition-colors">Clusters</span>
            </button>

            <button
              onClick={() => apiRef.current?.fitView()}
              className="px-3 py-1.5 bg-white/5 hover:bg-white/10 border border-white/[0.08] rounded-lg text-[10px] text-gray-400 hover:text-gray-300 transition-all"
            >
              Fit View
            </button>
          </div>

          {/* Side Panel */}
          {sidePanelOpen && (
            <div className="absolute top-0 right-0 bottom-0 w-[380px] z-50 overflow-y-auto memgraph-side-panel memgraph-glass-panel-solid">
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
            </div>
          )}

          {/* Category Legend (bottom-right, hidden when panel open) */}
          {categories.length > 0 && !sidePanelOpen && (
            <div className="absolute bottom-5 right-5 z-40 p-3 rounded-xl memgraph-glass-panel-light">
              <div className="text-[9px] text-gray-600 uppercase tracking-wider mb-2">Categories</div>
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
        </>
      )}
    </div>
  );
}
