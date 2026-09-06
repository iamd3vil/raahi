<script lang="ts">
  import { onMount } from 'svelte';
  import { api, ApiError } from '../lib/api';
  import type { Certificate } from '../lib/types';
  import { toast, hasRole } from '../lib/state.svelte';
  import Drawer from '../lib/components/Drawer.svelte';
  import EmptyState from '../lib/components/EmptyState.svelte';

  let certs = $state<Certificate[]>([]);
  let loading = $state(true);
  let open = $state(false);
  let cfConfigured = $state(false);
  let cfToken = $state('');
  let saving = $state(false);
  let formError = $state('');
  let eabKeyId = $state('');
  let eabHmacKey = $state('');
  let eabBusy = $state(false);
  let accountLoading = $state(false);
  let accountError = $state('');
  let accountStatus = $state<{ configured: boolean; account_registered: boolean } | null>(null);

  let form = $state({
    mode: 'acme' as 'acme' | 'manual',
    name: '',
    sni: '',
    cert_pem: '',
    key_pem: '',
    challenge: 'tls-alpn-01' as 'dns-01' | 'tls-alpn-01',
    provider: 'letsencrypt',
    directory: 'production',
    custom_directory: '',
    email: '',
  });

  const directoryUrl = $derived(form.provider === 'zerossl' ? 'zerossl'
    : form.provider === 'custom' ? form.custom_directory.trim() : form.directory);

  function changeProvider(provider: string) {
    form.provider = provider;
    if (provider === 'zerossl') form.challenge = 'dns-01';
    eabKeyId = ''; eabHmacKey = ''; formError = '';
  }

  function providerLabel(directory: string) {
    if (['staging', 'letsencrypt-staging', 'https://acme-staging-v02.api.letsencrypt.org/directory'].includes(directory)) return "Let's Encrypt (staging)";
    if (['zerossl', 'https://acme.zerossl.com/v2/DV90', 'https://acme.zerossl.com/v2/DV90/'].includes(directory)) return 'ZeroSSL';
    if (['', 'production', 'letsencrypt', 'https://acme-v02.api.letsencrypt.org/directory'].includes(directory)) return "Let's Encrypt";
    return 'Custom ACME';
  }

  $effect(() => {
    const directory = directoryUrl;
    if (!open || form.mode !== 'acme' || form.provider === 'letsencrypt' || !directory) {
      accountStatus = null; accountError = ''; accountLoading = false;
      return;
    }
    let cancelled = false;
    accountStatus = null; accountError = ''; accountLoading = true;
    const timer = setTimeout(async () => {
      try {
        const result = await api.getAcmeAccountStatus(directory);
        if (!cancelled) accountStatus = result;
      } catch (e) { if (!cancelled) accountError = (e as Error).message; }
      finally { if (!cancelled) accountLoading = false; }
    }, 200);
    return () => { cancelled = true; clearTimeout(timer); };
  });

  async function saveEab() {
    if (eabBusy) return;
    const directory = directoryUrl;
    eabBusy = true; formError = '';
    try {
      await api.setAcmeEab({ directory_url: directory, key_id: eabKeyId.trim(), hmac_key: eabHmacKey.trim() });
      eabKeyId = ''; eabHmacKey = '';
      accountStatus = await api.getAcmeAccountStatus(directory);
      accountError = '';
      toast('ACME registration credentials saved.', 'ok');
    } catch (e) { formError = (e as Error).message; }
    finally { eabBusy = false; }
  }

  async function removeEab() {
    eabBusy = true; formError = '';
    try {
      await api.deleteAcmeEab(directoryUrl);
      accountStatus = await api.getAcmeAccountStatus(directoryUrl);
      eabKeyId = ''; eabHmacKey = '';
    } catch (e) { formError = (e as Error).message; }
    finally { eabBusy = false; }
  }

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
    formError = ''; eabKeyId = ''; eabHmacKey = '';
    form = {
      mode: 'acme',
      name: '',
      sni: '',
      cert_pem: '',
      key_pem: '',
      challenge: 'tls-alpn-01',
      provider: 'letsencrypt',
      directory: 'production',
      custom_directory: '',
      email: '',
    };
    open = true;
  }

  async function save() {
    if (saving || eabBusy) return;
    formError = '';
    if (!form.name.trim() || !csv(form.sni).length) { formError = 'Enter a name and at least one SNI hostname.'; return; }
    if (form.mode === 'acme') {
      if (!directoryUrl) { formError = 'Enter your ACME directory URL.'; return; }
      if (form.challenge === 'dns-01' && !cfConfigured) { formError = 'Configure the Cloudflare DNS token before using DNS-01.'; return; }
      if (form.provider === 'zerossl' && !accountStatus?.configured && !accountStatus?.account_registered) {
        formError = 'Save your ZeroSSL EAB credentials before requesting a certificate.'; return;
      }
      if (eabKeyId || eabHmacKey) { formError = 'Save or clear the EAB credentials before requesting a certificate.'; return; }
    }
    saving = true;
    try {
      const base = { name: form.name, sni: csv(form.sni) };
      await api.createCertificate(
        form.mode === 'acme'
          ? {
              ...base,
              acme_config: {
                directory_url: directoryUrl,
                challenge: form.challenge,
                email: form.email.trim() || undefined,
              },
            }
          : { ...base, cert_pem: form.cert_pem, key_pem: form.key_pem },
      );
      toast(form.mode === 'acme' ? 'Certificate issuance queued.' : 'Certificate added — applied live.', 'ok');
      open = false;
      eabKeyId = ''; eabHmacKey = '';
      await load();
    } catch (e) {
      formError = (e as ApiError).message;
    } finally { saving = false; }
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
              <td>{c.acme_config ? providerLabel(c.acme_config.directory_url) : 'Manual'}</td>
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

<Drawer bind:open title="Add certificate" onclose={() => { eabKeyId = ''; eabHmacKey = ''; }}>
  {#if formError}<p class="form-error" role="alert">{formError}</p>{/if}
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
      ACME provider
      <select aria-label="ACME provider" value={form.provider} onchange={(e) => changeProvider(e.currentTarget.value)} disabled={eabBusy || saving}>
        <option value="letsencrypt">Let's Encrypt (default)</option>
        <option value="zerossl">ZeroSSL</option>
        <option value="custom">Custom ACME server</option>
      </select>
    </label>
    {#if form.provider === 'letsencrypt'}
      <label data-field>Environment
        <select aria-label="ACME environment" bind:value={form.directory}>
          <option value="production">Production</option>
          <option value="staging">Staging (test certificates)</option>
        </select>
      </label>
    {:else if form.provider === 'custom'}
      <label data-field>Directory URL
        <input aria-label="ACME directory URL" type="url" bind:value={form.custom_directory} disabled={eabBusy || saving} placeholder="https://ca.example.com/acme/directory" />
        <span data-hint>Use the directory endpoint supplied by your certificate authority.</span>
      </label>
    {/if}
    <label data-field>
      Challenge
      <select aria-label="ACME challenge" bind:value={form.challenge}>
        <option value="tls-alpn-01" disabled={form.provider === 'zerossl'}>TLS-ALPN-01</option>
        <option value="dns-01">DNS-01 (Cloudflare)</option>
      </select>
      <span data-hint>{form.provider === 'zerossl' ? 'ZeroSSL uses DNS-01 in Raahi. Configure Cloudflare DNS before requesting a certificate.' : 'Wildcard names require DNS-01. TLS-ALPN requires public port 443.'}</span>
    </label>
    {#if form.provider !== 'letsencrypt'}
      <section class="eab-card" aria-label="ACME account registration">
        <strong>Account registration</strong>
        {#if form.provider === 'zerossl'}
          <p class="muted">ZeroSSL needs an EAB key ID and HMAC key from its dashboard’s Developer section. These are different from its API access key.</p>
        {:else}
          <p class="muted">If your certificate authority requires external account binding (EAB), enter its registration credentials here.</p>
        {/if}
        <div aria-live="polite">
          {#if accountLoading}<p class="muted">Checking account…</p>
          {:else if accountError}<p class="form-error">{accountError}</p>
          {:else if accountStatus?.account_registered}<p class="configured">An account is already registered. Raahi will reuse it.</p>
          {:else if accountStatus?.configured}<p class="configured">EAB credentials are saved. The first certificate request will register the account.</p>
          {/if}
        </div>
        {#if hasRole('admin')}
          <label data-field>EAB key ID<input aria-label="EAB key ID" bind:value={eabKeyId} autocomplete="off" disabled={eabBusy || saving} /></label>
          <label data-field>EAB HMAC key<input aria-label="EAB HMAC key" type="password" bind:value={eabHmacKey} autocomplete="new-password" disabled={eabBusy || saving} />
            <span data-hint>Base64url-encoded key. Stored as a secret and never returned by ordinary APIs.</span>
          </label>
          <div class="eab-actions">
            <button class="outline small" onclick={saveEab} disabled={eabBusy || saving || !directoryUrl || !eabKeyId.trim() || !eabHmacKey.trim()}>{eabBusy ? 'Saving…' : 'Save credentials'}</button>
            {#if accountStatus?.configured}<button class="ghost small" onclick={removeEab} disabled={eabBusy || saving}>Remove saved credentials</button>{/if}
          </div>
          <p class="muted">Credentials are shared by certificates using this provider. Changing or removing them does not replace an existing account.</p>
        {:else if !accountStatus?.configured && !accountStatus?.account_registered}
          <p class="muted">Ask an admin to configure EAB credentials if this provider requires them.</p>
        {/if}
      </section>
    {/if}
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
    <button class="ghost" disabled={saving || eabBusy} onclick={() => { open = false; eabKeyId = ''; eabHmacKey = ''; }}>Cancel</button>
    <button onclick={save} disabled={saving || eabBusy || (form.mode === 'acme' && form.provider === 'zerossl' && accountLoading)}>{saving ? 'Adding…' : 'Add'}</button>
  {/snippet}
</Drawer>

<style>
  .form-error { color: var(--danger); font-size: 13px; }
  .eab-card { padding: 14px; border: 1px solid var(--border); border-radius: var(--radius-medium); margin: 16px 0; }
  .eab-card p { font-size: 12px; margin-top: 8px; }
  .eab-actions { display: flex; flex-wrap: wrap; gap: 8px; }

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
