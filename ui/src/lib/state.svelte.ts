// Shared reactive app state (Svelte 5 runes in a module).

export type View =
  | 'dashboard'
  | 'requests'
  | 'routes'
  | 'services'
  | 'stream-routes'
  | 'plugins'
  | 'consumers'
  | 'certificates'
  | 'settings';

const stored = typeof localStorage !== 'undefined' ? localStorage.getItem('raahi-theme') : null;

export const ui = $state({
  theme: stored === 'light' ? 'light' : 'dark',
  view: (typeof location !== 'undefined' && location.hash.slice(1)) || 'dashboard',
  connected: false,
  /// Admin auth is enabled and we have no (valid) token: show the login screen.
  authRequired: false,
});

export function applyTheme() {
  document.documentElement.setAttribute('data-theme', ui.theme);
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

// ---- toasts ----
export interface Toast {
  id: number;
  msg: string;
  kind: 'info' | 'ok' | 'err';
}
export const toasts = $state<Toast[]>([]);
let toastId = 0;

export function toast(msg: string, kind: Toast['kind'] = 'info') {
  const id = ++toastId;
  toasts.push({ id, msg, kind });
  setTimeout(() => {
    const i = toasts.findIndex((t) => t.id === id);
    if (i >= 0) toasts.splice(i, 1);
  }, 4000);
}

export function dismiss(id: number) {
  const i = toasts.findIndex((t) => t.id === id);
  if (i >= 0) toasts.splice(i, 1);
}
