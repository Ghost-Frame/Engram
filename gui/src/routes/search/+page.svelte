<script lang="ts">
  import { search, archiveMemory, deleteMemory, type Memory } from '$lib/stores/engram';

  let query = $state('');
  let mode = $state('');
  let results: Memory[] = $state([]);
  let abstained = $state(false);
  let loading = $state(false);
  let error = $state('');

  const modes = ['fact', 'timeline', 'preference', 'decision', 'recent'];

  const categoryColors: Record<string, string> = {
    task: 'text-blue-400', discovery: 'text-emerald-400', decision: 'text-amber-400',
    state: 'text-purple-400', issue: 'text-red-400', reference: 'text-cyan-400', general: 'text-gray-400',
  };

  async function doSearch() {
    if (!query.trim()) return;
    loading = true;
    error = '';
    try {
      const res = await search(query, mode || undefined);
      results = res.results;
      abstained = res.abstained;
    } catch (e: any) {
      error = e.message;
    } finally {
      loading = false;
    }
  }

  async function handleArchive(id: number) {
    try {
      await archiveMemory(id);
      results = results.filter((m) => m.id !== id);
    } catch (e: any) {
      error = e.message;
    }
  }

  async function handleDelete(id: number) {
    try {
      await deleteMemory(id);
      results = results.filter((m) => m.id !== id);
    } catch (e: any) {
      error = e.message;
    }
  }
</script>

<div class="p-6 max-w-5xl">
  <h2 class="text-2xl font-bold mb-6">Search</h2>

  {#if error}
    <div class="p-4 bg-red-900/30 border border-red-800 rounded-lg text-red-300 text-sm mb-4">{error}</div>
  {/if}

  <form onsubmit={(e) => { e.preventDefault(); doSearch(); }} class="mb-6 space-y-3">
    <div class="flex gap-2">
      <input
        type="text"
        bind:value={query}
        placeholder="Search memories..."
        class="flex-1 px-4 py-2.5 bg-gray-900/60 border border-gray-800 rounded-lg text-sm text-gray-200 placeholder-gray-500 focus:outline-none focus:border-indigo-500 transition-colors"
      />
      <button
        type="submit"
        disabled={loading || !query.trim()}
        class="px-5 py-2.5 bg-indigo-600 hover:bg-indigo-500 disabled:bg-gray-800 disabled:text-gray-600 rounded-lg text-sm font-medium transition-colors"
      >
        {loading ? 'Searching...' : 'Search'}
      </button>
    </div>

    <div class="flex gap-2 flex-wrap">
      <button
        type="button"
        onclick={() => mode = ''}
        class="px-3 py-1.5 rounded-lg text-xs font-medium transition-colors
          {mode === '' ? 'bg-indigo-500/20 text-indigo-400 border border-indigo-500/40' : 'bg-gray-900/40 text-gray-400 border border-gray-800 hover:border-gray-700'}"
      >
        auto
      </button>
      {#each modes as m}
        <button
          type="button"
          onclick={() => mode = m}
          class="px-3 py-1.5 rounded-lg text-xs font-medium transition-colors
            {mode === m ? 'bg-indigo-500/20 text-indigo-400 border border-indigo-500/40' : 'bg-gray-900/40 text-gray-400 border border-gray-800 hover:border-gray-700'}"
        >
          {m}
        </button>
      {/each}
    </div>
  </form>

  {#if abstained}
    <div class="p-3 bg-amber-900/20 border border-amber-800/40 rounded-lg text-amber-300 text-sm mb-4">
      Engram abstained from answering this query.
    </div>
  {/if}

  {#if results.length > 0}
    <p class="text-xs text-gray-500 mb-3">{results.length} result{results.length !== 1 ? 's' : ''}</p>
  {/if}

  <div class="space-y-2">
    {#each results as mem}
      <div class="p-3 bg-gray-900/40 border border-gray-800 rounded-lg hover:border-gray-700 transition-colors">
        <div class="flex items-center gap-2 mb-1">
          <span class="text-[10px] font-mono text-gray-600">#{mem.id}</span>
          <span class="text-[10px] font-medium {categoryColors[mem.category] || 'text-gray-400'}">{mem.category}</span>
          {#if mem.score != null}
            <span class="text-[10px] text-gray-500">score: {mem.score.toFixed(3)}</span>
          {/if}
          <span class="text-[10px] text-gray-600 ml-auto">{mem.created_at?.substring(0, 16)}</span>
        </div>
        <p class="text-sm text-gray-300 mb-2">{mem.content}</p>

        {#if mem.explain?.reasons?.length}
          <div class="flex flex-wrap gap-1 mb-2">
            {#each mem.explain.reasons as reason}
              <span class="text-[10px] px-1.5 py-0.5 bg-gray-800/60 border border-gray-700/40 rounded text-gray-400">{reason}</span>
            {/each}
          </div>
        {/if}

        <div class="flex gap-2">
          <button
            onclick={() => handleArchive(mem.id)}
            class="text-[10px] px-2 py-1 rounded bg-gray-800/50 hover:bg-gray-800 text-gray-400 hover:text-gray-200 transition-colors"
          >
            Archive
          </button>
          <button
            onclick={() => handleDelete(mem.id)}
            class="text-[10px] px-2 py-1 rounded bg-gray-800/50 hover:bg-red-900/40 text-gray-400 hover:text-red-400 transition-colors"
          >
            Delete
          </button>
        </div>
      </div>
    {/each}
  </div>
</div>
