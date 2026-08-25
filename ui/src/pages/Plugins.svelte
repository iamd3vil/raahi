<script lang="ts">
  import { onMount } from 'svelte';
  import { api, ApiError } from '../lib/api';
  import type { Plugin, PluginScope, PluginType, Route, Service, WasmModule } from '../lib/types';
  import { PLUGIN_TYPES, PLUGIN_LABELS } from '../lib/types';
  import { toast } from '../lib/state.svelte';
  import Drawer from '../lib/components/Drawer.svelte';
  import EmptyState from '../lib/components/EmptyState.svelte';

  let plugins = $state<Plugin[]>([]);
  let services = $state<Service[]>([]);
  let routes = $state<Route[]>([]);
  let modules = $state<WasmModule[]>([]);
  let loading = $state(true);
  let open = $state(false);
  let editing = $state<Plugin | null>(null);
  let rawMode = $state(false);

  // wasm module uploader
  let modOpen = $state(false);
  let modForm = $state({ name: '', description: '', wat: '', wasm_base64: '', fileName: '' });

  function onModuleFile(e: Event) {
    const file = (e.currentTarget as HTMLInputElement).files?.[0];
    if (!file) return;
    const reader = new FileReader();
    reader.onload = () => {
      const b64 = (reader.result as string).split(',')[1] ?? '';
      modForm.wasm_base64 = b64;
      modForm.fileName = file.name;
      if (!modForm.name) modForm.name = file.name.replace(/\.wasm$/, '');
    };
    reader.readAsDataURL(file);
  }

  async function uploadModule() {
    try {
      await api.createWasmModule({
        name: modForm.name,
        description: modForm.description,
        wasm_base64: modForm.wasm_base64 || undefined,
        wat: modForm.wat || undefined,
      });
      toast('Module uploaded', 'ok');
      modOpen = false;
      await load();
    } catch (e) {
      toast((e as ApiError).message, 'err');
    }
  }

  async function delModule(m: WasmModule) {
    if (!confirm(`Delete module "${m.name}"?`)) return;
    try {
      await api.deleteWasmModule(m.id);
      toast('Module deleted', 'ok');
      await load();
    } catch (e) {
      toast((e as ApiError).message, 'err');
    }
  }

  function fmtSize(b: number) {
    return b < 1024 ? `${b} B` : b < 1048576 ? `${(b / 1024).toFixed(1)} KB` : `${(b / 1048576).toFixed(1)} MB`;
  }

  const DEFAULTS: Record<PluginType, Record<string, unknown>> = {
    'key-auth': { key_names: ['apikey', 'x-api-key'], hide_credentials: false },
    'basic-auth': { realm: 'Raahi' },
    jwt: { key_claim_name: 'iss', uri_param_names: ['jwt'], require_exp: false, jwks_url: '', consumer_claim: 'sub' },
    acl: { allow: [], deny: [] },
    'ip-restriction': { allow: [], deny: [], status: 403, message: 'Your IP address is not allowed' },
    'rate-limit': { limit: 60, window_secs: 60, key: 'ip', headers: true },
    'proxy-cache': { ttl_secs: 60, max_body_bytes: 1048576, methods: ['GET'], cache_key_query: true },
    'request-size-limit': { max_bytes: 10485760, require_content_length: false },
    'request-termination': { status: 503, message: 'Service temporarily unavailable', content_type: 'text/plain; charset=utf-8' },
    redirect: { status: 302, location: '', preserve_path: true, http_only: false },
    cors: { allow_origins: ['*'], allow_methods: ['GET', 'POST', 'PUT', 'DELETE', 'OPTIONS'], allow_headers: ['*'], allow_credentials: false, max_age: 3600 },
    'request-transform': { add: {}, remove: [] },
    'response-transform': { add: {}, remove: [] },
    hsts: { max_age_secs: 63072000, include_subdomains: false, preload: false },
    'response-body-transform': { replace: [], max_body_bytes: 1048576, content_types: ['text/', 'application/json'] },
    'http-log': { endpoint: '', headers: {}, batch_max: 50, flush_interval_ms: 2000 },
    'request-id': { header_name: 'X-Request-Id', preserve: true, echo_downstream: true },
    'response-compression': { level: 5 },
    wasm: { module: '', config: {}, fuel: 100000000 },
  };

  const HELP: Record<PluginType, string> = {
    'key-auth': 'Requires a valid API key (header or ?query param) issued to a consumer.',
    'basic-auth': 'Requires HTTP Basic credentials matching a consumer (bcrypt-verified).',
    jwt: 'Verifies a Bearer JWT against a consumer’s jwt credential (looked up by the key claim, e.g. iss).',
    acl: 'Allows or denies authenticated consumers by group. Requires an auth plugin before it.',
    'ip-restriction': 'Allows or denies clients by IP or CIDR (checked before auth).',
    'rate-limit': 'Sliding-window request limiting keyed by client IP, consumer, or route. Sends RateLimit-* headers.',
    'proxy-cache': 'Caches upstream 200 responses in memory for a TTL and answers repeats directly (x-cache: HIT/MISS).',
    'request-size-limit': 'Rejects requests whose Content-Length exceeds the limit (413).',
    'request-termination': 'Short-circuits every request with a fixed status and message (maintenance mode).',
    redirect: 'Responds with a Location redirect instead of proxying.',
    cors: 'Answers preflight requests and adds CORS headers to responses.',
    'request-transform': 'Adds or removes request headers before proxying upstream.',
    'response-transform': 'Adds or removes response headers before returning downstream.',
    hsts: 'Adds Strict-Transport-Security to HTTPS responses only.',
    'response-body-transform': 'Find/replace on text response bodies (buffered up to a size cap; binary passes through).',
    'http-log': 'POSTs request records (JSON batches) to an external collector, off the hot path.',
    'request-id': 'Tags each request with a correlation id (UUID) forwarded upstream and echoed on the response.',
    'response-compression': 'Compresses responses (gzip / brotli / zstd) for clients that advertise Accept-Encoding.',
    wasm: 'Runs an uploaded WASM module on requests and responses (sandboxed, fuel-metered). Multiple wasm plugins stack.',
  };

  let form = $state({
    type: 'key-auth' as PluginType,
    scope: 'global' as PluginScope,
    service_id: null as number | null,
    route_id: null as number | null,
    config: '',
    ordering: 0,
    enabled: true,
  });

  // Typed config state, synced to/from JSON on open/save/toggle.
  let cfg = $state<Record<string, any>>({});
  // add-headers editor rows for transforms
  let addRows = $state<{ k: string; v: string }[]>([]);
  // find/replace editor rows for response-body-transform
  let replaceRows = $state<{ from: string; to: string }[]>([]);

  const csv = (s: string) => s.split(',').map((x) => x.trim()).filter(Boolean);
  const list = (v: unknown): string => (Array.isArray(v) ? v.join(', ') : '');

  function cfgFrom(config: Record<string, unknown>, type: PluginType) {
    const d = { ...DEFAULTS[type], ...config } as Record<string, any>;
    cfg = d;
    if (type === 'request-transform' || type === 'response-transform') {
      addRows = Object.entries((d.add as Record<string, string>) ?? {}).map(([k, v]) => ({ k, v: String(v) }));
      if (addRows.length === 0) addRows = [{ k: '', v: '' }];
    }
    if (type === 'response-body-transform') {
      replaceRows = ((d.replace as { from: string; to: string }[]) ?? []).map((r) => ({
        from: String(r.from ?? ''),
        to: String(r.to ?? ''),
      }));
      if (replaceRows.length === 0) replaceRows = [{ from: '', to: '' }];
    }
    if (type === 'wasm') {
      cfg.config_json = JSON.stringify(d.config ?? {}, null, 2);
      if (!cfg.module && modules.length) cfg.module = modules[0].name;
    }
  }

  function cfgToJson(): Record<string, unknown> {
    const t = form.type;
    if (t === 'key-auth')
      return { key_names: csv(String(cfg.key_names_csv ?? list(cfg.key_names))), hide_credentials: !!cfg.hide_credentials };
    if (t === 'basic-auth') return { realm: String(cfg.realm ?? 'Raahi') };
    if (t === 'jwt')
      return {
        key_claim_name: String(cfg.key_claim_name || 'iss'),
        uri_param_names: csv(String(cfg.uri_param_names_csv ?? list(cfg.uri_param_names))),
        require_exp: !!cfg.require_exp,
        jwks_url: String(cfg.jwks_url ?? '').trim() || null,
        consumer_claim: String(cfg.consumer_claim || 'sub'),
      };
    if (t === 'acl')
      return {
        allow: csv(String(cfg.allow_csv ?? list(cfg.allow))),
        deny: csv(String(cfg.deny_csv ?? list(cfg.deny))),
      };
    if (t === 'ip-restriction')
      return {
        allow: csv(String(cfg.allow_csv ?? list(cfg.allow))),
        deny: csv(String(cfg.deny_csv ?? list(cfg.deny))),
        status: Number(cfg.status) || 403,
        message: String(cfg.message ?? 'Your IP address is not allowed'),
      };
    if (t === 'rate-limit')
      return {
        limit: Number(cfg.limit) || 1,
        window_secs: Number(cfg.window_secs) || 60,
        key: cfg.key ?? 'ip',
        headers: cfg.headers !== false,
      };
    if (t === 'proxy-cache')
      return {
        ttl_secs: Number(cfg.ttl_secs) || 60,
        max_body_bytes: Number(cfg.max_body_bytes) || 1048576,
        methods: csv(String(cfg.methods_csv ?? list(cfg.methods))).map((m) => m.toUpperCase()),
        cache_key_query: cfg.cache_key_query !== false,
      };
    if (t === 'response-body-transform')
      return {
        replace: replaceRows.filter((r) => r.from).map((r) => ({ from: r.from, to: r.to })),
        max_body_bytes: Number(cfg.max_body_bytes) || 1048576,
        content_types: csv(String(cfg.content_types_csv ?? list(cfg.content_types))),
      };
    if (t === 'request-size-limit')
      return { max_bytes: Number(cfg.max_bytes) || 1, require_content_length: !!cfg.require_content_length };
    if (t === 'request-termination')
      return {
        status: Number(cfg.status) || 503,
        message: String(cfg.message ?? ''),
        content_type: String(cfg.content_type || 'text/plain; charset=utf-8'),
      };
    if (t === 'redirect')
      return {
        status: Number(cfg.status) || 302,
        location: String(cfg.location ?? ''),
        preserve_path: cfg.preserve_path !== false,
        http_only: !!cfg.http_only,
      };
    if (t === 'http-log')
      return {
        endpoint: String(cfg.endpoint ?? ''),
        headers: cfg.headers ?? {},
        batch_max: Number(cfg.batch_max) || 50,
        flush_interval_ms: Number(cfg.flush_interval_ms) || 2000,
      };
    if (t === 'request-id')
      return {
        header_name: String(cfg.header_name || 'X-Request-Id'),
        preserve: cfg.preserve !== false,
        echo_downstream: cfg.echo_downstream !== false,
      };
    if (t === 'response-compression') return { level: Math.min(9, Math.max(1, Number(cfg.level) || 5)) };
    if (t === 'hsts')
      return {
        max_age_secs: Math.max(1, Number(cfg.max_age_secs) || 63072000),
        include_subdomains: !!cfg.include_subdomains,
        preload: !!cfg.preload,
      };
    if (t === 'wasm') {
      let moduleConfig: unknown = cfg.config ?? {};
      if (typeof cfg.config_json === 'string') {
        try {
          moduleConfig = cfg.config_json.trim() ? JSON.parse(cfg.config_json) : {};
        } catch {
          /* keep prior config on parse failure; save() surfaces JSON errors in raw mode */
        }
      }
      return { module: String(cfg.module ?? ''), config: moduleConfig, fuel: Number(cfg.fuel) || 100000000 };
    }
    if (t === 'cors')
      return {
        allow_origins: csv(String(cfg.allow_origins_csv ?? list(cfg.allow_origins))),
        allow_methods: csv(String(cfg.allow_methods_csv ?? list(cfg.allow_methods))).map((m) => m.toUpperCase()),
        allow_headers: csv(String(cfg.allow_headers_csv ?? list(cfg.allow_headers))),
        allow_credentials: !!cfg.allow_credentials,
        max_age: Number(cfg.max_age) || 0,
      };
    // transforms
    const add: Record<string, string> = {};
    for (const r of addRows) if (r.k.trim()) add[r.k.trim()] = r.v;
    return { add, remove: csv(String(cfg.remove_csv ?? list(cfg.remove))) };
  }

  function setType(t: PluginType) {
    form.type = t;
    cfgFrom(DEFAULTS[t], t);
    form.config = JSON.stringify(DEFAULTS[t], null, 2);
  }

  function toggleRaw() {
    if (!rawMode) {
      form.config = JSON.stringify(cfgToJson(), null, 2);
    } else {
      try {
        cfgFrom(JSON.parse(form.config || '{}'), form.type);
      } catch {
        toast('Config is not valid JSON', 'err');
        return;
      }
    }
    rawMode = !rawMode;
  }

  async function load() {
    loading = true;
    try {
      [plugins, services, routes, modules] = await Promise.all([
        api.listPlugins(),
        api.listServices(),
        api.listRoutes(),
        api.listWasmModules(),
      ]);
    } catch (e) {
      toast((e as ApiError).message, 'err');
    } finally {
      loading = false;
    }
  }

  function openNew() {
    editing = null;
    rawMode = false;
    form = {
      type: 'key-auth',
      scope: 'global',
      service_id: services[0]?.id ?? null,
      route_id: routes[0]?.id ?? null,
      config: JSON.stringify(DEFAULTS['key-auth'], null, 2),
      ordering: 0,
      enabled: true,
    };
    cfgFrom(DEFAULTS['key-auth'], 'key-auth');
    open = true;
  }

  function openEdit(p: Plugin) {
    editing = p;
    rawMode = false;
    form = {
      type: p.type,
      scope: p.scope,
      service_id: p.service_id ?? services[0]?.id ?? null,
      route_id: p.route_id ?? routes[0]?.id ?? null,
      config: JSON.stringify(p.config ?? {}, null, 2),
      ordering: p.ordering,
      enabled: p.enabled,
    };
    cfgFrom(p.config ?? {}, p.type);
    open = true;
  }

  async function save() {
    let config: Record<string, unknown>;
    if (rawMode) {
      try {
        config = form.config.trim() ? JSON.parse(form.config) : {};
      } catch {
        toast('Config is not valid JSON', 'err');
        return;
      }
    } else {
      config = cfgToJson();
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
    if (!confirm(`Delete this ${PLUGIN_LABELS[p.type]} plugin?`)) return;
    try {
      await api.deletePlugin(p.id);
      toast('Plugin deleted', 'ok');
      await load();
    } catch (e) {
      toast((e as ApiError).message, 'err');
    }
  }

  async function toggleEnabled(p: Plugin) {
    try {
      await api.updatePlugin(p.id, {
        type: p.type,
        scope: p.scope,
        service_id: p.service_id,
        route_id: p.route_id,
        config: p.config,
        ordering: p.ordering,
        enabled: !p.enabled,
      });
      await load();
    } catch (e) {
      toast((e as ApiError).message, 'err');
    }
  }

  function scopeLabel(p: Plugin): { kind: string; name: string } {
    if (p.scope === 'global') return { kind: 'global', name: 'all routes' };
    if (p.scope === 'service')
      return { kind: 'service', name: String(services.find((s) => s.id === p.service_id)?.name ?? p.service_id) };
    return { kind: 'route', name: String(routes.find((r) => r.id === p.route_id)?.name ?? p.route_id) };
  }

  /** One-line human summary of a plugin's config for the table. */
  function summary(p: Plugin): string {
    const c = p.config as Record<string, any>;
    switch (p.type) {
      case 'key-auth':
        return `keys: ${(c.key_names ?? []).join(', ') || '—'}`;
      case 'basic-auth':
        return `realm: ${c.realm ?? 'Raahi'}`;
      case 'jwt':
        return `key claim: ${c.key_claim_name ?? 'iss'}`;
      case 'acl':
        return [
          (c.allow ?? []).length ? `allow: ${c.allow.join(', ')}` : '',
          (c.deny ?? []).length ? `deny: ${c.deny.join(', ')}` : '',
        ].filter(Boolean).join(' · ') || '—';
      case 'ip-restriction':
        return [
          (c.allow ?? []).length ? `allow: ${c.allow.join(', ')}` : '',
          (c.deny ?? []).length ? `deny: ${c.deny.join(', ')}` : '',
        ].filter(Boolean).join(' · ') || '—';
      case 'rate-limit':
        return `${c.limit ?? '?'} req / ${c.window_secs ?? '?'}s by ${c.key ?? 'ip'}`;
      case 'proxy-cache':
        return `ttl ${c.ttl_secs ?? 60}s`;
      case 'request-size-limit':
        return `max ${((c.max_bytes ?? 0) / 1048576).toFixed(1)} MB`;
      case 'request-termination':
        return `${c.status ?? 503}: ${c.message ?? ''}`;
      case 'redirect':
        return `${c.status ?? 302} → ${c.location ?? ''}`;
      case 'cors':
        return `origins: ${(c.allow_origins ?? []).join(', ') || '—'}`;
      case 'request-transform':
      case 'response-transform': {
        const adds = Object.keys(c.add ?? {}).length;
        const removes = (c.remove ?? []).length;
        return `+${adds} header${adds === 1 ? '' : 's'}, −${removes}`;
      }
      case 'hsts':
        return `max-age=${c.max_age_secs ?? 63072000}${c.include_subdomains ? ' · subdomains' : ''}${c.preload ? ' · preload' : ''}`;
      case 'response-body-transform': {
        const n = (c.replace ?? []).length;
        return `${n} replacement${n === 1 ? '' : 's'}`;
      }
      case 'http-log':
        return `→ ${c.endpoint || '—'}`;
      case 'request-id':
        return `header: ${c.header_name || 'X-Request-Id'}`;
      case 'response-compression':
        return `level: ${c.level ?? 5}`;
      case 'wasm':
        return `module: ${c.module || '—'}`;
    }
  }

  onMount(load);
</script>

<div class="head-actions">
  <p class="muted">Middleware applied per route, per service, or globally — ordered, hot-reloaded.</p>
  <button onclick={openNew}>+ New plugin</button>
</div>

<div class="card">
  {#if loading}
    <div class="empty"><span aria-busy="true" data-spinner="small"></span></div>
  {:else if plugins.length === 0}
    <EmptyState
      icon="M10 3v4m4-4v4M5 7h14l-1 12a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 7z"
      title="No plugins configured"
      description="Add auth, rate limiting, CORS, or header transforms. Plugins apply globally, to one service, or to one route."
    >
      {#snippet action()}
        <button onclick={openNew}>+ Add your first plugin</button>
      {/snippet}
    </EmptyState>
  {:else}
    <div class="table">
      <table>
        <thead><tr><th>Type</th><th>Config</th><th>Scope</th><th>Order</th><th>Enabled</th><th></th></tr></thead>
        <tbody>
          {#each plugins as p (p.id)}
            {@const sc = scopeLabel(p)}
            <tr style:opacity={p.enabled ? 1 : 0.55}>
              <td><span class="badge">{PLUGIN_LABELS[p.type]}</span></td>
              <td class="mono cfg-sum">{summary(p)}</td>
              <td>
                <span class="badge outline">{sc.kind}</span>
                {#if p.scope !== 'global'}<span class="scope-name">{sc.name}</span>{/if}
              </td>
              <td class="mono">{p.ordering}</td>
              <td><input type="checkbox" role="switch" aria-label="Toggle enabled" checked={p.enabled} onchange={() => toggleEnabled(p)} /></td>
              <td class="actions">
                <button class="ghost small" onclick={() => openEdit(p)}>Edit</button>
                <button class="ghost small" data-variant="danger" onclick={() => del(p)}>Delete</button>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</div>

<Drawer bind:open title={editing ? `Edit ${PLUGIN_LABELS[form.type]}` : 'New plugin'}>
  <label data-field>
    Type
    <select value={form.type} disabled={!!editing}
      onchange={(e) => setType((e.currentTarget as HTMLSelectElement).value as PluginType)}>
      {#each PLUGIN_TYPES as t}<option value={t}>{PLUGIN_LABELS[t]}</option>{/each}
    </select>
    <span data-hint>{HELP[form.type]}</span>
  </label>

  <div class="row">
    <label data-field>
      Scope
      <select bind:value={form.scope}>
        <option value="global">Global (all routes)</option>
        <option value="service">Service</option>
        <option value="route">Route</option>
      </select>
    </label>
    {#if form.scope === 'service'}
      <label data-field>
        Service
        <select bind:value={form.service_id}>
          {#each services as s}<option value={s.id}>{s.name}</option>{/each}
        </select>
      </label>
    {:else if form.scope === 'route'}
      <label data-field>
        Route
        <select bind:value={form.route_id}>
          {#each routes as r}<option value={r.id}>{r.name}</option>{/each}
        </select>
      </label>
    {:else}
      <label data-field>
        Ordering
        <input type="number" bind:value={form.ordering} />
      </label>
    {/if}
  </div>

  <div class="cfg-head">
    <span class="sub" style="margin:0">Configuration</span>
    <button class="ghost small" onclick={toggleRaw}>{rawMode ? 'Form editor' : 'Edit as JSON'}</button>
  </div>

  {#if rawMode}
    <div data-field>
      <textarea rows="10" bind:value={form.config} aria-label="Plugin config JSON"></textarea>
    </div>
  {:else if form.type === 'key-auth'}
    <label data-field>
      Key names <span class="faint">(header or query, comma-separated)</span>
      <input class="mono" value={cfg.key_names_csv ?? list(cfg.key_names)}
        oninput={(e) => (cfg.key_names_csv = (e.currentTarget as HTMLInputElement).value)} placeholder="apikey, x-api-key" />
    </label>
    <label class="opt">
      <input type="checkbox" role="switch" checked={cfg.hide_credentials} onchange={() => (cfg.hide_credentials = !cfg.hide_credentials)} />
      Strip the credential before proxying upstream
    </label>
  {:else if form.type === 'basic-auth'}
    <label data-field>
      Realm
      <input bind:value={cfg.realm} placeholder="Raahi" />
      <span data-hint>Shown in the browser's authentication prompt.</span>
    </label>
  {:else if form.type === 'jwt'}
    <div class="row">
      <label data-field>
        Key claim
        <input class="mono" bind:value={cfg.key_claim_name} placeholder="iss" />
        <span data-hint>Claim matched against consumers' jwt credentials.</span>
      </label>
      <label data-field>
        Query params <span class="faint">(besides Bearer header)</span>
        <input class="mono" value={cfg.uri_param_names_csv ?? list(cfg.uri_param_names)}
          oninput={(e) => (cfg.uri_param_names_csv = (e.currentTarget as HTMLInputElement).value)} placeholder="jwt" />
      </label>
    </div>
    <label class="opt">
      <input type="checkbox" role="switch" checked={cfg.require_exp} onchange={() => (cfg.require_exp = !cfg.require_exp)} />
      Reject tokens without an exp claim
    </label>
    <div class="row">
      <label data-field>
        JWKS URL <span class="faint">(optional — identity-provider mode)</span>
        <input class="mono" bind:value={cfg.jwks_url} placeholder="https://idp.example.com/.well-known/jwks.json" />
        <span data-hint>
          When set, RS256 tokens are verified against these keys (refreshed every 30s) instead of consumer
          credentials. Consumer identity comes from the claim below.
        </span>
      </label>
      <label data-field style="flex:0 0 140px">
        Consumer claim
        <input class="mono" bind:value={cfg.consumer_claim} placeholder="sub" />
      </label>
    </div>
  {:else if form.type === 'acl'}
    <label data-field>
      Allowed groups <span class="faint">(comma-separated; blank = allow all)</span>
      <input class="mono" value={cfg.allow_csv ?? list(cfg.allow)}
        oninput={(e) => (cfg.allow_csv = (e.currentTarget as HTMLInputElement).value)} placeholder="team-a, admins" />
    </label>
    <label data-field>
      Denied groups <span class="faint">(checked first)</span>
      <input class="mono" value={cfg.deny_csv ?? list(cfg.deny)}
        oninput={(e) => (cfg.deny_csv = (e.currentTarget as HTMLInputElement).value)} placeholder="suspended" />
      <span data-hint>Groups are set on each consumer. An auth plugin must run on the same route.</span>
    </label>
  {:else if form.type === 'ip-restriction'}
    <label data-field>
      Allowed IPs/CIDRs <span class="faint">(blank = allow all)</span>
      <input class="mono" value={cfg.allow_csv ?? list(cfg.allow)}
        oninput={(e) => (cfg.allow_csv = (e.currentTarget as HTMLInputElement).value)} placeholder="10.0.0.0/8, 192.168.1.5" />
    </label>
    <label data-field>
      Denied IPs/CIDRs <span class="faint">(checked first)</span>
      <input class="mono" value={cfg.deny_csv ?? list(cfg.deny)}
        oninput={(e) => (cfg.deny_csv = (e.currentTarget as HTMLInputElement).value)} placeholder="203.0.113.0/24" />
    </label>
    <div class="row">
      <label data-field>
        Reject status
        <input type="number" bind:value={cfg.status} />
      </label>
      <label data-field>
        Reject message
        <input bind:value={cfg.message} />
      </label>
    </div>
  {:else if form.type === 'request-size-limit'}
    <label data-field>
      Max body size <span class="faint">(bytes)</span>
      <input type="number" min="1" bind:value={cfg.max_bytes} />
      <span data-hint>Checked against Content-Length; {((Number(cfg.max_bytes) || 0) / 1048576).toFixed(1)} MB.</span>
    </label>
    <label class="opt">
      <input type="checkbox" role="switch" checked={cfg.require_content_length} onchange={() => (cfg.require_content_length = !cfg.require_content_length)} />
      Reject chunked uploads without Content-Length (411)
    </label>
  {:else if form.type === 'request-termination'}
    <div class="row">
      <label data-field style="flex:0 0 110px">
        Status
        <input type="number" bind:value={cfg.status} />
      </label>
      <label data-field>
        Message
        <input bind:value={cfg.message} />
      </label>
    </div>
    <label data-field>
      Content-Type
      <input class="mono" bind:value={cfg.content_type} />
    </label>
  {:else if form.type === 'redirect'}
    <div class="row">
      <label data-field style="flex:0 0 110px">
        Status
        <select bind:value={cfg.status}>
          <option value={301}>301</option>
          <option value={302}>302</option>
          <option value={307}>307</option>
          <option value={308}>308</option>
        </select>
      </label>
      <label data-field>
        Location
        <input class="mono" bind:value={cfg.location} placeholder="https://new.example.com" />
      </label>
    </div>
    <label class="opt">
      <input type="checkbox" role="switch" checked={cfg.preserve_path} onchange={() => (cfg.preserve_path = !cfg.preserve_path)} />
      Append the incoming path and query
    </label>
    <label class="opt">
      <input type="checkbox" role="switch" checked={cfg.http_only} onchange={() => (cfg.http_only = !cfg.http_only)} />
      Redirect HTTP only; proxy HTTPS normally
    </label>
  {:else if form.type === 'http-log'}
    <label data-field>
      Collector endpoint
      <input class="mono" bind:value={cfg.endpoint} placeholder="https://logs.example.com/ingest" />
      <span data-hint>Request records are POSTed there as JSON arrays, off the request path.</span>
    </label>
    <div class="row">
      <label data-field>
        Batch size
        <input type="number" min="1" bind:value={cfg.batch_max} />
      </label>
      <label data-field>
        Flush interval <span class="faint">(ms)</span>
        <input type="number" min="500" bind:value={cfg.flush_interval_ms} />
      </label>
    </div>
  {:else if form.type === 'request-id'}
    <label data-field>
      Header name
      <input class="mono" bind:value={cfg.header_name} placeholder="X-Request-Id" />
      <span data-hint>Set on the upstream request (and the response, if echoed below).</span>
    </label>
    <label class="opt">
      <input type="checkbox" role="switch" checked={cfg.preserve} onchange={() => (cfg.preserve = !cfg.preserve)} />
      Keep an id the client already sent
    </label>
    <label class="opt">
      <input type="checkbox" role="switch" checked={cfg.echo_downstream} onchange={() => (cfg.echo_downstream = !cfg.echo_downstream)} />
      Echo the id on the response
    </label>
  {:else if form.type === 'response-compression'}
    <label data-field>
      Compression level
      <input type="number" min="1" max="9" bind:value={cfg.level} />
      <span data-hint>
        1 (fastest) to 9 (smallest), applied to whichever algorithm the client accepts — gzip, brotli, or zstd.
      </span>
    </label>
  {:else if form.type === 'hsts'}
    <label data-field>
      Max age <span class="faint">(seconds)</span>
      <input type="number" min="1" bind:value={cfg.max_age_secs} />
    </label>
    <label class="opt">
      <input type="checkbox" role="switch" checked={cfg.include_subdomains} onchange={() => (cfg.include_subdomains = !cfg.include_subdomains)} />
      Include subdomains
    </label>
    <label class="opt">
      <input type="checkbox" role="switch" checked={cfg.preload} onchange={() => (cfg.preload = !cfg.preload)} />
      Emit the preload directive
    </label>
  {:else if form.type === 'wasm'}
    {#if modules.length === 0}
      <div role="alert">No WASM modules uploaded yet — add one from the "WASM modules" panel first.</div>
    {:else}
      <div class="row">
        <label data-field>
          Module
          <select bind:value={cfg.module}>
            {#each modules as m}<option value={m.name}>{m.name}</option>{/each}
          </select>
        </label>
        <label data-field>
          Fuel limit <span class="faint">(instructions/call)</span>
          <input type="number" min="1000" bind:value={cfg.fuel} />
        </label>
      </div>
      <label data-field>
        Module config <span class="faint">(JSON, passed on every call)</span>
        <textarea rows="5" bind:value={cfg.config_json}></textarea>
      </label>
    {/if}
  {:else if form.type === 'rate-limit'}
    <div class="row">
      <label data-field>
        Limit <span class="faint">(requests)</span>
        <input type="number" min="1" bind:value={cfg.limit} />
      </label>
      <label data-field>
        Window <span class="faint">(seconds)</span>
        <input type="number" min="1" bind:value={cfg.window_secs} />
      </label>
    </div>
    <label data-field>
      Count per
      <select bind:value={cfg.key}>
        <option value="ip">Client IP</option>
        <option value="consumer">Authenticated consumer</option>
        <option value="route">Route (shared bucket)</option>
      </select>
    </label>
    <label class="opt">
      <input type="checkbox" role="switch" checked={cfg.headers !== false} onchange={() => (cfg.headers = cfg.headers === false)} />
      Send RateLimit-* response headers
    </label>
  {:else if form.type === 'proxy-cache'}
    <div class="row">
      <label data-field>
        TTL <span class="faint">(seconds)</span>
        <input type="number" min="1" bind:value={cfg.ttl_secs} />
      </label>
      <label data-field>
        Max body size <span class="faint">(bytes)</span>
        <input type="number" min="1" bind:value={cfg.max_body_bytes} />
        <span data-hint>Larger responses are proxied but not cached.</span>
      </label>
    </div>
    <label data-field>
      Cacheable methods <span class="faint">(comma-separated)</span>
      <input class="mono" value={cfg.methods_csv ?? list(cfg.methods)}
        oninput={(e) => (cfg.methods_csv = (e.currentTarget as HTMLInputElement).value)} placeholder="GET" />
    </label>
    <label class="opt">
      <input type="checkbox" role="switch" checked={cfg.cache_key_query !== false} onchange={() => (cfg.cache_key_query = cfg.cache_key_query === false)} />
      Include the query string in the cache key
    </label>
  {:else if form.type === 'response-body-transform'}
    <div data-field>
      <label for="bt-rep-0f">Replacements</label>
      {#each replaceRows as row, i}
        <div class="hdr-row">
          <input id={'bt-rep-' + i + 'f'} class="mono" placeholder="find" bind:value={row.from} />
          <input class="mono" placeholder="replace with" bind:value={row.to} aria-label="Replacement value" />
          <button class="ghost small icon" aria-label="Remove replacement" onclick={() => (replaceRows = replaceRows.filter((_, j) => j !== i))}>✕</button>
        </div>
      {/each}
      <button class="outline small" onclick={() => replaceRows.push({ from: '', to: '' })}>+ Add replacement</button>
    </div>
    <div class="row">
      <label data-field>
        Max body size <span class="faint">(bytes)</span>
        <input type="number" min="1" bind:value={cfg.max_body_bytes} />
        <span data-hint>Larger responses pass through untransformed.</span>
      </label>
      <label data-field>
        Content-type prefixes <span class="faint">(comma-separated)</span>
        <input class="mono" value={cfg.content_types_csv ?? list(cfg.content_types)}
          oninput={(e) => (cfg.content_types_csv = (e.currentTarget as HTMLInputElement).value)} placeholder="text/, application/json" />
      </label>
    </div>
  {:else if form.type === 'cors'}
    <label data-field>
      Allowed origins <span class="faint">(comma-separated, * = any)</span>
      <input class="mono" value={cfg.allow_origins_csv ?? list(cfg.allow_origins)}
        oninput={(e) => (cfg.allow_origins_csv = (e.currentTarget as HTMLInputElement).value)} placeholder="https://app.example.com" />
    </label>
    <label data-field>
      Allowed methods
      <input class="mono" value={cfg.allow_methods_csv ?? list(cfg.allow_methods)}
        oninput={(e) => (cfg.allow_methods_csv = (e.currentTarget as HTMLInputElement).value)} placeholder="GET, POST" />
    </label>
    <label data-field>
      Allowed headers
      <input class="mono" value={cfg.allow_headers_csv ?? list(cfg.allow_headers)}
        oninput={(e) => (cfg.allow_headers_csv = (e.currentTarget as HTMLInputElement).value)} placeholder="*" />
    </label>
    <div class="row">
      <label data-field>
        Preflight max age <span class="faint">(seconds)</span>
        <input type="number" min="0" bind:value={cfg.max_age} />
      </label>
    </div>
    <label class="opt">
      <input type="checkbox" role="switch" checked={cfg.allow_credentials} onchange={() => (cfg.allow_credentials = !cfg.allow_credentials)} />
      Allow credentials (cookies, auth headers)
    </label>
  {:else}
    <!-- request/response transform -->
    <div data-field>
      <label for="tf-add-0k">{form.type === 'request-transform' ? 'Add request headers' : 'Add response headers'}</label>
      {#each addRows as row, i}
        <div class="hdr-row">
          <input id={'tf-add-' + i + 'k'} class="mono" placeholder="Header-Name" bind:value={row.k} />
          <input class="mono" placeholder="value" bind:value={row.v} aria-label="Header value" />
          <button class="ghost small icon" aria-label="Remove header" onclick={() => (addRows = addRows.filter((_, j) => j !== i))}>✕</button>
        </div>
      {/each}
      <button class="outline small" onclick={() => addRows.push({ k: '', v: '' })}>+ Add header</button>
    </div>
    <label data-field>
      Remove headers <span class="faint">(comma-separated)</span>
      <input class="mono" value={cfg.remove_csv ?? list(cfg.remove)}
        oninput={(e) => (cfg.remove_csv = (e.currentTarget as HTMLInputElement).value)} placeholder="server, x-powered-by" />
    </label>
  {/if}

  {#if form.scope !== 'global'}
    <div class="row">
      <label data-field>
        Ordering
        <input type="number" bind:value={form.ordering} />
        <span data-hint>Lower runs earlier among plugins of the same type priority.</span>
      </label>
    </div>
  {/if}
  <label class="opt">
    <input type="checkbox" role="switch" checked={form.enabled} onchange={() => (form.enabled = !form.enabled)} />
    Enabled
  </label>

  {#snippet footer()}
    <button class="ghost" onclick={() => (open = false)}>Cancel</button>
    <button onclick={save}>{editing ? 'Save' : 'Create'}</button>
  {/snippet}
</Drawer>

<div class="card" style="margin-top:16px">
  <div class="panel-head">
    <h2>WASM modules</h2>
    <button class="outline small" onclick={() => { modForm = { name: '', description: '', wat: '', wasm_base64: '', fileName: '' }; modOpen = true; }}>
      + Upload module
    </button>
  </div>
  {#if modules.length === 0}
    <div class="empty">
      No modules yet. Upload a <code>.wasm</code> binary or WAT source implementing
      <code>raahi_alloc</code> and <code>on_request</code> / <code>on_response</code>.
    </div>
  {:else}
    <div class="table">
      <table>
        <thead><tr><th>Name</th><th>Description</th><th>Size</th><th></th></tr></thead>
        <tbody>
          {#each modules as m (m.id)}
            <tr>
              <td><span class="mono">{m.name}</span></td>
              <td class="muted">{m.description || '—'}</td>
              <td class="mono faint">{fmtSize(m.size_bytes)}</td>
              <td class="actions">
                <button class="ghost small" data-variant="danger" onclick={() => delModule(m)}>Delete</button>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</div>

<Drawer bind:open={modOpen} title="Upload WASM module">
  <label data-field>
    Name
    <input class="mono" bind:value={modForm.name} placeholder="my-filter" />
  </label>
  <label data-field>
    Description <span class="faint">(optional)</span>
    <input bind:value={modForm.description} />
  </label>
  <label data-field>
    .wasm binary
    <input type="file" accept=".wasm" onchange={onModuleFile} />
    {#if modForm.fileName}<span data-hint>Loaded {modForm.fileName}</span>{/if}
  </label>
  <label data-field>
    …or WAT source text
    <textarea rows="9" bind:value={modForm.wat} placeholder={'(module\n  (memory (export "memory") 1)\n  …)'}></textarea>
    <span data-hint>
      ABI: export <code>memory</code>, <code>raahi_alloc(size)→ptr</code>, and
      <code>on_request(ptr,len)→i64</code> / <code>on_response(ptr,len)→i64</code>
      exchanging JSON; the return packs <code>(out_ptr &lt;&lt; 32) | out_len</code>.
    </span>
  </label>

  {#snippet footer()}
    <button class="ghost" onclick={() => (modOpen = false)}>Cancel</button>
    <button onclick={uploadModule} disabled={!modForm.name || (!modForm.wat && !modForm.wasm_base64)}>
      Upload
    </button>
  {/snippet}
</Drawer>

<style>
  .cfg-sum {
    font-size: 12px;
    color: var(--muted-foreground);
    max-width: 260px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .scope-name {
    margin-left: 6px;
    font-size: 13px;
  }
  .cfg-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin: 4px 0 10px;
    padding-top: 12px;
    border-top: 1px solid var(--border);
  }
  .hdr-row {
    display: grid;
    grid-template-columns: 1fr 1fr auto;
    gap: 8px;
    margin-bottom: 8px;
    align-items: center;
  }
  .hdr-row input {
    margin-block-start: 0;
  }
  /* Oat renders switch labels inline-flex; these are one-per-line option rows. */
  .opt {
    display: flex;
    margin-block-end: 14px;
  }
</style>
