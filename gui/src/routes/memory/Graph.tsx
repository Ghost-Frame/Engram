import { useQuery } from '@tanstack/react-query';
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  // Types the search form submission without importing a runtime symbol.
  type FormEvent,
  // Types canvas pointer gestures without importing a runtime symbol.
  type PointerEvent as ReactPointerEvent,
  // Types canvas wheel gestures without importing a runtime symbol.
  type WheelEvent
} from 'react';
import {
  getMemoryDetail,
  getMemoryGraph,
  searchGraph,
  // Describes one server search result shown in the Atlas inspector.
  type GraphSearchResult
} from '$lib/api/graph';
import {
  buildAtlasLayout,
  fitAtlasView,
  hitTestAtlas,
  // Describes the deterministic positioned graph returned by the layout helper.
  type AtlasLayout,
  // Describes one positioned memory node in the Atlas.
  type AtlasNode,
  // Describes the current canvas pan-and-zoom transform.
  type AtlasView
} from '$lib/graph/atlasLayout';
import { selectRenderEdges } from '$lib/graph/selectRenderEdges';
import type { GraphEdge } from '$lib/types';
import { EmptyState } from '../../ui/EmptyState';
import { Spinner } from '../../ui/Spinner';
import './graph.css';

// Caps exposed by the density control keep work explicit and predictable.
const DENSITY_OPTIONS = [
  { label: 'Focused · 400', value: 400 },
  { label: 'Working · 800', value: 800 },
  { label: 'Wide · 1,200', value: 1200 }
];

// Category colors are reserved for distinguishing memory semantics.
const CATEGORY_COLORS: Record<string, string> = {
  credential: '#b99be8',
  decision: '#d8aa55',
  directive: '#dc826f',
  discovery: '#75a9bd',
  general: '#8c9185',
  incident: '#dc6f66',
  infrastructure: '#78a99d',
  issue: '#d57d76',
  preference: '#be9e78',
  reference: '#8798b9',
  state: '#9ba870',
  task: '#72b58b'
};

// Reuse an immutable empty neighborhood so unrelated renders do not redraw the canvas.
const EMPTY_NEIGHBORS = new Set<string>();

// Stores the canvas dimensions in CSS pixels.
interface CanvasSize {
  height: number;
  width: number;
}

// Stores the active pointer pan gesture.
interface PanGesture {
  moved: boolean;
  pointerId: number;
  startOffsetX: number;
  startOffsetY: number;
  startX: number;
  startY: number;
}

// Render a bounded, deterministic 2D map of memory relationships.
export function Graph() {
  const [density, setDensity] = useState(800);
  const [edgeFloor, setEdgeFloor] = useState(0);
  const [showEdges, setShowEdges] = useState(true);
  const [showLabels, setShowLabels] = useState(true);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [searchQuery, setSearchQuery] = useState('');
  const [searchResults, setSearchResults] = useState<GraphSearchResult[]>([]);
  const [searchError, setSearchError] = useState('');
  const [searching, setSearching] = useState(false);
  const [size, setSize] = useState<CanvasSize>({ height: 620, width: 960 });
  const [view, setView] = useState<AtlasView>({ offsetX: 0, offsetY: 0, scale: 1 });
  const [viewReady, setViewReady] = useState(false);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const surfaceRef = useRef<HTMLDivElement>(null);
  const panRef = useRef<PanGesture | null>(null);
  const graph = useQuery({
    queryFn: () => getMemoryGraph(3, density, 2),
    queryKey: ['memory', 'atlas', density],
    staleTime: 30_000
  });
  const layout = useMemo(
    () => buildAtlasLayout(graph.data?.nodes ?? [], graph.data?.edges ?? []),
    [graph.data]
  );
  const renderEdges = useMemo(
    () => selectAtlasEdges(layout.edges, edgeFloor, Math.min(2400, Math.max(600, layout.nodes.length * 2))),
    [edgeFloor, layout.edges, layout.nodes.length]
  );
  const selectedNode = selectedId ? layout.nodeById.get(selectedId) ?? null : null;
  const selectedNeighbors = useMemo(
    () => selectedId ? layout.neighbors.get(selectedId) ?? EMPTY_NEIGHBORS : EMPTY_NEIGHBORS,
    [layout.neighbors, selectedId]
  );
  const selectedMemoryId = memoryIdFromNode(selectedNode);
  const detail = useQuery({
    enabled: selectedMemoryId !== null,
    queryFn: () => getMemoryDetail(selectedMemoryId!),
    queryKey: ['memory', 'detail', selectedMemoryId]
  });

  // Refit after a density change, but preserve deliberate operator navigation.
  useEffect(() => {
    setViewReady(false);
  }, [density]);

  // Observe the available canvas surface without using a continuous render loop.
  useEffect(() => {
    const surface = surfaceRef.current;
    if (!surface) return undefined;
    const observer = new ResizeObserver(([entry]) => {
      const width = Math.max(280, Math.floor(entry.contentRect.width));
      const height = Math.max(420, Math.floor(entry.contentRect.height));
      setSize({ height, width });
    });
    observer.observe(surface);
    return () => observer.disconnect();
  }, []);

  // Fit the newly loaded layout exactly once per density selection.
  useEffect(() => {
    if (!layout.nodes.length || !size.width || !size.height || viewReady) return;
    setView(fitAtlasView(layout.bounds, size.width, size.height, 58));
    setViewReady(true);
  }, [layout, size, viewReady]);

  // Draw only when data, controls, selection, or the viewport changes.
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    drawAtlas(canvas, size, view, layout, showEdges ? renderEdges : [], showLabels, selectedId, selectedNeighbors);
  }, [layout, renderEdges, selectedId, selectedNeighbors, showEdges, showLabels, size, view]);

  // Reset the viewport so every loaded node is visible.
  const fitView = useCallback(() => {
    setView(fitAtlasView(layout.bounds, size.width, size.height, 58));
    setViewReady(true);
  }, [layout.bounds, size]);

  // Resolve and select a node from either canvas interaction or search.
  const selectNode = useCallback((nodeId: string) => {
    const node = layout.nodeById.get(nodeId);
    if (!node) return;
    setSelectedId(nodeId);
    setSearchResults([]);
    setView((current) => ({
      ...current,
      offsetX: size.width / 2 - node.x * current.scale,
      offsetY: size.height / 2 - node.y * current.scale
    }));
  }, [layout.nodeById, size.height, size.width]);

  // Submit a relationship-expanding server search and expose linked local hits.
  const handleSearch = async (event: FormEvent) => {
    event.preventDefault();
    const query = searchQuery.trim();
    if (!query) {
      setSearchResults([]);
      setSearchError('');
      return;
    }
    setSearchError('');
    setSearching(true);
    try {
      const result = await searchGraph(query, 30);
      const results = result.results ?? [];
      setSearchResults(results);
      setSearchError(results.length === 0 ? 'No matching memories found.' : '');
    } catch {
      setSearchResults([]);
      setSearchError('Search is unavailable. Try again.');
    } finally {
      setSearching(false);
    }
  };

  // Begin a pointer gesture that can resolve to either pan or selection.
  const handlePointerDown = (event: ReactPointerEvent<HTMLCanvasElement>) => {
    event.currentTarget.setPointerCapture(event.pointerId);
    panRef.current = {
      moved: false,
      pointerId: event.pointerId,
      startOffsetX: view.offsetX,
      startOffsetY: view.offsetY,
      startX: event.clientX,
      startY: event.clientY
    };
  };

  // Pan the atlas only while the pointer is actively captured.
  const handlePointerMove = (event: ReactPointerEvent<HTMLCanvasElement>) => {
    const gesture = panRef.current;
    if (!gesture || gesture.pointerId !== event.pointerId) return;
    const dx = event.clientX - gesture.startX;
    const dy = event.clientY - gesture.startY;
    if (Math.abs(dx) + Math.abs(dy) > 3) gesture.moved = true;
    setView((current) => ({
      ...current,
      offsetX: gesture.startOffsetX + dx,
      offsetY: gesture.startOffsetY + dy
    }));
  };

  // Complete a gesture and select the nearest node when no pan occurred.
  const handlePointerUp = (event: ReactPointerEvent<HTMLCanvasElement>) => {
    const gesture = panRef.current;
    panRef.current = null;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
    if (!gesture || gesture.moved) return;
    const rect = event.currentTarget.getBoundingClientRect();
    const hit = hitTestAtlas(layout.nodes, view, event.clientX - rect.left, event.clientY - rect.top, 14);
    setSearchResults([]);
    setSearchError('');
    setSelectedId(hit?.id ?? null);
  };

  // Abandon a pointer gesture without interpreting cancellation as selection.
  const handlePointerCancel = (event: ReactPointerEvent<HTMLCanvasElement>) => {
    panRef.current = null;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
  };

  // Zoom around the cursor while constraining scale to a usable range.
  const handleWheel = (event: WheelEvent<HTMLCanvasElement>) => {
    event.preventDefault();
    const rect = event.currentTarget.getBoundingClientRect();
    const cursorX = event.clientX - rect.left;
    const cursorY = event.clientY - rect.top;
    const factor = event.deltaY < 0 ? 1.14 : 1 / 1.14;
    setView((current) => {
      const nextScale = Math.min(5, Math.max(0.08, current.scale * factor));
      const appliedFactor = nextScale / current.scale;
      return {
        offsetX: cursorX - (cursorX - current.offsetX) * appliedFactor,
        offsetY: cursorY - (cursorY - current.offsetY) * appliedFactor,
        scale: nextScale
      };
    });
  };

  return (
    <div className="atlas-page">
      <header className="atlas-header">
        <div>
          <span className="page-heading__eyebrow">Memory / relationship topology</span>
          <h1>Memory Atlas</h1>
          <p>A stable, inspectable map. It renders on change, never burns cycles while idle.</p>
        </div>
        <div className="atlas-report" aria-label="Graph connectivity report">
          <span><strong>{layout.nodes.length.toLocaleString()}</strong> loaded nodes</span>
          <span><strong>{layout.edges.length.toLocaleString()}</strong> loaded links</span>
          <span><strong>{(showEdges ? renderEdges.length : 0).toLocaleString()}</strong> visible links</span>
          <span><strong>{countLinkedNodes(layout).toLocaleString()}</strong> linked nodes</span>
        </div>
      </header>

      <form className="atlas-search" onSubmit={handleSearch}>
        <label htmlFor="atlas-query">Find a memory</label>
        <input
          id="atlas-query"
          onChange={(event) => setSearchQuery(event.target.value)}
          placeholder="Search content, entities, or decisions"
          value={searchQuery}
        />
        <button disabled={searching} type="submit">{searching ? 'Searching…' : 'Search'}</button>
        {searchError ? <p className="atlas-search__error" role="alert">{searchError}</p> : null}
      </form>

      <div className="atlas-workbench">
        <aside aria-label="Atlas controls" className="atlas-controls">
          <section>
            <h2>View</h2>
            <label>
              Density
              <select onChange={(event) => setDensity(Number(event.target.value))} value={density}>
                {DENSITY_OPTIONS.map((option) => (
                  <option key={option.value} value={option.value}>{option.label}</option>
                ))}
              </select>
            </label>
            <label>
              Edge floor
              <span className="atlas-controls__value">{edgeFloor.toFixed(2)}</span>
              <input
                aria-label="Minimum edge weight"
                max="1"
                min="0"
                onChange={(event) => setEdgeFloor(Number(event.target.value))}
                step="0.05"
                type="range"
                value={edgeFloor}
              />
            </label>
            <ToggleControl checked={showEdges} label="Show relationships" onChange={setShowEdges} />
            <ToggleControl checked={showLabels} label="Priority labels" onChange={setShowLabels} />
            <button className="atlas-controls__fit" onClick={fitView} type="button">Fit all nodes</button>
          </section>
          <CategoryLegend nodes={layout.nodes} />
        </aside>

        <div className="atlas-surface" ref={surfaceRef}>
          {graph.isLoading ? (
            <div className="atlas-state"><Spinner /></div>
          ) : graph.isError ? (
            <div className="atlas-state"><EmptyState message="The graph endpoint did not respond." /></div>
          ) : layout.nodes.length === 0 ? (
            <div className="atlas-state"><EmptyState message="No connected memories were returned." /></div>
          ) : (
            <canvas
              aria-label="Interactive memory relationship atlas"
              onPointerCancel={handlePointerCancel}
              onPointerDown={handlePointerDown}
              onPointerMove={handlePointerMove}
              onPointerUp={handlePointerUp}
              onWheel={handleWheel}
              ref={canvasRef}
            />
          )}
          <div aria-hidden="true" className="atlas-gesture">Drag to pan · scroll to zoom · click to inspect</div>
        </div>

        <aside aria-label="Memory inspector" className="atlas-inspector">
          {searchResults.length > 0 ? (
            <SearchResults layout={layout} onSelect={selectNode} results={searchResults} />
          ) : selectedNode ? (
            <MemoryInspector
              loading={detail.isLoading}
              loadedNodes={layout.nodeById}
              memory={detail.data}
              node={selectedNode}
              onClose={() => setSelectedId(null)}
              onSelect={selectNode}
            />
          ) : (
            <div className="atlas-inspector__empty">
              <span>NO SELECTION</span>
              <strong>Inspect the topology</strong>
              <p>Select a node to read the memory, its metadata, and its immediate neighborhood.</p>
            </div>
          )}
        </aside>
      </div>
    </div>
  );
}

// Render a switch-style checkbox without hiding its native semantics.
function ToggleControl({
  checked,
  label,
  onChange
}: {
  checked: boolean;
  label: string;
  onChange: (checked: boolean) => void;
}) {
  return (
    <label className="atlas-toggle">
      <input checked={checked} onChange={(event) => onChange(event.target.checked)} type="checkbox" />
      <span aria-hidden="true" />
      {label}
    </label>
  );
}

// Render category counts for the currently loaded graph.
function CategoryLegend({ nodes }: { nodes: AtlasNode[] }) {
  const categories = [...nodes.reduce((counts, node) => {
    counts.set(node.category, (counts.get(node.category) ?? 0) + 1);
    return counts;
  }, new Map<string, number>())].sort((left, right) => right[1] - left[1]);

  return (
    <section className="atlas-legend">
      <h2>Categories</h2>
      {categories.slice(0, 9).map(([category, count]) => (
        <div className="atlas-legend__row" key={category}>
          <i style={{ background: categoryColor(category) }} />
          <span>{category}</span>
          <strong>{count}</strong>
        </div>
      ))}
    </section>
  );
}

// Render server search hits and disclose whether each hit is loaded locally.
function SearchResults({
  layout,
  onSelect,
  results
}: {
  layout: AtlasLayout;
  onSelect: (nodeId: string) => void;
  results: GraphSearchResult[];
}) {
  return (
    <section className="atlas-results">
      <header>
        <span>SEARCH RESULTS</span>
        <strong>{results.length}</strong>
      </header>
      {results.map((result) => {
        const nodeId = `m${result.id}`;
        const isLoaded = layout.nodeById.has(nodeId);
        return (
          <button disabled={!isLoaded} key={result.id} onClick={() => onSelect(nodeId)} type="button">
            <span>#{result.id} · {result.category}</span>
            <strong>{result.content}</strong>
            <small>{isLoaded ? 'Open in atlas' : 'Outside current density'}</small>
          </button>
        );
      })}
    </section>
  );
}

// Render the selected memory and direct links to its loaded neighbors.
function MemoryInspector({
  loading,
  loadedNodes,
  memory,
  node,
  onClose,
  onSelect
}: {
  loading: boolean;
  loadedNodes: ReadonlyMap<string, AtlasNode>;
  memory: Awaited<ReturnType<typeof getMemoryDetail>> | undefined;
  node: AtlasNode;
  onClose: () => void;
  onSelect: (nodeId: string) => void;
}) {
  return (
    <section className="atlas-detail">
      <header>
        <span>MEMORY {node.id.replace(/^m/, '#')}</span>
        <button aria-label="Close inspector" onClick={onClose} type="button">×</button>
      </header>
      {loading ? <Spinner /> : null}
      <span className="atlas-detail__category" style={{ color: categoryColor(node.category) }}>{node.category}</span>
      <p>{memory?.content ?? node.content}</p>
      <dl>
        <div><dt>Importance</dt><dd>{memory?.importance ?? node.importance} / 10</dd></div>
        <div><dt>Source</dt><dd>{memory?.source ?? node.source}</dd></div>
        <div><dt>Connections</dt><dd>{node.degree}</dd></div>
        <div><dt>Created</dt><dd>{new Date(memory?.created_at ?? node.created_at).toLocaleDateString()}</dd></div>
      </dl>
      {memory?.tags?.length ? (
        <div className="atlas-detail__tags">
          {memory.tags.map((tag) => <span key={tag}>{tag}</span>)}
        </div>
      ) : null}
      {memory?.links?.length ? (
        <div className="atlas-detail__links">
          <h2>Neighborhood</h2>
          {memory.links.slice(0, 12).map((link) => (
            <button
              disabled={!loadedNodes.has(`m${link.id}`)}
              key={link.id}
              onClick={() => onSelect(`m${link.id}`)}
              type="button"
            >
              <span>{link.type} · {Math.round(link.similarity * 100)}%</span>
              <strong>{link.content}</strong>
            </button>
          ))}
        </div>
      ) : null}
    </section>
  );
}

// Filter by weight before applying the connectivity-preserving render cap.
function selectAtlasEdges(edges: GraphEdge[], floor: number, cap: number): GraphEdge[] {
  return selectRenderEdges(edges.filter((edge) => edge.weight >= floor), cap);
}

// Count nodes with at least one validated relationship.
function countLinkedNodes(layout: AtlasLayout): number {
  return layout.nodes.reduce((count, node) => count + (node.degree > 0 ? 1 : 0), 0);
}

// Extract the numeric API memory id from a graph node id.
function memoryIdFromNode(node: AtlasNode | null): number | null {
  if (!node) return null;
  const parsed = Number.parseInt(node.id.replace(/^m/, ''), 10);
  return Number.isFinite(parsed) ? parsed : null;
}

// Resolve a category to a stable semantic color.
function categoryColor(category: string): string {
  return CATEGORY_COLORS[category] ?? CATEGORY_COLORS.general;
}

// Draw the entire atlas once for the current state.
function drawAtlas(
  canvas: HTMLCanvasElement,
  size: CanvasSize,
  view: AtlasView,
  layout: AtlasLayout,
  edges: GraphEdge[],
  showLabels: boolean,
  selectedId: string | null,
  selectedNeighbors: Set<string>
) {
  const ratio = Math.min(2, window.devicePixelRatio || 1);
  canvas.width = Math.round(size.width * ratio);
  canvas.height = Math.round(size.height * ratio);
  const context = canvas.getContext('2d');
  if (!context) return;
  context.setTransform(ratio, 0, 0, ratio, 0, 0);
  context.clearRect(0, 0, size.width, size.height);
  context.fillStyle = '#0a0b09';
  context.fillRect(0, 0, size.width, size.height);
  drawGrid(context, size, view);
  drawEdges(context, layout, edges, view, selectedId, selectedNeighbors);
  drawNodes(context, layout.nodes, view, showLabels, selectedId, selectedNeighbors);
}

// Draw a world-anchored reference grid that makes panning legible.
function drawGrid(context: CanvasRenderingContext2D, size: CanvasSize, view: AtlasView) {
  const spacing = Math.max(28, 100 * view.scale);
  const startX = ((view.offsetX % spacing) + spacing) % spacing;
  const startY = ((view.offsetY % spacing) + spacing) % spacing;
  context.beginPath();
  for (let x = startX; x < size.width; x += spacing) {
    context.moveTo(x, 0);
    context.lineTo(x, size.height);
  }
  for (let y = startY; y < size.height; y += spacing) {
    context.moveTo(0, y);
    context.lineTo(size.width, y);
  }
  context.strokeStyle = 'rgba(232, 228, 217, 0.035)';
  context.lineWidth = 1;
  context.stroke();
}

// Draw bounded semantic relationships with neighborhood emphasis.
function drawEdges(
  context: CanvasRenderingContext2D,
  layout: AtlasLayout,
  edges: GraphEdge[],
  view: AtlasView,
  selectedId: string | null,
  selectedNeighbors: Set<string>
) {
  for (const edge of edges) {
    const source = layout.nodeById.get(edge.source);
    const target = layout.nodeById.get(edge.target);
    if (!source || !target) continue;
    const isSelected = selectedId != null
      && (edge.source === selectedId || edge.target === selectedId)
      && (selectedNeighbors.has(edge.source) || selectedNeighbors.has(edge.target));
    context.beginPath();
    context.moveTo(source.x * view.scale + view.offsetX, source.y * view.scale + view.offsetY);
    context.lineTo(target.x * view.scale + view.offsetX, target.y * view.scale + view.offsetY);
    context.strokeStyle = isSelected ? 'rgba(244, 119, 33, 0.72)' : `rgba(170, 166, 155, ${0.06 + edge.weight * 0.12})`;
    context.lineWidth = isSelected ? 1.6 : 0.6;
    context.stroke();
  }
}

// Draw nodes, selection halos, and a strictly bounded label set.
function drawNodes(
  context: CanvasRenderingContext2D,
  nodes: AtlasNode[],
  view: AtlasView,
  showLabels: boolean,
  selectedId: string | null,
  selectedNeighbors: Set<string>
) {
  const labelCandidates = showLabels
    ? new Set(
        [...nodes]
          .sort((left, right) => right.importance - left.importance || right.degree - left.degree)
          .slice(0, 32)
          .map((node) => node.id)
      )
    : new Set<string>();

  for (const node of nodes) {
    const screenX = node.x * view.scale + view.offsetX;
    const screenY = node.y * view.scale + view.offsetY;
    const selected = node.id === selectedId;
    const neighbor = selectedNeighbors.has(node.id);
    const radius = Math.max(2.2, Math.min(7.5, 2 + node.importance * 0.34 + Math.sqrt(node.degree) * 0.18));
    if (selected) {
      context.beginPath();
      context.arc(screenX, screenY, radius + 7, 0, Math.PI * 2);
      context.strokeStyle = 'rgba(244, 119, 33, 0.72)';
      context.lineWidth = 2;
      context.stroke();
    }
    context.beginPath();
    context.arc(screenX, screenY, selected ? radius + 1.5 : radius, 0, Math.PI * 2);
    context.fillStyle = selected ? '#fffaf0' : neighbor ? '#f47721' : categoryColor(node.category);
    context.globalAlpha = selectedId && !selected && !neighbor ? 0.28 : 0.88;
    context.fill();
    context.globalAlpha = 1;

    if (selected || labelCandidates.has(node.id)) {
      context.font = selected ? '600 11px "JetBrains Mono"' : '9px "JetBrains Mono"';
      context.fillStyle = selected ? '#fffaf0' : 'rgba(232, 228, 217, 0.66)';
      context.fillText(node.label.slice(0, 34), screenX + radius + 5, screenY + 3);
    }
  }
}
