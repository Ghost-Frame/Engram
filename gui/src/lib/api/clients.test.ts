import { beforeEach, describe, expect, it, vi } from 'vitest';
import * as axon from './axon';
import * as broca from './broca';
import * as chiasm from './chiasm';
import * as graph from './graph';
import * as loom from './loom';
import * as memory from './memory';
import * as soma from './soma';
import * as thymus from './thymus';

// Return a fetch mock that responds with a JSON body.
function ok(body: unknown) {
  return vi.fn(async () => new Response(JSON.stringify(body), { headers: { 'content-type': 'application/json' }, status: 200 }));
}

describe('service API clients', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it('calls Chiasm stats and task paths', async () => {
    const spy = ok({ count: 1, tasks: [{ id: 1 }] });
    vi.stubGlobal('fetch', spy);

    await chiasm.getChiasmStats();
    await chiasm.listTasks();

    const calls = spy.mock.calls as unknown as Array<[RequestInfo | URL, RequestInit?]>;
    expect(calls.map((call) => call[0])).toEqual(['/tasks/stats', '/tasks']);
  });

  it('unwraps Broca feed items', async () => {
    vi.stubGlobal('fetch', ok({ count: 1, items: [{ id: 1 }] }));

    expect((await broca.getFeed()).length).toBe(1);
  });

  it('unwraps Soma agents', async () => {
    vi.stubGlobal('fetch', ok({ agents: [{ id: 1 }], count: 1 }));

    expect((await soma.listAgents()).length).toBe(1);
  });

  it('unwraps Loom runs', async () => {
    vi.stubGlobal('fetch', ok({ runs: [{ id: 1 }] }));

    expect((await loom.listRuns()).length).toBe(1);
  });

  it('unwraps Axon events', async () => {
    vi.stubGlobal('fetch', ok({ count: 1, events: [{ id: 1 }] }));

    expect((await axon.listEvents()).length).toBe(1);
  });

  it('unwraps Thymus evaluations', async () => {
    vi.stubGlobal('fetch', ok({ evaluations: [{ id: 1 }] }));

    expect((await thymus.listEvaluations()).length).toBe(1);
  });

  it('unwraps memory search results', async () => {
    vi.stubGlobal('fetch', ok({ abstained: false, results: [{ id: 1 }], top_score: 1 }));

    expect((await memory.search('q')).length).toBe(1);
  });

  it('passes graph node cap', async () => {
    const spy = ok({ edge_count: 0, edges: [], node_count: 0, nodes: [] });
    vi.stubGlobal('fetch', spy);

    await graph.getGraph(1500);

    const calls = spy.mock.calls as unknown as Array<[RequestInfo | URL, RequestInit?]>;
    expect(calls[0][0]).toBe('/graph?max=1500');
  });

  it('requests the memory graph with depth, cap, and min_component', async () => {
    const spy = ok({ edge_count: 0, edges: [], node_count: 0, nodes: [] });
    vi.stubGlobal('fetch', spy);

    // Default min_component prunes singleton dust (2); the 3D graph relies on it.
    await graph.getMemoryGraph(3, 50000);

    const calls = spy.mock.calls as unknown as Array<[RequestInfo | URL, RequestInit?]>;
    expect(calls[0][0]).toBe('/graph?depth=3&max=50000&min_component=2');
  });
});
