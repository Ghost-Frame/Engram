<script lang="ts">
  import { onMount } from 'svelte';
  import { getEntities } from '$lib/stores/engram';

  let entities: any[] = $state([]);
  let typeFilter = $state('');
  let loading = $state(true);
  let error = $state('');

  const types = ['all', 'person', 'server', 'tool', 'service'];

  const typeColors: Record<string, string> = {
    person: 'text-blue-400', server: 'text-emerald-400', tool: 'text-amber-400',
    service: 'text-purple-400', project: 'text-cyan-400', organization: 'text-indigo-400',
  };

  const typeIcons: Record<string, string> = {
    person: '\u25C9', server: '\u25A3', tool: '\u2692', service: '\u25CE', project: '\u25A0', organization: '\u25C8',
  };

  let filtered = $derived(
    typeFilter && typeFilter !== 'all'
      ? entities.filter((e) => e.type === typeFilter)
      : entities
  );

  onMount(async () => {
    await loadEntities();
  });

  async function loadEntities() {
    loading = true;
    error = '';
    try {
      entities = await getEntities();
    } catch (e: any) {
      error = e.message;
    } finally {
      loading = false;
    }
  }
</script>

<div class="p-6 max-w-5xl">
  <h2 class="text-2xl font-bold mb-6">Entities</h2>

  {#if error}
    <div class="p-4 bg-red-900/30 border border-red-800 rounded-lg text-red-300 text-sm mb-4">{error}</div>
  {/if}

  <div class="flex gap-2 flex-wrap mb-4">
    {#each types as t}
      <button
        onclick={() => typeFilter = t === 'all' ? '' : t}
        class="px-3 py-1.5 rounded-lg text-xs font-medium transition-colors
          {(t === 'all' && !typeFilter) || typeFilter === t
            ? 'bg-indigo-500/20 text-indigo-400 border border-indigo-500/40'
            : 'bg-gray-900/40 text-gray-400 border border-gray-800 hover:border-gray-700'}"
      >
        {t}
      </button>
    {/each}
  </div>

  {#if loading}
    <div class="text-sm text-gray-500">Loading entities...</div>
  {:else if filtered.length === 0}
    <div class="p-6 bg-gray-900/40 border border-gray-800 rounded-xl text-center">
      <p class="text-gray-400 text-sm">No entities found</p>
    </div>
  {:else}
    <p class="text-xs text-gray-500 mb-3">{filtered.length} entit{filtered.length !== 1 ? 'ies' : 'y'}</p>
    <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
      {#each filtered as entity}
        <div class="p-4 bg-gray-900/40 border border-gray-800 rounded-lg hover:border-gray-700 transition-colors">
          <div class="flex items-center gap-2 mb-2">
            <span class="text-base {typeColors[entity.type] || 'text-gray-400'}">{typeIcons[entity.type] || '\u25CB'}</span>
            <span class="text-sm font-medium text-gray-200">{entity.name}</span>
            <span class="text-[10px] px-1.5 py-0.5 bg-gray-800/60 border border-gray-700/40 rounded {typeColors[entity.type] || 'text-gray-400'}">{entity.type}</span>
          </div>
          {#if entity.description}
            <p class="text-xs text-gray-400 mb-2 line-clamp-2">{entity.description}</p>
          {/if}
          {#if entity.memory_count != null}
            <div class="text-[10px] text-gray-600">
              {entity.memory_count} memor{entity.memory_count !== 1 ? 'ies' : 'y'}
            </div>
          {/if}
        </div>
      {/each}
    </div>
  {/if}
</div>
