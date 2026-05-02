<script lang="ts">
  import { onMount } from 'svelte';
  import { listMemories, guard, type Memory } from '$lib/stores/engram';

  let memories: Memory[] = $state([]);
  let category = $state('');
  let loading = $state(true);
  let error = $state('');

  let guardInput = $state('');
  let guardResult: { verdict: string; reasons: string[] } | null = $state(null);
  let guardLoading = $state(false);

  const categories = ['all', 'task', 'discovery', 'decision', 'state', 'issue'];

  const categoryColors: Record<string, string> = {
    task: 'text-blue-400', discovery: 'text-emerald-400', decision: 'text-amber-400',
    state: 'text-purple-400', issue: 'text-red-400', reference: 'text-cyan-400', general: 'text-gray-400',
  };

  const categoryBg: Record<string, string> = {
    task: 'bg-blue-500/10 border-blue-500/20', discovery: 'bg-emerald-500/10 border-emerald-500/20',
    decision: 'bg-amber-500/10 border-amber-500/20', state: 'bg-purple-500/10 border-purple-500/20',
    issue: 'bg-red-500/10 border-red-500/20', reference: 'bg-cyan-500/10 border-cyan-500/20',
    general: 'bg-gray-500/10 border-gray-500/20',
  };

  let filtered = $derived(
    category && category !== 'all'
      ? memories.filter((m) => m.category === category)
      : memories
  );

  onMount(async () => {
    await loadMemories();
  });

  async function loadMemories() {
    loading = true;
    error = '';
    try {
      memories = await listMemories({ limit: 50 });
    } catch (e: any) {
      error = e.message;
    } finally {
      loading = false;
    }
  }

  async function checkGuard() {
    if (!guardInput.trim()) return;
    guardLoading = true;
    guardResult = null;
    try {
      guardResult = await guard(guardInput);
    } catch (e: any) {
      error = e.message;
    } finally {
      guardLoading = false;
    }
  }
</script>

<div class="p-6 max-w-5xl">
  <h2 class="text-2xl font-bold mb-6">Timeline</h2>

  {#if error}
    <div class="p-4 bg-red-900/30 border border-red-800 rounded-lg text-red-300 text-sm mb-4">{error}</div>
  {/if}

  <div class="mb-6 p-4 bg-gray-900/40 border border-gray-800 rounded-lg">
    <p class="text-xs text-gray-500 uppercase tracking-wide mb-2">Guard Check</p>
    <div class="flex gap-2">
      <input
        type="text"
        bind:value={guardInput}
        placeholder="Describe an action to check..."
        class="flex-1 px-3 py-2 bg-gray-900/60 border border-gray-800 rounded-lg text-sm text-gray-200 placeholder-gray-500 focus:outline-none focus:border-indigo-500 transition-colors"
        onkeydown={(e) => { if (e.key === 'Enter') checkGuard(); }}
      />
      <button
        onclick={checkGuard}
        disabled={guardLoading || !guardInput.trim()}
        class="px-4 py-2 bg-indigo-600 hover:bg-indigo-500 disabled:bg-gray-800 disabled:text-gray-600 rounded-lg text-sm font-medium transition-colors"
      >
        {guardLoading ? 'Checking...' : 'Check'}
      </button>
    </div>
    {#if guardResult}
      <div class="mt-3 p-3 rounded-lg border {guardResult.verdict === 'allow' ? 'bg-emerald-900/20 border-emerald-800/40' : 'bg-red-900/20 border-red-800/40'}">
        <span class="text-sm font-medium {guardResult.verdict === 'allow' ? 'text-emerald-400' : 'text-red-400'}">
          {guardResult.verdict.toUpperCase()}
        </span>
        {#if guardResult.reasons.length > 0}
          <ul class="mt-1 space-y-0.5">
            {#each guardResult.reasons as reason}
              <li class="text-xs text-gray-400">- {reason}</li>
            {/each}
          </ul>
        {/if}
      </div>
    {/if}
  </div>

  <div class="flex gap-2 flex-wrap mb-4">
    {#each categories as cat}
      <button
        onclick={() => category = cat === 'all' ? '' : cat}
        class="px-3 py-1.5 rounded-lg text-xs font-medium transition-colors
          {(cat === 'all' && !category) || category === cat
            ? 'bg-indigo-500/20 text-indigo-400 border border-indigo-500/40'
            : 'bg-gray-900/40 text-gray-400 border border-gray-800 hover:border-gray-700'}"
      >
        {cat}
      </button>
    {/each}
  </div>

  {#if loading}
    <div class="text-sm text-gray-500">Loading timeline...</div>
  {:else if filtered.length === 0}
    <div class="p-6 bg-gray-900/40 border border-gray-800 rounded-xl text-center">
      <p class="text-gray-400 text-sm">No memories found</p>
    </div>
  {:else}
    <div class="relative">
      <div class="absolute left-[7px] top-2 bottom-2 w-px bg-gray-800"></div>
      <div class="space-y-3">
        {#each filtered as mem}
          <div class="flex gap-4 relative">
            <div class="w-[15px] shrink-0 flex items-start justify-center pt-4">
              <div class="w-2.5 h-2.5 rounded-full border-2 border-gray-700 bg-gray-950 z-10"></div>
            </div>
            <div class="flex-1 p-3 border rounded-lg {categoryBg[mem.category] || categoryBg.general}">
              <div class="flex items-center gap-2 mb-1">
                <span class="text-[10px] font-mono text-gray-600">#{mem.id}</span>
                <span class="text-[10px] font-medium {categoryColors[mem.category] || 'text-gray-400'}">{mem.category}</span>
                {#if mem.source}
                  <span class="text-[10px] text-gray-600">{mem.source}</span>
                {/if}
                <span class="text-[10px] text-gray-600 ml-auto">{mem.created_at?.substring(0, 16)}</span>
              </div>
              <p class="text-sm text-gray-300">{mem.content}</p>
            </div>
          </div>
        {/each}
      </div>
    </div>
  {/if}
</div>
