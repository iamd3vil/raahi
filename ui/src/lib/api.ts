// Thin REST client for the Raahi admin API. All mutations on the server rebuild and
// hot-swap the proxy config, so callers just re-fetch after a change.

import type {
  ApplicationSpec,
  ApplicationCreated,
  UpstreamProbe,
  Certificate,
  ConfigSummary,
  Consumer,
  Credential,
  MetricsSnapshot,
  Plugin,
  RequestRecord,
  Route,
  RouterTestResult,
  Service,
  Settings,
  StreamRoute,
  Target,
  ImportReport,
  TargetHealth,
  WasmModule,
  AuthStatus,
  Principal,
  Role,
  SsoConfigView,
  User,
} from './types';

import { ui } from './state.svelte';

const BASE = '/api/v1';
const TOKEN_KEY = 'raahi-admin-token';

export function adminToken(): string | null {
  return localStorage.getItem(TOKEN_KEY);
}

export function setAdminToken(token: string | null) {
  if (token) localStorage.setItem(TOKEN_KEY, token);
  else localStorage.removeItem(TOKEN_KEY);
}

/** SSE endpoint URL; EventSource can't set headers, so the token rides as a query param. */
export function eventsUrl(): string {
  const t = adminToken();
  return `${BASE}/events${t ? `?access_token=${encodeURIComponent(t)}` : ''}`;
}

export class ApiError extends Error {
  status: number;
  constructor(status: number, message: string) {
    super(message);
    this.status = status;
  }
}

async function req<T>(method: string, path: string, body?: unknown): Promise<T> {
  const headers: Record<string, string> = {};
  if (body !== undefined) headers['Content-Type'] = 'application/json';
  const token = adminToken();
  if (token) headers['Authorization'] = `Bearer ${token}`;

  const res = await fetch(BASE + path, {
    method,
    headers,
    body: body !== undefined ? JSON.stringify(body) : undefined,
  });
  const text = await res.text();
  const data = text ? JSON.parse(text) : null;
  if (!res.ok) {
    if (res.status === 401 && !path.startsWith('/admin/') && !path.startsWith('/auth/')) {
      ui.authRequired = true;
    }
    const msg = (data && (data.error as string)) || `${res.status} ${res.statusText}`;
    throw new ApiError(res.status, msg);
  }
  return data as T;
}

export const api = {
  createApplication: (spec: ApplicationSpec) => req<ApplicationCreated>('POST', '/applications', spec),
  testUpstream: (upstream_url: string) => req<UpstreamProbe>('POST', '/applications/test-upstream', { upstream_url }),
  // services
  listServices: () => req<Service[]>('GET', '/services'),
  createService: (s: Partial<Service>) => req<Service>('POST', '/services', s),
  updateService: (id: number, s: Partial<Service>) => req<Service>('PUT', `/services/${id}`, s),
  deleteService: (id: number) => req('DELETE', `/services/${id}`),

  // targets
  listTargets: (serviceId: number) => req<Target[]>('GET', `/services/${serviceId}/targets`),
  createTarget: (serviceId: number, t: Partial<Target>) =>
    req<Target>('POST', `/services/${serviceId}/targets`, t),
  updateTarget: (id: number, t: Partial<Target>) => req<Target>('PUT', `/targets/${id}`, t),
  deleteTarget: (id: number) => req('DELETE', `/targets/${id}`),

  // routes
  listRoutes: () => req<Route[]>('GET', '/routes'),
  createRoute: (r: Partial<Route>) => req<Route>('POST', '/routes', r),
  updateRoute: (id: number, r: Partial<Route>) => req<Route>('PUT', `/routes/${id}`, r),
  deleteRoute: (id: number) => req('DELETE', `/routes/${id}`),

  // stream routes (L4). Mutations may carry a `note` when a restart is needed.
  listStreamRoutes: () => req<StreamRoute[]>('GET', '/stream-routes'),
  createStreamRoute: (r: Partial<StreamRoute>) =>
    req<StreamRoute & { note?: string }>('POST', '/stream-routes', r),
  updateStreamRoute: (id: number, r: Partial<StreamRoute>) =>
    req<StreamRoute & { note?: string }>('PUT', `/stream-routes/${id}`, r),
  deleteStreamRoute: (id: number) => req<{ note?: string }>('DELETE', `/stream-routes/${id}`),

  // plugins
  listPlugins: () => req<Plugin[]>('GET', '/plugins'),
  createPlugin: (p: Partial<Plugin>) => req<Plugin>('POST', '/plugins', p),
  updatePlugin: (id: number, p: Partial<Plugin>) => req<Plugin>('PUT', `/plugins/${id}`, p),
  deletePlugin: (id: number) => req('DELETE', `/plugins/${id}`),
  purgeCache: () => req<{ purged: number }>('POST', '/cache/purge'),

  // consumers + credentials
  listConsumers: () => req<Consumer[]>('GET', '/consumers'),
  createConsumer: (c: Partial<Consumer>) => req<Consumer>('POST', '/consumers', c),
  updateConsumer: (id: number, c: Partial<Consumer>) => req<Consumer>('PUT', `/consumers/${id}`, c),
  deleteConsumer: (id: number) => req('DELETE', `/consumers/${id}`),
  listCredentials: (consumerId: number) =>
    req<Credential[]>('GET', `/consumers/${consumerId}/credentials`),
  createCredential: (
    consumerId: number,
    c: { type: string; identifier: string; secret?: string; algorithm?: string },
  ) => req<Credential>('POST', `/consumers/${consumerId}/credentials`, c),
  deleteCredential: (id: number) => req('DELETE', `/credentials/${id}`),

  // ACME account registration (EAB values are write-only)
  getAcmeAccountStatus: (directory_url: string) =>
    req<{ configured: boolean; account_registered: boolean }>('GET', `/acme/eab?directory_url=${encodeURIComponent(directory_url)}`),
  setAcmeEab: (credentials: { directory_url: string; key_id: string; hmac_key: string }) =>
    req<{ configured: boolean }>('PUT', '/acme/eab', credentials),
  deleteAcmeEab: (directory_url: string) =>
    req<{ configured: boolean }>('DELETE', `/acme/eab?directory_url=${encodeURIComponent(directory_url)}`),

  // certificates
  listCertificates: () => req<Certificate[]>('GET', '/certificates'),
  createCertificate: (c: {
    name: string;
    sni: string[];
    cert_pem?: string;
    key_pem?: string;
    acme_config?: {
      directory_url: string;
      challenge: 'dns-01' | 'tls-alpn-01';
      email?: string;
    };
  }) =>
    req<Certificate>('POST', '/certificates', c),
  deleteCertificate: (id: number) => req('DELETE', `/certificates/${id}`),
  renewCertificate: (id: number) => req('POST', `/certificates/${id}/renew`),
  getCloudflareTokenStatus: () =>
    req<{ configured: boolean }>('GET', '/acme/cloudflare-token'),
  setCloudflareToken: (token: string) =>
    req<{ configured: boolean }>('PUT', '/acme/cloudflare-token', { token }),
  deleteCloudflareToken: () =>
    req<{ configured: boolean }>('DELETE', '/acme/cloudflare-token'),

  // settings + observability
  getSettings: () => req<Settings>('GET', '/settings'),
  updateSettings: (s: Partial<Settings>) => req<Settings>('PUT', '/settings', s),
  metrics: () => req<MetricsSnapshot>('GET', '/metrics'),
  requests: (limit = 100) => req<RequestRecord[]>('GET', `/requests?limit=${limit}`),
  health: () => req<TargetHealth[]>('GET', '/health'),
  configSummary: () => req<ConfigSummary>('GET', '/config'),
  exportConfig: (includeSecrets = false) =>
    req<unknown>('GET', `/export?include_secrets=${includeSecrets}`),
  importConfig: (doc: unknown) => req<ImportReport>('POST', '/import', doc),

  // wasm modules
  listWasmModules: () => req<WasmModule[]>('GET', '/wasm-modules'),
  createWasmModule: (m: { name: string; description?: string; wasm_base64?: string; wat?: string }) =>
    req<WasmModule>('POST', '/wasm-modules', m),
  deleteWasmModule: (id: number) => req('DELETE', `/wasm-modules/${id}`),

  // admin auth: token, sessions, users, SSO
  adminStatus: () => req<AuthStatus>('GET', '/admin/status'),
  createAdminToken: () => req<{ token: string }>('POST', '/admin/token'),
  deleteAdminToken: () => req('DELETE', '/admin/token'),
  login: (email: string, password: string) =>
    req<Principal>('POST', '/auth/login', { email, password }),
  logout: () => req('POST', '/auth/logout'),
  me: () => req<Principal>('GET', '/auth/me'),
  changePassword: (current_password: string, new_password: string) =>
    req('PUT', '/auth/me/password', { current_password, new_password }),
  listUsers: () => req<User[]>('GET', '/users'),
  createUser: (u: { email: string; name: string; role: Role; password?: string }) =>
    req<User>('POST', '/users', u),
  updateUser: (id: number, u: { email: string; name: string; role: Role; password?: string }) =>
    req<User>('PUT', `/users/${id}`, u),
  deleteUser: (id: number) => req('DELETE', `/users/${id}`),
  getSsoConfig: () => req<{ enabled: boolean; config: SsoConfigView | null }>('GET', '/sso/config'),
  setSsoConfig: (c: {
    issuer: string;
    client_id: string;
    client_secret?: string;
    label: string;
    auto_provision_role: Role | null;
    allowed_domains: string[];
  }) => req<{ enabled: boolean; config: SsoConfigView }>('PUT', '/sso/config', c),
  deleteSsoConfig: () => req('DELETE', '/sso/config'),
  /** Top-level navigation target that starts the OIDC login. */
  ssoStartUrl: () => `${BASE}/auth/sso/start`,
  routerTest: (q: { host: string; path: string; method: string; headers?: string }) =>
    req<RouterTestResult>(
      'GET',
      `/router/test?host=${encodeURIComponent(q.host)}&path=${encodeURIComponent(q.path)}&method=${encodeURIComponent(q.method)}${q.headers ? `&headers=${encodeURIComponent(q.headers)}` : ''}`,
    ),
};

/** Human-friendly latency: sub-ms shows µs, otherwise adaptive ms precision. */
export function fmtLatency(ms: number): string {
  if (ms <= 0) return '0ms';
  if (ms < 1) return `${Math.round(ms * 1000)}µs`;
  if (ms < 10) return `${ms.toFixed(2)}ms`;
  if (ms < 1000) return `${ms.toFixed(1)}ms`;
  return `${(ms / 1000).toFixed(2)}s`;
}
