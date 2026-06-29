<script lang="ts">
  import { onMount } from 'svelte';
  import { api, ApiError } from '../lib/api';
  import type { Certificate, LbAlgorithm, Settings } from '../lib/types';
  import { toast } from '../lib/state.svelte';

  let settings = $state<Settings | null>(null);
  let certs = $state<Certificate[]>([]);
  let loading = $state(true);

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
      [settings, certs] = await Promise.all([api.getSettings(), api.listCertificates()]);
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
      toast('Settings saved. Listener/cert changes apply on restart.', 'ok');
      await load();
    } catch (e) {
      toast((e as ApiError).message, 'err');
    }
  }

  onMount(load);
</script>

{#if loading}
  <div class="empty"><span class="spinner"></span></div>
{:else}
  <div class="panel" style="max-width:620px">
    <div class="panel-head"><h2>Listeners & defaults</h2></div>

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

    <div class="row">
      <div class="field">
        <label for="s-lb">Default load balancing</label>
        <select id="s-lb" class="select" bind:value={form.default_lb}>
          <option value="round_robin">round_robin</option>
          <option value="weighted">weighted</option>
          <option value="random">random</option>
          <option value="consistent">consistent</option>
        </select>
      </div>
      <div class="field">
        <label for="s-cert">Active TLS certificate</label>
        <select id="s-cert" class="select" bind:value={form.active_certificate_id}>
          <option value={null}>— none (HTTPS off) —</option>
          {#each certs as c}<option value={c.id}>{c.name}</option>{/each}
        </select>
      </div>
    </div>

    <p class="hint">Listener addresses and the active certificate are read at startup; changing them requires a restart.</p>

    <div style="margin-top:8px">
      <button class="btn btn-primary" onclick={save}>Save settings</button>
    </div>
  </div>
{/if}
