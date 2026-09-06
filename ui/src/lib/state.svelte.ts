// Shared reactive app state (Svelte 5 runes in a module).

import type { Principal, Role } from './types';

export type View =
  | 'add-application'
  | 'dashboard'
  | 'requests'
  | 'routes'
  | 'services'
  | 'stream-routes'
  | 'plugins'
  | 'consumers'
  | 'certificates'
  | 'users'
  | 'settings';

const stored = typeof localStorage !== 'undefined' ? localStorage.getItem('raahi-theme') : null;

export const ui = $state({
  theme: stored === 'light' ? 'light' : 'dark',
  view: (typeof location !== 'undefined' && location.hash.slice(1)) || 'dashboard',
  connected: false,
  /// Admin auth is enabled and we have no valid session/token: show the login screen.
  authRequired: false,
  /// Who we are, once known (null while loading or when signed out).
  me: null as Principal | null,
});

const RANK: Record<Role, number> = { viewer: 0, editor: 1, admin: 2 };

/** Whether the current principal holds at least `role`. Unknown = optimistic (server enforces). */
export function hasRole(role: Role): boolean {
  return ui.me ? RANK[ui.me.role] >= RANK[role] : true;
}

export function applyTheme() {
  // Oat themes via light-dark(); forcing color-scheme flips every variable.
  document.documentElement.style.colorScheme = ui.theme;
}

export function toggleTheme() {
  ui.theme = ui.theme === 'dark' ? 'light' : 'dark';
  localStorage.setItem('raahi-theme', ui.theme);
  applyTheme();
}

export function go(view: View) {
  ui.view = view;
  location.hash = view;
}

// ---- toasts (rendered by Oat's ot.toast) ----
declare global {
  interface Window {
    ot: {
      toast: (msg: string, title?: string, opts?: { variant?: string; placement?: string; duration?: number }) => void;
    };
  }
}

export function toast(msg: string, kind: 'info' | 'ok' | 'err' = 'info') {
  const variant = kind === 'ok' ? 'success' : kind === 'err' ? 'danger' : 'info';
  window.ot.toast(msg, undefined, { variant, placement: 'bottom-right' });
}

// Keep an unfinished application while visiting Certificates or Settings.
export const applicationDraft = $state({
  name: '', domain: '', upstream_url: '', https: true, hsts: false,
  access: 'public', networks: '',
});
