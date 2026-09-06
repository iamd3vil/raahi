<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { api } from '../lib/api';
  import { applicationDraft as form, go, hasRole } from '../lib/state.svelte';
  import type { ApplicationCreated, ApplicationSpec, Certificate, Settings, UpstreamProbe } from '../lib/types';

  let certificates = $state<Certificate[]>([]);
  let settings = $state<Settings | null>(null);
  let loading = $state(true);
  let loadError = $state('');
  let error = $state('');
  let step = $state<'setup' | 'review'>('setup');
  let saving = $state(false);
  let testing = $state(false);
  let probe = $state<UpstreamProbe | null>(null);
  let probedUrl = $state('');
  let created = $state<ApplicationCreated | null>(null);
  let heading = $state<HTMLHeadingElement>();

  const domain = $derived(form.domain.trim().replace(/\.$/, '').toLowerCase());
  const matching = $derived(certificates.filter(c => c.cert_pem && c.sni.some(pattern => {
    const p = pattern.toLowerCase();
    return p === domain || (p.startsWith('*.') && domain.endsWith(p.slice(1)) && domain !== p.slice(2) && domain.split('.').length === p.split('.').length);
  })));
  const tlsReady = $derived(!!settings?.proxy_https_addr && matching.length > 0);
  const visibleProbe = $derived(probedUrl === form.upstream_url.trim() ? probe : null);
  const networks = $derived(form.networks.split(/[\s,]+/).filter(Boolean));
  const publicUrl = $derived(`${form.https ? 'https' : 'http'}://${domain}`);

  async function load() {
    loading = true; loadError = '';
    try { [certificates, settings] = await Promise.all([api.listCertificates(), api.getSettings()]); }
    catch (e) { loadError = (e as Error).message; }
    finally { loading = false; }
  }
  onMount(load);

  function upstreamError() {
    try {
      const url = new URL(form.upstream_url.trim());
      if (!['http:', 'https:'].includes(url.protocol) || !url.hostname || url.port === '0') return 'Use an HTTP or HTTPS URL with a valid port.';
      if (url.username || url.password || url.pathname !== '/' || url.search || url.hash) return 'Use an upstream URL without credentials, a path, query, or fragment.';
    } catch { return 'Enter a complete URL, such as http://127.0.0.1:3000.'; }
    return '';
  }
  function validate() {
    if (!form.name.trim()) return 'Give this application a name.';
    if (!/^(?=.{1,253}$)[a-z0-9](?:[a-z0-9-]*[a-z0-9])?(?:\.[a-z0-9](?:[a-z0-9-]*[a-z0-9])?)+$/.test(domain)) return 'Enter a domain without a scheme, port, path, or wildcard.';
    const upstream = upstreamError();
    if (upstream) return upstream;
    if (form.https && !tlsReady) return 'HTTPS needs an enabled listener and a matching certificate. Configure them first, or select HTTP.';
    if (form.access === 'restricted' && !networks.length) return 'Enter at least one allowed IP address or network.';
    return '';
  }
  async function focusHeading() { await tick(); heading?.focus(); }
  async function review(event: SubmitEvent) {
    event.preventDefault();
    error = validate();
    if (!error) { step = 'review'; await focusHeading(); }
  }
  async function testConnection() {
    error = upstreamError();
    if (error) return;
    const url = form.upstream_url.trim();
    testing = true; probe = null;
    try { const result = await api.testUpstream(url); probedUrl = url; probe = result; }
    catch (e) { error = (e as Error).message; }
    finally { testing = false; }
  }
  async function create() {
    if (saving) return;
    error = validate();
    if (error) return;
    const spec: ApplicationSpec = {
      name: form.name.trim(), domain, upstream_url: form.upstream_url.trim(),
      https: form.https, hsts: form.https && form.hsts,
      allowed_cidrs: form.access === 'restricted' ? networks : [],
    };
    saving = true;
    try {
      created = await api.createApplication(spec);
      Object.assign(form, { name: '', domain: '', upstream_url: '', https: true, hsts: false, access: 'public', networks: '' });
      await focusHeading();
    } catch (e) { error = (e as Error).message; }
    finally { saving = false; }
  }
</script>

<div class="application">
  {#if !hasRole('editor')}
    <div class="card notice">You need the editor or admin role to add an application.</div>
  {:else if created}
    <section class="card success">
      <span class="success-mark" aria-hidden="true">✓</span>
      <h2 bind:this={heading} tabindex="-1">{created.name} is connected</h2>
      <p>Raahi is ready to route requests to your application.</p>
      <a class="application-url" href={created.url} target="_blank" rel="noopener noreferrer">{created.url} ↗</a>
      <p class="muted">Point this domain’s DNS record at your Raahi server if you haven’t already.</p>
      <div class="success-actions">
        <button onclick={() => go('routes')}>View routes</button>
        <button class="outline" onclick={() => go('services')}>View services</button>
        <button class="ghost" onclick={() => { created = null; step = 'setup'; error = ''; }}>Add another</button>
      </div>
    </section>
  {:else}
    <div class="steps" aria-label="Application setup progress">
      <span class:current={step === 'setup'} aria-current={step === 'setup' ? 'step' : undefined}>1 <span>Set up</span></span>
      <span class="step-line" aria-hidden="true"></span>
      <span class:current={step === 'review'} aria-current={step === 'review' ? 'step' : undefined}>2 <span>Review</span></span>
    </div>
    {#if loadError}
      <div class="card notice" role="alert">Could not load HTTPS settings: {loadError} <button class="outline small" onclick={load}>Retry</button></div>
    {:else if loading}
      <div class="card notice" role="status"><span aria-busy="true" data-spinner="small"></span> Loading application settings…</div>
    {:else}
      <form class="card setup" onsubmit={review}>
        <h2 bind:this={heading} tabindex="-1">{step === 'setup' ? 'Where should requests go?' : 'Ready to add your application?'}</h2>
        {#if error}<div class="form-error" role="alert">{error}</div>{/if}
        {#if step === 'setup'}
          <div class="row">
            <label data-field>Application name<input required maxlength="100" bind:value={form.name} placeholder="Photos" autocomplete="off" /></label>
            <label data-field>Domain<input required bind:value={form.domain} placeholder="photos.example.com" autocomplete="off" autocapitalize="none" spellcheck="false" /></label>
          </div>
          <label data-field>Upstream URL
            <input aria-label="Upstream URL" aria-describedby="application-upstream-hint" type="url" required bind:value={form.upstream_url} placeholder="http://127.0.0.1:3000" autocomplete="off" autocapitalize="none" spellcheck="false" />
            <span id="application-upstream-hint" data-hint>The address Raahi can reach. Use a hostname or an IPv4/IPv6 address, with a port if needed.</span>
          </label>
          <div class="probe">
            <button type="button" class="outline small" onclick={testConnection} disabled={testing || !form.upstream_url.trim()}>
              {#if testing}<span aria-busy="true" data-spinner="small"></span>{/if}
              {testing ? 'Testing…' : 'Test connection'}
            </button>
            <span class="muted">Optional · sends a GET request from Raahi</span>
          </div>
          <div aria-live="polite">
            {#if visibleProbe}<p class="probe-result" class:failed={!visibleProbe.reachable}>{visibleProbe.message}{visibleProbe.reachable ? ` · ${visibleProbe.latency_ms}ms` : ''}</p>{/if}
          </div>
          <hr />
          <h3>HTTPS</h3>
          <label class="toggle"><input type="checkbox" role="switch" bind:checked={form.https} /> Redirect HTTP requests to HTTPS</label>
          {#if form.https}
            {#if !settings?.proxy_https_addr}
              <div class="hint-box">The HTTPS listener is disabled. <button type="button" class="ghost small" onclick={() => go('settings')}>Open Settings</button></div>
            {:else if matching.length}
              <p class="muted">Uses the configured certificate matching this domain: {matching.map(c => c.name).join(', ')}.</p>
            {:else}
              <div class="hint-box">{domain ? `No issued certificate matches ${domain}.` : 'Enter a domain to check its certificate.'}
                <button type="button" class="ghost small" onclick={() => go('certificates')}>Manage certificates</button>
                <span class="muted">Your draft stays here while you configure HTTPS.</span>
              </div>
            {/if}
            <label class="toggle"><input type="checkbox" bind:checked={form.hsts} /> Remember HTTPS in browsers (HSTS)</label>
            <p class="muted detail">When enabled, browsers require HTTPS for this domain for one year. Subdomains are excluded.</p>
          {:else}
            <p class="muted">HTTP will remain available. Existing certificates can still serve HTTPS for this domain.</p>
          {/if}
          <hr />
          <label data-field>Access
            <select aria-label="Access" bind:value={form.access}><option value="public">No additional restriction</option><option value="restricted">Allow specific IPs or networks</option></select>
          </label>
          {#if form.access === 'restricted'}
            <label data-field>Allowed IP addresses or networks<textarea aria-label="Allowed IP addresses or networks" required rows="3" bind:value={form.networks} placeholder={'100.64.0.0/10\n192.168.1.0/24'}></textarea><span data-hint>Separate entries with commas or newlines. All other client IPs will be denied.</span></label>
          {/if}
          <p class="muted detail">Existing global policies and your application’s own sign-in still apply.</p>
        {:else}
          <dl class="review">
            <dt>Application</dt><dd>{form.name.trim()}</dd>
            <dt>Public address</dt><dd class="mono">{publicUrl}</dd>
            <dt>Upstream</dt><dd class="mono">{form.upstream_url.trim()}</dd>
            <dt>Connection test</dt><dd>{visibleProbe?.message ?? 'Not tested — you can still add this application.'}</dd>
            <dt>HTTPS redirect</dt><dd>{form.https ? 'Enabled' : 'Disabled'}</dd>
            {#if form.https}<dt>HSTS</dt><dd>{form.hsts ? 'One year, this domain only' : 'Disabled'}</dd>{/if}
            <dt>Access</dt><dd>{form.access === 'restricted' ? networks.join(', ') : 'No additional restriction'}</dd>
          </dl>
          <div class="hint-box">Requests to this domain will keep their original paths and Host header. Point the domain’s DNS record at your Raahi server; this form does not create DNS records.</div>
          <p class="muted detail">You can adjust routing, upstreams, and access policies later in Routes, Services, and Plugins.</p>
        {/if}
        <div class="footer">
          {#if step === 'setup'}
            <button type="button" class="ghost" onclick={() => go('routes')}>Cancel</button>
            <button type="submit">Review application →</button>
          {:else}
            <button type="button" class="ghost" disabled={saving} onclick={async () => { step = 'setup'; error = ''; await focusHeading(); }}>← Back</button>
            <button type="button" onclick={create} disabled={saving}>
              {#if saving}<span aria-busy="true" data-spinner="small"></span>{/if}
              {saving ? 'Adding application…' : 'Add application'}
            </button>
          {/if}
        </div>
      </form>
    {/if}
  {/if}
</div>

<style>
  .application { max-width: 760px; margin: 0 auto; }
  .steps { display: flex; align-items: center; gap: 16px; margin-bottom: 22px; color: var(--faint-foreground); font-size: 13px; }
  .steps > span:not(.step-line) { display: flex; gap: 8px; align-items: center; }
  .steps .current { color: var(--foreground); font-weight: 650; }
  .step-line { width: 40px; height: 1px; background: var(--border); }
  .setup, .notice, .success { padding: 28px; }
  h2 { font-size: 19px; margin: 0 0 24px; }
  h3 { font-size: 14px; margin-bottom: 12px; }
  .row { display: grid; grid-template-columns: 1fr 1fr; gap: 18px; }
  .toggle { display: flex; gap: 10px; align-items: center; margin: 12px 0; }
  .toggle input { flex-shrink: 0; }
  .probe, .success-actions { display: flex; align-items: center; flex-wrap: wrap; gap: 12px; }
  .probe .muted, .detail { font-size: 12px; }
  .probe-result { font-size: 13px; color: var(--foreground); }
  .probe-result.failed, .form-error { color: var(--danger, #fb7185); }
  .form-error { margin-bottom: 18px; padding: 12px; border: 1px solid currentColor; border-radius: var(--radius-medium); }
  .hint-box { padding: 14px; background: var(--muted); border: 1px solid var(--border); border-radius: var(--radius-medium); font-size: 13px; line-height: 1.6; overflow-wrap: anywhere; }
  .hint-box .muted { display: block; }
  .review { display: grid; grid-template-columns: 140px 1fr; gap: 18px; margin-bottom: 24px; font-size: 14px; }
  .review dt { color: var(--faint-foreground); }
  .review dd { margin: 0; overflow-wrap: anywhere; }
  .footer { display: flex; justify-content: space-between; gap: 12px; border-top: 1px solid var(--border); padding-top: 20px; margin-top: 26px; }
  .success { text-align: center; }
  .success-mark { display: inline-grid; place-items: center; width: 44px; height: 44px; border-radius: 50%; background: var(--muted); color: var(--primary); font-size: 24px; margin-bottom: 20px; }
  .success h2 { margin-bottom: 12px; }
  .application-url { font-size: 20px; overflow-wrap: anywhere; }
  .success-actions { justify-content: center; margin-top: 26px; }
  @media (max-width: 600px) { .setup, .notice, .success { padding: 18px; } .row { grid-template-columns: 1fr; gap: 0; } .review { grid-template-columns: 1fr; gap: 8px; } .review dd { margin-bottom: 10px; } }
</style>
