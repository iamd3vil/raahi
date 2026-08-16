<script lang="ts">
  import { onMount } from 'svelte';
  import { api, ApiError } from '../lib/api';
  import type { Service, StreamRoute } from '../lib/types';
  import { toast, go } from '../lib/state.svelte';
  import Drawer from '../lib/components/Drawer.svelte';
  import EmptyState from '../lib/components/EmptyState.svelte';

  let routes = $state<StreamRoute[]>([]);
  let services = $state<Service[]>([]);
  let loading = $state(true);
  let open = $state(false);
  let editing = $state<StreamRoute | null>(null);
  let form = $state({ name: '', listen_addr: '', service_id: 0, enabled: true });

  const svcName = (id: number) => services.find((s) => s.id === id)?.name ?? `#${id}`;

  async function load() {
    loading = true;
    try {
      [routes, services] = await Promise.all([api.listStreamRoutes(), api.listServices()]);
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
    form = { name: '', listen_addr: '', service_id: services[0].id, enabled: true };
    open = true;
  }

  function openEdit(r: StreamRoute) {
    editing = r;
    form = { name: r.name, listen_addr: r.listen_addr, service_id: r.service_id, enabled: r.enabled };
    open = true;
  }

  async function save() {
    const payload = {
      name: form.name,
      listen_addr: form.listen_addr.trim(),
      service_id: Number(form.service_id),
      enabled: form.enabled,
    };
    try {
      const res = editing
        ? await api.updateStreamRoute(editing.id, payload)
        : await api.createStreamRoute(payload);
      toast(
        res.note
          ? `Stream route ${editing ? 'updated' : 'created'} — ${res.note}.`
          : `Stream route ${editing ? 'updated' : 'created'} — applied live.`,
        'ok',
      );
      open = false;
      await load();
    } catch (e) {
      toast((e as ApiError).message, 'err');
    }
  }

  async function toggleEnabled(r: StreamRoute) {
    try {
      await api.updateStreamRoute(r.id, { ...r, enabled: !r.enabled });
      await load();
    } catch (e) {
      toast((e as ApiError).message, 'err');
    }
  }

  async function del(r: StreamRoute) {
    if (!confirm(`Delete stream route "${r.name}"?`)) return;
    try {
      const res = await api.deleteStreamRoute(r.id);
      toast(res.note ? `Stream route deleted — ${res.note}.` : 'Stream route deleted', 'ok');
      await load();
    } catch (e) {
      toast((e as ApiError).message, 'err');
    }
  }

  onMount(load);
</script>

<div class="head-actions">
  <p class="muted">Raw TCP (L4) listeners spliced to a service's targets — databases, message queues, anything over TCP.</p>
  <button class="btn btn-primary" onclick={openNew}>+ New stream route</button>
</div>

<div class="note">
  <strong>Note:</strong> Stream listeners bind at startup — <strong>adding or removing</strong> a stream
  route (or changing its listen address) takes effect after a proxy restart. <strong>Retargeting</strong> an
  existing route to another service, and enable/disable, apply live.
</div>

<div class="panel">
  {#if loading}
    <div class="empty"><span class="spinner"></span></div>
  {:else if routes.length === 0}
    <EmptyState
      icon="M8 3v18m8-18v18M3 8h18M3 16h18"
      title="No stream routes yet"
      description="A stream route binds a local TCP port and proxies every connection to a service's targets, byte for byte."
    >
      {#snippet action()}
        <button class="btn btn-primary" onclick={openNew}>+ Create your first stream route</button>
      {/snippet}
    </EmptyState>
  {:else}
    <div class="table-wrap">
      <table class="table">
        <thead><tr><th>Name</th><th>Listen address</th><th>Service</th><th>Enabled</th><th></th></tr></thead>
        <tbody>
          {#each routes as r (r.id)}
            <tr style:opacity={r.enabled ? 1 : 0.55}>
              <td><strong>{r.name}</strong></td>
              <td class="mono">{r.listen_addr}</td>
              <td>
                <button class="link" onclick={() => go('services')}>{svcName(r.service_id)}</button>
              </td>
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

<Drawer bind:open title={editing ? `Edit ${editing.name}` : 'New stream route'}>
  <div class="field">
    <label for="sr-name">Name</label>
    <input id="sr-name" class="input" bind:value={form.name} placeholder="postgres" />
  </div>
  <div class="field">
    <label for="sr-addr">Listen address</label>
    <input id="sr-addr" class="input mono" bind:value={form.listen_addr} placeholder="0.0.0.0:5432" />
    <span class="hint">Binding a new address requires a proxy restart.</span>
  </div>
  <div class="field">
    <label for="sr-svc">Service</label>
    <select id="sr-svc" class="select" bind:value={form.service_id}>
      {#each services as s}<option value={s.id}>{s.name}</option>{/each}
    </select>
    <span class="hint">Retargeting applies live — no restart.</span>
  </div>
  <div class="toggles">
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
  }
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
</style>
