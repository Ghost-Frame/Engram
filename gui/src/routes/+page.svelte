<script lang="ts">
  import { onMount } from 'svelte';
  import { getHealth, listMemories, type Memory } from '$lib/stores/engram';

  let health: any = $state(null);
  let recent: Memory[] = $state([]);
  let error = $state('');

  const categoryColors: Record<string, string> = {
    task: 'text-blue-400', discovery: 'text-emerald-400', decision: 'text-amber-400',
    state: 'text-purple-400', issue: 'text-red-400', reference: 'text-cyan-400', general: 'text-gray-400',
  };

  onMount(async () => {
    try {
      [health, recent] = await Promise.all([getHealth(), listMemories({ limit: 8 })]);
    } catch (e: any) { error = e.message; }
  });
</script>

<div class="p-6 max-w-5xl">
  <h2 class="text-2xl font-bold mb-6">Dashboard</h2>

  {#if error}
    <div class="p-4 bg-red-900/30 border border-red-800 rounded-lg text-red-300 text-sm mb-4">{error}</div>
  {/if}

  {#if health}
    <div class="grid grid-cols-4 gap-4 mb-8">
      {#each [
        { label: 'Memories', value: health.memories ?? 0, color: 'text-indigo-400' },
        { label: 'Entities', value: health.entities ?? 0, color: 'text-emerald-400' },
        { label: 'Episodes', value: health.episodes ?? 0, color: 'text-amber-400' },
        { label: 'Pending', value: health.pending ?? 0, color: (health.pending ?? 0) > 0 ? 'text-orange-400' : 'text-gray-500' },
      ] as stat}
        <div class="p-4 bg-gray-900/60 border border-gray-800 rounded-xl">
          <div class="text-2xl font-bold {stat.color}">{stat.value}</div>
          <div class="text-xs text-gray-500 mt-1">{stat.label}</div>
        </div>
      {/each}
    </div>

    <div class="grid grid-cols-3 gap-3 mb-8">
      <div class="p-3 bg-gray-900/40 border border-gray-800 rounded-lg text-xs">
        <span class="text-gray-500">LLM:</span>
        <span class="ml-1 {health.llm_configured ? 'text-emerald-400' : 'text-red-400'}">{health.llm_configured ? 'Active' : 'Off'}</span>
      </div>
      <div class="p-3 bg-gray-900/40 border border-gray-800 rounded-lg text-xs">
        <span class="text-gray-500">Embeddings:</span>
        <span class="ml-1 text-indigo-400">{health.embedding_model}</span>
      </div>
      <div class="p-3 bg-gray-900/40 border border-gray-800 rounded-lg text-xs">
        <span class="text-gray-500">Static:</span> <span class="ml-1">{health.static ?? 0}</span>
        <span class="text-gray-600 mx-1">|</span>
        <span class="text-gray-500">Versioned:</span> <span class="ml-1">{health.versioned ?? 0}</span>
      </div>
    </div>
  {/if}

  <h3 class="text-sm font-semibold text-gray-400 uppercase tracking-wide mb-3">Recent Memories</h3>
  <div class="space-y-2">
    {#each recent as mem}
      <div class="p-3 bg-gray-900/40 border border-gray-800 rounded-lg hover:border-gray-700 transition-colors">
        <div class="flex items-center gap-2 mb-1">
          <span class="text-[10px] font-mono text-gray-600">#{mem.id}</span>
          <span class="text-[10px] font-medium {categoryColors[mem.category] || 'text-gray-400'}">{mem.category}</span>
          <span class="text-[10px] text-gray-600 ml-auto">{mem.created_at?.substring(0, 16)}</span>
        </div>
        <p class="text-sm text-gray-300 line-clamp-2">{mem.content}</p>
      </div>
    {/each}
  </div>
</div>
