<script lang="ts">
  import { onMount } from 'svelte';
  import { api, ApiError } from '../lib/api';
  import type { Certificate, ConfigSummary, LbAlgorithm, Settings } from '../lib/types';
  import { toast } from '../lib/state.svelte';

  let settings = $state<Settings | null>(null);
  let certs = $state<Certificate[]>([]);
  let summary = $state<ConfigSummary | null>(null);
  let loading = $state(true);
  let exporting = $state(false);

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

  async function exportConfig() {
    exporting = true;
    try {
      const data = await api.exportConfig();
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

  onMount(load);
</script>

{#if loading}
  <div class="empty"><span class="spinner"></span></div>
{:else}
  <div class="cols">
    <div>
      <div class="panel">
        <div class="panel-head"><h2>Listeners</h2></div>
        <div class="row">
          <div class="field">
            <label for="s-http">Proxy HTTP address</label>
            <input id="s-http" class="input mono" bind:value={form.proxy_http_addr} />
          </div>
          <div class="field">
            <label for="s-https">Proxy HTTPS address</label>
            <input id="s-https" class="input mono" bind:value={form.proxy_https_addr} placeholder="(disabled)" />
          </div>
        </div>
        <div class="field">
          <label for="s-admin">Admin API address</label>
          <input id="s-admin" class="input mono" bind:value={form.admin_addr} />
        </div>
        <p class="hint">Listener addresses are bound at startup — changing them requires a restart.</p>
      </div>

      <div class="panel" style="margin-top:16px">
        <div class="panel-head"><h2>Defaults & TLS</h2></div>
        <div class="row">
          <div class="field">
            <label for="s-lb">Default load balancing</label>
            <select id="s-lb" class="select" bind:value={form.default_lb}>
              <option value="round_robin">round_robin</option>
              <option value="weighted">weighted</option>
              <option value="random">random</option>
              <option value="consistent">consistent (by client IP)</option>
            </select>
          </div>
          <div class="field">
            <label for="s-cert">Default TLS certificate</label>
            <select id="s-cert" class="select" bind:value={form.active_certificate_id}>
              <option value={null}>— none —</option>
              {#each certs as c}<option value={c.id}>{c.name}</option>{/each}
            </select>
            <span class="hint">Fallback when no certificate matches the SNI. Applies live.</span>
          </div>
        </div>
        <div style="margin-top:8px">
          <button class="btn btn-primary" onclick={save}>Save settings</button>
        </div>
      </div>
    </div>

    <div>
      <div class="panel">
        <div class="panel-head"><h2>Running configuration</h2></div>
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

      <div class="panel" style="margin-top:16px">
        <div class="panel-head"><h2>Backup</h2></div>
        <p class="muted" style="margin:0 0 12px">
          Download the full configuration — services, targets, routes, plugins, consumers, certificates, and
          settings — as a single JSON document. Secrets (private keys, password hashes) are never included.
        </p>
        <button class="btn" onclick={exportConfig} disabled={exporting}>
          {#if exporting}<span class="spinner"></span>{:else}
            <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
              <path d="M12 3v12m0 0 4-4m-4 4-4-4M4 17v2a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-2" />
            </svg>
          {/if}
          Export configuration
        </button>
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
    background: var(--surface-2);
    border: 1px solid var(--border);
    border-radius: var(--r-md);
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
    color: var(--faint);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  @media (max-width: 980px) {
    .cols {
      grid-template-columns: 1fr;
    }
  }
</style>
