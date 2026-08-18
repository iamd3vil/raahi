<script lang="ts">
  import { onMount } from 'svelte';
  import { api, ApiError } from '../lib/api';
  import type { Route, RouterTestResult, Service } from '../lib/types';
  import { toast, go } from '../lib/state.svelte';
  import Drawer from '../lib/components/Drawer.svelte';
  import EmptyState from '../lib/components/EmptyState.svelte';

  let routes = $state<Route[]>([]);
  let services = $state<Service[]>([]);
  let loading = $state(true);
  let open = $state(false);
  let editing = $state<Route | null>(null);
  let q = $state('');

  // route tester
  let testOpen = $state(false);
  let test = $state({ method: 'GET', host: '', path: '/', headers: '' });
  let testResult = $state<RouterTestResult | null>(null);
  let testing = $state(false);

  const filtered = $derived(
    q
      ? routes.filter(
          (r) =>
            r.name.toLowerCase().includes(q.toLowerCase()) ||
            r.hosts.some((h) => h.includes(q)) ||
            r.paths.some((p) => p.includes(q)),
        )
      : routes,
  );

  async function runTest() {
    testing = true;
    testResult = null;
    try {
      testResult = await api.routerTest(test);
    } catch (e) {
      toast((e as ApiError).message, 'err');
    } finally {
      testing = false;
    }
  }

  let form = $state({
    name: '',
    service_id: 0,
    priority: 0,
    hosts: '',
    paths: '/',
    methods: '',
    strip_path: false,
    preserve_host: false,
    enabled: true,
  });

  // header-conditions + traffic-split editor rows
  let hdrRows = $state<{ k: string; v: string }[]>([]);
  let splitRows = $state<{ service_id: number; weight: number }[]>([]);

  const csv = (s: string) =>
    s
      .split(',')
      .map((x) => x.trim())
      .filter(Boolean);

  const svcName = (id: number) => services.find((s) => s.id === id)?.name ?? `#${id}`;

  async function load() {
    loading = true;
    try {
      [routes, services] = await Promise.all([api.listRoutes(), api.listServices()]);
    } catch (e) {
      toast((e as ApiError).message, 'err');
    } finally {
      loading = false;
    }
  }

  function openNew() {
    if (services.length === 0) {
      toast('Create a service first', 'err');
      return;
    }
    editing = null;
    form = {
      name: '',
      service_id: services[0].id,
      priority: 0,
      hosts: '',
      paths: '/',
      methods: '',
      strip_path: false,
      preserve_host: false,
      enabled: true,
    };
    hdrRows = [];
    splitRows = [];
    open = true;
  }

  function openEdit(r: Route) {
    editing = r;
    form = {
      name: r.name,
      service_id: r.service_id,
      priority: r.priority,
      hosts: r.hosts.join(', '),
      paths: r.paths.join(', '),
      methods: r.methods.join(', '),
      strip_path: r.strip_path,
      preserve_host: r.preserve_host,
      enabled: r.enabled,
    };
    hdrRows = Object.entries(r.headers ?? {}).map(([k, v]) => ({ k, v }));
    splitRows = (r.splits ?? []).map((sp) => ({ ...sp }));
    open = true;
  }

  async function save() {
    const headers: Record<string, string> = {};
    for (const row of hdrRows) if (row.k.trim()) headers[row.k.trim()] = row.v;
    const payload = {
      name: form.name,
      service_id: Number(form.service_id),
      priority: Number(form.priority),
      hosts: csv(form.hosts),
      paths: csv(form.paths),
      methods: csv(form.methods).map((m) => m.toUpperCase()),
      headers,
      splits: splitRows
        .filter((sp) => sp.service_id)
        .map((sp) => ({ service_id: Number(sp.service_id), weight: Number(sp.weight) })),
      strip_path: form.strip_path,
      preserve_host: form.preserve_host,
      enabled: form.enabled,
    };
    try {
      if (editing) await api.updateRoute(editing.id, payload);
      else await api.createRoute(payload);
      toast(editing ? 'Route updated' : 'Route created', 'ok');
      open = false;
      await load();
    } catch (e) {
      toast((e as ApiError).message, 'err');
    }
  }

  async function toggleEnabled(r: Route) {
    try {
      await api.updateRoute(r.id, { ...r, enabled: !r.enabled });
      await load();
    } catch (e) {
      toast((e as ApiError).message, 'err');
    }
  }

  async function del(r: Route) {
    if (!confirm(`Delete route "${r.name}"?`)) return;
    try {
      await api.deleteRoute(r.id);
      toast('Route deleted', 'ok');
      await load();
    } catch (e) {
      toast((e as ApiError).message, 'err');
    }
  }

  onMount(load);
</script>

<div class="head-actions">
  <div class="search">
    <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
      <circle cx="11" cy="11" r="7" /><path d="m20 20-3.5-3.5" />
    </svg>
    <input class="input" placeholder="Search routes…" bind:value={q} />
  </div>
  <div class="flex" style="gap:8px">
    <button class="btn" onclick={() => { testResult = null; testOpen = true; }}>
      <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
        <path d="m5 3 14 9-14 9V3z" />
      </svg>
      Test a request
    </button>
    <button class="btn btn-primary" onclick={openNew}>+ New route</button>
  </div>
</div>

<div class="panel">
  {#if loading}
    <div class="empty"><span class="spinner"></span></div>
  {:else if routes.length === 0}
    <EmptyState
      icon="M4 7h11M4 7a2 2 0 1 0 0-4 2 2 0 0 0 0 4zm0 10h11m-11 0a2 2 0 1 0 0 4 2 2 0 0 0 0-4zm15-5a2 2 0 1 0 0-4 2 2 0 0 0 0 4zm0 0H8"
      title="No routes yet"
      description="Routes match incoming requests by host, path, and method, then send them to a service."
    >
      {#snippet action()}
        <button class="btn btn-primary" onclick={openNew}>+ Create your first route</button>
      {/snippet}
    </EmptyState>
  {:else if filtered.length === 0}
    <div class="empty">No routes match “{q}”.</div>
  {:else}
    <div class="table-wrap">
      <table class="table">
        <thead>
          <tr><th>Name</th><th>Match</th><th>Service</th><th>Flags</th><th>Prio</th><th>Enabled</th><th></th></tr>
        </thead>
        <tbody>
          {#each filtered as r (r.id)}
            <tr style:opacity={r.enabled ? 1 : 0.55}>
              <td><strong>{r.name}</strong></td>
              <td>
                {#if r.hosts.length}{#each r.hosts as h}<span class="chip">{h}</span>{/each}{/if}
                {#each r.paths as p}<span class="chip">{p}</span>{/each}
                {#each r.methods as m}<span class="chip">{m}</span>{/each}
                {#each Object.entries(r.headers ?? {}) as [hk, hv]}<span class="chip">{hk}: {hv}</span>{/each}
              </td>
              <td>
                <button class="link" onclick={() => go('services')}>{svcName(r.service_id)}</button>
              </td>
              <td>
                {#if r.strip_path}<span class="badge">strip</span>{/if}
                {#if r.preserve_host}<span class="badge">host</span>{/if}
                {#if r.splits?.length}<span class="badge">split</span>{/if}
              </td>
              <td class="mono">{r.priority}</td>
              <td><button class="toggle {r.enabled ? 'on' : ''}" aria-label="Toggle enabled" onclick={() => toggleEnabled(r)}></button></td>
              <td class="actions">
                <button class="btn btn-sm btn-ghost" onclick={() => openEdit(r)}>Edit</button>
                <button class="btn btn-sm btn-danger" onclick={() => del(r)}>Delete</button>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</div>

<Drawer bind:open title={editing ? `Edit ${editing.name}` : 'New route'}>
  <div class="field">
    <label for="r-name">Name</label>
    <input id="r-name" class="input" bind:value={form.name} placeholder="public-api" />
  </div>
  <div class="row">
    <div class="field">
      <label for="r-svc">Service</label>
      <select id="r-svc" class="select" bind:value={form.service_id}>
        {#each services as s}<option value={s.id}>{s.name}</option>{/each}
      </select>
    </div>
    <div class="field">
      <label for="r-prio">Priority</label>
      <input id="r-prio" class="input" type="number" bind:value={form.priority} />
      <span class="hint">Higher wins; ties broken by longest path.</span>
    </div>
  </div>
  <div class="field">
    <label for="r-hosts">Hosts <span class="faint">(comma-separated, blank = any)</span></label>
    <input id="r-hosts" class="input" bind:value={form.hosts} placeholder="api.example.com, *.example.com" />
  </div>
  <div class="field">
    <label for="r-paths">Paths <span class="faint">(comma-separated; prefixes, or ~regex matched from the start)</span></label>
    <input id="r-paths" class="input" bind:value={form.paths} placeholder={'/api, /v1, ~/users/\\d+'} />
  </div>
  <div class="field">
    <label for="r-methods">Methods <span class="faint">(comma-separated, blank = any)</span></label>
    <input id="r-methods" class="input" bind:value={form.methods} placeholder="GET, POST" />
  </div>
  <div class="field">
    <label for="rh-0k">Header conditions <span class="faint">(all must match; value * = present with any value)</span></label>
    {#each hdrRows as row, i}
      <div class="hdr-row">
        <input id={'rh-' + i + 'k'} class="input mono" placeholder="X-Version" bind:value={row.k} />
        <input class="input mono" placeholder="beta" bind:value={row.v} aria-label="Header value" />
        <button class="btn btn-sm btn-ghost" aria-label="Remove header condition" onclick={() => (hdrRows = hdrRows.filter((_, j) => j !== i))}>✕</button>
      </div>
    {/each}
    <button class="btn btn-sm" style="align-self:flex-start" onclick={() => hdrRows.push({ k: '', v: '' })}>+ Add header condition</button>
  </div>
  <div class="field">
    <label for="rs-0s">Traffic split <span class="faint">(optional)</span></label>
    {#each splitRows as row, i}
      <div class="hdr-row">
        <select id={'rs-' + i + 's'} class="select" bind:value={row.service_id} aria-label="Split service">
          {#each services as s}<option value={s.id}>{s.name}</option>{/each}
        </select>
        <input class="input" type="number" min="1" bind:value={row.weight} aria-label="Split weight" />
        <button class="btn btn-sm btn-ghost" aria-label="Remove split" onclick={() => (splitRows = splitRows.filter((_, j) => j !== i))}>✕</button>
      </div>
    {/each}
    <button class="btn btn-sm" style="align-self:flex-start" onclick={() => splitRows.push({ service_id: services[0]?.id ?? 0, weight: 1 })}>+ Add split</button>
    <span class="hint">When set, splits override the primary service by weighted round-robin.</span>
  </div>
  <div class="toggles">
    <button class="opt" onclick={() => (form.strip_path = !form.strip_path)}>
      <span class="toggle {form.strip_path ? 'on' : ''}"></span> Strip matched path prefix
    </button>
    <button class="opt" onclick={() => (form.preserve_host = !form.preserve_host)}>
      <span class="toggle {form.preserve_host ? 'on' : ''}"></span> Preserve original Host header
    </button>
    <button class="opt" onclick={() => (form.enabled = !form.enabled)}>
      <span class="toggle {form.enabled ? 'on' : ''}"></span> Enabled
    </button>
  </div>

  {#snippet footer()}
    <button class="btn btn-ghost" onclick={() => (open = false)}>Cancel</button>
    <button class="btn btn-primary" onclick={save}>{editing ? 'Save' : 'Create'}</button>
  {/snippet}
</Drawer>

<Drawer bind:open={testOpen} title="Test a request">
  <p class="muted" style="margin-top:0">
    Dry-run the router: see which route and service a request would hit, without sending it.
  </p>
  <div class="row">
    <div class="field" style="flex:0 0 110px">
      <label for="t-method">Method</label>
      <select id="t-method" class="select" bind:value={test.method}>
        {#each ['GET', 'POST', 'PUT', 'PATCH', 'DELETE', 'HEAD', 'OPTIONS'] as m}<option value={m}>{m}</option>{/each}
      </select>
    </div>
    <div class="field">
      <label for="t-host">Host</label>
      <input id="t-host" class="input mono" bind:value={test.host} placeholder="api.example.com" />
    </div>
  </div>
  <div class="field">
    <label for="t-path">Path</label>
    <input id="t-path" class="input mono" bind:value={test.path} placeholder="/api/v1/users" onkeydown={(e) => e.key === 'Enter' && runTest()} />
  </div>
  <div class="field">
    <label for="t-headers">Headers <span class="faint">(optional, Name:value pairs)</span></label>
    <input id="t-headers" class="input mono" bind:value={test.headers} placeholder="X-Version:beta, X-Debug:1" onkeydown={(e) => e.key === 'Enter' && runTest()} />
  </div>

  {#if testResult}
    {#if testResult.matched}
      <div class="test-result ok-r">
        <div class="tr-head"><span class="dot ok"></span> Matched</div>
        <dl>
          <dt>Route</dt><dd><strong>{testResult.route_name}</strong> <span class="faint">#{testResult.route_id}</span></dd>
          <dt>Service</dt><dd>{testResult.service_name} <span class="faint">#{testResult.service_id}</span></dd>
          <dt>Prefix</dt><dd class="mono">{testResult.matched_prefix || '—'}</dd>
          <dt>Rewrite</dt>
          <dd>
            {#if testResult.strip_path}<span class="badge">strip path</span>{/if}
            {#if testResult.preserve_host}<span class="badge">preserve host</span>{/if}
            {#if !testResult.strip_path && !testResult.preserve_host}<span class="faint">none</span>{/if}
          </dd>
          {#if testResult.splits?.length}
            <dt>Splits</dt>
            <dd>
              {#each testResult.splits as sp}<span class="chip">{sp.service_name} × {sp.weight}</span>{/each}
            </dd>
          {/if}
          <dt>Plugins</dt>
          <dd>
            {#if testResult.plugins?.length}
              {#each testResult.plugins as p}<span class="chip">{p}</span>{/each}
            {:else}<span class="faint">none</span>{/if}
          </dd>
        </dl>
      </div>
    {:else}
      <div class="test-result err-r">
        <div class="tr-head"><span class="dot err"></span> No route matched — the proxy would return 404.</div>
      </div>
    {/if}
  {/if}

  {#snippet footer()}
    <button class="btn btn-ghost" onclick={() => (testOpen = false)}>Close</button>
    <button class="btn btn-primary" onclick={runTest} disabled={testing}>
      {#if testing}<span class="spinner"></span>{/if}
      Test
    </button>
  {/snippet}
</Drawer>

<style>
  .link {
    background: none;
    border: none;
    color: var(--accent-text);
    cursor: pointer;
    font: inherit;
    padding: 0;
  }
  .toggles {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin-top: 6px;
  }
  .hdr-row {
    display: grid;
    grid-template-columns: 1fr 1fr auto;
    gap: 8px;
    margin-bottom: 8px;
  }
  .test-result {
    margin-top: 6px;
    border: 1px solid var(--border);
    border-radius: var(--r-md);
    padding: 14px;
    background: var(--surface-2);
  }
  .test-result.ok-r {
    border-color: rgba(52, 211, 153, 0.35);
  }
  .test-result.err-r {
    border-color: rgba(251, 113, 133, 0.35);
  }
  .tr-head {
    display: flex;
    align-items: center;
    gap: 9px;
    font-weight: 650;
  }
  .test-result dl {
    display: grid;
    grid-template-columns: 70px 1fr;
    gap: 8px 12px;
    margin: 12px 0 0;
    font-size: 13px;
  }
  .test-result dt {
    color: var(--faint);
  }
  .test-result dd {
    margin: 0;
  }
</style>
