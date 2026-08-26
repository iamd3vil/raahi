<script lang="ts">
  import { onMount } from 'svelte';
  import { api, ApiError } from '../lib/api';
  import type { Role, User } from '../lib/types';
  import { toast, ui } from '../lib/state.svelte';
  import Drawer from '../lib/components/Drawer.svelte';
  import EmptyState from '../lib/components/EmptyState.svelte';

  let users = $state<User[]>([]);
  let loading = $state(true);
  let open = $state(false);
  let editing = $state<User | null>(null);
  let form = $state({ email: '', name: '', role: 'viewer' as Role, password: '' });
  let saving = $state(false);

  const roleHelp: Record<Role, string> = {
    viewer: 'Read-only access to everything.',
    editor: 'Create, change, and delete gateway configuration.',
    admin: 'Editor plus users, SSO, admin token, listeners, and import.',
  };

  async function load() {
    loading = true;
    try {
      users = await api.listUsers();
    } catch (e) {
      toast((e as ApiError).message, 'err');
    } finally {
      loading = false;
    }
  }

  function openNew() {
    editing = null;
    form = { email: '', name: '', role: 'viewer', password: '' };
    open = true;
  }
  function openEdit(u: User) {
    editing = u;
    form = { email: u.email, name: u.name, role: u.role, password: '' };
    open = true;
  }

  async function save() {
    saving = true;
    try {
      const body = {
        email: form.email.trim(),
        name: form.name.trim(),
        role: form.role,
        password: form.password || undefined,
      };
      if (editing) {
        await api.updateUser(editing.id, body);
        toast(form.password ? 'User updated — their sessions were signed out' : 'User updated', 'ok');
      } else {
        await api.createUser(body);
        toast('User created', 'ok');
      }
      open = false;
      await load();
    } catch (e) {
      toast((e as ApiError).message, 'err');
    } finally {
      saving = false;
    }
  }

  async function del(u: User) {
    if (!confirm(`Delete user "${u.email}"? Their sessions end immediately.`)) return;
    try {
      await api.deleteUser(u.id);
      toast('User deleted', 'ok');
      await load();
    } catch (e) {
      toast((e as ApiError).message, 'err');
    }
  }

  const fmt = (s: string | null) => (s ? new Date(s).toLocaleString() : '—');
  const isMe = (u: User) => ui.me?.user?.id === u.id;

  onMount(load);
</script>

<div class="head-actions">
  <p class="muted">
    People who can sign in to this admin UI. Roles are enforced by the API: viewers read, editors change
    gateway config, admins manage everything.
  </p>
  <button onclick={openNew}>+ New user</button>
</div>

<div class="card">
  {#if loading}
    <div class="empty"><span aria-busy="true" data-spinner="small"></span></div>
  {:else if users.length === 0}
    <EmptyState
      icon="M16 7a4 4 0 1 1-8 0 4 4 0 0 1 8 0zM4 21v-2a4 4 0 0 1 4-4h8a4 4 0 0 1 4 4v2"
      title="No users yet"
      description="Until a user or admin token exists, anyone who can reach the admin port has full access. Create an admin to require sign-in."
    >
      {#snippet action()}
        <button onclick={openNew}>+ Create the first admin</button>
      {/snippet}
    </EmptyState>
  {:else}
    <div class="table">
      <table>
        <thead><tr><th>User</th><th>Role</th><th>Sign-in</th><th>Last login</th><th></th></tr></thead>
        <tbody>
          {#each users as u (u.id)}
            <tr>
              <td>
                <strong>{u.email}</strong>
                {#if isMe(u)}<span class="badge outline" style="margin-left:6px">you</span>{/if}
                {#if u.name}<div class="faint">{u.name}</div>{/if}
              </td>
              <td><span class="badge" data-variant={u.role === 'admin' ? 'warning' : u.role === 'editor' ? 'info' : undefined}>{u.role}</span></td>
              <td class="faint">{u.has_password ? 'password + SSO' : 'SSO only'}</td>
              <td class="faint">{fmt(u.last_login_at)}</td>
              <td class="actions">
                <button class="ghost small" onclick={() => openEdit(u)}>Edit</button>
                <button class="ghost small" data-variant="danger" onclick={() => del(u)}>Delete</button>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</div>

<Drawer bind:open title={editing ? `Edit ${editing.email}` : 'New user'}>
  <label data-field>
    Email
    <input type="email" bind:value={form.email} placeholder="user@example.com" />
  </label>
  <label data-field>
    Name <span class="faint">(optional)</span>
    <input bind:value={form.name} placeholder="Display name" />
  </label>
  <label data-field>
    Role
    <select bind:value={form.role}>
      <option value="viewer">viewer</option>
      <option value="editor">editor</option>
      <option value="admin">admin</option>
    </select>
    <span data-hint>{roleHelp[form.role]}</span>
  </label>
  <label data-field>
    {editing ? 'New password' : 'Password'}
    <input
      type="password"
      autocomplete="new-password"
      bind:value={form.password}
      placeholder={editing ? 'leave blank to keep the current one' : 'at least 8 characters (blank = SSO only)'}
    />
    {#if editing}
      <span data-hint>Setting a password signs that user out of every session.</span>
    {:else}
      <span data-hint>Leave blank for an SSO-only account (requires SSO to be configured).</span>
    {/if}
  </label>

  {#snippet footer()}
    <button class="ghost" onclick={() => (open = false)}>Cancel</button>
    <button onclick={save} disabled={saving || !form.email.trim()}>{editing ? 'Save' : 'Create'}</button>
  {/snippet}
</Drawer>
