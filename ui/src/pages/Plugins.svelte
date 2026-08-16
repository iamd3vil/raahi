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
    redirect: { status: 302, location: '', preserve_path: true },
    cors: { allow_origins: ['*'], allow_methods: ['GET', 'POST', 'PUT', 'DELETE', 'OPTIONS'], allow_headers: ['*'], allow_credentials: false, max_age: 3600 },
    'request-transform': { add: {}, remove: [] },
    'response-transform': { add: {}, remove: [] },
    'response-body-transform': { replace: [], max_body_bytes: 1048576, content_types: ['text/', 'application/json'] },
    'http-log': { endpoint: '', headers: {}, batch_max: 50, flush_interval_ms: 2000 },
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
    'response-body-transform': 'Find/replace on text response bodies (buffered up to a size cap; binary passes through).',
    'http-log': 'POSTs request records (JSON batches) to an external collector, off the hot path.',
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
      };
    if (t === 'http-log')
      return {
        endpoint: String(cfg.endpoint ?? ''),
        headers: cfg.headers ?? {},
        batch_max: Number(cfg.batch_max) || 50,
        flush_interval_ms: Number(cfg.flush_interval_ms) || 2000,
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
      case 'response-body-transform': {
        const n = (c.replace ?? []).length;
        return `${n} replacement${n === 1 ? '' : 's'}`;
      }
      case 'http-log':
        return `→ ${c.endpoint || '—'}`;
      case 'wasm':
        return `module: ${c.module || '—'}`;
    }
  }

  onMount(load);
</script>

<div class="head-actions">
  <p class="muted">Middleware applied per route, per service, or globally — ordered, hot-reloaded.</p>
  <button class="btn btn-primary" onclick={openNew}>+ New plugin</button>
</div>

<div class="panel">
  {#if loading}
    <div class="empty"><span class="spinner"></span></div>
  {:else if plugins.length === 0}
    <EmptyState
      icon="M10 3v4m4-4v4M5 7h14l-1 12a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 7z"
      title="No plugins configured"
      description="Add auth, rate limiting, CORS, or header transforms. Plugins apply globally, to one service, or to one route."
    >
      {#snippet action()}
        <button class="btn btn-primary" onclick={openNew}>+ Add your first plugin</button>
      {/snippet}
    </EmptyState>
  {:else}
    <div class="table-wrap">
      <table class="table">
        <thead><tr><th>Type</th><th>Config</th><th>Scope</th><th>Order</th><th>Enabled</th><th></th></tr></thead>
        <tbody>
          {#each plugins as p (p.id)}
            {@const sc = scopeLabel(p)}
            <tr style:opacity={p.enabled ? 1 : 0.55}>
              <td><span class="badge accent">{PLUGIN_LABELS[p.type]}</span></td>
              <td class="mono cfg-sum">{summary(p)}</td>
              <td>
                <span class="badge">{sc.kind}</span>
                {#if p.scope !== 'global'}<span class="scope-name">{sc.name}</span>{/if}
              </td>
              <td class="mono">{p.ordering}</td>
              <td><button class="toggle {p.enabled ? 'on' : ''}" aria-label="Toggle enabled" onclick={() => toggleEnabled(p)}></button></td>
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

<Drawer bind:open title={editing ? `Edit ${PLUGIN_LABELS[form.type]}` : 'New plugin'}>
  <div class="field">
    <label for="p-type">Type</label>
    <select id="p-type" class="select" value={form.type} disabled={!!editing}
      onchange={(e) => setType((e.currentTarget as HTMLSelectElement).value as PluginType)}>
      {#each PLUGIN_TYPES as t}<option value={t}>{PLUGIN_LABELS[t]}</option>{/each}
    </select>
    <span class="hint">{HELP[form.type]}</span>
  </div>

  <div class="row">
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
    {:else}
      <div class="field">
        <label for="p-order">Ordering</label>
        <input id="p-order" class="input" type="number" bind:value={form.ordering} />
      </div>
    {/if}
  </div>

  <div class="cfg-head">
    <span class="sub" style="margin:0">Configuration</span>
    <button class="btn btn-ghost btn-sm" onclick={toggleRaw}>{rawMode ? 'Form editor' : 'Edit as JSON'}</button>
  </div>

  {#if rawMode}
    <div class="field">
      <textarea class="textarea" rows="10" bind:value={form.config} aria-label="Plugin config JSON"></textarea>
    </div>
  {:else if form.type === 'key-auth'}
    <div class="field">
      <label for="ka-keys">Key names <span class="faint">(header or query, comma-separated)</span></label>
      <input id="ka-keys" class="input mono" value={cfg.key_names_csv ?? list(cfg.key_names)}
        oninput={(e) => (cfg.key_names_csv = (e.currentTarget as HTMLInputElement).value)} placeholder="apikey, x-api-key" />
    </div>
    <button class="opt" onclick={() => (cfg.hide_credentials = !cfg.hide_credentials)}>
      <span class="toggle {cfg.hide_credentials ? 'on' : ''}"></span> Strip the credential before proxying upstream
    </button>
  {:else if form.type === 'basic-auth'}
    <div class="field">
      <label for="ba-realm">Realm</label>
      <input id="ba-realm" class="input" bind:value={cfg.realm} placeholder="Raahi" />
      <span class="hint">Shown in the browser's authentication prompt.</span>
    </div>
  {:else if form.type === 'jwt'}
    <div class="row">
      <div class="field">
        <label for="jwt-claim">Key claim</label>
        <input id="jwt-claim" class="input mono" bind:value={cfg.key_claim_name} placeholder="iss" />
        <span class="hint">Claim matched against consumers' jwt credentials.</span>
      </div>
      <div class="field">
        <label for="jwt-params">Query params <span class="faint">(besides Bearer header)</span></label>
        <input id="jwt-params" class="input mono" value={cfg.uri_param_names_csv ?? list(cfg.uri_param_names)}
          oninput={(e) => (cfg.uri_param_names_csv = (e.currentTarget as HTMLInputElement).value)} placeholder="jwt" />
      </div>
    </div>
    <button class="opt" onclick={() => (cfg.require_exp = !cfg.require_exp)}>
      <span class="toggle {cfg.require_exp ? 'on' : ''}"></span> Reject tokens without an exp claim
    </button>
    <div class="row">
      <div class="field">
        <label for="jwt-jwks">JWKS URL <span class="faint">(optional — identity-provider mode)</span></label>
        <input id="jwt-jwks" class="input mono" bind:value={cfg.jwks_url} placeholder="https://idp.example.com/.well-known/jwks.json" />
        <span class="hint">
          When set, RS256 tokens are verified against these keys (refreshed every 30s) instead of consumer
          credentials. Consumer identity comes from the claim below.
        </span>
      </div>
      <div class="field" style="flex:0 0 140px">
        <label for="jwt-cclaim">Consumer claim</label>
        <input id="jwt-cclaim" class="input mono" bind:value={cfg.consumer_claim} placeholder="sub" />
      </div>
    </div>
  {:else if form.type === 'acl'}
    <div class="field">
      <label for="acl-allow">Allowed groups <span class="faint">(comma-separated; blank = allow all)</span></label>
      <input id="acl-allow" class="input mono" value={cfg.allow_csv ?? list(cfg.allow)}
        oninput={(e) => (cfg.allow_csv = (e.currentTarget as HTMLInputElement).value)} placeholder="team-a, admins" />
    </div>
    <div class="field">
      <label for="acl-deny">Denied groups <span class="faint">(checked first)</span></label>
      <input id="acl-deny" class="input mono" value={cfg.deny_csv ?? list(cfg.deny)}
        oninput={(e) => (cfg.deny_csv = (e.currentTarget as HTMLInputElement).value)} placeholder="suspended" />
      <span class="hint">Groups are set on each consumer. An auth plugin must run on the same route.</span>
    </div>
  {:else if form.type === 'ip-restriction'}
    <div class="field">
      <label for="ip-allow">Allowed IPs/CIDRs <span class="faint">(blank = allow all)</span></label>
      <input id="ip-allow" class="input mono" value={cfg.allow_csv ?? list(cfg.allow)}
        oninput={(e) => (cfg.allow_csv = (e.currentTarget as HTMLInputElement).value)} placeholder="10.0.0.0/8, 192.168.1.5" />
    </div>
    <div class="field">
      <label for="ip-deny">Denied IPs/CIDRs <span class="faint">(checked first)</span></label>
      <input id="ip-deny" class="input mono" value={cfg.deny_csv ?? list(cfg.deny)}
        oninput={(e) => (cfg.deny_csv = (e.currentTarget as HTMLInputElement).value)} placeholder="203.0.113.0/24" />
    </div>
    <div class="row">
      <div class="field">
        <label for="ip-status">Reject status</label>
        <input id="ip-status" class="input" type="number" bind:value={cfg.status} />
      </div>
      <div class="field">
        <label for="ip-msg">Reject message</label>
        <input id="ip-msg" class="input" bind:value={cfg.message} />
      </div>
    </div>
  {:else if form.type === 'request-size-limit'}
    <div class="field">
      <label for="sl-max">Max body size <span class="faint">(bytes)</span></label>
      <input id="sl-max" class="input" type="number" min="1" bind:value={cfg.max_bytes} />
      <span class="hint">Checked against Content-Length; {((Number(cfg.max_bytes) || 0) / 1048576).toFixed(1)} MB.</span>
    </div>
    <button class="opt" onclick={() => (cfg.require_content_length = !cfg.require_content_length)}>
      <span class="toggle {cfg.require_content_length ? 'on' : ''}"></span> Reject chunked uploads without Content-Length (411)
    </button>
  {:else if form.type === 'request-termination'}
    <div class="row">
      <div class="field" style="flex:0 0 110px">
        <label for="rt-status">Status</label>
        <input id="rt-status" class="input" type="number" bind:value={cfg.status} />
      </div>
      <div class="field">
        <label for="rt-msg">Message</label>
        <input id="rt-msg" class="input" bind:value={cfg.message} />
      </div>
    </div>
    <div class="field">
      <label for="rt-ct">Content-Type</label>
      <input id="rt-ct" class="input mono" bind:value={cfg.content_type} />
    </div>
  {:else if form.type === 'redirect'}
    <div class="row">
      <div class="field" style="flex:0 0 110px">
        <label for="rd-status">Status</label>
        <select id="rd-status" class="select" bind:value={cfg.status}>
          <option value={301}>301</option>
          <option value={302}>302</option>
          <option value={307}>307</option>
          <option value={308}>308</option>
        </select>
      </div>
      <div class="field">
        <label for="rd-loc">Location</label>
        <input id="rd-loc" class="input mono" bind:value={cfg.location} placeholder="https://new.example.com" />
      </div>
    </div>
    <button class="opt" onclick={() => (cfg.preserve_path = !cfg.preserve_path)}>
      <span class="toggle {cfg.preserve_path ? 'on' : ''}"></span> Append the incoming path and query
    </button>
  {:else if form.type === 'http-log'}
    <div class="field">
      <label for="hl-endpoint">Collector endpoint</label>
      <input id="hl-endpoint" class="input mono" bind:value={cfg.endpoint} placeholder="https://logs.example.com/ingest" />
      <span class="hint">Request records are POSTed there as JSON arrays, off the request path.</span>
    </div>
    <div class="row">
      <div class="field">
        <label for="hl-batch">Batch size</label>
        <input id="hl-batch" class="input" type="number" min="1" bind:value={cfg.batch_max} />
      </div>
      <div class="field">
        <label for="hl-flush">Flush interval <span class="faint">(ms)</span></label>
        <input id="hl-flush" class="input" type="number" min="500" bind:value={cfg.flush_interval_ms} />
      </div>
    </div>
  {:else if form.type === 'wasm'}
    {#if modules.length === 0}
      <div class="note">No WASM modules uploaded yet — add one from the "WASM modules" panel first.</div>
    {:else}
      <div class="row">
        <div class="field">
          <label for="w-module">Module</label>
          <select id="w-module" class="select" bind:value={cfg.module}>
            {#each modules as m}<option value={m.name}>{m.name}</option>{/each}
          </select>
        </div>
        <div class="field">
          <label for="w-fuel">Fuel limit <span class="faint">(instructions/call)</span></label>
          <input id="w-fuel" class="input" type="number" min="1000" bind:value={cfg.fuel} />
        </div>
      </div>
      <div class="field">
        <label for="w-config">Module config <span class="faint">(JSON, passed on every call)</span></label>
        <textarea id="w-config" class="textarea" rows="5" bind:value={cfg.config_json}></textarea>
      </div>
    {/if}
  {:else if form.type === 'rate-limit'}
    <div class="row">
      <div class="field">
        <label for="rl-limit">Limit <span class="faint">(requests)</span></label>
        <input id="rl-limit" class="input" type="number" min="1" bind:value={cfg.limit} />
      </div>
      <div class="field">
        <label for="rl-window">Window <span class="faint">(seconds)</span></label>
        <input id="rl-window" class="input" type="number" min="1" bind:value={cfg.window_secs} />
      </div>
    </div>
    <div class="field">
      <label for="rl-key">Count per</label>
      <select id="rl-key" class="select" bind:value={cfg.key}>
        <option value="ip">Client IP</option>
        <option value="consumer">Authenticated consumer</option>
        <option value="route">Route (shared bucket)</option>
      </select>
    </div>
    <button class="opt" onclick={() => (cfg.headers = cfg.headers === false)}>
      <span class="toggle {cfg.headers !== false ? 'on' : ''}"></span> Send RateLimit-* response headers
    </button>
  {:else if form.type === 'proxy-cache'}
    <div class="row">
      <div class="field">
        <label for="pc-ttl">TTL <span class="faint">(seconds)</span></label>
        <input id="pc-ttl" class="input" type="number" min="1" bind:value={cfg.ttl_secs} />
      </div>
      <div class="field">
        <label for="pc-max">Max body size <span class="faint">(bytes)</span></label>
        <input id="pc-max" class="input" type="number" min="1" bind:value={cfg.max_body_bytes} />
        <span class="hint">Larger responses are proxied but not cached.</span>
      </div>
    </div>
    <div class="field">
      <label for="pc-methods">Cacheable methods <span class="faint">(comma-separated)</span></label>
      <input id="pc-methods" class="input mono" value={cfg.methods_csv ?? list(cfg.methods)}
        oninput={(e) => (cfg.methods_csv = (e.currentTarget as HTMLInputElement).value)} placeholder="GET" />
    </div>
    <button class="opt" onclick={() => (cfg.cache_key_query = cfg.cache_key_query === false)}>
      <span class="toggle {cfg.cache_key_query !== false ? 'on' : ''}"></span> Include the query string in the cache key
    </button>
  {:else if form.type === 'response-body-transform'}
    <div class="field">
      <label for="bt-rep-0f">Replacements</label>
      {#each replaceRows as row, i}
        <div class="hdr-row">
          <input id={'bt-rep-' + i + 'f'} class="input mono" placeholder="find" bind:value={row.from} />
          <input class="input mono" placeholder="replace with" bind:value={row.to} aria-label="Replacement value" />
          <button class="btn btn-sm btn-ghost" aria-label="Remove replacement" onclick={() => (replaceRows = replaceRows.filter((_, j) => j !== i))}>✕</button>
        </div>
      {/each}
      <button class="btn btn-sm" style="align-self:flex-start" onclick={() => replaceRows.push({ from: '', to: '' })}>+ Add replacement</button>
    </div>
    <div class="row">
      <div class="field">
        <label for="bt-max">Max body size <span class="faint">(bytes)</span></label>
        <input id="bt-max" class="input" type="number" min="1" bind:value={cfg.max_body_bytes} />
        <span class="hint">Larger responses pass through untransformed.</span>
      </div>
      <div class="field">
        <label for="bt-ct">Content-type prefixes <span class="faint">(comma-separated)</span></label>
        <input id="bt-ct" class="input mono" value={cfg.content_types_csv ?? list(cfg.content_types)}
          oninput={(e) => (cfg.content_types_csv = (e.currentTarget as HTMLInputElement).value)} placeholder="text/, application/json" />
      </div>
    </div>
  {:else if form.type === 'cors'}
    <div class="field">
      <label for="c-origins">Allowed origins <span class="faint">(comma-separated, * = any)</span></label>
      <input id="c-origins" class="input mono" value={cfg.allow_origins_csv ?? list(cfg.allow_origins)}
        oninput={(e) => (cfg.allow_origins_csv = (e.currentTarget as HTMLInputElement).value)} placeholder="https://app.example.com" />
    </div>
    <div class="field">
      <label for="c-methods">Allowed methods</label>
      <input id="c-methods" class="input mono" value={cfg.allow_methods_csv ?? list(cfg.allow_methods)}
        oninput={(e) => (cfg.allow_methods_csv = (e.currentTarget as HTMLInputElement).value)} placeholder="GET, POST" />
    </div>
    <div class="field">
      <label for="c-headers">Allowed headers</label>
      <input id="c-headers" class="input mono" value={cfg.allow_headers_csv ?? list(cfg.allow_headers)}
        oninput={(e) => (cfg.allow_headers_csv = (e.currentTarget as HTMLInputElement).value)} placeholder="*" />
    </div>
    <div class="row">
      <div class="field">
        <label for="c-age">Preflight max age <span class="faint">(seconds)</span></label>
        <input id="c-age" class="input" type="number" min="0" bind:value={cfg.max_age} />
      </div>
    </div>
    <button class="opt" onclick={() => (cfg.allow_credentials = !cfg.allow_credentials)}>
      <span class="toggle {cfg.allow_credentials ? 'on' : ''}"></span> Allow credentials (cookies, auth headers)
    </button>
  {:else}
    <!-- request/response transform -->
    <div class="field">
      <label for="tf-add-0k">{form.type === 'request-transform' ? 'Add request headers' : 'Add response headers'}</label>
      {#each addRows as row, i}
        <div class="hdr-row">
          <input id={'tf-add-' + i + 'k'} class="input mono" placeholder="Header-Name" bind:value={row.k} />
          <input class="input mono" placeholder="value" bind:value={row.v} aria-label="Header value" />
          <button class="btn btn-sm btn-ghost" aria-label="Remove header" onclick={() => (addRows = addRows.filter((_, j) => j !== i))}>✕</button>
        </div>
      {/each}
      <button class="btn btn-sm" style="align-self:flex-start" onclick={() => addRows.push({ k: '', v: '' })}>+ Add header</button>
    </div>
    <div class="field">
      <label for="tf-remove">Remove headers <span class="faint">(comma-separated)</span></label>
      <input id="tf-remove" class="input mono" value={cfg.remove_csv ?? list(cfg.remove)}
        oninput={(e) => (cfg.remove_csv = (e.currentTarget as HTMLInputElement).value)} placeholder="server, x-powered-by" />
    </div>
  {/if}

  {#if form.scope !== 'global'}
    <div class="row">
      <div class="field">
        <label for="p-order2">Ordering</label>
        <input id="p-order2" class="input" type="number" bind:value={form.ordering} />
        <span class="hint">Lower runs earlier among plugins of the same type priority.</span>
      </div>
    </div>
  {/if}
  <button class="opt" onclick={() => (form.enabled = !form.enabled)}>
    <span class="toggle {form.enabled ? 'on' : ''}"></span> Enabled
  </button>

  {#snippet footer()}
    <button class="btn btn-ghost" onclick={() => (open = false)}>Cancel</button>
    <button class="btn btn-primary" onclick={save}>{editing ? 'Save' : 'Create'}</button>
  {/snippet}
</Drawer>

<div class="panel" style="margin-top:16px">
  <div class="panel-head">
    <h2>WASM modules</h2>
    <button class="btn btn-sm" onclick={() => { modForm = { name: '', description: '', wat: '', wasm_base64: '', fileName: '' }; modOpen = true; }}>
      + Upload module
    </button>
  </div>
  {#if modules.length === 0}
    <div class="empty">
      No modules yet. Upload a <span class="code">.wasm</span> binary or WAT source implementing
      <span class="code">raahi_alloc</span> and <span class="code">on_request</span> / <span class="code">on_response</span>.
    </div>
  {:else}
    <div class="table-wrap">
      <table class="table">
        <thead><tr><th>Name</th><th>Description</th><th>Size</th><th></th></tr></thead>
        <tbody>
          {#each modules as m (m.id)}
            <tr>
              <td><span class="mono">{m.name}</span></td>
              <td class="muted">{m.description || '—'}</td>
              <td class="mono faint">{fmtSize(m.size_bytes)}</td>
              <td class="actions">
                <button class="btn btn-sm btn-danger" onclick={() => delModule(m)}>Delete</button>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</div>

<Drawer bind:open={modOpen} title="Upload WASM module">
  <div class="field">
    <label for="m-name">Name</label>
    <input id="m-name" class="input mono" bind:value={modForm.name} placeholder="my-filter" />
  </div>
  <div class="field">
    <label for="m-desc">Description <span class="faint">(optional)</span></label>
    <input id="m-desc" class="input" bind:value={modForm.description} />
  </div>
  <div class="field">
    <label for="m-file">.wasm binary</label>
    <input id="m-file" class="input" type="file" accept=".wasm" onchange={onModuleFile} />
    {#if modForm.fileName}<span class="hint">Loaded {modForm.fileName}</span>{/if}
  </div>
  <div class="field">
    <label for="m-wat">…or WAT source text</label>
    <textarea id="m-wat" class="textarea" rows="9" bind:value={modForm.wat} placeholder={'(module\n  (memory (export "memory") 1)\n  …)'}></textarea>
    <span class="hint">
      ABI: export <span class="code">memory</span>, <span class="code">raahi_alloc(size)→ptr</span>, and
      <span class="code">on_request(ptr,len)→i64</span> / <span class="code">on_response(ptr,len)→i64</span>
      exchanging JSON; the return packs <span class="code">(out_ptr &lt;&lt; 32) | out_len</span>.
    </span>
  </div>

  {#snippet footer()}
    <button class="btn btn-ghost" onclick={() => (modOpen = false)}>Cancel</button>
    <button class="btn btn-primary" onclick={uploadModule} disabled={!modForm.name || (!modForm.wat && !modForm.wasm_base64)}>
      Upload
    </button>
  {/snippet}
</Drawer>

<style>
  .cfg-sum {
    font-size: 12px;
    color: var(--muted);
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
  }
</style>
