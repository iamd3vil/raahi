// Thin REST client for the Raahi admin API. All mutations on the server rebuild and
// hot-swap the proxy config, so callers just re-fetch after a change.

import type {
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
  Target,
  TargetHealth,
} from './types';

const BASE = '/api/v1';

export class ApiError extends Error {
  status: number;
  constructor(status: number, message: string) {
    super(message);
    this.status = status;
  }
}

async function req<T>(method: string, path: string, body?: unknown): Promise<T> {
  const res = await fetch(BASE + path, {
    method,
    headers: body !== undefined ? { 'Content-Type': 'application/json' } : undefined,
    body: body !== undefined ? JSON.stringify(body) : undefined,
  });
  const text = await res.text();
  const data = text ? JSON.parse(text) : null;
  if (!res.ok) {
    const msg = (data && (data.error as string)) || `${res.status} ${res.statusText}`;
    throw new ApiError(res.status, msg);
  }
  return data as T;
}

export const api = {
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

  // plugins
  listPlugins: () => req<Plugin[]>('GET', '/plugins'),
  createPlugin: (p: Partial<Plugin>) => req<Plugin>('POST', '/plugins', p),
  updatePlugin: (id: number, p: Partial<Plugin>) => req<Plugin>('PUT', `/plugins/${id}`, p),
  deletePlugin: (id: number) => req('DELETE', `/plugins/${id}`),

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

  // certificates
  listCertificates: () => req<Certificate[]>('GET', '/certificates'),
  createCertificate: (c: { name: string; sni: string[]; cert_pem: string; key_pem: string }) =>
    req<Certificate>('POST', '/certificates', c),
  deleteCertificate: (id: number) => req('DELETE', `/certificates/${id}`),

  // settings + observability
  getSettings: () => req<Settings>('GET', '/settings'),
  updateSettings: (s: Partial<Settings>) => req<Settings>('PUT', '/settings', s),
  metrics: () => req<MetricsSnapshot>('GET', '/metrics'),
  requests: (limit = 100) => req<RequestRecord[]>('GET', `/requests?limit=${limit}`),
  health: () => req<TargetHealth[]>('GET', '/health'),
  configSummary: () => req<ConfigSummary>('GET', '/config'),
  exportConfig: () => req<unknown>('GET', '/export'),
  routerTest: (q: { host: string; path: string; method: string }) =>
    req<RouterTestResult>(
      'GET',
      `/router/test?host=${encodeURIComponent(q.host)}&path=${encodeURIComponent(q.path)}&method=${encodeURIComponent(q.method)}`,
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
