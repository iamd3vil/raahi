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
