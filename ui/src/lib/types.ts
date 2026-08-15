// Types mirroring the Raahi core model (JSON shapes from the admin API).

export type Protocol = 'http' | 'https';
export type LbAlgorithm = 'round_robin' | 'random' | 'consistent' | 'weighted';
export type PluginType =
  | 'key-auth'
  | 'basic-auth'
  | 'jwt'
  | 'acl'
  | 'ip-restriction'
  | 'rate-limit'
  | 'request-size-limit'
  | 'request-termination'
  | 'redirect'
  | 'cors'
  | 'request-transform'
  | 'response-transform'
  | 'http-log';
export type PluginScope = 'global' | 'service' | 'route';
export type CredentialType = 'key-auth' | 'basic-auth' | 'jwt';

export interface Service {
  id: number;
  name: string;
  protocol: Protocol;
  connect_timeout_ms: number;
  read_timeout_ms: number;
  write_timeout_ms: number;
  retries: number;
  lb_algorithm: LbAlgorithm;
  tls_sni: string | null;
  created_at: string;
  updated_at: string;
}

export interface Target {
  id: number;
  service_id: number;
  host: string;
  port: number;
  weight: number;
  enabled: boolean;
}

export interface Route {
  id: number;
  name: string;
  service_id: number;
  priority: number;
  hosts: string[];
  paths: string[];
  methods: string[];
  strip_path: boolean;
  preserve_host: boolean;
  enabled: boolean;
}

export interface Plugin {
  id: number;
  type: PluginType;
  scope: PluginScope;
  service_id: number | null;
  route_id: number | null;
  config: Record<string, unknown>;
  ordering: number;
  enabled: boolean;
}

export interface Consumer {
  id: number;
  username: string;
  groups: string[];
}

export interface Credential {
  id: number;
  consumer_id: number;
  type: CredentialType;
  identifier: string;
}

export interface Certificate {
  id: number;
  name: string;
  sni: string[];
  cert_pem: string;
}

export interface Settings {
  proxy_http_addr: string;
  proxy_https_addr: string | null;
  admin_addr: string;
  default_lb: LbAlgorithm;
  active_certificate_id: number | null;
}

export interface MetricsSnapshot {
  total: number;
  class_2xx: number;
  class_3xx: number;
  class_4xx: number;
  class_5xx: number;
  no_route: number;
  avg_latency_ms: number;
  p50_latency_ms: number;
  p95_latency_ms: number;
  p99_latency_ms: number;
  top_routes: { route_id: number; count: number; errors: number; avg_latency_ms: number }[];
  top_consumers: { consumer: string; count: number }[];
}

export interface TargetHealth {
  service_id: number;
  target_id: number;
  host: string;
  port: number;
  healthy: boolean;
}

export interface RouterTestResult {
  matched: boolean;
  route_id?: number;
  route_name?: string;
  service_id?: number;
  service_name?: string;
  matched_prefix?: string;
  strip_path?: boolean;
  preserve_host?: boolean;
  plugins?: string[];
}

export interface ConfigSummary {
  version: number;
  routes: number;
  services: number;
  plugins: number;
  consumers: number;
  key_credentials: number;
}

export interface RequestRecord {
  ts_ms: number;
  method: string;
  host: string;
  path: string;
  status: number;
  latency_ms: number;
  route_id: number | null;
  service_id: number | null;
  upstream: string | null;
  consumer: string | null;
}

export const PLUGIN_TYPES: PluginType[] = [
  'key-auth',
  'basic-auth',
  'jwt',
  'acl',
  'ip-restriction',
  'rate-limit',
  'request-size-limit',
  'request-termination',
  'redirect',
  'cors',
  'request-transform',
  'response-transform',
  'http-log',
];

export const PLUGIN_LABELS: Record<PluginType, string> = {
  'key-auth': 'Key Auth',
  'basic-auth': 'Basic Auth',
  jwt: 'JWT',
  acl: 'ACL',
  'ip-restriction': 'IP Restriction',
  'rate-limit': 'Rate Limit',
  'request-size-limit': 'Size Limit',
  'request-termination': 'Termination',
  redirect: 'Redirect',
  cors: 'CORS',
  'request-transform': 'Request Transform',
  'response-transform': 'Response Transform',
  'http-log': 'HTTP Log',
};
