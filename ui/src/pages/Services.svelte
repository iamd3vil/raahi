<script lang="ts">
  import { onMount } from 'svelte';
  import { api, ApiError } from '../lib/api';
  import type {
    DiscoveryConfig,
    DiscoveryProvider,
    DiscoveryProviderId,
    DiscoverySource,
    DiscoverySourceInput,
    DiscoverySourceStatus,
    LbAlgorithm,
    Protocol,
    Service,
    Target,
    TargetHealth,
  } from '../lib/types';
  import { DISCOVERY_PROVIDERS, DISCOVERY_PROVIDER_LABELS } from '../lib/types';
  import { toast } from '../lib/state.svelte';
  import Drawer from '../lib/components/Drawer.svelte';
  import EmptyState from '../lib/components/EmptyState.svelte';

  let services = $state<Service[]>([]);
  let targetsBy = $state<Record<number, Target[]>>({});
  let sourcesBy = $state<Record<number, DiscoverySource[]>>({});
  let statusBy = $state<Record<number, DiscoverySourceStatus>>({});
  let providers = $state<DiscoveryProvider[]>([]);
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
  // disabled targets are "off", discovery-degraded ones "warn", the rest reflect the live check
  const tState = (t: Target): 'ok' | 'err' | 'warn' | 'off' =>
    !t.enabled ? 'off' : !(health.get(t.id) ?? true) ? 'err' : t.state && t.state !== 'active' ? 'warn' : 'ok';
  const tTitle = (t: Target) =>
    !t.enabled ? 'disabled' : !(health.get(t.id) ?? true) ? 'unhealthy' : (t.state ?? 'active');

  let form = $state({
    name: '',
    protocol: 'http' as Protocol,
    lb_algorithm: 'round_robin' as LbAlgorithm,
    connect_timeout_ms: 5000,
    read_timeout_ms: 60000,
    write_timeout_ms: 60000,
    retries: 1,
    tls_sni: '',
    upstream_authority: '',
    health_path: '',
  });
  let tForm = $state({ host: '', port: 80, weight: 100, priority: 0 });

  async function load() {
    loading = true;
    try {
      services = await api.listServices();
      const [tLists, sLists] = await Promise.all([
        Promise.all(services.map((s) => api.listTargets(s.id))),
        // discovery is optional; a service with no sources (or an older server) must not break the page
        Promise.all(services.map((s) => api.listDiscoverySources(s.id).catch(() => [] as DiscoverySource[]))),
      ]);
      const tMap: Record<number, Target[]> = {};
      const sMap: Record<number, DiscoverySource[]> = {};
      services.forEach((s, i) => {
        tMap[s.id] = tLists[i];
        sMap[s.id] = sLists[i];
      });
      targetsBy = tMap;
      sourcesBy = sMap;
      if (editing) await loadStatuses(editing.id);
    } catch (e) {
      toast((e as ApiError).message, 'err');
    } finally {
      loading = false;
    }
  }

  /** Live state per discovery source, fetched only for the service being edited. */
  async function loadStatuses(serviceId: number) {
    const list = sourcesBy[serviceId] ?? [];
    if (list.length === 0) return;
    const got = await Promise.all(
      list.map(async (s) => [s.id, await api.discoverySourceStatus(s.id).catch(() => null)] as const),
    );
    const map = { ...statusBy };
    for (const [id, st] of got) if (st) map[id] = st;
    statusBy = map;
  }

  async function loadProviders() {
    try {
      // only the providers this UI knows how to configure
      providers = (await api.listDiscoveryProviders()).filter((p) => DISCOVERY_PROVIDERS.includes(p.id));
    } catch {
      providers = [];
    }
  }

  // fall back to the built-in list when the server can't be reached
  const providerOptions = $derived(
    providers.length
      ? providers.map((p) => ({ id: p.id, name: p.name, description: p.description }))
      : DISCOVERY_PROVIDERS.map((id) => ({ id, name: DISCOVERY_PROVIDER_LABELS[id], description: '' })),
  );
  const providerLabel = (id: DiscoveryProviderId) => DISCOVERY_PROVIDER_LABELS[id] ?? id;

  function openNew() {
    editing = null;
    srcOpen = false;
    form = {
      name: '',
      protocol: 'http',
      lb_algorithm: 'round_robin',
      connect_timeout_ms: 5000,
      read_timeout_ms: 60000,
      write_timeout_ms: 60000,
      retries: 1,
      tls_sni: '',
      upstream_authority: '',
      health_path: '',
    };
    open = true;
  }

  function openEdit(s: Service) {
    editing = s;
    srcOpen = false;
    form = {
      name: s.name,
      protocol: s.protocol,
      lb_algorithm: s.lb_algorithm,
      connect_timeout_ms: s.connect_timeout_ms,
      read_timeout_ms: s.read_timeout_ms,
      write_timeout_ms: s.write_timeout_ms,
      retries: s.retries,
      tls_sni: s.tls_sni ?? '',
      upstream_authority: s.upstream_authority ?? '',
      health_path: s.health_path ?? '',
    };
    open = true;
    loadStatuses(s.id);
  }

  async function save() {
    const payload = {
      ...form,
      tls_sni: form.tls_sni || null,
      upstream_authority: form.upstream_authority.trim() || null,
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
      tForm = { host: '', port: 80, weight: 100, priority: 0 };
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
      await api.updateTarget(t.id, {
        host: t.host,
        port: t.port,
        weight: t.weight,
        priority: t.priority,
        enabled: !t.enabled,
      });
      await load();
    } catch (e) {
      toast((e as ApiError).message, 'err');
    }
  }

  // ---- discovery sources ----
  let srcOpen = $state(false);
  let srcEditing = $state<DiscoverySource | null>(null);
  let srcBusy = $state<number | null>(null);
  let headerRows = $state<{ k: string; v: string }[]>([{ k: '', v: '' }]);

  // Flat mirror of DiscoverySourceInput + every provider's config; only the
  // fields for the selected provider are read back by srcConfig().
  const emptySrcForm = () => ({
    name: '',
    provider: 'dns' as DiscoveryProviderId,
    enabled: true,
    stale_after_ms: 300000,
    removal_grace_ms: 60000,
    // shared config
    interval_ms: 30000,
    timeout_ms: 5000,
    port: 80,
    weight: 100,
    priority: 0,
    // dns
    hostname: '',
    record_types: 'a, aaaa',
    resolver: '',
    // dns-srv
    service_name: '',
    use_record_weight: true,
    use_record_priority: true,
    // http
    url: '',
    endpoints_path: '',
    tls_verify: true,
  });
  let srcForm = $state(emptySrcForm());

  const csv = (s: string) =>
    s
      .split(',')
      .map((x) => x.trim())
      .filter(Boolean);

  function openNewSource() {
    srcEditing = null;
    srcForm = emptySrcForm();
    srcForm.provider = providerOptions[0]?.id ?? 'dns';
    headerRows = [{ k: '', v: '' }];
    srcOpen = true;
  }

  function openEditSource(s: DiscoverySource) {
    const c = s.config as Record<string, any>;
    const d = emptySrcForm();
    srcForm = {
      ...d,
      name: s.name,
      provider: s.provider,
      enabled: s.enabled,
      stale_after_ms: s.stale_after_ms,
      removal_grace_ms: s.removal_grace_ms,
      interval_ms: Number(c.interval_ms ?? d.interval_ms),
      timeout_ms: Number(c.timeout_ms ?? d.timeout_ms),
      port: Number(c.port ?? d.port),
      weight: Number(c.weight ?? d.weight),
      priority: Number(c.priority ?? d.priority),
      hostname: String(c.hostname ?? ''),
      record_types: Array.isArray(c.record_types) ? c.record_types.join(', ') : d.record_types,
      resolver: String(c.resolver ?? ''),
      service_name: String(c.service_name ?? ''),
      use_record_weight: c.use_record_weight !== false,
      use_record_priority: c.use_record_priority !== false,
      url: String(c.url ?? ''),
      endpoints_path: String(c.endpoints_path ?? ''),
      tls_verify: c.tls_verify !== false,
    };
    headerRows = Object.entries((c.headers as Record<string, string>) ?? {}).map(([k, v]) => ({ k, v: String(v) }));
    if (headerRows.length === 0) headerRows = [{ k: '', v: '' }];
    srcEditing = s;
    srcOpen = true;
  }

  function srcConfig(): DiscoveryConfig {
    const common = {
      interval_ms: Math.max(1000, Number(srcForm.interval_ms) || 30000),
      timeout_ms: Math.max(100, Number(srcForm.timeout_ms) || 5000),
      weight: Math.max(0, Number(srcForm.weight) || 0),
      priority: Math.max(0, Number(srcForm.priority) || 0),
    };
    const port = Number(srcForm.port) || 0;
    const resolver = srcForm.resolver.trim() || null;

    if (srcForm.provider === 'dns')
      return {
        ...common,
        port,
        hostname: srcForm.hostname.trim(),
        record_types: csv(srcForm.record_types).map((r) => r.toLowerCase()) as ('a' | 'aaaa')[],
        resolver,
      };
    if (srcForm.provider === 'dns-srv')
      return {
        ...common,
        port: port || null,
        service_name: srcForm.service_name.trim(),
        resolver,
        use_record_weight: srcForm.use_record_weight,
        use_record_priority: srcForm.use_record_priority,
      };
    const headers: Record<string, string> = {};
    for (const r of headerRows) if (r.k.trim()) headers[r.k.trim()] = r.v;
    return {
      ...common,
      port: port || null,
      url: srcForm.url.trim(),
      headers,
      endpoints_path: srcForm.endpoints_path.trim() || null,
      tls_verify: srcForm.tls_verify,
    };
  }

  async function saveSource() {
    if (!editing) return;
    const name = srcForm.name.trim();
    if (!name) return toast('Source name is required', 'err');
    const config = srcConfig();
    if ('hostname' in config) {
      if (!config.hostname) return toast('A hostname to resolve is required', 'err');
      if (!config.port) return toast('DNS A/AAAA records carry no port — set one', 'err');
      if (config.record_types.length === 0) return toast('Pick at least one record type', 'err');
    }
    if ('service_name' in config && !config.service_name) return toast('An SRV service name is required', 'err');
    if ('url' in config && !config.url) return toast('A registry URL is required', 'err');

    const input: DiscoverySourceInput = {
      name,
      provider: srcForm.provider,
      config,
      enabled: srcForm.enabled,
      stale_after_ms: Math.max(0, Number(srcForm.stale_after_ms) || 0),
      removal_grace_ms: Math.max(0, Number(srcForm.removal_grace_ms) || 0),
    };
    try {
      if (srcEditing) {
        await api.updateDiscoverySource(srcEditing.id, input);
        toast('Discovery source updated', 'ok');
      } else {
        await api.createDiscoverySource(editing.id, input);
        toast('Discovery source created', 'ok');
      }
      srcOpen = false;
      srcEditing = null;
      await load();
    } catch (e) {
      toast((e as ApiError).message, 'err');
    }
  }

  async function toggleSource(s: DiscoverySource) {
    try {
      // PUT is a full replacement, so echo the stored source back with one field flipped
      await api.updateDiscoverySource(s.id, {
        name: s.name,
        provider: s.provider,
        config: s.config,
        enabled: !s.enabled,
        stale_after_ms: s.stale_after_ms,
        removal_grace_ms: s.removal_grace_ms,
      });
      await load();
    } catch (e) {
      toast((e as ApiError).message, 'err');
    }
  }

  async function refreshSource(s: DiscoverySource) {
    srcBusy = s.id;
    try {
      await api.refreshDiscoverySource(s.id);
      toast(`Refresh requested for "${s.name}"`, 'ok');
      await new Promise((resolve) => setTimeout(resolve, 250));
      await load();
      await loadStatuses(s.service_id);
    } catch (e) {
      toast((e as ApiError).message, 'err');
    } finally {
      srcBusy = null;
    }
  }

  async function delSource(s: DiscoverySource) {
    if (!confirm(`Delete discovery source "${s.name}" and the targets it owns?`)) return;
    try {
      await api.deleteDiscoverySource(s.id);
      toast('Discovery source deleted', 'ok');
      if (srcEditing?.id === s.id) srcOpen = false;
      await load();
    } catch (e) {
      toast((e as ApiError).message, 'err');
    }
  }

  const srcDot = (st: DiscoverySourceStatus | undefined) =>
    st?.state === 'healthy' ? 'ok' : st?.state === 'failing' || st?.state === 'stale' ? 'warn' : '';
  const fmtWhen = (s: string | null | undefined) => (s ? new Date(s).toLocaleTimeString() : 'never');
  /** One-line identity of a source, for the collapsed row. */
  const summarize = (s: DiscoverySource) => {
    const c = s.config as Record<string, any>;
    if (s.provider === 'dns') return `${c.hostname ?? ''}:${c.port ?? ''}`;
    if (s.provider === 'dns-srv') return String(c.service_name ?? '');
    return String(c.url ?? '');
  };

  const editingTargets = $derived(editing ? (targetsBy[editing.id] ?? []) : []);
  const staticTargets = $derived(editingTargets.filter((t) => !t.source_id));
  const discoveredTargets = $derived(editingTargets.filter((t) => !!t.source_id));
  const editingSources = $derived(editing ? (sourcesBy[editing.id] ?? []) : []);
  const sourceName = (id: number | null | undefined) =>
    editingSources.find((s) => s.id === id)?.name ?? 'discovered';

  onMount(() => {
    load();
    loadHealth();
    loadProviders();
    const t = setInterval(() => {
      loadHealth();
      if (open && editing) loadStatuses(editing.id);
    }, 5000);
    return () => clearInterval(t);
  });
</script>

<div class="head-actions">
  <p class="muted">Upstreams: named groups of backend targets with load balancing.</p>
  <button onclick={openNew}>+ New service</button>
</div>

<div class="card">
  {#if loading}
    <div class="empty"><span aria-busy="true" data-spinner="small"></span></div>
  {:else if services.length === 0}
    <EmptyState
      icon="M5 4h14a1 1 0 0 1 1 1v4a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1zm0 10h14a1 1 0 0 1 1 1v4a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1v-4a1 1 0 0 1 1-1z"
      title="No services yet"
      description="A service groups one or more upstream targets behind a load-balancing policy. Routes send traffic to services."
    >
      {#snippet action()}
        <button onclick={openNew}>+ Create your first service</button>
      {/snippet}
    </EmptyState>
  {:else}
    <div class="table">
      <table>
        <thead>
          <tr><th>Name</th><th>Protocol</th><th>Balancing</th><th>Targets</th><th></th></tr>
        </thead>
        <tbody>
          {#each services as s (s.id)}
            {@const ts = targetsBy[s.id] ?? []}
            {@const srcs = sourcesBy[s.id] ?? []}
            <tr>
              <td><strong>{s.name}</strong></td>
              <td><span class="badge {s.protocol === 'https' ? '' : 'outline'}">{s.protocol}</span></td>
              <td class="mono">{s.lb_algorithm}</td>
              <td>
                {#if ts.length === 0 && srcs.length === 0}
                  <span class="badge" data-variant="warning">no targets</span>
                {:else}
                  {#each ts as t}
                    {@const st = tState(t)}
                    <span
                      class="chip"
                      class:auto={!!t.source_id}
                      title="{tTitle(t)}{t.source_id ? ' · discovered' : ''}"
                    >
                      <span class="dot {st === 'off' ? '' : st}"></span>
                      {t.host}:{t.port}
                    </span>
                  {/each}
                  {#if srcs.length}
                    <span class="badge outline" title="Targets are kept in sync by discovery">
                      ⟳ {srcs.length} source{srcs.length === 1 ? '' : 's'}
                    </span>
                  {/if}
                {/if}
              </td>
              <td class="actions">
                <button class="ghost small" onclick={() => openEdit(s)}>Edit</button>
                <button class="ghost small" data-variant="danger" onclick={() => del(s)}>Delete</button>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</div>

<Drawer bind:open title={editing ? `Edit ${editing.name}` : 'New service'}>
  <label data-field>
    Name
    <input bind:value={form.name} placeholder="my-api" />
  </label>
  <div class="row">
    <label data-field>
      Protocol
      <select bind:value={form.protocol}>
        <option value="http">http</option>
        <option value="https">https</option>
      </select>
    </label>
    <label data-field>
      Load balancing
      <select bind:value={form.lb_algorithm}>
        <option value="round_robin">round_robin</option>
        <option value="weighted">weighted</option>
        <option value="random">random</option>
        <option value="consistent">consistent (by IP)</option>
      </select>
    </label>
  </div>
  <div class="row">
    <label data-field>
      Connect timeout (ms)
      <input type="number" bind:value={form.connect_timeout_ms} />
    </label>
    <label data-field>
      Read timeout (ms)
      <input type="number" bind:value={form.read_timeout_ms} />
    </label>
  </div>
  <div class="row">
    <label data-field>
      Retries
      <input type="number" bind:value={form.retries} />
    </label>
    <label data-field>
      TLS SNI (https upstreams)
      <input bind:value={form.tls_sni} placeholder="api.internal" />
    </label>
  </div>
  <label data-field>
    Upstream authority <span class="faint">(optional)</span>
    <input class="mono" bind:value={form.upstream_authority} placeholder="api.internal:8443" />
    <span data-hint>
      Host / :authority header sent upstream, overriding the target's host:port. Useful when discovery yields raw IPs
      but the backend routes by name. Blank = use the target address.
    </span>
  </label>
  <label data-field>
    Health check path <span class="faint">(optional)</span>
    <input class="mono" bind:value={form.health_path} placeholder="/healthz" />
    <span data-hint>
      HTTP GET every 5s per target; healthy = 2xx/3xx, ejected after 2 consecutive failures.
      Blank = TCP connect check. HTTPS services use HTTPS with certificate verification and the configured TLS SNI.
    </span>
  </label>

  {#if editing}
    <hr />
    <h3 class="sub">Targets</h3>
    <div class="tgts">
      {#each staticTargets as t (t.id)}
        {@const st = tState(t)}
        <div class="tgt-row">
          <span class="dot {st === 'off' ? '' : st}" title={tTitle(t)}></span>
          <span class="mono">{t.host}:{t.port}</span>
          <span class="faint">weight {t.weight}{t.priority ? ` · priority ${t.priority}` : ''}</span>
          {#if st === 'err'}<span class="badge" data-variant="danger">down</span>{/if}
          <div class="spacer"></div>
          <input type="checkbox" role="switch" checked={t.enabled} aria-label="Enable" onchange={() => toggleTarget(t)} />
          <button class="ghost small icon" aria-label="Remove target" onclick={() => delTarget(t)}>✕</button>
        </div>
      {:else}
        <div class="faint" style="padding:6px 0">No static targets — add one below.</div>
      {/each}
    </div>
    <div class="add-tgt">
      <input placeholder="host" bind:value={tForm.host} />
      <input type="number" placeholder="port" bind:value={tForm.port} style="max-width:80px" />
      <input type="number" placeholder="weight" bind:value={tForm.weight} style="max-width:86px" />
      <input type="number" placeholder="prio" bind:value={tForm.priority} style="max-width:72px" />
      <button class="outline" onclick={addTarget}>Add</button>
    </div>

    {#if discoveredTargets.length}
      <h3 class="sub" style="margin-top:20px">Discovered targets</h3>
      <div class="tgts">
        {#each discoveredTargets as t (t.id)}
          {@const st = tState(t)}
          {@const meta = Object.entries(t.metadata ?? {})}
          <div class="tgt-row">
            <span class="dot {st === 'off' ? '' : st}" title={tTitle(t)}></span>
            <span class="mono">{t.host}:{t.port}</span>
            <span class="faint">weight {t.weight}{t.priority ? ` · priority ${t.priority}` : ''}</span>
            {#if t.state && t.state !== 'active'}
              <span class="badge" data-variant="warning">{t.state}</span>
            {:else if st === 'err'}
              <span class="badge" data-variant="danger">down</span>
            {/if}
            <div class="spacer"></div>
            <span class="badge outline" title="Managed by this discovery source">{sourceName(t.source_id)}</span>
          </div>
          {#if t.provider_key || meta.length}
            <div class="tgt-meta">
              {#if t.provider_key}<span class="chip" title="Provider key">{t.provider_key}</span>{/if}
              {#each meta as [k, v]}<span class="chip">{k}={v}</span>{/each}
            </div>
          {/if}
        {/each}
      </div>
      <span data-hint>Read-only: these targets are created and removed by their discovery source.</span>
    {/if}

    <hr />
    <div class="sec-head">
      <h3 class="sub" style="margin:0">Discovery sources</h3>
      {#if !srcOpen}
        <button class="ghost small" onclick={openNewSource}>+ Add source</button>
      {/if}
    </div>
    <div class="tgts">
      {#each editingSources as s (s.id)}
        {@const st = statusBy[s.id]}
        <div class="src-row">
          <div class="src-line">
            <span class="dot {srcDot(st)}" title={st?.state ?? 'pending'}></span>
            <strong>{s.name}</strong>
            <span class="badge outline">{providerLabel(s.provider)}</span>
            <div class="spacer"></div>
            <input
              type="checkbox"
              role="switch"
              checked={s.enabled}
              aria-label="Enable source"
              onchange={() => toggleSource(s)}
            />
          </div>
          <div class="src-line faint mono small-txt">{summarize(s)}</div>
          <div class="src-line faint small-txt">
            {#if st}
              <span>
                {st.state} · {st.active_count} active{st.draining_count ? ` · ${st.draining_count} draining` : ''}{st.stale_count
                  ? ` · ${st.stale_count} stale`
                  : ''} · last success {fmtWhen(st.last_success_at)}
              </span>
            {:else}
              <span>status unknown</span>
            {/if}
            <div class="spacer"></div>
            <button class="ghost small" disabled={srcBusy === s.id} onclick={() => refreshSource(s)}>
              {srcBusy === s.id ? 'Refreshing…' : 'Refresh'}
            </button>
            <button class="ghost small" onclick={() => openEditSource(s)}>Edit</button>
            <button class="ghost small" data-variant="danger" onclick={() => delSource(s)}>Delete</button>
          </div>
          {#if st?.last_error}
            <div class="src-err">{st.last_error}</div>
          {/if}
        </div>
      {:else}
        {#if !srcOpen}
          <div class="faint" style="padding:6px 0">
            No discovery sources — targets are managed by hand. Add one to resolve them from DNS or an HTTP endpoint.
          </div>
        {/if}
      {/each}
    </div>

    {#if srcOpen}
      <div class="src-form">
        <h3 class="sub">{srcEditing ? `Edit source “${srcEditing.name}”` : 'New discovery source'}</h3>
        <div class="row">
          <label data-field>
            Source name
            <input bind:value={srcForm.name} placeholder="prod-pool" />
          </label>
          <label data-field>
            Provider
            <select bind:value={srcForm.provider}>
              {#each providerOptions as p (p.id)}
                <option value={p.id} title={p.description}>{p.name}</option>
              {/each}
            </select>
          </label>
        </div>

        {#if srcForm.provider === 'dns'}
          <div class="row">
            <label data-field>
              Hostname
              <input class="mono" bind:value={srcForm.hostname} placeholder="orders.service.internal" />
            </label>
            <label data-field style="flex:0 0 96px">
              Port
              <input type="number" bind:value={srcForm.port} />
            </label>
          </div>
          <label data-field>
            Record types
            <input class="mono" bind:value={srcForm.record_types} placeholder="a, aaaa" />
            <span data-hint>Comma-separated (a, aaaa). Every resolved address becomes a target on the port above.</span>
          </label>
        {:else if srcForm.provider === 'dns-srv'}
          <label data-field>
            SRV service name
            <input class="mono" bind:value={srcForm.service_name} placeholder="_http._tcp.orders.internal" />
            <span data-hint>Each SRV record supplies a host, port, weight and priority.</span>
          </label>
          <div class="row">
            <label data-field class="inline-check">
              <input type="checkbox" role="switch" bind:checked={srcForm.use_record_weight} />
              Use record weight
            </label>
            <label data-field class="inline-check">
              <input type="checkbox" role="switch" bind:checked={srcForm.use_record_priority} />
              Use record priority
            </label>
          </div>
        {:else}
          <label data-field>
            Registry URL
            <input class="mono" bind:value={srcForm.url} placeholder="https://registry.internal/v1/endpoints" />
            <span data-hint>Polled with GET; must answer with JSON.</span>
          </label>
          <div class="row">
            <label data-field>
              Endpoints path <span class="faint">(optional)</span>
              <input class="mono" bind:value={srcForm.endpoints_path} placeholder="data.endpoints" />
            </label>
            <label data-field class="inline-check" style="flex:0 0 130px">
              <input type="checkbox" role="switch" bind:checked={srcForm.tls_verify} />
              Verify TLS
            </label>
          </div>
          <span data-hint>
            Dot path to the endpoint array when it isn't at the document root. Each item needs a host, plus optional
            port, weight, priority, enabled, key and metadata.
          </span>
          <div data-field>
            Headers <span class="faint">(optional)</span>
            {#each headerRows as row, i (i)}
              <div class="hdr-row">
                <input placeholder="Header" bind:value={row.k} />
                <input placeholder="value" bind:value={row.v} />
                <button
                  class="ghost small icon"
                  aria-label="Remove header"
                  onclick={() => (headerRows = headerRows.filter((_, j) => j !== i))}>✕</button
                >
              </div>
            {/each}
            <button class="ghost small" onclick={() => (headerRows = [...headerRows, { k: '', v: '' }])}>
              + Header
            </button>
          </div>
        {/if}

        {#if srcForm.provider !== 'dns'}
          <label data-field>
            Default port <span class="faint">(optional)</span>
            <input type="number" bind:value={srcForm.port} />
            <span data-hint>Applied to endpoints the provider returns without a port. 0 = none.</span>
          </label>
        {/if}
        {#if srcForm.provider !== 'http'}
          <label data-field>
            Resolver <span class="faint">(optional)</span>
            <input class="mono" bind:value={srcForm.resolver} placeholder="10.0.0.53:53" />
            <span data-hint>Blank uses the system resolver.</span>
          </label>
        {/if}

        <div class="row">
          <label data-field>
            Refresh interval (ms)
            <input type="number" bind:value={srcForm.interval_ms} />
          </label>
          <label data-field>
            Timeout (ms)
            <input type="number" bind:value={srcForm.timeout_ms} />
          </label>
        </div>
        <div class="row">
          <label data-field>
            Default weight
            <input type="number" bind:value={srcForm.weight} />
          </label>
          <label data-field>
            Default priority
            <input type="number" bind:value={srcForm.priority} />
          </label>
        </div>
        <span data-hint>Weight and priority apply to endpoints the provider returns without their own.</span>

        <div class="row">
          <label data-field>
            Stale after (ms)
            <input type="number" bind:value={srcForm.stale_after_ms} />
          </label>
          <label data-field>
            Removal grace (ms)
            <input type="number" bind:value={srcForm.removal_grace_ms} />
          </label>
        </div>
        <span data-hint>
          Targets turn stale this long after the last successful refresh but keep taking traffic while healthy; ones
          that vanish from a successful refresh drain for the grace period before deletion.
        </span>

        <label data-field class="inline-check">
          <input type="checkbox" role="switch" bind:checked={srcForm.enabled} />
          Enabled
        </label>

        <div class="src-actions">
          <button class="ghost" onclick={() => ((srcOpen = false), (srcEditing = null))}>Cancel</button>
          <button class="outline" onclick={saveSource}>{srcEditing ? 'Save source' : 'Add source'}</button>
        </div>
      </div>
    {/if}
  {/if}

  {#snippet footer()}
    <button class="ghost" onclick={() => (open = false)}>Close</button>
    <button onclick={save}>{editing ? 'Save' : 'Create'}</button>
  {/snippet}
</Drawer>

<style>
  .tgt-row {
    display: flex;
    align-items: center;
    gap: 9px;
    padding: 8px 0;
    border-bottom: 1px solid var(--border);
  }
  .tgt-row input[role='switch'] {
    margin-block-start: 0;
  }
  .spacer {
    flex: 1;
  }
  .add-tgt {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 12px;
    flex-wrap: wrap;
  }
  .add-tgt input {
    margin-block-start: 0;
  }
  .tgt-meta {
    padding: 0 0 8px 16px;
    border-bottom: 1px solid var(--border);
  }
  .chip.auto {
    border-style: dashed;
  }

  /* discovery sources */
  .sec-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 10px;
    gap: 10px;
  }
  .src-row {
    padding: 10px 0;
    border-bottom: 1px solid var(--border);
  }
  .src-line {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .src-line + .src-line {
    margin-top: 4px;
  }
  .src-line input[role='switch'] {
    margin-block-start: 0;
  }
  .small-txt {
    font-size: 12px;
  }
  .src-err {
    margin-top: 6px;
    font-size: 12px;
    color: var(--danger);
    word-break: break-word;
  }
  .src-form {
    margin-top: 14px;
    padding: 14px;
    border: 1px solid var(--input);
    border-radius: var(--radius, 8px);
    background: var(--muted);
  }
  .hdr-row {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 6px;
  }
  .hdr-row input {
    margin-block-start: 0;
  }
  .inline-check {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .inline-check input {
    margin-block-start: 0;
  }
  .src-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 12px;
  }
</style>
