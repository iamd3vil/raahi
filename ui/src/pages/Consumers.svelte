<script lang="ts">
  import { onMount } from 'svelte';
  import { api, ApiError } from '../lib/api';
  import type { Consumer, Credential, CredentialType } from '../lib/types';
  import { toast } from '../lib/state.svelte';
  import Drawer from '../lib/components/Drawer.svelte';
  import EmptyState from '../lib/components/EmptyState.svelte';

  let consumers = $state<Consumer[]>([]);
  let credsBy = $state<Record<number, Credential[]>>({});
  let loading = $state(true);
  let open = $state(false);
  let editing = $state<Consumer | null>(null);
  let username = $state('');
  let groups = $state('');
  let credForm = $state({
    type: 'key-auth' as CredentialType,
    identifier: '',
    secret: '',
    algorithm: 'HS256',
  });

  const csv = (s: string) => s.split(',').map((x) => x.trim()).filter(Boolean);

  async function load() {
    loading = true;
    try {
      consumers = await api.listConsumers();
      const lists = await Promise.all(consumers.map((c) => api.listCredentials(c.id)));
      const map: Record<number, Credential[]> = {};
      consumers.forEach((c, i) => (map[c.id] = lists[i]));
      credsBy = map;
    } catch (e) {
      toast((e as ApiError).message, 'err');
    } finally {
      loading = false;
    }
  }

  function openNew() {
    editing = null;
    username = '';
    groups = '';
    credForm = { type: 'key-auth', identifier: '', secret: '', algorithm: 'HS256' };
    open = true;
  }
  function openEdit(c: Consumer) {
    editing = c;
    username = c.username;
    groups = (c.groups ?? []).join(', ');
    credForm = { type: 'key-auth', identifier: '', secret: '', algorithm: 'HS256' };
    open = true;
  }

  async function save() {
    try {
      if (editing) {
        await api.updateConsumer(editing.id, { username, groups: csv(groups) });
        toast('Consumer updated', 'ok');
      } else {
        editing = await api.createConsumer({ username, groups: csv(groups) });
        toast('Consumer created — add credentials below', 'ok');
      }
      await load();
    } catch (e) {
      toast((e as ApiError).message, 'err');
    }
  }

  async function del(c: Consumer) {
    if (!confirm(`Delete consumer "${c.username}"?`)) return;
    try {
      await api.deleteConsumer(c.id);
      toast('Consumer deleted', 'ok');
      if (editing?.id === c.id) open = false;
      await load();
    } catch (e) {
      toast((e as ApiError).message, 'err');
    }
  }

  async function addCred() {
    if (!editing || !credForm.identifier) return;
    try {
      await api.createCredential(editing.id, {
        type: credForm.type,
        identifier: credForm.identifier,
        secret: credForm.type === 'key-auth' ? undefined : credForm.secret,
        algorithm: credForm.type === 'jwt' ? credForm.algorithm : undefined,
      });
      credForm = { type: credForm.type, identifier: '', secret: '', algorithm: credForm.algorithm };
      toast('Credential added', 'ok');
      await load();
    } catch (e) {
      toast((e as ApiError).message, 'err');
    }
  }

  async function delCred(c: Credential) {
    try {
      await api.deleteCredential(c.id);
      await load();
    } catch (e) {
      toast((e as ApiError).message, 'err');
    }
  }

  const editingCreds = $derived(editing ? (credsBy[editing.id] ?? []) : []);

  onMount(load);
</script>

<div class="head-actions">
  <p class="muted">Identities that auth plugins authenticate. Key-auth uses an API key; basic-auth uses username + password.</p>
  <button onclick={openNew}>+ New consumer</button>
</div>

<div class="card">
  {#if loading}
    <div class="empty"><span aria-busy="true" data-spinner="small"></span></div>
  {:else if consumers.length === 0}
    <EmptyState
      icon="M16 7a4 4 0 1 1-8 0 4 4 0 0 1 8 0zM4 21v-2a4 4 0 0 1 4-4h8a4 4 0 0 1 4 4v2"
      title="No consumers yet"
      description="Consumers are the identities that key-auth and basic-auth plugins authenticate. Create one and attach credentials."
    >
      {#snippet action()}
        <button onclick={openNew}>+ Create your first consumer</button>
      {/snippet}
    </EmptyState>
  {:else}
    <div class="table">
      <table>
        <thead><tr><th>Username</th><th>Groups</th><th>Credentials</th><th></th></tr></thead>
        <tbody>
          {#each consumers as c (c.id)}
            {@const creds = credsBy[c.id] ?? []}
            <tr>
              <td><strong>{c.username}</strong></td>
              <td>
                {#if !c.groups?.length}<span class="faint">—</span>{/if}
                {#each c.groups ?? [] as g}<span class="badge outline">{g}</span>{/each}
              </td>
              <td>
                {#if creds.length === 0}<span class="faint">none</span>{/if}
                {#each creds as cr}<span class="chip">{cr.type}: {cr.identifier}</span>{/each}
              </td>
              <td class="actions">
                <button class="ghost small" onclick={() => openEdit(c)}>Manage</button>
                <button class="ghost small" data-variant="danger" onclick={() => del(c)}>Delete</button>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</div>

<Drawer bind:open title={editing ? `Manage ${editing.username}` : 'New consumer'}>
  <label data-field>
    Username
    <input bind:value={username} placeholder="alice" />
  </label>
  <div data-field>
    <label for="c-groups">Groups <span class="faint">(comma-separated, used by the ACL plugin)</span></label>
    <div class="row">
      <input id="c-groups" class="mono" bind:value={groups} placeholder="team-a, admins" />
      <button class="outline" style="flex:none" onclick={save}>{editing ? 'Save' : 'Create'}</button>
    </div>
  </div>

  {#if editing}
    <hr />
    <h3 class="sub">Credentials</h3>
    <div class="creds">
      {#each editingCreds as cr (cr.id)}
        <div class="cred-row">
          <span class="badge">{cr.type}</span>
          <span class="mono">{cr.identifier}</span>
          <div class="spacer"></div>
          <button class="ghost small icon" onclick={() => delCred(cr)}>✕</button>
        </div>
      {:else}
        <div class="faint" style="padding:6px 0">No credentials yet.</div>
      {/each}
    </div>

    <div class="add-cred">
      <select bind:value={credForm.type}>
        <option value="key-auth">key-auth</option>
        <option value="basic-auth">basic-auth</option>
        <option value="jwt">jwt</option>
      </select>
      <input style="min-width:140px"
        placeholder={credForm.type === 'key-auth' ? 'API key' : credForm.type === 'jwt' ? 'key (iss claim value)' : 'username'}
        bind:value={credForm.identifier} />
      {#if credForm.type === 'basic-auth'}
        <input type="password" placeholder="password" bind:value={credForm.secret} />
      {/if}
      {#if credForm.type === 'jwt'}
        <select style="max-width:100px" bind:value={credForm.algorithm}>
          <option value="HS256">HS256</option>
          <option value="HS384">HS384</option>
          <option value="HS512">HS512</option>
          <option value="RS256">RS256</option>
        </select>
      {/if}
      <button class="outline" onclick={addCred}>Add</button>
    </div>
    {#if credForm.type === 'jwt'}
      <label data-field style="margin-top:10px">
        {credForm.algorithm === 'RS256' ? 'RSA public key (PEM)' : 'HMAC secret'}
        <textarea rows={credForm.algorithm === 'RS256' ? 5 : 2}
          placeholder={credForm.algorithm === 'RS256' ? '-----BEGIN PUBLIC KEY-----' : 'shared signing secret'}
          bind:value={credForm.secret}></textarea>
      </label>
    {/if}
    <p class="hint">basic-auth passwords are bcrypt-hashed; secrets are never returned by the API.</p>
  {/if}

  {#snippet footer()}
    <button class="ghost" onclick={() => (open = false)}>Close</button>
  {/snippet}
</Drawer>

<style>
  .row input {
    margin-block-start: 0;
  }
  .cred-row {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 0;
    border-bottom: 1px solid var(--border);
  }
  .spacer {
    flex: 1;
  }
  .add-cred {
    display: flex;
    gap: 8px;
    margin-top: 12px;
    flex-wrap: wrap;
  }
  .add-cred :is(input, select) {
    width: auto;
    flex: 1;
    margin-block-start: 0;
  }
  .add-cred select {
    flex: none;
    max-width: 130px;
  }
  textarea {
    min-height: auto;
  }
  .hint {
    font-size: 12.5px;
    color: var(--muted-foreground);
  }
</style>
