<script lang="ts">
  import { onMount } from 'svelte';
  import { ui, applyTheme, toggleTheme, go, type View } from './lib/state.svelte';
  import Toasts from './lib/components/Toasts.svelte';
  import Dashboard from './pages/Dashboard.svelte';
  import Routes from './pages/Routes.svelte';
  import Services from './pages/Services.svelte';
  import Plugins from './pages/Plugins.svelte';
  import Consumers from './pages/Consumers.svelte';
  import Certificates from './pages/Certificates.svelte';
  import Settings from './pages/Settings.svelte';

  const nav: { id: View; label: string; icon: string }[] = [
    { id: 'dashboard', label: 'Dashboard', icon: 'M3 13h8V3H3v10zm0 8h8v-6H3v6zm10 0h8V11h-8v10zm0-18v6h8V3h-8z' },
    { id: 'routes', label: 'Routes', icon: 'M4 7h11M4 7a2 2 0 1 0 0-4 2 2 0 0 0 0 4zm0 10h11m-11 0a2 2 0 1 0 0 4 2 2 0 0 0 0-4zm15-5a2 2 0 1 0 0-4 2 2 0 0 0 0 4zm0 0H8' },
    { id: 'services', label: 'Services', icon: 'M5 4h14a1 1 0 0 1 1 1v4a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1zm0 10h14a1 1 0 0 1 1 1v4a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1v-4a1 1 0 0 1 1-1z' },
    { id: 'plugins', label: 'Plugins', icon: 'M10 3v4m4-4v4M5 7h14l-1 12a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 7z' },
    { id: 'consumers', label: 'Consumers', icon: 'M16 7a4 4 0 1 1-8 0 4 4 0 0 1 8 0zM4 21v-2a4 4 0 0 1 4-4h8a4 4 0 0 1 4 4v2' },
    { id: 'certificates', label: 'Certificates', icon: 'M12 15a4 4 0 1 0 0-8 4 4 0 0 0 0 8zm0 0v6l-2-2-2 2-1-7m10 7-2-2-2 2' },
    { id: 'settings', label: 'Settings', icon: 'M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6zm7-3a7 7 0 0 0-.1-1l2-1.6-2-3.4-2.4 1a7 7 0 0 0-1.7-1L14.5 2h-4l-.4 2.5a7 7 0 0 0-1.7 1l-2.4-1-2 3.4L6 11a7 7 0 0 0 0 2l-2 1.6 2 3.4 2.4-1a7 7 0 0 0 1.7 1l.4 2.5h4l.4-2.5a7 7 0 0 0 1.7-1l2.4 1 2-3.4-2-1.6a7 7 0 0 0 .1-1z' },
  ];

  const titles: Record<View, string> = {
    dashboard: 'Dashboard',
    routes: 'Routes',
    services: 'Services',
    plugins: 'Plugins',
    consumers: 'Consumers',
    certificates: 'Certificates',
    settings: 'Settings',
  };

  onMount(() => {
    applyTheme();
    const onHash = () => {
      const v = location.hash.slice(1) as View;
      if (v && titles[v]) ui.view = v;
    };
    window.addEventListener('hashchange', onHash);

    const ping = async () => {
      try {
        const r = await fetch('/healthz');
        ui.connected = r.ok;
      } catch {
        ui.connected = false;
      }
    };
    ping();
    const t = setInterval(ping, 5000);
    return () => {
      window.removeEventListener('hashchange', onHash);
      clearInterval(t);
    };
  });
</script>

<div class="shell">
  <aside class="sidebar">
    <div class="brand">
      <svg viewBox="0 0 32 32" width="30" height="30">
        <defs>
          <linearGradient id="logo" x1="0" y1="0" x2="1" y2="1">
            <stop offset="0" stop-color="#2DD4BF" />
            <stop offset="0.5" stop-color="#7C5CFF" />
            <stop offset="1" stop-color="#E84CC6" />
          </linearGradient>
        </defs>
        <circle cx="16" cy="16" r="13" fill="none" stroke="url(#logo)" stroke-width="3" />
        <circle cx="16" cy="16" r="4" fill="url(#logo)" />
      </svg>
      <div>
        <div class="brand-name gradient-text">Raahi</div>
        <div class="brand-sub">proxy control</div>
      </div>
    </div>

    <nav>
      {#each nav as item}
        <button class="nav-item" class:active={ui.view === item.id} onclick={() => go(item.id)}>
          <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
            <path d={item.icon} />
          </svg>
          <span>{item.label}</span>
        </button>
      {/each}
    </nav>

    <div class="side-foot">
      <span class="dot {ui.connected ? 'ok' : 'err'}"></span>
      <span class="faint">{ui.connected ? 'connected' : 'offline'}</span>
    </div>
  </aside>

  <main>
    <header class="topbar">
      <h1>{titles[ui.view as View]}</h1>
      <button class="btn btn-ghost btn-sm" onclick={toggleTheme} aria-label="Toggle theme">
        {ui.theme === 'dark' ? '☀' : '☾'}
      </button>
    </header>

    <div class="content">
      {#if ui.view === 'dashboard'}
        <Dashboard />
      {:else if ui.view === 'routes'}
        <Routes />
      {:else if ui.view === 'services'}
        <Services />
      {:else if ui.view === 'plugins'}
        <Plugins />
      {:else if ui.view === 'consumers'}
        <Consumers />
      {:else if ui.view === 'certificates'}
        <Certificates />
      {:else if ui.view === 'settings'}
        <Settings />
      {/if}
    </div>
  </main>
</div>

<Toasts />

<style>
  .shell {
    display: grid;
    grid-template-columns: 244px 1fr;
    height: 100%;
  }
  .sidebar {
    border-right: 1px solid var(--border);
    background: var(--surface);
    display: flex;
    flex-direction: column;
    padding: 18px 14px;
    gap: 8px;
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 11px;
    padding: 6px 8px 16px;
  }
  .brand-name {
    font-size: 19px;
    font-weight: 750;
    letter-spacing: -0.02em;
  }
  .brand-sub {
    font-size: 11px;
    color: var(--faint);
    text-transform: uppercase;
    letter-spacing: 0.1em;
  }
  nav {
    display: flex;
    flex-direction: column;
    gap: 3px;
    flex: 1;
  }
  .nav-item {
    display: flex;
    align-items: center;
    gap: 11px;
    padding: 9px 12px;
    border-radius: var(--r-sm);
    border: none;
    background: transparent;
    color: var(--muted);
    font: inherit;
    font-weight: 550;
    cursor: pointer;
    text-align: left;
    transition: background 0.12s ease, color 0.12s ease;
  }
  .nav-item:hover {
    background: var(--surface-2);
    color: var(--text);
  }
  .nav-item.active {
    color: var(--text);
    background: var(--aurora-soft);
    box-shadow: inset 2px 0 0 var(--violet);
  }
  .side-foot {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 12px;
    font-size: 12px;
    border-top: 1px solid var(--border);
  }
  main {
    display: flex;
    flex-direction: column;
    height: 100%;
    overflow: hidden;
  }
  .topbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 16px 28px;
    border-bottom: 1px solid var(--border);
  }
  .topbar h1 {
    font-size: 20px;
  }
  .content {
    padding: 24px 28px;
    overflow-y: auto;
    flex: 1;
  }
  @media (max-width: 760px) {
    .shell {
      grid-template-columns: 64px 1fr;
    }
    .brand-name,
    .brand-sub,
    .nav-item span,
    .side-foot .faint {
      display: none;
    }
  }
</style>
