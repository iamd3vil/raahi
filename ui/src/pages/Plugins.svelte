<script lang="ts">
  import { onMount } from 'svelte';
  import { api, ApiError } from '../lib/api';
  import type { Plugin, PluginScope, PluginType, Route, Service } from '../lib/types';
  import { PLUGIN_TYPES, PLUGIN_LABELS } from '../lib/types';
  import { toast } from '../lib/state.svelte';
  import Drawer from '../lib/components/Drawer.svelte';

  let plugins = $state<Plugin[]>([]);
  let services = $state<Service[]>([]);
  let routes = $state<Route[]>([]);
  let loading = $state(true);
  let open = $state(false);
  let editing = $state<Plugin | null>(null);

  const DEFAULTS: Record<PluginType, Record<string, unknown>> = {
    'key-auth': { key_names: ['apikey', 'x-api-key'], hide_credentials: false },
    'basic-auth': { realm: 'Raahi' },
    'rate-limit': { limit: 60, window_secs: 60, key: 'ip' },
    cors: { allow_origins: ['*'], allow_methods: ['GET', 'POST', 'PUT', 'DELETE', 'OPTIONS'], allow_headers: ['*'], allow_credentials: false, max_age: 3600 },
    'request-transform': { add: { 'x-example': 'value' }, remove: [] },
    'response-transform': { add: { 'x-powered-by': 'raahi' }, remove: [] },
  };

  let form = $state({
    type: 'key-auth' as PluginType,
    scope: 'route' as PluginScope,
    service_id: null as number | null,
    route_id: null as number | null,
    config: '',
    ordering: 0,
    enabled: true,
  });

  function setType(t: PluginType) {
    form.type = t;
    if (!editing) form.config = JSON.stringify(DEFAULTS[t], null, 2);
  }

  async function load() {
    loading = true;
    try {
      [plugins, services, routes] = await Promise.all([
        api.listPlugins(),
        api.listServices(),
        api.listRoutes(),
      ]);
    } catch (e) {
      toast((e as ApiError).message, 'err');
    } finally {
      loading = false;
    }
  }

  function openNew() {
    editing = null;
    form = {
      type: 'key-auth',
      scope: 'route',
      service_id: null,
      route_id: routes[0]?.id ?? null,
      config: JSON.stringify(DEFAULTS['key-auth'], null, 2),
      ordering: 0,
      enabled: true,
    };
    open = true;
  }

  function openEdit(p: Plugin) {
    editing = p;
    form = {
      type: p.type,
      scope: p.scope,
      service_id: p.service_id,
      route_id: p.route_id,
      config: JSON.stringify(p.config ?? {}, null, 2),
      ordering: p.ordering,
      enabled: p.enabled,
    };
    open = true;
  }

  async function save() {
    let config: Record<string, unknown>;
    try {
      config = form.config.trim() ? JSON.parse(form.config) : {};
    } catch {
      toast('Config is not valid JSON', 'err');
      return;
    }
    const payload = {
      type: form.type,
      scope: form.scope,
      service_id: form.scope === 'service' ? Number(form.service_id) : null,
      route_id: form.scope === 'route' ? Number(form.route_id) : null,
      config,
      ordering: Number(form.ordering),
      enabled: form.enabled,
    };
    try {
      if (editing) await api.updatePlugin(editing.id, payload);
      else await api.createPlugin(payload);
      toast(editing ? 'Plugin updated' : 'Plugin created', 'ok');
      open = false;
      await load();
    } catch (e) {
      toast((e as ApiError).message, 'err');
    }
  }

  async function del(p: Plugin) {
    if (!confirm('Delete this plugin?')) return;
    try {
      await api.deletePlugin(p.id);
      toast('Plugin deleted', 'ok');
      await load();
    } catch (e) {
      toast((e as ApiError).message, 'err');
    }
  }

  function scopeLabel(p: Plugin): string {
    if (p.scope === 'global') return 'global';
    if (p.scope === 'service') return 'service: ' + (services.find((s) => s.id === p.service_id)?.name ?? p.service_id);
    return 'route: ' + (routes.find((r) => r.id === p.route_id)?.name ?? p.route_id);
  }

  onMount(load);
</script>

<div class="head-actions">
  <p class="muted">Native middleware applied per route, per service, or globally. (WASM plugins arrive in a later phase.)</p>
  <button class="btn btn-primary" onclick={openNew}>+ New plugin</button>
</div>

<div class="panel">
  {#if loading}
    <div class="empty"><span class="spinner"></span></div>
  {:else if plugins.length === 0}
    <div class="empty">No plugins configured.</div>
  {:else}
    <div class="table-wrap">
      <table class="table">
        <thead><tr><th>Type</th><th>Scope</th><th>Order</th><th>Enabled</th><th></th></tr></thead>
        <tbody>
          {#each plugins as p (p.id)}
            <tr>
              <td><span class="badge accent">{PLUGIN_LABELS[p.type]}</span></td>
              <td class="mono">{scopeLabel(p)}</td>
              <td class="mono">{p.ordering}</td>
              <td><span class="dot {p.enabled ? 'ok' : 'err'}"></span></td>
              <td class="actions">
                <button class="btn btn-sm btn-ghost" onclick={() => openEdit(p)}>Edit</button>
                <button class="btn btn-sm btn-danger" onclick={() => del(p)}>Delete</button>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</div>

<Drawer bind:open title={editing ? 'Edit plugin' : 'New plugin'}>
  <div class="field">
    <label for="p-type">Type</label>
    <select id="p-type" class="select" value={form.type} onchange={(e) => setType((e.currentTarget as HTMLSelectElement).value as PluginType)}>
      {#each PLUGIN_TYPES as t}<option value={t}>{PLUGIN_LABELS[t]}</option>{/each}
    </select>
  </div>
  <div class="field">
    <label for="p-scope">Scope</label>
    <select id="p-scope" class="select" bind:value={form.scope}>
      <option value="global">Global (all routes)</option>
      <option value="service">Service</option>
      <option value="route">Route</option>
    </select>
  </div>
  {#if form.scope === 'service'}
    <div class="field">
      <label for="p-svc">Service</label>
      <select id="p-svc" class="select" bind:value={form.service_id}>
        {#each services as s}<option value={s.id}>{s.name}</option>{/each}
      </select>
    </div>
  {:else if form.scope === 'route'}
    <div class="field">
      <label for="p-route">Route</label>
      <select id="p-route" class="select" bind:value={form.route_id}>
        {#each routes as r}<option value={r.id}>{r.name}</option>{/each}
      </select>
    </div>
  {/if}
  <div class="field">
    <label for="p-config">Config (JSON)</label>
    <textarea id="p-config" class="textarea" rows="9" bind:value={form.config}></textarea>
  </div>
  <div class="row">
    <div class="field">
      <label for="p-order">Ordering</label>
      <input id="p-order" class="input" type="number" bind:value={form.ordering} />
    </div>
  </div>
  <button class="opt" onclick={() => (form.enabled = !form.enabled)}>
    <span class="toggle {form.enabled ? 'on' : ''}"></span> Enabled
  </button>

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
