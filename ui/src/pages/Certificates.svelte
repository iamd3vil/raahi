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

<div class="page-intro">
  <div>
    <p class="eyebrow">HTTPS security</p>
    <p class="muted">Issue, upload, and renew the certificates served by Raahi.</p>
  </div>
  <button class="primary-action" onclick={openNew}>
    <svg viewBox="0 0 20 20" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" aria-hidden="true">
      <path d="M10 4v12M4 10h12" />
    </svg>
    Add certificate
  </button>
</div>

<aside class="sni-note" aria-labelledby="sni-note-title">
  <span class="note-icon" aria-hidden="true">
    <svg viewBox="0 0 20 20" width="17" height="17" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round">
      <circle cx="10" cy="10" r="7.25" />
      <path d="M10 9v4M10 6.5v.1" />
    </svg>
  </span>
  <div>
    <strong id="sni-note-title">Automatic SNI matching</strong>
    <p>
      Exact and <code>*.wildcard</code> hostnames are matched per request. If none match, Raahi uses
      the default certificate from <code>Settings</code>. Changes apply immediately while HTTPS is running.
    </p>
  </div>
</aside>

<section class="card provider-card" aria-labelledby="cloudflare-title">
  <div class="provider-copy">
    <span class="provider-icon" aria-hidden="true">
      <svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
        <path d="M7.5 18.5h10.8a3.2 3.2 0 0 0 .5-6.4A6.8 6.8 0 0 0 5.6 10a4.3 4.3 0 0 0 1.9 8.5Z" />
        <path d="M8.5 14.5h7" />
      </svg>
    </span>
    <div>
      <div class="provider-heading">
        <h2 id="cloudflare-title">Cloudflare DNS</h2>
        <span class="provider-status" class:configured={cfConfigured}>
          <span class="status-dot"></span>
          {cfConfigured ? 'Connected' : 'Not configured'}
        </span>
      </div>
      <p class="muted">Used for DNS-01 challenges and wildcard certificates. The token is encrypted and never returned.</p>
    </div>
  </div>
  <form class="provider-form" onsubmit={(event) => { event.preventDefault(); saveCloudflareToken(); }}>
    <label for="cloudflare-token">{cfConfigured ? 'Replace API token' : 'API token'}</label>
    <div class="token-control">
      <input
        id="cloudflare-token"
        type="password"
        autocomplete="off"
        bind:value={cfToken}
        placeholder={cfConfigured ? 'Enter a new token' : 'Cloudflare API token'}
      />
      <button class="small" type="submit" disabled={!cfToken.trim()}>{cfConfigured ? 'Replace' : 'Save token'}</button>
      {#if cfConfigured}
        <button class="ghost small remove-token" type="button" data-variant="danger" onclick={clearCloudflareToken}>Remove</button>
      {/if}
    </div>
  </form>
</section>

<section class="card certificate-card" aria-labelledby="certificate-list-title">
  <div class="certificate-card-head">
    <div>
      <div class="title-row">
        <h2 id="certificate-list-title">Certificates</h2>
        {#if !loading}<span class="count-badge">{certs.length}</span>{/if}
      </div>
      <p class="muted">Certificates currently available to the HTTPS listener.</p>
    </div>
  </div>
  {#if loading}
    <div class="empty"><span aria-busy="true" data-spinner="small"></span></div>
  {:else if certs.length === 0}
    <EmptyState
      icon="M12 15a4 4 0 1 0 0-8 4 4 0 0 0 0 8zm0 0v6l-2-2-2 2-1-7m10 7-2-2-2 2"
      title="No certificates yet"
      description="Add a PEM certificate + key to serve HTTPS. Certificates are matched per request by SNI and hot-reload without a restart."
    >
      {#snippet action()}
        <button onclick={openNew}>Add your first certificate</button>
      {/snippet}
    </EmptyState>
  {:else}
    <div class="table certificate-table">
      <table>
        <thead>
          <tr><th>Name</th><th>Hostnames</th><th>Managed by</th><th>Status</th><th>ID</th><th><span class="sr-only">Actions</span></th></tr>
        </thead>
        <tbody>
          {#each certs as c (c.id)}
            <tr>
              <td class="name-cell"><strong>{c.name}</strong></td>
              <td class="sni-cell" data-label="Hostnames">
                <div class="sni-list">
                  {#if c.sni.length === 0}<span class="faint">Any hostname</span>{/if}
                  {#each c.sni as s}<code class="hostname">{s}</code>{/each}
                </div>
              </td>
              <td class="management-cell" data-label="Managed by">
                <span class="management">{c.acme_config ? providerLabel(c.acme_config.directory_url) : 'Manual upload'}</span>
              </td>
              <td class="status-cell" data-label="Status">
                <div class="status-stack">
                  {#if c.acme_status}
                    <span class="certificate-status status-{c.acme_status.state}">
                      <span class="status-dot"></span>{c.acme_status.state}
                    </span>
                    {#if c.acme_status.expires_at}
                      <span class="expiry">Expires {new Date(c.acme_status.expires_at).toLocaleDateString()}</span>
                    {/if}
                    {#if c.acme_status.last_error}
                      <span class="error-detail" title={c.acme_status.last_error}>{c.acme_status.last_error}</span>
                    {/if}
                  {:else}
                    <span class="certificate-status status-ready"><span class="status-dot"></span>ready</span>
                  {/if}
                </div>
              </td>
              <td class="mono id-cell" data-label="ID">#{c.id}</td>
              <td class="actions row-actions">
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
</section>

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
  .page-intro {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 20px;
    margin-bottom: 16px;
  }

  .page-intro p {
    margin: 0;
  }

  .page-intro .muted {
    font-size: 13.5px;
  }

  .eyebrow {
    margin-bottom: 3px !important;
    color: var(--faint-foreground);
    font-size: 10.5px;
    font-weight: 650;
    letter-spacing: 0.09em;
    text-transform: uppercase;
  }

  .primary-action svg {
    flex: none;
  }

  .primary-action:active {
    transform: scale(0.98);
  }

  .sni-note {
    display: flex;
    align-items: flex-start;
    gap: 11px;
    margin-bottom: 16px;
    padding: 13px 15px;
    border: 1px solid color-mix(in srgb, var(--primary) 22%, var(--border));
    border-radius: var(--radius-medium);
    background: color-mix(in srgb, var(--primary) 5%, var(--card));
    font-size: 12.5px;
  }

  .sni-note strong {
    display: block;
    margin-bottom: 2px;
    color: var(--foreground);
    font-size: 12.5px;
  }

  .sni-note p {
    margin: 0;
    color: var(--muted-foreground);
    line-height: 1.55;
  }

  .note-icon {
    display: grid;
    flex: none;
    width: 28px;
    height: 28px;
    place-items: center;
    border-radius: 50%;
    background: var(--accent-soft);
    color: var(--primary);
  }

  .provider-card {
    display: grid;
    grid-template-columns: minmax(280px, 1fr) minmax(420px, 580px);
    align-items: center;
    gap: 32px;
    margin-bottom: 16px;
  }

  .provider-copy {
    display: flex;
    align-items: flex-start;
    gap: 12px;
    min-width: 0;
  }

  .provider-copy h2,
  .certificate-card h2 {
    margin: 0;
    font-size: 14px;
    font-weight: var(--font-semibold);
  }

  .provider-copy p,
  .certificate-card-head p {
    margin: 4px 0 0;
    font-size: 12.5px;
    line-height: 1.5;
  }

  .provider-icon {
    display: grid;
    flex: none;
    width: 36px;
    height: 36px;
    place-items: center;
    border: 1px solid var(--border);
    border-radius: var(--radius-medium);
    background: var(--muted);
    color: var(--muted-foreground);
  }

  .provider-heading,
  .title-row {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px;
  }

  .provider-status,
  .certificate-status {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    width: max-content;
    color: var(--faint-foreground);
    font-size: 11.5px;
    font-weight: 550;
  }

  .status-dot {
    width: 6px;
    height: 6px;
    flex: none;
    border-radius: 50%;
    background: currentColor;
    box-shadow: 0 0 0 3px color-mix(in srgb, currentColor 12%, transparent);
  }

  .configured,
  .status-issued,
  .status-ready {
    color: var(--success);
  }

  .status-failed,
  .error-detail {
    color: var(--danger);
  }

  .status-pending,
  .status-issuing,
  .status-renewing {
    color: var(--warning);
  }

  .provider-form {
    min-width: 0;
  }

  .provider-form > label {
    display: block;
    margin-bottom: 6px;
    color: var(--muted-foreground);
    font-size: 11.5px;
    font-weight: 550;
  }

  .token-control {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .token-control input {
    min-width: 160px;
    flex: 1;
  }

  .remove-token {
    padding-inline: 9px;
  }

  .certificate-card {
    padding-bottom: 10px;
  }

  .certificate-card-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    padding-bottom: 14px;
    border-bottom: 1px solid var(--border);
  }

  .count-badge {
    min-width: 20px;
    padding: 1px 6px;
    border: 1px solid var(--border);
    border-radius: 999px;
    background: var(--muted);
    color: var(--muted-foreground);
    font-size: 11px;
    font-variant-numeric: tabular-nums;
    text-align: center;
  }

  .certificate-table {
    margin-top: 2px;
  }

  .certificate-table th {
    padding-block: 12px;
    color: var(--faint-foreground);
    font-size: 10.5px;
    font-weight: 650;
    letter-spacing: 0.055em;
    text-transform: uppercase;
  }

  .certificate-table td {
    padding-block: 14px;
    vertical-align: middle;
  }

  .certificate-table th:first-child,
  .certificate-table td:first-child {
    padding-left: 8px;
  }

  .certificate-table th:last-child,
  .certificate-table td:last-child {
    padding-right: 8px;
  }

  .certificate-table tbody tr:last-child {
    border-bottom: none;
  }

  .name-cell strong {
    font-size: 13.5px;
    font-weight: 600;
  }

  .sni-list {
    display: flex;
    flex-wrap: wrap;
    gap: 5px;
  }

  .hostname {
    display: inline-flex;
    padding: 3px 7px;
    border: 1px solid var(--border);
    border-radius: 5px;
    background: var(--muted);
    color: var(--muted-foreground);
    font-size: 11.5px;
    line-height: 1.25;
  }

  .management {
    color: var(--muted-foreground);
    font-size: 12.5px;
  }

  .status-stack {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 3px;
  }

  .certificate-status {
    text-transform: capitalize;
  }

  .expiry {
    color: var(--faint-foreground);
    font-size: 11.5px;
    white-space: nowrap;
  }

  .error-detail {
    display: block;
    max-width: 220px;
    overflow: hidden;
    font-size: 11.5px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .id-cell {
    color: var(--faint-foreground);
    font-size: 12px;
  }

  .row-actions {
    min-width: 132px;
  }

  .form-error {
    color: var(--danger);
    font-size: 13px;
  }

  .eab-card {
    padding: 14px;
    border: 1px solid var(--border);
    border-radius: var(--radius-medium);
    margin: 16px 0;
  }

  .eab-card p {
    margin-top: 8px;
    font-size: 12px;
  }

  .eab-actions,
  .mode-switch {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px;
  }

  .mode-switch {
    width: fit-content;
    margin-bottom: 16px;
    padding: 3px;
    border: 1px solid var(--border);
    border-radius: var(--radius-medium);
    background: var(--muted);
  }

  .mode-switch button {
    border-color: transparent;
  }

  .mode-switch .active {
    border-color: var(--border);
    background: var(--card);
    color: var(--foreground);
    box-shadow: var(--shadow-small);
  }

  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    padding: 0;
    margin: -1px;
    overflow: hidden;
    clip: rect(0, 0, 0, 0);
    white-space: nowrap;
    border: 0;
  }

  @media (max-width: 1050px) {
    .provider-card {
      grid-template-columns: 1fr;
      gap: 18px;
    }

    .provider-form {
      padding-left: 48px;
    }
  }

  @media (max-width: 820px) {
    .certificate-table th:nth-child(5),
    .certificate-table td:nth-child(5) {
      display: none;
    }
  }

  @media (max-width: 760px) {
    .page-intro {
      align-items: flex-start;
    }

    .sni-note {
      padding: 12px;
    }

    .provider-form {
      padding-left: 0;
    }

    .token-control {
      align-items: stretch;
      flex-wrap: wrap;
    }

    .token-control input {
      flex-basis: 100%;
    }

    .certificate-card {
      padding-bottom: 16px;
    }

    .certificate-table {
      min-width: 0;
      margin-top: 14px;
      overflow: visible;
    }

    .certificate-table table,
    .certificate-table tbody {
      display: block;
    }

    .certificate-table thead {
      display: none;
    }

    .certificate-table tbody {
      display: grid;
      gap: 10px;
    }

    .certificate-table tbody tr {
      display: grid;
      grid-template-columns: minmax(0, 1fr) auto;
      gap: 12px;
      padding: 14px;
      border: 1px solid var(--border);
      border-radius: var(--radius-medium);
      background: color-mix(in srgb, var(--muted) 45%, transparent);
    }

    .certificate-table tbody tr:hover {
      background: color-mix(in srgb, var(--muted) 45%, transparent);
    }

    .certificate-table td,
    .certificate-table th:first-child,
    .certificate-table td:first-child,
    .certificate-table th:last-child,
    .certificate-table td:last-child {
      display: block;
      padding: 0;
    }

    .name-cell {
      min-width: 0;
    }

    .id-cell {
      grid-column: 2;
      grid-row: 1;
    }

    .sni-cell,
    .management-cell,
    .status-cell,
    .row-actions {
      grid-column: 1 / -1;
    }

    .certificate-table td[data-label]::before {
      content: attr(data-label);
      display: block;
      margin-bottom: 5px;
      color: var(--faint-foreground);
      font-family: var(--font-sans);
      font-size: 10px;
      font-weight: 650;
      letter-spacing: 0.055em;
      text-transform: uppercase;
    }

    .row-actions {
      display: flex !important;
      justify-content: flex-start;
      padding-top: 10px !important;
      border-top: 1px solid var(--border);
      text-align: left;
    }
  }

  @media (max-width: 480px) {
    .page-intro {
      flex-direction: column;
    }

    .primary-action {
      width: 100%;
    }

    .provider-copy {
      gap: 10px;
    }

    .provider-icon {
      width: 32px;
      height: 32px;
    }
  }
</style>
