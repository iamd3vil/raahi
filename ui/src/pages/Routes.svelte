<script lang="ts">
  import { onMount } from 'svelte';
  import { api, ApiError } from '../lib/api';
  import type { Route, Service } from '../lib/types';
  import { toast, go } from '../lib/state.svelte';
  import Drawer from '../lib/components/Drawer.svelte';

  let routes = $state<Route[]>([]);
  let services = $state<Service[]>([]);
  let loading = $state(true);
  let open = $state(false);
  let editing = $state<Route | null>(null);

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
    open = true;
  }

  async function save() {
    const payload = {
      name: form.name,
      service_id: Number(form.service_id),
      priority: Number(form.priority),
      hosts: csv(form.hosts),
      paths: csv(form.paths),
      methods: csv(form.methods).map((m) => m.toUpperCase()),
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
  <p class="muted">Match incoming requests by host, path, and method, then bind them to a service.</p>
  <button class="btn btn-primary" onclick={openNew}>+ New route</button>
</div>

<div class="panel">
  {#if loading}
    <div class="empty"><span class="spinner"></span></div>
  {:else if routes.length === 0}
    <div class="empty">No routes yet. <button class="btn btn-sm" onclick={openNew}>Create one</button></div>
  {:else}
    <div class="table-wrap">
      <table class="table">
        <thead>
          <tr><th>Name</th><th>Match</th><th>Service</th><th>Flags</th><th>Prio</th><th></th></tr>
        </thead>
        <tbody>
          {#each routes as r (r.id)}
            <tr style:opacity={r.enabled ? 1 : 0.55}>
              <td><strong>{r.name}</strong></td>
              <td>
                {#if r.hosts.length}{#each r.hosts as h}<span class="chip">{h}</span>{/each}{/if}
                {#each r.paths as p}<span class="chip">{p}</span>{/each}
                {#each r.methods as m}<span class="chip">{m}</span>{/each}
              </td>
              <td>
                <button class="link" onclick={() => go('services')}>{svcName(r.service_id)}</button>
              </td>
              <td>
                {#if r.strip_path}<span class="badge">strip</span>{/if}
                {#if r.preserve_host}<span class="badge">host</span>{/if}
                {#if !r.enabled}<span class="badge err">off</span>{/if}
              </td>
              <td class="mono">{r.priority}</td>
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
    <label for="r-paths">Path prefixes <span class="faint">(comma-separated)</span></label>
    <input id="r-paths" class="input" bind:value={form.paths} placeholder="/api, /v1" />
  </div>
  <div class="field">
    <label for="r-methods">Methods <span class="faint">(comma-separated, blank = any)</span></label>
    <input id="r-methods" class="input" bind:value={form.methods} placeholder="GET, POST" />
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

<style>
  .head-actions {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 16px;
    gap: 12px;
  }
  .actions {
    text-align: right;
    white-space: nowrap;
  }
  .link {
    background: none;
    border: none;
    color: var(--violet);
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
  .opt {
    display: flex;
    align-items: center;
    gap: 11px;
    background: none;
    border: none;
    color: var(--text);
    font: inherit;
    cursor: pointer;
    padding: 8px 0;
  }
</style>
