<script lang="ts">
  import { onMount } from 'svelte';
  import { getInbox, approveMemory, rejectMemory, type Memory } from '$lib/stores/engram';

  let pending: Memory[] = $state([]);
  let loading = $state(true);
  let error = $state('');

  let pendingCount = $derived(pending.length);

  const categoryColors: Record<string, string> = {
    task: 'text-blue-400', discovery: 'text-emerald-400', decision: 'text-amber-400',
    state: 'text-purple-400', issue: 'text-red-400', reference: 'text-cyan-400', general: 'text-gray-400',
  };

  onMount(async () => {
    await loadInbox();
  });

  async function loadInbox() {
    loading = true;
    error = '';
    try {
      pending = await getInbox();
    } catch (e: any) {
      error = e.message;
    } finally {
      loading = false;
    }
  }

  async function handleApprove(id: number) {
    try {
      await approveMemory(id);
      pending = pending.filter((m) => m.id !== id);
    } catch (e: any) {
      error = e.message;
    }
  }

  async function handleReject(id: number) {
    try {
      await rejectMemory(id);
      pending = pending.filter((m) => m.id !== id);
    } catch (e: any) {
      error = e.message;
    }
  }
</script>

<div class="p-6 max-w-5xl">
  <div class="flex items-center gap-3 mb-6">
    <h2 class="text-2xl font-bold">Inbox</h2>
    {#if pendingCount > 0}
      <span class="px-2.5 py-0.5 bg-orange-500/20 text-orange-400 text-xs font-bold rounded-full border border-orange-500/30">
        {pendingCount}
      </span>
    {/if}
  </div>

  {#if error}
    <div class="p-4 bg-red-900/30 border border-red-800 rounded-lg text-red-300 text-sm mb-4">{error}</div>
  {/if}

  {#if loading}
    <div class="text-sm text-gray-500">Loading inbox...</div>
  {:else if pending.length === 0}
    <div class="p-8 bg-gray-900/40 border border-gray-800 rounded-xl text-center">
      <div class="text-3xl mb-3 text-gray-600">&#x2713;</div>
      <p class="text-gray-400 text-sm">Inbox empty</p>
      <p class="text-gray-600 text-xs mt-1">No memories pending review</p>
    </div>
  {:else}
    <div class="space-y-2">
      {#each pending as mem}
        <div class="p-4 bg-gray-900/40 border border-gray-800 rounded-lg hover:border-gray-700 transition-colors">
          <div class="flex items-center gap-2 mb-2">
            <span class="text-[10px] font-mono text-gray-600">#{mem.id}</span>
            <span class="text-[10px] font-medium {categoryColors[mem.category] || 'text-gray-400'}">{mem.category}</span>
            {#if mem.source}
              <span class="text-[10px] px-1.5 py-0.5 bg-gray-800/60 border border-gray-700/40 rounded text-gray-500">{mem.source}</span>
            {/if}
            <span class="text-[10px] text-gray-600 ml-auto">{mem.created_at?.substring(0, 16)}</span>
          </div>
          <p class="text-sm text-gray-300 mb-3">{mem.content}</p>
          <div class="flex gap-2">
            <button
              onclick={() => handleApprove(mem.id)}
              class="px-3 py-1.5 rounded-lg text-xs font-medium bg-emerald-600/20 hover:bg-emerald-600/30 text-emerald-400 border border-emerald-600/30 transition-colors"
            >
              Approve
            </button>
            <button
              onclick={() => handleReject(mem.id)}
              class="px-3 py-1.5 rounded-lg text-xs font-medium bg-red-600/20 hover:bg-red-600/30 text-red-400 border border-red-600/30 transition-colors"
            >
              Reject
            </button>
          </div>
        </div>
      {/each}
    </div>
  {/if}
</div>
