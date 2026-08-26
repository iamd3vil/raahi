<script lang="ts">
  import { onMount } from 'svelte';
  import { api, ApiError, setAdminToken } from '../lib/api';
  import type { AuthStatus, Certificate, ConfigSummary, LbAlgorithm, Role, Settings, SsoConfigView } from '../lib/types';
  import { toast, ui, hasRole } from '../lib/state.svelte';

  let settings = $state<Settings | null>(null);
  let certs = $state<Certificate[]>([]);
  let summary = $state<ConfigSummary | null>(null);
  let loading = $state(true);
  let exporting = $state(false);
  let auth = $state<AuthStatus | null>(null);
  const authEnabled = $derived(auth?.auth_enabled ?? false);
  const tokenEnabled = $derived(auth?.token_enabled ?? false);
  let freshToken = $state('');

  async function loadAuth() {
    try {
      auth = await api.adminStatus();
    } catch {
      /* ignore */
    }
  }

  async function generateToken() {
    if (
      tokenEnabled &&
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

  async function disableToken() {
    const warn = auth?.users_exist
      ? 'Disable the admin token? Automation using it stops working; user sign-in is unaffected.'
      : 'Disable the admin token? With no users, anyone who can reach the admin port gets full access.';
    if (!confirm(warn)) return;
    try {
      await api.deleteAdminToken();
      setAdminToken(null);
      freshToken = '';
      await loadAuth();
      toast('Admin token disabled', 'ok');
    } catch (e) {
      toast((e as ApiError).message, 'err');
    }
  }

  // ---- SSO (OpenID Connect) ----
  let sso = $state<SsoConfigView | null>(null);
  let ssoForm = $state({
    issuer: '',
    client_id: '',
    client_secret: '',
    label: '',
    auto_provision: false,
    auto_provision_role: 'viewer' as Role,
    allowed_domains: '',
  });
  let ssoSaving = $state(false);
  const redirectUri = `${location.origin}/api/v1/auth/sso/callback`;

  async function loadSso() {
    try {
      const r = await api.getSsoConfig();
      sso = r.config;
      ssoForm = {
        issuer: sso?.issuer ?? '',
        client_id: sso?.client_id ?? '',
        client_secret: '',
        label: sso?.label ?? '',
        auto_provision: !!sso?.auto_provision_role,
        auto_provision_role: sso?.auto_provision_role ?? 'viewer',
        allowed_domains: (sso?.allowed_domains ?? []).join(', '),
      };
    } catch {
      /* non-admins get 403; the card is hidden for them */
    }
  }

  async function saveSso() {
    ssoSaving = true;
    try {
      await api.setSsoConfig({
        issuer: ssoForm.issuer,
        client_id: ssoForm.client_id,
        client_secret: ssoForm.client_secret || undefined,
        label: ssoForm.label,
        auto_provision_role: ssoForm.auto_provision ? ssoForm.auto_provision_role : null,
        allowed_domains: ssoForm.allowed_domains.split(',').map((d) => d.trim()).filter(Boolean),
      });
      toast('SSO configuration saved', 'ok');
      await Promise.all([loadSso(), loadAuth()]);
    } catch (e) {
      toast((e as ApiError).message, 'err');
    } finally {
      ssoSaving = false;
    }
  }

  async function disableSso() {
    if (!confirm('Disable SSO? SSO-only users will no longer be able to sign in.')) return;
    try {
      await api.deleteSsoConfig();
      toast('SSO disabled', 'ok');
      await Promise.all([loadSso(), loadAuth()]);
    } catch (e) {
      toast((e as ApiError).message, 'err');
    }
  }

  function copyRedirect() {
    navigator.clipboard?.writeText(redirectUri);
    toast('Redirect URI copied', 'ok');
  }

  // ---- own password ----
  let pw = $state({ current: '', next: '', confirm: '' });
  async function changePassword() {
    if (pw.next !== pw.confirm) {
      toast('New passwords do not match', 'err');
      return;
    }
    try {
      await api.changePassword(pw.current, pw.next);
      pw = { current: '', next: '', confirm: '' };
      toast('Password updated', 'ok');
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
    if (hasRole('admin')) loadSso();
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
            Sign-in is required. Users get roles (viewer / editor / admin); the admin token is a separate
            full-access credential for automation.
          {:else}
            The admin API is unauthenticated (loopback binding is the only protection). Create a user under
            <strong>Users</strong> or generate an admin token to require sign-in.
          {/if}
        </p>
        {#if hasRole('admin')}
          <h3 class="sub">Admin token</h3>
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
            <button class="outline" onclick={generateToken}>{tokenEnabled ? 'Rotate token' : 'Generate token'}</button>
            {#if tokenEnabled}
              <button class="ghost" data-variant="danger" onclick={disableToken}>Disable token</button>
            {/if}
          </div>
          <p class="hint" style="margin-top:8px">
            Send it as <code>Authorization: Bearer …</code> or <code>X-Admin-Token</code>. Token callers act as admin.
          </p>
        {/if}
      </div>

      {#if hasRole('admin')}
        <div class="card" style="margin-top:16px">
          <div class="panel-head">
            <h2>Single sign-on</h2>
            <span class="badge" data-variant={sso ? 'success' : undefined}>{sso ? 'enabled' : 'off'}</span>
          </div>
          <p class="muted" style="margin:0 0 12px">
            OpenID Connect authorization-code flow with PKCE. Works with Google, Keycloak, Authentik, Pocket ID,
            Auth0, Zitadel, and any compliant provider. Register this redirect URI with the provider:
          </p>
          <div class="token-row" style="margin-bottom:12px">
            <code class="token">{redirectUri}</code>
            <button class="outline small" onclick={copyRedirect}>Copy</button>
          </div>
          <label data-field>
            Issuer URL
            <input class="mono" bind:value={ssoForm.issuer} placeholder="https://accounts.google.com" />
            <span data-hint>Discovery is read from <code>/.well-known/openid-configuration</code>.</span>
          </label>
          <div class="row">
            <label data-field>
              Client ID
              <input class="mono" bind:value={ssoForm.client_id} />
            </label>
            <label data-field>
              Client secret
              <input class="mono" type="password" autocomplete="off" bind:value={ssoForm.client_secret}
                placeholder={sso?.client_secret_set ? '•••••••• (stored; blank keeps it)' : ''} />
            </label>
          </div>
          <label data-field>
            Button label <span class="faint">(optional)</span>
            <input bind:value={ssoForm.label} placeholder="Google" />
          </label>
          <label style="margin-top:6px">
            <input type="checkbox" role="switch" bind:checked={ssoForm.auto_provision} />
            Create accounts on first sign-in
          </label>
          {#if ssoForm.auto_provision}
            <div class="row" style="margin-top:8px">
              <label data-field>
                Role for new accounts
                <select bind:value={ssoForm.auto_provision_role}>
                  <option value="viewer">viewer</option>
                  <option value="editor">editor</option>
                  <option value="admin">admin</option>
                </select>
              </label>
              <label data-field>
                Allowed email domains
                <input class="mono" bind:value={ssoForm.allowed_domains} placeholder="example.com, corp.example" />
                <span data-hint>Comma-separated. Empty allows any domain — avoid that with public providers.</span>
              </label>
            </div>
          {:else}
            <p class="hint" style="margin-top:6px">Only users already listed under <strong>Users</strong> can sign in via SSO.</p>
          {/if}
          <div class="flex" style="gap:8px; margin-top:10px">
            <button onclick={saveSso} disabled={ssoSaving || !ssoForm.issuer || !ssoForm.client_id}>
              {sso ? 'Save SSO settings' : 'Enable SSO'}
            </button>
            {#if sso}
              <button class="ghost" data-variant="danger" onclick={disableSso}>Disable SSO</button>
            {/if}
          </div>
        </div>
      {/if}

      {#if ui.me?.user}
        <div class="card" style="margin-top:16px">
          <div class="panel-head"><h2>Your password</h2></div>
          {#if ui.me.user.has_password}
            <label data-field>
              Current password
              <input type="password" autocomplete="current-password" bind:value={pw.current} />
            </label>
          {:else}
            <p class="hint" style="margin:0 0 8px">Your account is SSO-only. Set a password to also sign in with email.</p>
          {/if}
          <div class="row">
            <label data-field>
              New password
              <input type="password" autocomplete="new-password" bind:value={pw.next} />
            </label>
            <label data-field>
              Confirm
              <input type="password" autocomplete="new-password" bind:value={pw.confirm} />
            </label>
          </div>
          <div style="margin-top:8px">
            <button class="outline" onclick={changePassword} disabled={pw.next.length < 8 || pw.next !== pw.confirm}>
              Update password
            </button>
          </div>
        </div>
      {/if}

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
