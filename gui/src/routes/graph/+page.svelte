<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { getGraph, getCommunities, getStats, getMemoryDetail, searchGraph } from '$lib/stores/engram';

  // ── Types ──────────────────────────────────────────────

  interface GNode {
    id: string;
    label: string;
    type: string;
    category: string;
    importance: number;
    group: string;
    size: number;
    source: string;
    created_at: string;
    is_static: boolean;
    content: string;
    source_count: number;
    community_id?: number;
    decay_score?: number;
    x?: number; y?: number; z?: number;
    vx?: number; vy?: number; vz?: number;
    neighbors?: GNode[];
    links?: GLink[];
  }

  interface GLink {
    source: string | GNode;
    target: string | GNode;
    type: string;
    weight: number;
  }

  interface MemoryDetail {
    id: number;
    content: string;
    category: string;
    source: string;
    importance: number;
    version: number;
    is_latest: boolean;
    is_static: boolean;
    created_at: string;
    updated_at: string;
    tags: string[];
    decay_score: number;
    access_count: number;
    last_accessed_at: string;
    episode?: { id: number; title: string };
    links?: Array<{ id: number; similarity: number; type: string; content: string; category: string }>;
    version_chain?: Array<{ id: number; content: string; version: number; is_latest: boolean }>;
  }

  // ── State ──────────────────────────────────────────────

  let container: HTMLDivElement;
  let graphInstance: any = null;
  let threeRef: any = null;
  let clusterCentroids: Record<string, { x: number; y: number; z: number }> = {};

  let selectedMemory: MemoryDetail | null = $state(null);
  let sidePanelOpen = $state(false);
  let searchQuery = $state('');
  let searchResults: any[] = $state([]);
  let showSearchResults = $state(false);
  let stats: any = $state(null);
  let showLabels = $state(false);
  let weightThreshold = $state(0);
  let clusterEnabled = $state(true);
  let loading = $state(true);
  let loadError = $state('');
  let nodeCount = $state(0);
  let edgeCount = $state(0);

  const highlightNodes = new Set<GNode>();
  const highlightLinks = new Set<GLink>();
  const searchHighlights = new Set<string>();
  let hoverNode: GNode | null = null;
  let pinnedNode: GNode | null = null;

  const nodeSprites = new Map<string, { material: any; baseSize: number; sprite: any }>();
  const nodeLabels = new Map<string, any>();
  const nodeMap = new Map<string, GNode>();

  // ── Constants ──────────────────────────────────────────

  const COMMUNITY_COLORS = [
    '#4fc3f7', '#ba68c8', '#81c784', '#ff8a65',
    '#64b5f6', '#f06292', '#fff176', '#4db6ac',
    '#e57373', '#7986cb', '#aed581', '#ffb74d',
  ];

  const CATEGORY_FALLBACK: Record<string, string> = {
    general: '#4fc3f7', decision: '#ba68c8', task: '#81c784',
    state: '#ff8a65', discovery: '#64b5f6', reference: '#f06292',
    issue: '#e57373', preference: '#fff176', credential: '#7986cb',
  };

  // ── Textures ───────────────────────────────────────────

  function createOrganismTexture(THREE: any, seed: number) {
    const size = 128;
    const c = document.createElement('canvas');
    c.width = size; c.height = size;
    const ctx = c.getContext('2d')!;
    const cx = size / 2, cy = size / 2;

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
      s = ((s * 1103515245 + 12345) & 0x7fffffff);
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

    // Clip to circle -- eliminates square sprite boundary artifacts
    // Without this, sub-pixel alpha residue in corners gets amplified by bloom
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
    c.width = 64; c.height = 64;
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

  // ── Color Helpers ──────────────────────────────────────

  function getNodeColor(node: GNode): string {
    if (searchHighlights.has(node.id)) return '#ffd700';
    if (node.category && CATEGORY_FALLBACK[node.category]) return CATEGORY_FALLBACK[node.category];
    if (node.community_id != null) return COMMUNITY_COLORS[node.community_id % COMMUNITY_COLORS.length];
    return '#4fc3f7';
  }

  function getNodeOpacity(node: GNode): number {
    if (highlightNodes.has(node) || searchHighlights.has(node.id)) return 1.0;
    const decay = node.decay_score ?? 5;
    return Math.max(0.5, Math.min(1.0, decay / 6));
  }

  function getLinkColor(link: GLink): string {
    const src = typeof link.source === 'object' ? link.source as GNode : null;
    return src ? getNodeColor(src) : '#4fc3f7';
  }

  function getParticleColor(link: GLink): string {
    if (link.type === 'corrects' || link.type === 'updates') return '#ff8a65';
    if (link.type === 'contradicts' || link.type === 'conflicts') return '#e57373';
    return getLinkColor(link);
  }

  // ── Cluster Force ──────────────────────────────────────

  function makeClusterForce(centroids: Record<string, { x: number; y: number; z: number }>, dim: 'x' | 'y' | 'z', strength: number) {
    let nodes: GNode[] = [];
    const vDim = ('v' + dim) as 'vx' | 'vy' | 'vz';
    const force: any = (alpha: number) => {
      for (const node of nodes) {
        const cid = String(node.community_id ?? node.category ?? 'default');
        const c = centroids[cid];
        if (c && node[dim] != null) {
          (node as any)[vDim] = ((node as any)[vDim] ?? 0) + (c[dim] - node[dim]!) * strength * alpha;
        }
      }
    };
    force.initialize = (n: GNode[]) => { nodes = n; };
    return force;
  }

  // ── Star Field ─────────────────────────────────────────

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
    scene.add(new THREE.Points(geo, new THREE.PointsMaterial({
      size: 0.8, vertexColors: true, transparent: true, opacity: 0.7, sizeAttenuation: true,
    })));
  }

  // ── Node Visual Updates ────────────────────────────────

  function updateNodeVisuals() {
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
  }

  // ── Hover Handler ──────────────────────────────────────

  function handleNodeHover(node: GNode | null) {
    highlightNodes.clear();
    highlightLinks.clear();

    if (node) {
      highlightNodes.add(node);
      node.neighbors?.forEach(n => { if (n) highlightNodes.add(n); });
      node.links?.forEach(l => {
        if ((l.weight ?? 0) >= weightThreshold) highlightLinks.add(l);
      });
    }

    if (pinnedNode && pinnedNode !== node) {
      highlightNodes.add(pinnedNode);
      pinnedNode.neighbors?.forEach(n => { if (n) highlightNodes.add(n); });
      pinnedNode.links?.forEach(l => {
        if ((l.weight ?? 0) >= weightThreshold) highlightLinks.add(l);
      });
    }

    hoverNode = node;
    updateNodeVisuals();
  }

  // ── Click Handler ──────────────────────────────────────

  async function handleNodeClick(node: GNode) {
    if (!node) return;
    pinnedNode = node;
    const memId = node.id.startsWith('m') ? node.id.slice(1) : node.id;
    try {
      selectedMemory = await getMemoryDetail(parseInt(memId));
      sidePanelOpen = true;
      showSearchResults = false;
    } catch (e: any) {
      console.error('Failed to fetch memory:', e);
    }
  }

  // ── Search Handler ─────────────────────────────────────

  async function handleSearch() {
    if (!searchQuery.trim()) {
      searchHighlights.clear();
      searchResults = [];
      showSearchResults = false;
      updateNodeVisuals();
      return;
    }
    try {
      const data = await searchGraph(searchQuery, 20);
      searchResults = data.results || [];
      showSearchResults = true;
      sidePanelOpen = true;
      selectedMemory = null;
      searchHighlights.clear();
      searchResults.forEach(r => searchHighlights.add('m' + r.id));
      updateNodeVisuals();
    } catch (e: any) {
      console.error('Search failed:', e);
    }
  }

  // ── Zoom to Node ───────────────────────────────────────

  function zoomToNode(memId: number | string) {
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
      handleNodeClick(node);
    }
  }

  // ── Close Panel ────────────────────────────────────────

  function closePanel() {
    sidePanelOpen = false;
    selectedMemory = null;
    showSearchResults = false;
    pinnedNode = null;
    highlightNodes.clear();
    highlightLinks.clear();
    searchHighlights.clear();
    updateNodeVisuals();
  }

  // ── Reactive Effects ───────────────────────────────────

  // Labels: toggle visibility of text label sprites created at init
  $effect(() => {
    const show = showLabels;
    nodeLabels.forEach((label) => { label.visible = show; });
  });

  // Weight threshold: re-set link accessors so graph re-evaluates visibility
  $effect(() => {
    if (!graphInstance) return;
    const wt = weightThreshold;
    graphInstance
      .linkWidth((link: any) => {
        if (highlightLinks.has(link)) return Math.max(0.5, (link.weight ?? 0.5) * 2);
        if ((link.weight ?? 0) >= wt) return 0.15;
        return 0;
      })
      .linkOpacity((link: any) => {
        if (highlightLinks.has(link)) return Math.max(0.3, (link.weight ?? 0.5) * 0.8);
        if (hoverNode && !highlightLinks.has(link)) return 0.04;
        if ((link.weight ?? 0) >= wt) return 0.05 + (link.weight ?? 0) * 0.12;
        return 0;
      })
      .linkColor((link: any) => {
        if (highlightLinks.has(link)) return getLinkColor(link);
        if ((link.weight ?? 0) >= wt) return getLinkColor(link);
        return 'rgba(0,0,0,0)';
      })
      .linkVisibility((link: any) => {
        if (highlightLinks.has(link)) return true;
        return (link.weight ?? 0) >= wt;
      });
  });

  // Clusters: toggle community clustering forces
  $effect(() => {
    if (!graphInstance) return;
    if (clusterEnabled && Object.keys(clusterCentroids).length) {
      graphInstance.d3Force('clusterX', makeClusterForce(clusterCentroids, 'x', 0.03));
      graphInstance.d3Force('clusterY', makeClusterForce(clusterCentroids, 'y', 0.03));
      graphInstance.d3Force('clusterZ', makeClusterForce(clusterCentroids, 'z', 0.03));
    } else {
      graphInstance.d3Force('clusterX', null);
      graphInstance.d3Force('clusterY', null);
      graphInstance.d3Force('clusterZ', null);
    }
    graphInstance.d3ReheatSimulation();
  });

  // ── Lifecycle ──────────────────────────────────────────

  let resizeHandler: (() => void) | null = null;

  onMount(async () => {
    try {
      const [FG3D, THREE] = await Promise.all([
        import('3d-force-graph'),
        import('three'),
      ]);
      const ForceGraph3D = FG3D.default;
      threeRef = THREE;

      const [graphData, commData, statsData] = await Promise.all([
        getGraph(3, 1500),
        getCommunities().catch(() => ({ communities: [], count: 0 })),
        getStats().catch(() => null),
      ]);

      stats = statsData;
      nodeCount = graphData.node_count || graphData.nodes?.length || 0;
      edgeCount = graphData.edge_count || graphData.edges?.length || 0;

      if (!graphData.nodes?.length) {
        loadError = 'No memories found. Store some memories first.';
        loading = false;
        return;
      }

      // Map community IDs onto nodes
      const commMap = new Map<string, number>();
      (commData.communities || []).forEach((c: any) => {
        (c.top_memories || []).forEach((mid: number) => commMap.set('m' + mid, c.id));
      });

      // Pre-process: build neighbor/link lookups
      graphData.nodes.forEach((node: GNode) => {
        node.neighbors = [];
        node.links = [];
        node.community_id = commMap.get(node.id);
        nodeMap.set(node.id, node);
      });

      graphData.edges.forEach((link: GLink) => {
        const src = nodeMap.get(link.source as string);
        const tgt = nodeMap.get(link.target as string);
        if (src && tgt) {
          src.neighbors!.push(tgt);
          src.links!.push(link);
          tgt.neighbors!.push(src);
          tgt.links!.push(link);
        }
      });

      // Top-200 strongest edges get persistent flow particles (Layer 2)
      const particleEdges = new Set<GLink>(
        [...graphData.edges]
          .filter((l: GLink) => (l.weight ?? 0) >= 0.5)
          .sort((a: GLink, b: GLink) => (b.weight ?? 0) - (a.weight ?? 0))
          .slice(0, 200)
      );

      // Compute cluster centroids using Fibonacci sphere
      const clusterIds = new Set<string>();
      graphData.nodes.forEach((n: GNode) => clusterIds.add(String(n.community_id ?? n.category ?? 'default')));
      const clusters = Array.from(clusterIds);
      const radius = Math.max(120, clusters.length * 35);
      clusters.forEach((id, i) => {
        const phi = Math.acos(1 - 2 * (i + 0.5) / clusters.length);
        const theta = Math.PI * (1 + Math.sqrt(5)) * i;
        clusterCentroids[id] = {
          x: radius * Math.sin(phi) * Math.cos(theta),
          y: radius * Math.sin(phi) * Math.sin(theta),
          z: radius * Math.cos(phi),
        };
      });

      const ringTexture = createRingTexture(THREE);

      // Pre-generate a pool of organism textures (8 variants, reused across nodes)
      const organismTextures = Array.from({ length: 8 }, (_, i) => createOrganismTexture(THREE, i * 137));

      // Track breathing phase per node for animation
      const breathPhases = new Map<string, number>();

      // ── Initialize Force Graph ────────────────────────

      const graph = ForceGraph3D()(container)
        .graphData({ nodes: graphData.nodes, links: graphData.edges })
        .backgroundColor('#0a0a0a')
        .showNavInfo(false)
        .nodeLabel(() => '')
        .nodeVal((n: any) => (n as GNode).importance || 5)
        .linkSource('source')
        .linkTarget('target')

        // Living organism nodes with optional text labels
        .nodeThreeObject((node: any) => {
          const n = node as GNode;
          const baseSize = Math.max(4, (n.importance || 5) * 1.8 + (n.size || 0) * 0.4);
          const idNum = parseInt(n.id.replace(/\D/g, '') || '0');

          const tex = organismTextures[idNum % organismTextures.length];
          breathPhases.set(n.id, (idNum * 0.7) % (Math.PI * 2));

          const material = new THREE.SpriteMaterial({
            map: tex,
            color: new THREE.Color(getNodeColor(n)),
            transparent: true,
            opacity: getNodeOpacity(n),
            depthWrite: false,
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
            label.visible = showLabels;
            group.add(label);
            nodeLabels.set(n.id, label);

            return group;
          }

          return sprite;
        })

        // Breathing animation -- nodes gently pulse like living cells
        .onEngineTick(() => {
          const t = performance.now() * 0.001;
          nodeSprites.forEach((entry, id) => {
            const phase = breathPhases.get(id) ?? 0;
            const breathScale = 1 + Math.sin(t * 0.8 + phase) * 0.08;
            const size = entry.baseSize * breathScale;
            const isHovered = highlightNodes.has(nodeMap.get(id)!);
            const scale = isHovered ? size * 1.3 : size;
            entry.sprite.scale.set(scale, scale, scale);
          });
        })

        // Layer 1: Faint static edges (topology always visible)
        // Layer 2: Flow trail particles on strong connections
        // Layer 3: Hover amplification
        .linkWidth((link: any) => {
          if (highlightLinks.has(link)) return Math.max(0.5, (link.weight ?? 0.5) * 2);
          if ((link.weight ?? 0) >= weightThreshold) return 0.15;
          return 0;
        })
        .linkOpacity((link: any) => {
          if (highlightLinks.has(link)) return Math.max(0.3, (link.weight ?? 0.5) * 0.8);
          if (hoverNode && !highlightLinks.has(link)) return 0.04;
          if ((link.weight ?? 0) >= weightThreshold) return 0.05 + (link.weight ?? 0) * 0.12;
          return 0;
        })
        .linkColor((link: any) => {
          if (highlightLinks.has(link)) return getLinkColor(link);
          if ((link.weight ?? 0) >= weightThreshold) return getLinkColor(link);
          return 'rgba(0,0,0,0)';
        })

        // Flow trail particles (Layer 2 + Layer 3 doubling)
        .linkDirectionalParticles((link: any) => {
          if (highlightLinks.has(link)) {
            return Math.floor((link.weight ?? 0.5) * 6) * 2;
          }
          if (particleEdges.has(link)) {
            return Math.floor((link.weight ?? 0.5) * 6);
          }
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
        .onNodeClick((node: any) => handleNodeClick(node as GNode))
        .onBackgroundClick(() => {
          if (!showSearchResults) closePanel();
        })

        .warmupTicks(150)
        .cooldownTicks(400);

      graphInstance = graph;
      (window as any).__graph = graph;

      // Force canvas background to near-black (belt-and-suspenders with backgroundColor)
      const canvas = graph.renderer().domElement;
      canvas.style.backgroundColor = '#0a0a0a';

      // Star field
      addStarField(THREE, graph.scene());

      // Bloom removed -- scattered light from 800+ nodes was turning #0a0a0a background grey
      // To re-enable with selective bloom, use threshold >= 0.9 and strength <= 0.2

      // Cluster forces (gentle -- let topology dominate, clusters just hint)
      graph.d3Force('clusterX', makeClusterForce(clusterCentroids, 'x', 0.03));
      graph.d3Force('clusterY', makeClusterForce(clusterCentroids, 'y', 0.03));
      graph.d3Force('clusterZ', makeClusterForce(clusterCentroids, 'z', 0.03));

      // Force tuning: tree-like branching structure
      // Extreme repulsion forces branches apart
      graph.d3Force('charge')?.strength(-800).distanceMax(1500);
      // Only strong connections create pull -- weak ones contribute nothing
      // This lets unrelated clusters fly apart and form branches
      graph.d3Force('link')
        ?.distance((link: any) => {
          const w = link.weight ?? 0.3;
          if (w > 0.7) return 8;
          if (w > 0.5) return 25;
          return 400;
        })
        .strength((link: any) => {
          const w = link.weight ?? 0.3;
          if (w > 0.7) return 1.5;
          if (w > 0.5) return 0.5;
          return 0;  // weak links exert ZERO force -- they're visual only
        });
      // Minimal center gravity -- just prevent flying to infinity
      graph.d3Force('center')?.strength(0.005);

      // Resize
      resizeHandler = () => graph.width(window.innerWidth).height(window.innerHeight);
      window.addEventListener('resize', resizeHandler);

      // Fit after settling
      setTimeout(() => graph.zoomToFit(800, 50), 3000);

      loading = false;
    } catch (e: any) {
      loadError = e.message || 'Unknown error';
      loading = false;
      console.error('Graph init failed:', e);
    }
  });

  onDestroy(() => {
    if (resizeHandler) window.removeEventListener('resize', resizeHandler);
    graphInstance?._destructor?.();
  });
</script>

<svelte:window onkeydown={(e) => { if (e.key === 'Escape') closePanel(); }} />

<!-- ── Template ─────────────────────────────────────────── -->

<div class="fixed inset-0 z-40 bg-[#0a0a0a] overflow-hidden">
  <div bind:this={container} class="w-full h-full"></div>

  <!-- Loading -->
  {#if loading}
    <div class="absolute inset-0 flex items-center justify-center z-50 bg-[#0a0a0a]">
      <div class="text-center">
        <div class="w-12 h-12 border-2 border-teal-500/30 border-t-teal-400 rounded-full animate-spin mx-auto mb-4"></div>
        <p class="text-gray-500 text-sm">Loading memory graph...</p>
      </div>
    </div>
  {/if}

  <!-- Error -->
  {#if loadError}
    <div class="absolute inset-0 flex items-center justify-center z-50 bg-[#0a0a0a]">
      <div class="bg-red-900/20 border border-red-800/50 rounded-xl p-6 max-w-md text-center">
        <p class="text-red-400 text-sm mb-2">Failed to load graph</p>
        <p class="text-red-300/60 text-xs font-mono">{loadError}</p>
        <a href="/" class="inline-block mt-4 px-4 py-2 bg-gray-800 hover:bg-gray-700 rounded-lg text-sm text-gray-300 transition-colors">Back to Dashboard</a>
      </div>
    </div>
  {/if}

  {#if !loading && !loadError}
    <!-- Top Bar -->
    <div class="absolute top-0 left-0 right-0 z-50 flex items-center gap-4 px-5 py-3 topbar-gradient">
      <a href="/" class="flex items-center gap-2 text-gray-500 hover:text-gray-300 transition-colors shrink-0">
        <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15 19l-7-7 7-7"/></svg>
        <span class="text-xs">Back</span>
      </a>

      <span class="text-sm font-bold tracking-widest bg-gradient-to-r from-teal-300 to-cyan-400 bg-clip-text text-transparent shrink-0">ENGRAM</span>

      <form class="flex-1 max-w-md" onsubmit={(e) => { e.preventDefault(); handleSearch(); }}>
        <div class="relative">
          <input
            type="text"
            bind:value={searchQuery}
            placeholder="Search memories..."
            class="w-full px-4 py-2 pl-9 bg-white/5 border border-white/10 rounded-lg text-sm text-gray-200 placeholder-gray-600 focus:outline-none focus:border-teal-500/50 transition-all"
          />
          <svg class="absolute left-3 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-gray-600" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z"/></svg>
        </div>
      </form>

      <div class="flex items-center gap-4 text-[11px] text-gray-500 shrink-0">
        <span><span class="text-gray-400 font-medium">{nodeCount}</span> nodes</span>
        <span><span class="text-gray-400 font-medium">{edgeCount}</span> edges</span>
        {#if stats?.db_size_mb}
          <span>{stats.db_size_mb.toFixed(1)} MB</span>
        {/if}
      </div>
    </div>

    <!-- Controls (bottom-left) -->
    <div class="absolute bottom-5 left-5 z-50 flex flex-col gap-3 p-4 rounded-xl glass-panel">
      <div>
        <div class="text-[10px] text-gray-500 uppercase tracking-wider mb-1.5">Edge Floor</div>
        <div class="flex items-center gap-2">
          <input
            type="range" min="0" max="1" step="0.05"
            bind:value={weightThreshold}
            class="range-slider w-28"
          />
          <span class="text-[10px] text-gray-600 w-7 text-right">{weightThreshold.toFixed(2)}</span>
        </div>
      </div>

      <button onclick={() => showLabels = !showLabels} class="flex items-center gap-2 group">
        <div class="w-7 h-4 rounded-full relative transition-colors {showLabels ? 'bg-teal-500/60' : 'bg-gray-700'}">
          <div class="absolute left-0.5 top-0.5 w-3 h-3 rounded-full transition-all {showLabels ? 'translate-x-3 bg-teal-300' : 'bg-gray-400'}"></div>
        </div>
        <span class="text-[10px] text-gray-500 group-hover:text-gray-400 transition-colors">Labels</span>
      </button>

      <button onclick={() => clusterEnabled = !clusterEnabled} class="flex items-center gap-2 group">
        <div class="w-7 h-4 rounded-full relative transition-colors {clusterEnabled ? 'bg-teal-500/60' : 'bg-gray-700'}">
          <div class="absolute left-0.5 top-0.5 w-3 h-3 rounded-full transition-all {clusterEnabled ? 'translate-x-3 bg-teal-300' : 'bg-gray-400'}"></div>
        </div>
        <span class="text-[10px] text-gray-500 group-hover:text-gray-400 transition-colors">Clusters</span>
      </button>

      <button
        onclick={() => graphInstance?.zoomToFit(800, 50)}
        class="px-3 py-1.5 bg-white/5 hover:bg-white/10 border border-white/[0.08] rounded-lg text-[10px] text-gray-400 hover:text-gray-300 transition-all"
      >
        Fit View
      </button>
    </div>

    <!-- Side Panel -->
    {#if sidePanelOpen}
      <div class="absolute top-0 right-0 bottom-0 w-[380px] z-50 overflow-y-auto side-panel glass-panel-solid">
        <button onclick={closePanel} aria-label="Close panel" class="absolute top-4 right-4 w-7 h-7 flex items-center justify-center rounded-lg bg-white/5 hover:bg-white/10 text-gray-500 hover:text-gray-300 transition-all z-10">
          <svg class="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12"/></svg>
        </button>

        <div class="p-5 pt-6">
          {#if showSearchResults}
            <h3 class="text-xs font-semibold text-gray-500 uppercase tracking-wider mb-3">Search Results</h3>
            {#if searchResults.length === 0}
              <p class="text-sm text-gray-600">No results found</p>
            {:else}
              <div class="space-y-2">
                {#each searchResults as result}
                  <button
                    onclick={() => zoomToNode(result.id)}
                    class="w-full text-left p-3 bg-white/[0.03] hover:bg-white/[0.06] border border-white/[0.05] rounded-lg transition-all group"
                  >
                    <div class="flex items-center gap-2 mb-1">
                      <span class="text-[10px] font-mono text-gray-600">#{result.id}</span>
                      <span class="px-1.5 py-0.5 rounded text-[9px] font-medium"
                        style="background: {CATEGORY_FALLBACK[result.category] || '#4fc3f7'}20; color: {CATEGORY_FALLBACK[result.category] || '#4fc3f7'}">
                        {result.category}
                      </span>
                      {#if result.score}
                        <span class="text-[10px] text-gray-600 ml-auto">{(result.score * 100).toFixed(0)}%</span>
                      {/if}
                    </div>
                    <p class="text-xs text-gray-400 line-clamp-2 group-hover:text-gray-300 transition-colors">{result.content}</p>
                  </button>
                {/each}
              </div>
            {/if}

          {:else if selectedMemory}
            <div class="space-y-5">
              <p class="text-sm text-gray-300 leading-relaxed whitespace-pre-wrap">{selectedMemory.content}</p>

              <div class="flex flex-wrap gap-1.5">
                <span class="px-2 py-0.5 rounded-full text-[10px] font-medium"
                  style="background: {CATEGORY_FALLBACK[selectedMemory.category] || '#4fc3f7'}20; color: {CATEGORY_FALLBACK[selectedMemory.category] || '#4fc3f7'}">
                  {selectedMemory.category}
                </span>
                <span class="px-2 py-0.5 rounded-full text-[10px] bg-gray-800 text-gray-500">{selectedMemory.source}</span>
                {#if selectedMemory.is_static}
                  <span class="px-2 py-0.5 rounded-full text-[10px] bg-amber-900/30 text-amber-400">static</span>
                {/if}
                <span class="px-2 py-0.5 rounded-full text-[10px] bg-gray-800 text-gray-500">v{selectedMemory.version}</span>
              </div>

              <div class="grid grid-cols-2 gap-3">
                <div>
                  <div class="text-[10px] text-gray-600 mb-1">Importance</div>
                  <div class="h-1.5 bg-gray-800 rounded-full overflow-hidden">
                    <div class="h-full rounded-full transition-all" style="width: {selectedMemory.importance * 10}%; background: {CATEGORY_FALLBACK[selectedMemory.category] || '#4fc3f7'}"></div>
                  </div>
                  <div class="text-[10px] text-gray-500 mt-0.5">{selectedMemory.importance}/10</div>
                </div>
                <div>
                  <div class="text-[10px] text-gray-600 mb-1">Decay</div>
                  <div class="h-1.5 bg-gray-800 rounded-full overflow-hidden">
                    <div class="h-full bg-teal-500/60 rounded-full transition-all" style="width: {Math.min(100, ((selectedMemory.decay_score ?? 0) / Math.max(1, selectedMemory.importance)) * 100)}%"></div>
                  </div>
                  <div class="text-[10px] text-gray-500 mt-0.5">{selectedMemory.decay_score?.toFixed(2) ?? 'N/A'}</div>
                </div>
              </div>

              <div class="space-y-1.5 text-[11px]">
                <div class="flex justify-between"><span class="text-gray-600">Created</span><span class="text-gray-400">{new Date(selectedMemory.created_at).toLocaleDateString()} {new Date(selectedMemory.created_at).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}</span></div>
                <div class="flex justify-between"><span class="text-gray-600">Accessed</span><span class="text-gray-400">{selectedMemory.access_count ?? 0}x</span></div>
                {#if selectedMemory.last_accessed_at}
                  <div class="flex justify-between"><span class="text-gray-600">Last accessed</span><span class="text-gray-400">{new Date(selectedMemory.last_accessed_at).toLocaleDateString()}</span></div>
                {/if}
                {#if selectedMemory.episode}
                  <div class="flex justify-between"><span class="text-gray-600">Episode</span><span class="text-gray-400">{selectedMemory.episode.title}</span></div>
                {/if}
              </div>

              {#if selectedMemory.tags?.length}
                <div>
                  <h4 class="text-[10px] text-gray-600 uppercase tracking-wider mb-2">Tags</h4>
                  <div class="flex flex-wrap gap-1.5">
                    {#each selectedMemory.tags as tag}
                      <span class="px-2 py-0.5 rounded-md text-[10px] bg-teal-500/10 text-teal-400/80 border border-teal-500/10">{tag}</span>
                    {/each}
                  </div>
                </div>
              {/if}

              {#if selectedMemory.links?.length}
                <div>
                  <h4 class="text-[10px] text-gray-600 uppercase tracking-wider mb-2">Linked Memories ({selectedMemory.links.length})</h4>
                  <div class="space-y-1.5">
                    {#each selectedMemory.links as link}
                      <button
                        onclick={() => zoomToNode(link.id)}
                        class="w-full text-left p-2.5 bg-white/[0.02] hover:bg-white/[0.05] border border-white/[0.04] rounded-lg transition-all group"
                      >
                        <div class="flex items-center gap-2 mb-0.5">
                          <span class="text-[9px] px-1.5 py-0.5 rounded bg-gray-800 text-gray-500">{link.type}</span>
                          <span class="text-[9px] text-gray-600 ml-auto">{(link.similarity * 100).toFixed(0)}%</span>
                        </div>
                        <p class="text-[11px] text-gray-500 line-clamp-1 group-hover:text-gray-400 transition-colors">{link.content}</p>
                      </button>
                    {/each}
                  </div>
                </div>
              {/if}

              {#if selectedMemory.version_chain && selectedMemory.version_chain.length > 1}
                <div>
                  <h4 class="text-[10px] text-gray-600 uppercase tracking-wider mb-2">Version History</h4>
                  <div class="relative ml-2 pl-4 border-l border-gray-800 space-y-3">
                    {#each selectedMemory.version_chain as ver}
                      <div class="relative">
                        <div class="absolute -left-[21px] top-1 w-2.5 h-2.5 rounded-full border-2 {ver.is_latest ? 'bg-teal-400 border-teal-400' : 'bg-gray-800 border-gray-700'}"></div>
                        <div class="text-[10px] text-gray-600">v{ver.version} {ver.is_latest ? '(latest)' : ''}</div>
                        <p class="text-[11px] text-gray-500 line-clamp-2 mt-0.5">{ver.content}</p>
                      </div>
                    {/each}
                  </div>
                </div>
              {/if}
            </div>
          {/if}
        </div>
      </div>
    {/if}

    <!-- Category Legend (bottom-right, hidden when panel open) -->
    {#if stats?.memories?.categories && !sidePanelOpen}
      <div class="absolute bottom-5 right-5 z-40 p-3 rounded-xl glass-panel-light">
        <div class="text-[9px] text-gray-600 uppercase tracking-wider mb-2">Categories</div>
        <div class="space-y-1">
          {#each stats.memories.categories.slice(0, 8) as cat}
            <div class="flex items-center gap-2">
              <div class="w-2 h-2 rounded-full" style="background: {CATEGORY_FALLBACK[cat.category] || '#4fc3f7'}"></div>
              <span class="text-[10px] text-gray-500">{cat.category}</span>
              <span class="text-[10px] text-gray-700 ml-auto">{cat.count}</span>
            </div>
          {/each}
        </div>
      </div>
    {/if}
  {/if}
</div>

<style>
  .topbar-gradient {
    background: linear-gradient(to bottom, rgba(10,10,10,0.85) 0%, rgba(10,10,10,0) 100%);
  }

  .glass-panel {
    background: rgba(10, 14, 20, 0.85);
    backdrop-filter: blur(20px);
    border: 1px solid rgba(255, 255, 255, 0.06);
  }

  .glass-panel-solid {
    background: rgba(10, 14, 20, 0.92);
    backdrop-filter: blur(30px);
    border-left: 1px solid rgba(255, 255, 255, 0.06);
  }

  .glass-panel-light {
    background: rgba(10, 14, 20, 0.7);
    backdrop-filter: blur(10px);
    border: 1px solid rgba(255, 255, 255, 0.04);
  }

  .side-panel {
    animation: slideIn 0.3s ease-out;
  }

  @keyframes slideIn {
    from { transform: translateX(100%); opacity: 0; }
    to { transform: translateX(0); opacity: 1; }
  }

  .range-slider {
    -webkit-appearance: none;
    appearance: none;
    height: 3px;
    background: rgba(255, 255, 255, 0.1);
    border-radius: 2px;
    outline: none;
  }

  .range-slider::-webkit-slider-thumb {
    -webkit-appearance: none;
    width: 12px;
    height: 12px;
    border-radius: 50%;
    background: #2dd4bf;
    cursor: pointer;
  }

  .range-slider::-moz-range-thumb {
    width: 12px;
    height: 12px;
    border-radius: 50%;
    background: #2dd4bf;
    border: none;
    cursor: pointer;
  }

  .side-panel::-webkit-scrollbar { width: 4px; }
  .side-panel::-webkit-scrollbar-track { background: transparent; }
  .side-panel::-webkit-scrollbar-thumb { background: rgba(255,255,255,0.08); border-radius: 2px; }
  .side-panel::-webkit-scrollbar-thumb:hover { background: rgba(255,255,255,0.15); }
</style>
