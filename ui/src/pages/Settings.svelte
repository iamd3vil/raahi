<script lang="ts">
  import { onMount } from 'svelte';
  import { api, ApiError, setAdminToken } from '../lib/api';
  import type { Certificate, ConfigSummary, LbAlgorithm, Settings } from '../lib/types';
  import { toast } from '../lib/state.svelte';

  let settings = $state<Settings | null>(null);
  let certs = $state<Certificate[]>([]);
  let summary = $state<ConfigSummary | null>(null);
  let loading = $state(true);
  let exporting = $state(false);
  let authEnabled = $state(false);
  let freshToken = $state('');

  async function loadAuth() {
    try {
      authEnabled = (await api.adminStatus()).auth_enabled;
    } catch {
      /* ignore */
    }
  }

  async function generateToken() {
    if (
      authEnabled &&
      !confirm('Rotate the admin token? The old token stops working immediately.')
    )
      return;
    try {
      const r = await api.createAdminToken();
      freshToken = r.token;
      setAdminToken(r.token); // keep this browser session working
      await loadAuth();
      toast('Admin token generated', 'ok');
    } catch (e) {
      toast((e as ApiError).message, 'err');
    }
  }

  async function disableAuth() {
    if (!confirm('Disable admin API auth? Anyone who can reach the admin port gets full access.')) return;
    try {
      await api.deleteAdminToken();
      setAdminToken(null);
      freshToken = '';
      await loadAuth();
      toast('Admin auth disabled', 'ok');
    } catch (e) {
      toast((e as ApiError).message, 'err');
    }
  }

  function copyToken() {
    navigator.clipboard?.writeText(freshToken);
    toast('Token copied', 'ok');
  }

  let form = $state({
    proxy_http_addr: '',
    proxy_https_addr: '',
    admin_addr: '',
    default_lb: 'round_robin' as LbAlgorithm,
    active_certificate_id: null as number | null,
  });

  async function load() {
    loading = true;
    try {
      [settings, certs, summary] = await Promise.all([
        api.getSettings(),
        api.listCertificates(),
        api.configSummary(),
      ]);
      form = {
        proxy_http_addr: settings.proxy_http_addr,
        proxy_https_addr: settings.proxy_https_addr ?? '',
        admin_addr: settings.admin_addr,
        default_lb: settings.default_lb,
        active_certificate_id: settings.active_certificate_id,
      };
    } catch (e) {
      toast((e as ApiError).message, 'err');
    } finally {
      loading = false;
    }
  }

  async function save() {
    try {
      await api.updateSettings({
        proxy_http_addr: form.proxy_http_addr,
        proxy_https_addr: form.proxy_https_addr || null,
        admin_addr: form.admin_addr,
        default_lb: form.default_lb,
        active_certificate_id: form.active_certificate_id ? Number(form.active_certificate_id) : null,
      });
      toast('Settings saved', 'ok');
      await load();
    } catch (e) {
      toast((e as ApiError).message, 'err');
    }
  }

  async function purgeCache() {
    try {
      const r = await api.purgeCache();
      toast(`Cache purged (${r.purged} entr${r.purged === 1 ? 'y' : 'ies'})`, 'ok');
    } catch (e) {
      toast((e as ApiError).message, 'err');
    }
  }

  let includeSecrets = $state(false);
  let importing = $state(false);

  async function exportConfig() {
    exporting = true;
    try {
      const data = await api.exportConfig(includeSecrets);
      const blob = new Blob([JSON.stringify(data, null, 2)], { type: 'application/json' });
      const url = URL.createObjectURL(blob);
      const a = document.createElement('a');
      a.href = url;
      a.download = `raahi-config-${new Date().toISOString().slice(0, 10)}.json`;
      a.click();
      URL.revokeObjectURL(url);
      toast('Configuration exported', 'ok');
    } catch (e) {
      toast((e as ApiError).message, 'err');
    } finally {
      exporting = false;
    }
  }

  async function importConfig(e: Event) {
    const input = e.currentTarget as HTMLInputElement;
    const file = input.files?.[0];
    input.value = '';
    if (!file) return;
    if (
      !confirm(
        `Replace the ENTIRE configuration with "${file.name}"?\n\nAll current services, routes, plugins, consumers, and certificates will be wiped and rebuilt from the file. This applies immediately to live traffic.`,
      )
    )
      return;
    importing = true;
    try {
      const doc = JSON.parse(await file.text());
      const r = await api.importConfig(doc);
      const summary = `Imported ${r.services} services, ${r.routes} routes, ${r.plugins} plugins, ${r.consumers} consumers, ${r.certificates} certs`;
      toast(r.skipped.length ? `${summary} — ${r.skipped.length} skipped (see console)` : summary, 'ok');
      if (r.skipped.length) console.warn('Import skipped:', r.skipped);
      await load();
    } catch (e) {
      toast((e as ApiError).message ?? 'Import failed: invalid JSON', 'err');
    } finally {
      importing = false;
    }
  }

  onMount(() => {
    load();
    loadAuth();
  });
</script>

{#if loading}
  <div class="empty"><span aria-busy="true" data-spinner="small"></span></div>
{:else}
  <div class="cols">
    <div>
      <div class="card">
        <div class="panel-head"><h2>Listeners</h2></div>
        <div class="row">
          <label data-field>
            Proxy HTTP address
            <input class="mono" bind:value={form.proxy_http_addr} />
          </label>
          <label data-field>
            Proxy HTTPS address
            <input class="mono" bind:value={form.proxy_https_addr} placeholder="(disabled)" />
          </label>
        </div>
        <label data-field>
          Admin API address
          <input class="mono" bind:value={form.admin_addr} />
        </label>
        <p class="hint">Listener addresses are bound at startup — changing them requires a restart.</p>
      </div>

      <div class="card" style="margin-top:16px">
        <div class="panel-head"><h2>Defaults & TLS</h2></div>
        <div class="row">
          <label data-field>
            Default load balancing
            <select bind:value={form.default_lb}>
              <option value="round_robin">round_robin</option>
              <option value="weighted">weighted</option>
              <option value="random">random</option>
              <option value="consistent">consistent (by client IP)</option>
            </select>
          </label>
          <label data-field>
            Default TLS certificate
            <select bind:value={form.active_certificate_id}>
              <option value={null}>— none —</option>
              {#each certs as c}<option value={c.id}>{c.name}</option>{/each}
            </select>
            <span data-hint>Fallback when no certificate matches the SNI. Applies live.</span>
          </label>
        </div>
        <div style="margin-top:8px">
          <button onclick={save}>Save settings</button>
        </div>
      </div>
    </div>

    <div>
      <div class="card">
        <div class="panel-head">
          <h2>Running configuration</h2>
          <button class="outline small" onclick={purgeCache} title="Drop every proxy-cache entry">Purge cache</button>
        </div>
        {#if summary}
          <div class="sum-grid">
            <div class="sum"><span class="sum-v">{summary.version}</span><span class="sum-l">config version</span></div>
            <div class="sum"><span class="sum-v">{summary.routes}</span><span class="sum-l">routes</span></div>
            <div class="sum"><span class="sum-v">{summary.services}</span><span class="sum-l">services</span></div>
            <div class="sum"><span class="sum-v">{summary.plugins}</span><span class="sum-l">plugins</span></div>
            <div class="sum"><span class="sum-v">{summary.consumers}</span><span class="sum-l">consumers</span></div>
            <div class="sum"><span class="sum-v">{summary.key_credentials}</span><span class="sum-l">API keys</span></div>
          </div>
          <p class="hint" style="margin-top:12px">
            The version increments on every hot-reload. All config changes apply to the running proxy without a restart.
          </p>
        {/if}
      </div>

      <div class="card" style="margin-top:16px">
        <div class="panel-head">
          <h2>Admin access</h2>
          <span class="badge" data-variant={authEnabled ? 'success' : 'warning'}>{authEnabled ? 'protected' : 'open'}</span>
        </div>
        <p class="muted" style="margin:0 0 12px">
          {#if authEnabled}
            The admin API requires a bearer token. Rotate it any time — the old token stops working immediately.
          {:else}
            The admin API is unauthenticated (loopback binding is the only protection). Generate a token to require
            <code>Authorization: Bearer …</code> on every request.
          {/if}
        </p>
        {#if freshToken}
          <div class="token-box">
            <div class="hint" style="margin-bottom:6px">Your new token — store it now, it is not retrievable later:</div>
            <div class="token-row">
              <code class="token">{freshToken}</code>
              <button class="outline small" onclick={copyToken}>Copy</button>
            </div>
          </div>
        {/if}
        <div class="flex" style="gap:8px">
          <button class="outline" onclick={generateToken}>{authEnabled ? 'Rotate token' : 'Generate token'}</button>
          {#if authEnabled}
            <button class="ghost" data-variant="danger" onclick={disableAuth}>Disable auth</button>
          {/if}
        </div>
      </div>

      <div class="card" style="margin-top:16px">
        <div class="panel-head"><h2>Backup & restore</h2></div>
        <p class="muted" style="margin:0 0 10px">
          Export the full configuration as one JSON document, or import one to
          <strong>replace</strong> the running configuration declaratively.
        </p>
        <label>
          <input type="checkbox" role="switch" checked={includeSecrets} onchange={() => (includeSecrets = !includeSecrets)} />
          Include secrets (restorable backup: cert keys, credential hashes)
        </label>
        <div class="flex" style="gap:8px; margin-top:8px">
          <button class="outline" onclick={exportConfig} disabled={exporting}>
            {#if exporting}<span aria-busy="true" data-spinner="small"></span>{:else}
              <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                <path d="M12 3v12m0 0 4-4m-4 4-4-4M4 17v2a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-2" />
              </svg>
            {/if}
            Export
          </button>
          <label class="import-btn" class:disabled={importing}>
            {#if importing}<span aria-busy="true" data-spinner="small"></span>{:else}
              <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                <path d="M12 15V3m0 0 4 4m-4-4-4 4M4 17v2a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-2" />
              </svg>
            {/if}
            Import…
            <input type="file" accept=".json,application/json" style="display:none" onchange={importConfig} />
          </label>
        </div>
      </div>
    </div>
  </div>
{/if}

<style>
  .cols {
    display: grid;
    grid-template-columns: minmax(320px, 620px) minmax(280px, 420px);
    gap: 16px;
    align-items: start;
  }
  .sum-grid {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 10px;
  }
  .sum {
    background: var(--muted);
    border: 1px solid var(--border);
    border-radius: var(--radius-medium);
    padding: 10px 12px;
    display: flex;
    flex-direction: column;
  }
  .sum-v {
    font-size: 20px;
    font-weight: 700;
  }
  .sum-l {
    font-size: 11px;
    color: var(--faint-foreground);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  .token-box {
    background: var(--muted);
    border: 1px solid var(--border);
    border-radius: var(--radius-medium);
    padding: 12px;
    margin-bottom: 12px;
  }
  .token-row {
    display: flex;
    gap: 8px;
    align-items: center;
  }
  .token {
    word-break: break-all;
    flex: 1;
  }
  .hint {
    font-size: 12.5px;
    color: var(--muted-foreground);
  }
  /* The file-import control is a <label> wrapping a hidden input; Oat only
     styles real buttons, so replicate its outline button look here. */
  .import-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: var(--space-2);
    padding: var(--space-2) var(--space-4);
    font-size: var(--text-7);
    font-weight: var(--font-medium);
    line-height: var(--leading-normal);
    white-space: nowrap;
    color: var(--foreground);
    background: transparent;
    border: 1px solid var(--border);
    border-radius: var(--radius-medium);
    cursor: pointer;
  }
  .import-btn:hover {
    background: var(--accent);
  }
  .import-btn.disabled {
    opacity: 0.6;
    pointer-events: none;
  }
  @media (max-width: 980px) {
    .cols {
      grid-template-columns: 1fr;
    }
  }
</style>
