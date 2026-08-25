<script lang="ts">
  import { onMount } from 'svelte';
  import { api, ApiError } from '../lib/api';
  import type { Certificate } from '../lib/types';
  import { toast } from '../lib/state.svelte';
  import Drawer from '../lib/components/Drawer.svelte';
  import EmptyState from '../lib/components/EmptyState.svelte';

  let certs = $state<Certificate[]>([]);
  let loading = $state(true);
  let open = $state(false);
  let cfConfigured = $state(false);
  let cfToken = $state('');
  let form = $state({
    mode: 'acme' as 'acme' | 'manual',
    name: '',
    sni: '',
    cert_pem: '',
    key_pem: '',
    challenge: 'tls-alpn-01' as 'dns-01' | 'tls-alpn-01',
    directory: 'production',
    email: '',
  });

  const csv = (s: string) => s.split(',').map((x) => x.trim()).filter(Boolean);

  async function load() {
    loading = true;
    try {
      const [certificates, cloudflare] = await Promise.all([
        api.listCertificates(),
        api.getCloudflareTokenStatus(),
      ]);
      certs = certificates;
      cfConfigured = cloudflare.configured;
    } catch (e) {
      toast((e as ApiError).message, 'err');
    } finally {
      loading = false;
    }
  }

  function openNew() {
    form = {
      mode: 'acme',
      name: '',
      sni: '',
      cert_pem: '',
      key_pem: '',
      challenge: 'tls-alpn-01',
      directory: 'production',
      email: '',
    };
    open = true;
  }

  async function save() {
    try {
      const base = { name: form.name, sni: csv(form.sni) };
      await api.createCertificate(
        form.mode === 'acme'
          ? {
              ...base,
              acme_config: {
                directory_url: form.directory,
                challenge: form.challenge,
                email: form.email.trim() || undefined,
              },
            }
          : { ...base, cert_pem: form.cert_pem, key_pem: form.key_pem },
      );
      toast(form.mode === 'acme' ? 'Certificate issuance queued.' : 'Certificate added — applied live.', 'ok');
      open = false;
      await load();
    } catch (e) {
      toast((e as ApiError).message, 'err');
    }
  }

  async function renew(c: Certificate) {
    try {
      await api.renewCertificate(c.id);
      toast('Certificate renewal queued.', 'ok');
      await load();
    } catch (e) {
      toast((e as ApiError).message, 'err');
    }
  }

  async function saveCloudflareToken() {
    try {
      await api.setCloudflareToken(cfToken);
      cfToken = '';
      cfConfigured = true;
      toast('Cloudflare API token saved.', 'ok');
    } catch (e) {
      toast((e as ApiError).message, 'err');
    }
  }

  async function clearCloudflareToken() {
    try {
      await api.deleteCloudflareToken();
      cfConfigured = false;
      toast('Cloudflare API token removed.', 'ok');
    } catch (e) {
      toast((e as ApiError).message, 'err');
    }
  }

  async function del(c: Certificate) {
    if (!confirm(`Delete certificate "${c.name}"?`)) return;
    try {
      await api.deleteCertificate(c.id);
      toast('Certificate deleted', 'ok');
      await load();
    } catch (e) {
      toast((e as ApiError).message, 'err');
    }
  }

  onMount(load);
</script>

<div class="head-actions">
  <p class="muted">TLS certificates for the HTTPS listener, selected per request by SNI.</p>
  <button onclick={openNew}>+ Add certificate</button>
</div>

<div role="alert">
  <strong>Note:</strong> Certificates are served by <strong>SNI</strong> — the HTTPS listener picks the
  matching certificate per request (exact or <code>*.wildcard</code>), falling back to the
  active certificate set in <code>Settings</code>. Certificate changes apply live (no restart)
  while HTTPS is running.
</div>

<div class="card provider-card">
  <div>
    <strong>Cloudflare DNS</strong>
    <p class="muted">API token used for DNS-01 challenges. It is stored as a secret and never returned.</p>
  </div>
  <div class="provider-actions">
    <span class:configured={cfConfigured}>{cfConfigured ? 'Configured' : 'Not configured'}</span>
    <input type="password" bind:value={cfToken} placeholder="Cloudflare API token" />
    <button class="small" disabled={!cfToken.trim()} onclick={saveCloudflareToken}>Save token</button>
    {#if cfConfigured}
      <button class="ghost small" data-variant="danger" onclick={clearCloudflareToken}>Remove</button>
    {/if}
  </div>
</div>

<div class="card">
  {#if loading}
    <div class="empty"><span aria-busy="true" data-spinner="small"></span></div>
  {:else if certs.length === 0}
    <EmptyState
      icon="M12 15a4 4 0 1 0 0-8 4 4 0 0 0 0 8zm0 0v6l-2-2-2 2-1-7m10 7-2-2-2 2"
      title="No certificates yet"
      description="Add a PEM certificate + key to serve HTTPS. Certificates are matched per request by SNI and hot-reload without a restart."
    >
      {#snippet action()}
        <button onclick={openNew}>+ Add your first certificate</button>
      {/snippet}
    </EmptyState>
  {:else}
    <div class="table">
      <table>
        <thead><tr><th>Name</th><th>SNI</th><th>Management</th><th>Status</th><th>ID</th><th></th></tr></thead>
        <tbody>
          {#each certs as c (c.id)}
            <tr>
              <td><strong>{c.name}</strong></td>
              <td>
                {#if c.sni.length === 0}<span class="faint">any</span>{/if}
                {#each c.sni as s}<span class="chip">{s}</span>{/each}
              </td>
              <td>{c.acme_config ? 'ACME' : 'Manual'}</td>
              <td>
                {#if c.acme_status}
                  <span class="chip status-{c.acme_status.state}">{c.acme_status.state}</span>
                  {#if c.acme_status.expires_at}
                    <span class="faint">expires {new Date(c.acme_status.expires_at).toLocaleDateString()}</span>
                  {/if}
                  {#if c.acme_status.last_error}
                    <span class="error-detail" title={c.acme_status.last_error}>{c.acme_status.last_error}</span>
                  {/if}
                {:else}
                  <span class="faint">ready</span>
                {/if}
              </td>
              <td class="mono">#{c.id}</td>
              <td class="actions">
                {#if c.acme_config}
                  <button class="ghost small" onclick={() => renew(c)}>Renew</button>
                {/if}
                <button class="ghost small" data-variant="danger" onclick={() => del(c)}>Delete</button>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</div>

<Drawer bind:open title="Add certificate">
  <div class="mode-switch">
    <button class:active={form.mode === 'acme'} class="ghost" onclick={() => (form.mode = 'acme')}>Automatic (ACME)</button>
    <button class:active={form.mode === 'manual'} class="ghost" onclick={() => (form.mode = 'manual')}>Manual PEM</button>
  </div>
  <label data-field>
    Name
    <input bind:value={form.name} placeholder="wildcard-2026" />
  </label>
  <label data-field>
    SNI hostnames <span class="faint">(comma-separated)</span>
    <input bind:value={form.sni} placeholder="example.com, www.example.com" />
  </label>
  {#if form.mode === 'acme'}
    <label data-field>
      Challenge
      <select bind:value={form.challenge}>
        <option value="tls-alpn-01">TLS-ALPN-01</option>
        <option value="dns-01">DNS-01 (Cloudflare)</option>
      </select>
      <span data-hint>Wildcard names require DNS-01. TLS-ALPN requires public port 443.</span>
    </label>
    <label data-field>
      ACME directory
      <select bind:value={form.directory}>
        <option value="production">Let's Encrypt production</option>
        <option value="staging">Let's Encrypt staging</option>
      </select>
    </label>
    <label data-field>
      Contact email <span class="faint">(optional)</span>
      <input type="email" bind:value={form.email} placeholder="ops@example.com" />
    </label>
  {:else}
    <label data-field>
      Certificate (PEM)
      <textarea rows="6" bind:value={form.cert_pem} placeholder="-----BEGIN CERTIFICATE-----"></textarea>
    </label>
    <label data-field>
      Private key (PEM)
      <textarea rows="6" bind:value={form.key_pem} placeholder="-----BEGIN PRIVATE KEY-----"></textarea>
      <span data-hint>Stored securely; never returned by the API.</span>
    </label>
  {/if}

  {#snippet footer()}
    <button class="ghost" onclick={() => (open = false)}>Cancel</button>
    <button onclick={save}>Add</button>
  {/snippet}
</Drawer>

<style>
  [role='alert'] {
    margin-bottom: 16px;
    font-size: 13px;
  }

  .provider-card,
  .provider-actions,
  .mode-switch {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .provider-card {
    justify-content: space-between;
    margin-bottom: 16px;
  }

  .provider-card p {
    margin: 4px 0 0;
  }

  .provider-actions input {
    width: 230px;
  }

  .configured,
  .status-issued {
    color: var(--success, #2f9e62);
  }

  .status-failed,
  .error-detail {
    color: var(--danger, #c84d4d);
  }

  .error-detail {
    display: block;
    max-width: 240px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .mode-switch {
    margin-bottom: 16px;
  }

  .mode-switch .active {
    border-color: currentColor;
  }

  @media (max-width: 760px) {
    .provider-card,
    .provider-actions {
      align-items: stretch;
      flex-direction: column;
    }

    .provider-actions input {
      width: auto;
    }
  }
</style>
