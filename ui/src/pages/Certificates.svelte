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
  let form = $state({ name: '', sni: '', cert_pem: '', key_pem: '' });

  const csv = (s: string) => s.split(',').map((x) => x.trim()).filter(Boolean);

  async function load() {
    loading = true;
    try {
      certs = await api.listCertificates();
    } catch (e) {
      toast((e as ApiError).message, 'err');
    } finally {
      loading = false;
    }
  }

  function openNew() {
    form = { name: '', sni: '', cert_pem: '', key_pem: '' };
    open = true;
  }

  async function save() {
    try {
      await api.createCertificate({
        name: form.name,
        sni: csv(form.sni),
        cert_pem: form.cert_pem,
        key_pem: form.key_pem,
      });
      toast('Certificate added — applied live.', 'ok');
      open = false;
      await load();
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
        <thead><tr><th>Name</th><th>SNI</th><th>ID</th><th></th></tr></thead>
        <tbody>
          {#each certs as c (c.id)}
            <tr>
              <td><strong>{c.name}</strong></td>
              <td>
                {#if c.sni.length === 0}<span class="faint">any</span>{/if}
                {#each c.sni as s}<span class="chip">{s}</span>{/each}
              </td>
              <td class="mono">#{c.id}</td>
              <td class="actions">
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
  <label data-field>
    Name
    <input bind:value={form.name} placeholder="wildcard-2026" />
  </label>
  <label data-field>
    SNI hostnames <span class="faint">(comma-separated, optional)</span>
    <input bind:value={form.sni} placeholder="*.example.com" />
  </label>
  <label data-field>
    Certificate (PEM)
    <textarea rows="6" bind:value={form.cert_pem} placeholder="-----BEGIN CERTIFICATE-----"></textarea>
  </label>
  <label data-field>
    Private key (PEM)
    <textarea rows="6" bind:value={form.key_pem} placeholder="-----BEGIN PRIVATE KEY-----"></textarea>
    <span data-hint>Stored securely; never returned by the API.</span>
  </label>

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
</style>
