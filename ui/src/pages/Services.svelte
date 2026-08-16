<script lang="ts">
  import { onMount } from 'svelte';
  import { api, ApiError } from '../lib/api';
  import type { LbAlgorithm, Protocol, Service, Target, TargetHealth } from '../lib/types';
  import { toast } from '../lib/state.svelte';
  import Drawer from '../lib/components/Drawer.svelte';
  import EmptyState from '../lib/components/EmptyState.svelte';

  let services = $state<Service[]>([]);
  let targetsBy = $state<Record<number, Target[]>>({});
  let health = $state<Map<number, boolean>>(new Map());
  let loading = $state(true);
  let open = $state(false);
  let editing = $state<Service | null>(null);

  async function loadHealth() {
    try {
      const hs: TargetHealth[] = await api.health();
      health = new Map(hs.map((h) => [h.target_id, h.healthy]));
    } catch {
      /* ignore */
    }
  }
  // disabled targets are "off", enabled ones reflect the live check
  const tState = (t: Target): 'ok' | 'err' | 'off' =>
    !t.enabled ? 'off' : (health.get(t.id) ?? true) ? 'ok' : 'err';

  let form = $state({
    name: '',
    protocol: 'http' as Protocol,
    lb_algorithm: 'round_robin' as LbAlgorithm,
    connect_timeout_ms: 5000,
    read_timeout_ms: 60000,
    write_timeout_ms: 60000,
    retries: 1,
    tls_sni: '',
    health_path: '',
  });
  let tForm = $state({ host: '', port: 80, weight: 100 });

  async function load() {
    loading = true;
    try {
      services = await api.listServices();
      const lists = await Promise.all(services.map((s) => api.listTargets(s.id)));
      const map: Record<number, Target[]> = {};
      services.forEach((s, i) => (map[s.id] = lists[i]));
      targetsBy = map;
    } catch (e) {
      toast((e as ApiError).message, 'err');
    } finally {
      loading = false;
    }
  }

  function openNew() {
    editing = null;
    form = {
      name: '',
      protocol: 'http',
      lb_algorithm: 'round_robin',
      connect_timeout_ms: 5000,
      read_timeout_ms: 60000,
      write_timeout_ms: 60000,
      retries: 1,
      tls_sni: '',
      health_path: '',
    };
    open = true;
  }

  function openEdit(s: Service) {
    editing = s;
    form = {
      name: s.name,
      protocol: s.protocol,
      lb_algorithm: s.lb_algorithm,
      connect_timeout_ms: s.connect_timeout_ms,
      read_timeout_ms: s.read_timeout_ms,
      write_timeout_ms: s.write_timeout_ms,
      retries: s.retries,
      tls_sni: s.tls_sni ?? '',
      health_path: s.health_path ?? '',
    };
    open = true;
  }

  async function save() {
    const payload = {
      ...form,
      tls_sni: form.tls_sni || null,
      health_path: form.health_path.trim() || null,
    };
    try {
      if (editing) {
        const s = await api.updateService(editing.id, payload);
        editing = s;
        toast('Service updated', 'ok');
      } else {
        const s = await api.createService(payload);
        editing = s; // keep drawer open to add targets
        toast('Service created — add targets below', 'ok');
      }
      await load();
    } catch (e) {
      toast((e as ApiError).message, 'err');
    }
  }

  async function del(s: Service) {
    if (!confirm(`Delete service "${s.name}" and its targets?`)) return;
    try {
      await api.deleteService(s.id);
      toast('Service deleted', 'ok');
      if (editing?.id === s.id) open = false;
      await load();
    } catch (e) {
      toast((e as ApiError).message, 'err');
    }
  }

  async function addTarget() {
    if (!editing || !tForm.host) return;
    try {
      await api.createTarget(editing.id, { ...tForm });
      tForm = { host: '', port: 80, weight: 100 };
      await load();
    } catch (e) {
      toast((e as ApiError).message, 'err');
    }
  }

  async function delTarget(t: Target) {
    try {
      await api.deleteTarget(t.id);
      await load();
    } catch (e) {
      toast((e as ApiError).message, 'err');
    }
  }

  async function toggleTarget(t: Target) {
    try {
      await api.updateTarget(t.id, { host: t.host, port: t.port, weight: t.weight, enabled: !t.enabled });
      await load();
    } catch (e) {
      toast((e as ApiError).message, 'err');
    }
  }

  const editingTargets = $derived(editing ? (targetsBy[editing.id] ?? []) : []);

  onMount(() => {
    load();
    loadHealth();
    const t = setInterval(loadHealth, 5000);
    return () => clearInterval(t);
  });
</script>

<div class="head-actions">
  <p class="muted">Upstreams: named groups of backend targets with load balancing.</p>
  <button class="btn btn-primary" onclick={openNew}>+ New service</button>
</div>

<div class="panel">
  {#if loading}
    <div class="empty"><span class="spinner"></span></div>
  {:else if services.length === 0}
    <EmptyState
      icon="M5 4h14a1 1 0 0 1 1 1v4a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1zm0 10h14a1 1 0 0 1 1 1v4a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1v-4a1 1 0 0 1 1-1z"
      title="No services yet"
      description="A service groups one or more upstream targets behind a load-balancing policy. Routes send traffic to services."
    >
      {#snippet action()}
        <button class="btn btn-primary" onclick={openNew}>+ Create your first service</button>
      {/snippet}
    </EmptyState>
  {:else}
    <div class="table-wrap">
      <table class="table">
        <thead>
          <tr><th>Name</th><th>Protocol</th><th>Balancing</th><th>Targets</th><th></th></tr>
        </thead>
        <tbody>
          {#each services as s (s.id)}
            {@const ts = targetsBy[s.id] ?? []}
            <tr>
              <td><strong>{s.name}</strong></td>
              <td><span class="badge {s.protocol === 'https' ? 'accent' : ''}">{s.protocol}</span></td>
              <td class="mono">{s.lb_algorithm}</td>
              <td>
                {#if ts.length === 0}
                  <span class="badge warn">no targets</span>
                {:else}
                  {#each ts as t}
                    {@const st = tState(t)}
                    <span class="chip" title={st === 'off' ? 'disabled' : st === 'ok' ? 'healthy' : 'unhealthy'}>
                      <span class="dot {st === 'off' ? '' : st}"></span>
                      {t.host}:{t.port}
                    </span>
                  {/each}
                {/if}
              </td>
              <td class="actions">
                <button class="btn btn-sm btn-ghost" onclick={() => openEdit(s)}>Edit</button>
                <button class="btn btn-sm btn-danger" onclick={() => del(s)}>Delete</button>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</div>

<Drawer bind:open title={editing ? `Edit ${editing.name}` : 'New service'}>
  <div class="field">
    <label for="svc-name">Name</label>
    <input id="svc-name" class="input" bind:value={form.name} placeholder="my-api" />
  </div>
  <div class="row">
    <div class="field">
      <label for="svc-proto">Protocol</label>
      <select id="svc-proto" class="select" bind:value={form.protocol}>
        <option value="http">http</option>
        <option value="https">https</option>
      </select>
    </div>
    <div class="field">
      <label for="svc-lb">Load balancing</label>
      <select id="svc-lb" class="select" bind:value={form.lb_algorithm}>
        <option value="round_robin">round_robin</option>
        <option value="weighted">weighted</option>
        <option value="random">random</option>
        <option value="consistent">consistent (by IP)</option>
      </select>
    </div>
  </div>
  <div class="row">
    <div class="field">
      <label for="svc-ct">Connect timeout (ms)</label>
      <input id="svc-ct" class="input" type="number" bind:value={form.connect_timeout_ms} />
    </div>
    <div class="field">
      <label for="svc-rt">Read timeout (ms)</label>
      <input id="svc-rt" class="input" type="number" bind:value={form.read_timeout_ms} />
    </div>
  </div>
  <div class="row">
    <div class="field">
      <label for="svc-retries">Retries</label>
      <input id="svc-retries" class="input" type="number" bind:value={form.retries} />
    </div>
    <div class="field">
      <label for="svc-sni">TLS SNI (https upstreams)</label>
      <input id="svc-sni" class="input" bind:value={form.tls_sni} placeholder="api.internal" />
    </div>
  </div>
  <div class="field">
    <label for="svc-health">Health check path <span class="faint">(optional)</span></label>
    <input id="svc-health" class="input mono" bind:value={form.health_path} placeholder="/healthz" />
    <span class="hint">
      HTTP GET every 5s per target; healthy = 2xx/3xx, ejected after 2 consecutive failures.
      Blank = TCP connect check. Plaintext HTTP — leave blank for TLS upstreams.
    </span>
  </div>

  {#if editing}
    <hr class="sep" />
    <h3 class="sub">Targets</h3>
    <div class="tgts">
      {#each editingTargets as t (t.id)}
        {@const st = tState(t)}
        <div class="tgt-row">
          <span class="dot {st === 'off' ? '' : st}" title={st === 'off' ? 'disabled' : st === 'ok' ? 'healthy' : 'unhealthy'}></span>
          <span class="mono">{t.host}:{t.port}</span>
          <span class="faint">weight {t.weight}</span>
          {#if st === 'err'}<span class="badge err">down</span>{/if}
          <div class="spacer"></div>
          <button class="toggle {t.enabled ? 'on' : ''}" aria-label="Enable" onclick={() => toggleTarget(t)}></button>
          <button class="btn btn-sm btn-ghost" onclick={() => delTarget(t)}>✕</button>
        </div>
      {:else}
        <div class="faint" style="padding:6px 0">No targets — add one below.</div>
      {/each}
    </div>
    <div class="add-tgt">
      <input class="input" placeholder="host" bind:value={tForm.host} />
      <input class="input" type="number" placeholder="port" bind:value={tForm.port} style="max-width:90px" />
      <input class="input" type="number" placeholder="weight" bind:value={tForm.weight} style="max-width:90px" />
      <button class="btn" onclick={addTarget}>Add</button>
    </div>
  {/if}

  {#snippet footer()}
    <button class="btn btn-ghost" onclick={() => (open = false)}>Close</button>
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
  .sep {
    border: none;
    border-top: 1px solid var(--border);
    margin: 18px 0;
  }
  .sub {
    font-size: 13px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--muted);
    margin-bottom: 10px;
  }
  .tgt-row {
    display: flex;
    align-items: center;
    gap: 9px;
    padding: 8px 0;
    border-bottom: 1px solid var(--border);
  }
  .spacer {
    flex: 1;
  }
  .add-tgt {
    display: flex;
    gap: 8px;
    margin-top: 12px;
  }
</style>
