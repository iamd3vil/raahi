<script lang="ts">
  import { onMount } from 'svelte';
  import { ui, applyTheme, toggleTheme, go, hasRole, toast, type View } from './lib/state.svelte';
  import { adminToken, api, ApiError, setAdminToken } from './lib/api';
  import type { AuthStatus } from './lib/types';
  import AddApplication from './pages/AddApplication.svelte';
  import Dashboard from './pages/Dashboard.svelte';
  import Requests from './pages/Requests.svelte';
  import Routes from './pages/Routes.svelte';
  import Services from './pages/Services.svelte';
  import StreamRoutes from './pages/StreamRoutes.svelte';
  import Plugins from './pages/Plugins.svelte';
  import Consumers from './pages/Consumers.svelte';
  import Certificates from './pages/Certificates.svelte';
  import Settings from './pages/Settings.svelte';
  import Users from './pages/Users.svelte';

  type NavItem = { id: View; label: string; icon: string; admin?: boolean };
  const nav: { section: string; items: NavItem[] }[] = [
    {
      section: 'Observe',
      items: [
        { id: 'dashboard', label: 'Dashboard', icon: 'M3 13h8V3H3v10zm0 8h8v-6H3v6zm10 0h8V11h-8v10zm0-18v6h8V3h-8z' },
        { id: 'requests', label: 'Requests', icon: 'M4 6h16M4 12h16M4 18h10' },
      ],
    },
    {
      section: 'Gateway',
      items: [
        { id: 'routes', label: 'Routes', icon: 'M4 7h11M4 7a2 2 0 1 0 0-4 2 2 0 0 0 0 4zm0 10h11m-11 0a2 2 0 1 0 0 4 2 2 0 0 0 0-4zm15-5a2 2 0 1 0 0-4 2 2 0 0 0 0 4zm0 0H8' },
        { id: 'services', label: 'Services', icon: 'M5 4h14a1 1 0 0 1 1 1v4a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1zm0 10h14a1 1 0 0 1 1 1v4a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1v-4a1 1 0 0 1 1-1z' },
        { id: 'stream-routes', label: 'Stream routes', icon: 'M8 3v18m8-18v18M3 8h18M3 16h18' },
        { id: 'plugins', label: 'Plugins', icon: 'M10 3v4m4-4v4M5 7h14l-1 12a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 7z' },
      ],
    },
    {
      section: 'Access',
      items: [
        { id: 'consumers', label: 'Consumers', icon: 'M16 7a4 4 0 1 1-8 0 4 4 0 0 1 8 0zM4 21v-2a4 4 0 0 1 4-4h8a4 4 0 0 1 4 4v2' },
        { id: 'certificates', label: 'Certificates', icon: 'M12 15a4 4 0 1 0 0-8 4 4 0 0 0 0 8zm0 0v6l-2-2-2 2-1-7m10 7-2-2-2 2' },
      ],
    },
    {
      section: 'System',
      items: [
        { id: 'users', label: 'Users', icon: 'M16 7a4 4 0 1 1-8 0 4 4 0 0 1 8 0zM4 21v-2a4 4 0 0 1 4-4h8a4 4 0 0 1 4 4v2m3-9v6m3-3h-6', admin: true },
        { id: 'settings', label: 'Settings', icon: 'M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6zm7-3a7 7 0 0 0-.1-1l2-1.6-2-3.4-2.4 1a7 7 0 0 0-1.7-1L14.5 2h-4l-.4 2.5a7 7 0 0 0-1.7 1l-2.4-1-2 3.4L6 11a7 7 0 0 0 0 2l-2 1.6 2 3.4 2.4-1a7 7 0 0 0 1.7 1l.4 2.5h4l.4-2.5a7 7 0 0 0 1.7-1l2.4 1 2-3.4-2-1.6a7 7 0 0 0 .1-1z' },
      ],
    },
  ];

  const titles: Record<View, { title: string; sub: string }> = {
    'add-application': { title: 'Add application', sub: 'Connect an application to your domain' },
    dashboard: { title: 'Dashboard', sub: 'Live traffic, latency, and topology at a glance' },
    requests: { title: 'Requests', sub: 'Recent requests through the proxy, live-tailed' },
    routes: { title: 'Routes', sub: 'Match requests by host, path, and method' },
    services: { title: 'Services', sub: 'Upstream groups with load balancing and health' },
    'stream-routes': { title: 'Stream routes', sub: 'Raw TCP listeners proxied to services (L4)' },
    plugins: { title: 'Plugins', sub: 'Auth, rate limiting, CORS, and header transforms' },
    consumers: { title: 'Consumers', sub: 'Identities that auth plugins authenticate' },
    certificates: { title: 'Certificates', sub: 'TLS certificates served by SNI, hot-reloaded' },
    users: { title: 'Users', sub: 'Who can sign in, and what they may change' },
    settings: { title: 'Settings', sub: 'Listeners, defaults, auth, and configuration export' },
  };

  // ---- sign-in ----
  let status = $state<AuthStatus | null>(null);
  let email = $state('');
  let password = $state('');
  let tokenInput = $state('');
  let useToken = $state(false);
  let loginError = $state('');
  let loggingIn = $state(false);
  let menuOpen = $state(false);

  async function loginPassword() {
    loginError = '';
    loggingIn = true;
    try {
      await api.login(email.trim(), password);
      password = '';
      location.reload(); // restart live streams with the session cookie attached
    } catch (e) {
      loginError = (e as ApiError).message;
    } finally {
      loggingIn = false;
    }
  }

  async function loginToken() {
    loginError = '';
    setAdminToken(tokenInput.trim());
    try {
      await api.me(); // validates the token
      tokenInput = '';
      location.reload();
    } catch {
      setAdminToken(null);
      loginError = 'Invalid token';
    }
  }

  async function signOut() {
    menuOpen = false;
    try {
      if (ui.me?.method === 'session') await api.logout();
    } catch {
      /* cookie is cleared server-side regardless */
    }
    setAdminToken(null);
    location.reload();
  }

  function submitLogin(e: KeyboardEvent) {
    if (e.key !== 'Enter') return;
    if (useToken) loginToken();
    else if (email && password) loginPassword();
  }

  onMount(() => {
    applyTheme();
    // An SSO failure redirects back here with ?sso_error=...; show it on the login card.
    const params = new URLSearchParams(location.search);
    const ssoErr = params.get('sso_error');
    if (ssoErr) {
      loginError = ssoErr;
      history.replaceState(null, '', location.pathname + location.hash);
    }
    api
      .adminStatus()
      .then((s) => {
        status = s;
        useToken = s.token_enabled && !s.users_exist && !s.sso.enabled;
        // Resolve who we are (in open mode this is an implicit admin); a 401 flips
        // ui.authRequired via the API client.
        return api.me().then((p) => (ui.me = p)).catch(() => {});
      })
      .catch(() => {});
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
      <div class="brand-mark" aria-hidden="true">
        <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="#ffffff" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
          <path d="M5 19V9a4 4 0 0 1 4-4h1" />
          <circle cx="17" cy="7" r="2.4" fill="#ffffff" stroke="none" />
          <path d="M13 19h2a4 4 0 0 0 4-4v-3" />
        </svg>
      </div>
      <div>
        <div class="brand-name">Raahi</div>
        <div class="brand-sub">proxy control</div>
      </div>
    </div>

    <nav>
      {#each nav as group}
        <div class="nav-section">{group.section}</div>
        {#each group.items.filter((i) => !i.admin || hasRole('admin')) as item}
          <button class="nav-item" class:active={ui.view === item.id} onclick={() => go(item.id)}>
            <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
              <path d={item.icon} />
            </svg>
            <span>{item.label}</span>
          </button>
        {/each}
      {/each}
    </nav>

    <div class="side-foot">
      <span class="dot {ui.connected ? 'ok' : 'err'}"></span>
      <span class="faint">{ui.connected ? 'connected' : 'offline'}</span>
    </div>
  </aside>

  <main>
    <header class="topbar">
      <div>
        <h1>{titles[ui.view as View].title}</h1>
        <div class="topbar-sub">{titles[ui.view as View].sub}</div>
      </div>
      <div class="topbar-right">
        {#if hasRole('editor') && ui.view !== 'add-application'}
          <button class="outline small" onclick={() => go('add-application')}>+ Add application</button>
        {/if}
        {#if ui.me}
          <div class="user-menu">
            <button class="ghost small user-btn" onclick={() => (menuOpen = !menuOpen)} aria-haspopup="menu" aria-expanded={menuOpen}>
              <span class="badge" data-variant={ui.me.role === 'admin' ? 'warning' : ui.me.role === 'editor' ? 'info' : undefined}>{ui.me.role}</span>
              <span class="user-name">{ui.me.user?.name || ui.me.user?.email || (ui.me.method === 'token' ? 'admin token' : 'open access')}</span>
            </button>
            {#if menuOpen}
              <div class="menu" role="menu">
                {#if ui.me.user}
                  <div class="menu-head">
                    <div><strong>{ui.me.user.email}</strong></div>
                    <div class="faint">{ui.me.role} · signed in with {ui.me.method}</div>
                  </div>
                {:else}
                  <div class="menu-head faint">
                    {ui.me.method === 'token' ? 'Authenticated with the admin token' : 'No auth configured — everyone is admin'}
                  </div>
                {/if}
                {#if ui.me.user}
                  <button class="ghost small" role="menuitem" onclick={() => { menuOpen = false; go('settings'); }}>Change password</button>
                {/if}
                {#if ui.me.method !== 'open'}
                  <button class="ghost small" role="menuitem" onclick={signOut}>Sign out</button>
                {/if}
              </div>
            {/if}
          </div>
        {/if}
      <button class="ghost small icon" onclick={toggleTheme} aria-label="Toggle theme" title="Toggle theme">
        {#if ui.theme === 'dark'}
          <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round">
            <circle cx="12" cy="12" r="4" />
            <path d="M12 2v2m0 16v2M4.9 4.9l1.4 1.4m11.4 11.4 1.4 1.4M2 12h2m16 0h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4" />
          </svg>
        {:else}
          <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
            <path d="M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8z" />
          </svg>
        {/if}
      </button>
      </div>
    </header>

    <div class="content">
      <div class="page">
        {#if ui.view === 'add-application'}
          <AddApplication />
        {:else if ui.view === 'dashboard'}
          <Dashboard />
        {:else if ui.view === 'requests'}
          <Requests />
        {:else if ui.view === 'routes'}
          <Routes />
        {:else if ui.view === 'services'}
          <Services />
        {:else if ui.view === 'stream-routes'}
          <StreamRoutes />
        {:else if ui.view === 'plugins'}
          <Plugins />
        {:else if ui.view === 'consumers'}
          <Consumers />
        {:else if ui.view === 'certificates'}
          <Certificates />
        {:else if ui.view === 'users'}
          <Users />
        {:else if ui.view === 'settings'}
          <Settings />
        {/if}
      </div>
    </div>
  </main>
</div>

{#if ui.authRequired}
  <div class="login-overlay">
    <div class="login-card">
      <div class="brand" style="padding:0 0 6px">
        <div class="brand-mark" aria-hidden="true">
          <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="#ffffff" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M5 19V9a4 4 0 0 1 4-4h1" />
            <circle cx="17" cy="7" r="2.4" fill="#ffffff" stroke="none" />
            <path d="M13 19h2a4 4 0 0 0 4-4v-3" />
          </svg>
        </div>
        <div class="brand-name">Raahi</div>
      </div>
      <h2>Sign in</h2>

      {#if status?.sso.enabled && !useToken}
        <a class="sso-btn" href={api.ssoStartUrl()}>
          <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
            <path d="M15 3h4a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2h-4M10 17l5-5-5-5M15 12H3" />
          </svg>
          Continue with {status.sso.label}
        </a>
        {#if status.users_exist}<div class="or"><span>or</span></div>{/if}
      {/if}

      {#if useToken}
        <p class="muted">Paste the admin API bearer token.</p>
        <input class="mono" type="password" placeholder="admin token" bind:value={tokenInput} onkeydown={submitLogin} />
        {#if loginError}<div class="login-err">{loginError}</div>{/if}
        <button style="width:100%" onclick={loginToken} disabled={!tokenInput.trim()}>Unlock</button>
      {:else if status?.users_exist || !status?.sso.enabled}
        <input type="email" placeholder="email" autocomplete="username" bind:value={email} onkeydown={submitLogin} />
        <input type="password" placeholder="password" autocomplete="current-password" bind:value={password} onkeydown={submitLogin} />
        {#if loginError}<div class="login-err">{loginError}</div>{/if}
        <button style="width:100%" onclick={loginPassword} disabled={loggingIn || !email.trim() || !password}>
          {#if loggingIn}<span aria-busy="true" data-spinner="small"></span>{:else}Sign in{/if}
        </button>
      {:else if loginError}
        <div class="login-err">{loginError}</div>
      {/if}

      {#if status?.token_enabled}
        <button class="ghost small" style="align-self:center" onclick={() => { useToken = !useToken; loginError = ''; }}>
          {useToken ? 'Sign in with email instead' : 'Use the admin token instead'}
        </button>
      {/if}
    </div>
  </div>
{/if}

<style>
  .login-overlay {
    position: fixed;
    inset: 0;
    background: var(--background);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 950;
  }
  .login-card {
    width: min(380px, calc(100% - 32px));
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: var(--radius-large);
    padding: 24px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .login-card h2 {
    font-size: 16px;
  }
  .login-card p {
    margin: 0;
    font-size: 13px;
  }
  .login-err {
    color: var(--danger);
    font-size: 13px;
  }
  .sso-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    padding: 9px 14px;
    border-radius: var(--radius-medium);
    background: var(--primary);
    color: var(--primary-foreground, #fff);
    font-weight: 550;
    font-size: 13.5px;
    text-decoration: none;
  }
  .sso-btn:hover {
    filter: brightness(1.08);
  }
  .or {
    display: flex;
    align-items: center;
    gap: 10px;
    color: var(--faint-foreground);
    font-size: 12px;
  }
  .or::before,
  .or::after {
    content: '';
    flex: 1;
    border-top: 1px solid var(--border);
  }
  .topbar-right {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .user-menu {
    position: relative;
  }
  .user-btn {
    display: inline-flex;
    align-items: center;
    gap: 8px;
  }
  .user-name {
    max-width: 220px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .menu {
    position: absolute;
    right: 0;
    top: calc(100% + 6px);
    min-width: 240px;
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: var(--radius-medium);
    box-shadow: var(--shadow-large);
    padding: 6px;
    display: flex;
    flex-direction: column;
    gap: 2px;
    z-index: 50;
  }
  .menu .ghost {
    justify-content: flex-start;
    width: 100%;
  }
  .menu-head {
    padding: 8px 10px 10px;
    border-bottom: 1px solid var(--border);
    margin-bottom: 4px;
    font-size: 13px;
  }
  .shell {
    display: grid;
    grid-template-columns: 232px 1fr;
    height: 100%;
  }
  .sidebar {
    border-right: 1px solid var(--border);
    background: var(--background);
    display: flex;
    flex-direction: column;
    padding: 16px 10px 10px;
    gap: 8px;
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 2px 8px 14px;
  }
  .brand-mark {
    width: 28px;
    height: 28px;
    border-radius: 7px;
    background: var(--primary);
    display: flex;
    align-items: center;
    justify-content: center;
    flex: none;
  }
  .brand-name {
    font-size: 15px;
    font-weight: 650;
    letter-spacing: -0.01em;
    line-height: 1.2;
  }
  .brand-sub {
    font-size: 10.5px;
    color: var(--faint-foreground);
    text-transform: uppercase;
    letter-spacing: 0.08em;
  }
  nav {
    display: flex;
    flex-direction: column;
    gap: 2px;
    flex: 1;
    overflow-y: auto;
  }
  .nav-section {
    font-size: 10.5px;
    text-transform: uppercase;
    letter-spacing: 0.12em;
    color: var(--faint-foreground);
    font-weight: 650;
    padding: 12px 12px 5px;
  }
  .nav-section:first-child {
    padding-top: 2px;
  }
  .nav-item {
    display: flex;
    align-items: center;
    justify-content: flex-start;
    gap: 10px;
    padding: 7px 10px;
    border-radius: var(--radius-medium);
    border: none;
    background: transparent;
    color: var(--muted-foreground);
    font: inherit;
    font-size: 13.5px;
    font-weight: 500;
    cursor: pointer;
    text-align: left;
    transition: background 0.12s ease, color 0.12s ease;
  }
  .nav-item:hover {
    background: var(--muted);
    color: var(--foreground);
  }
  .nav-item:active {
    transform: none;
  }
  .nav-item.active {
    color: var(--primary);
    background: var(--accent-soft);
    font-weight: 550;
  }
  .side-foot {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 12px 4px;
    font-size: 12px;
    border-top: 1px solid var(--border);
  }
  main {
    display: flex;
    flex-direction: column;
    height: 100%;
    overflow: hidden;
    padding-block-start: 0;
  }
  .topbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 13px 28px;
    border-bottom: 1px solid var(--border);
    gap: 12px;
  }
  .topbar h1 {
    font-size: 16px;
    font-weight: 600;
  }
  .topbar-sub {
    font-size: 12.5px;
    color: var(--faint-foreground);
    margin-top: 1px;
  }
  .content {
    padding: 22px 28px 40px;
    overflow-y: auto;
    flex: 1;
    max-width: 1400px;
    width: 100%;
    margin: 0 auto;
  }
  @media (max-width: 760px) {
    .topbar { flex-wrap: wrap; padding: 14px 16px; }
    .topbar > div:first-child { flex: 1 1 180px; }
    .topbar-right { flex-wrap: wrap; }
    .shell {
      grid-template-columns: 60px 1fr;
    }
    .brand-name,
    .brand-sub,
    .nav-item span,
    .nav-section,
    .side-foot .faint {
      display: none;
    }
    .nav-item {
      justify-content: center;
      padding: 10px 0;
    }
    .content {
      padding: 16px;
    }
  }
</style>
