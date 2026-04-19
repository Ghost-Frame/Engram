<script lang="ts">
  import { onMount } from 'svelte';
  import { getProjects } from '$lib/stores/engram';

  let projects: any[] = $state([]);
  let statusFilter = $state('');
  let loading = $state(true);
  let error = $state('');

  const statuses = ['all', 'active', 'completed', 'paused', 'archived'];

  const statusColors: Record<string, string> = {
    active: 'text-emerald-400', completed: 'text-blue-400', paused: 'text-amber-400', archived: 'text-gray-500',
  };

  const statusBg: Record<string, string> = {
    active: 'bg-emerald-500/10 border-emerald-500/30',
    completed: 'bg-blue-500/10 border-blue-500/30',
    paused: 'bg-amber-500/10 border-amber-500/30',
    archived: 'bg-gray-500/10 border-gray-500/30',
  };

  let filtered = $derived(
    statusFilter && statusFilter !== 'all'
      ? projects.filter((p) => p.status === statusFilter)
      : projects
  );

  onMount(async () => {
    await loadProjects();
  });

  async function loadProjects() {
    loading = true;
    error = '';
    try {
      projects = await getProjects();
    } catch (e: any) {
      error = e.message;
    } finally {
      loading = false;
    }
  }
</script>

<div class="p-6 max-w-5xl">
  <h2 class="text-2xl font-bold mb-6">Projects</h2>

  {#if error}
    <div class="p-4 bg-red-900/30 border border-red-800 rounded-lg text-red-300 text-sm mb-4">{error}</div>
  {/if}

  <div class="flex gap-2 flex-wrap mb-4">
    {#each statuses as s}
      <button
        onclick={() => statusFilter = s === 'all' ? '' : s}
        class="px-3 py-1.5 rounded-lg text-xs font-medium transition-colors
          {(s === 'all' && !statusFilter) || statusFilter === s
            ? 'bg-indigo-500/20 text-indigo-400 border border-indigo-500/40'
            : 'bg-gray-900/40 text-gray-400 border border-gray-800 hover:border-gray-700'}"
      >
        {s}
      </button>
    {/each}
  </div>

  {#if loading}
    <div class="text-sm text-gray-500">Loading projects...</div>
  {:else if filtered.length === 0}
    <div class="p-6 bg-gray-900/40 border border-gray-800 rounded-xl text-center">
      <p class="text-gray-400 text-sm">No projects found</p>
    </div>
  {:else}
    <p class="text-xs text-gray-500 mb-3">{filtered.length} project{filtered.length !== 1 ? 's' : ''}</p>
    <div class="space-y-2">
      {#each filtered as project}
        <div class="p-4 bg-gray-900/40 border border-gray-800 rounded-lg hover:border-gray-700 transition-colors">
          <div class="flex items-center gap-3 mb-2">
            <span class="text-sm font-medium text-gray-200">{project.name}</span>
            {#if project.status}
              <span class="text-[10px] px-2 py-0.5 rounded-full border font-medium {statusBg[project.status] || 'bg-gray-500/10 border-gray-500/30'} {statusColors[project.status] || 'text-gray-400'}">
                {project.status}
              </span>
            {/if}
          </div>
          {#if project.description}
            <p class="text-xs text-gray-400 line-clamp-2">{project.description}</p>
          {/if}
        </div>
      {/each}
    </div>
  {/if}
</div>
