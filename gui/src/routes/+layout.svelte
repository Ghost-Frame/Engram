<script lang="ts">
  import '../app.css';
  import { page } from '$app/stores';
  import { apiKey, isAuthed, getHealth } from '$lib/stores/engram';
  import { onMount } from 'svelte';

  let { children } = $props();
  let health: any = $state(null);
  let keyInput = $state('');
  let showKeyModal = $state(false);

  const nav = [
    { path: '/', label: 'Dashboard', icon: '⊞' },
    { path: '/graph', label: 'Graph', icon: '◉' },
    { path: '/search', label: 'Search', icon: '⌕' },
    { path: '/inbox', label: 'Inbox', icon: '☐' },
    { path: '/timeline', label: 'Timeline', icon: '☰' },
    { path: '/entities', label: 'Entities', icon: '◎' },
    { path: '/projects', label: 'Projects', icon: '▦' },
  ];

  onMount(async () => {
    try { health = await getHealth(); } catch {}
  });

  function saveKey() {
    apiKey.set(keyInput);
    showKeyModal = false;
    location.reload();
  }
</script>

<div class="flex h-screen bg-gray-950 text-gray-200">
  <nav class="w-52 bg-gray-900/80 border-r border-gray-800 flex flex-col shrink-0">
    <div class="p-4 border-b border-gray-800">
      <h1 class="text-lg font-bold tracking-wider">
        <span class="bg-gradient-to-r from-indigo-400 to-purple-400 bg-clip-text text-transparent">ENGRAM</span>
      </h1>
      {#if health}
        <p class="text-[10px] text-gray-500 mt-1">v{health.version} | {health.memories ?? '?'} memories</p>
      {/if}
    </div>

    <div class="flex-1 py-2">
      {#each nav as item}
        <a
          href={item.path}
          class="flex items-center gap-3 px-4 py-2.5 text-sm transition-colors
            {$page.url.pathname === item.path
              ? 'bg-indigo-500/10 text-indigo-400 border-r-2 border-indigo-400'
              : 'text-gray-400 hover:bg-gray-800/50 hover:text-gray-200'}"
        >
          <span class="text-base w-5 text-center">{item.icon}</span>
          {item.label}
        </a>
      {/each}
    </div>

    <div class="p-3 border-t border-gray-800">
      <button
        onclick={() => showKeyModal = true}
        class="w-full px-3 py-2 text-xs rounded-lg bg-gray-800/50 hover:bg-gray-800 text-gray-400 hover:text-gray-200 transition-colors"
      >
        {$isAuthed ? 'API Key Set' : 'Set API Key'}
      </button>
    </div>
  </nav>

  <main class="flex-1 overflow-auto">
    {@render children()}
  </main>
</div>

{#if showKeyModal}
  <div class="fixed inset-0 bg-black/60 flex items-center justify-center z-50" role="dialog">
    <div class="bg-gray-900 border border-gray-700 rounded-xl p-6 w-96">
      <h2 class="text-lg font-semibold mb-4">API Key</h2>
      <input
        type="password"
        bind:value={keyInput}
        placeholder="eg_..."
        class="w-full px-3 py-2 bg-gray-800 border border-gray-700 rounded-lg text-sm text-gray-200 placeholder-gray-500 focus:outline-none focus:border-indigo-500"
      />
      <div class="flex gap-2 mt-4">
        <button onclick={saveKey} class="flex-1 px-4 py-2 bg-indigo-600 hover:bg-indigo-500 rounded-lg text-sm font-medium transition-colors">Save</button>
        <button onclick={() => showKeyModal = false} class="px-4 py-2 bg-gray-800 hover:bg-gray-700 rounded-lg text-sm transition-colors">Cancel</button>
      </div>
    </div>
  </div>
{/if}
