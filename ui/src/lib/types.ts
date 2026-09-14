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
  | 'proxy-cache'
  | 'request-size-limit'
  | 'request-termination'
  | 'redirect'
  | 'cors'
  | 'wasm'
  | 'request-transform'
  | 'response-transform'
  | 'hsts'
  | 'response-body-transform'
  | 'http-log'
  | 'request-id'
  | 'response-compression';
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
  /** Host/:authority sent upstream, overriding the target's host:port. */
  upstream_authority: string | null;
  health_path: string | null;
  created_at: string;
  updated_at: string;
}

/** Lifecycle of a discovered target. Static targets are always `active`. */
export type TargetState = 'active' | 'draining' | 'stale';

export interface Target {
  id: number;
  service_id: number;
  host: string;
  port: number;
  weight: number;
  /** Failover tier: the lowest value with a healthy target wins. */
  priority: number;
  enabled: boolean;
  state: TargetState;
  /** Discovery source that owns this target; null for operator-created ones. */
  source_id: number | null;
  /** Stable identity the provider reported, matched across refreshes. */
  provider_key: string | null;
  /** Labels the provider returned with the endpoint (zone, version, ...). */
  metadata: Record<string, string>;
  last_seen_at: string | null;
  missing_since: string | null;
}

// ---- service discovery ----
export type DiscoveryProviderId = 'dns' | 'dns-srv' | 'http';

export const DISCOVERY_PROVIDERS: DiscoveryProviderId[] = ['dns', 'dns-srv', 'http'];

export const DISCOVERY_PROVIDER_LABELS: Record<DiscoveryProviderId, string> = {
  dns: 'DNS (A/AAAA)',
  'dns-srv': 'DNS SRV',
  http: 'HTTP registry',
};

/** Static provider metadata from GET /discovery/providers. */
export interface DiscoveryProvider {
  id: DiscoveryProviderId;
  name: string;
  description: string;
  /** False = the config's port/weight/priority applies to every endpoint. */
  supplies_port: boolean;
  supplies_weight: boolean;
  supplies_priority: boolean;
  config_schema: Record<string, unknown>;
  defaults: Record<string, unknown>;
}

/** Scheduling and endpoint defaults shared by every provider config. */
export interface DiscoveryConfigCommon {
  interval_ms: number;
  timeout_ms: number;
  port?: number | null;
  weight: number;
  priority: number;
}

export interface DnsDiscoveryConfig extends DiscoveryConfigCommon {
  hostname: string;
  record_types: ('a' | 'aaaa')[];
  resolver?: string | null;
}

export interface DnsSrvDiscoveryConfig extends DiscoveryConfigCommon {
  service_name: string;
  resolver?: string | null;
  use_record_weight: boolean;
  use_record_priority: boolean;
}

export interface HttpDiscoveryConfig extends DiscoveryConfigCommon {
  url: string;
  headers: Record<string, string>;
  /** Dot path to the endpoint array when it isn't at the document root. */
  endpoints_path?: string | null;
  tls_verify: boolean;
}

export type DiscoveryConfig = DnsDiscoveryConfig | DnsSrvDiscoveryConfig | HttpDiscoveryConfig;

/** Request body for creating/replacing a discovery source. */
export interface DiscoverySourceInput {
  name: string;
  provider: DiscoveryProviderId;
  config: DiscoveryConfig;
  enabled: boolean;
  /** How long the last known good endpoints stay trusted once refreshes fail. */
  stale_after_ms: number;
  /** How long a vanished target drains before deletion. */
  removal_grace_ms: number;
}

export interface DiscoverySource extends DiscoverySourceInput {
  id: number;
  service_id: number;
  created_at: string;
  updated_at: string;
}

export type DiscoveryState = 'pending' | 'healthy' | 'failing' | 'stale' | 'disabled';

export interface DiscoverySourceStatus {
  source_id: number;
  state: DiscoveryState;
  last_attempt_at: string | null;
  last_success_at: string | null;
  next_refresh_at: string | null;
  /** Opaque provider or endpoint-set revision; null until the first success. */
  revision: string | null;
  endpoint_count: number;
  active_count: number;
  draining_count: number;
  stale_count: number;
  last_error: string | null;
}

export interface Route {
  id: number;
  name: string;
  service_id: number;
  priority: number;
  hosts: string[];
  paths: string[];
  methods: string[];
  headers: Record<string, string>;
  splits: { service_id: number; weight: number }[];
  strip_path: boolean;
  preserve_host: boolean;
  enabled: boolean;
}

export interface StreamRoute {
  id: number;
  name: string;
  listen_addr: string;
  service_id: number;
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
  acme_config?: {
    directory_url: string;
    challenge: 'dns-01' | 'tls-alpn-01';
    email?: string;
  };
  acme_status?: {
    state: 'pending' | 'issuing' | 'issued' | 'failed';
    issued_at?: string;
    expires_at?: string;
    last_attempt?: string;
    last_error?: string;
  };
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
  splits?: { service_id: number; service_name: string; weight: number }[];
}

export interface WasmModule {
  id: number;
  name: string;
  description: string;
  size_bytes: number;
  created_at: string;
}

export interface ImportReport {
  services: number;
  targets: number;
  discovery_sources: number;
  routes: number;
  plugins: number;
  consumers: number;
  credentials: number;
  certificates: number;
  wasm_modules: number;
  skipped: string[];
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
  'proxy-cache',
  'request-size-limit',
  'request-termination',
  'redirect',
  'cors',
  'wasm',
  'request-transform',
  'response-transform',
  'hsts',
  'response-body-transform',
  'http-log',
  'request-id',
  'response-compression',
];

export const PLUGIN_LABELS: Record<PluginType, string> = {
  'key-auth': 'Key Auth',
  'basic-auth': 'Basic Auth',
  jwt: 'JWT',
  acl: 'ACL',
  'ip-restriction': 'IP Restriction',
  'rate-limit': 'Rate Limit',
  'proxy-cache': 'Proxy Cache',
  'request-size-limit': 'Size Limit',
  'request-termination': 'Termination',
  redirect: 'Redirect',
  cors: 'CORS',
  wasm: 'WASM',
  'request-transform': 'Request Transform',
  'response-transform': 'Response Transform',
  hsts: 'HSTS',
  'response-body-transform': 'Body Transform',
  'http-log': 'HTTP Log',
  'request-id': 'Request ID',
  'response-compression': 'Compression',
};

// ---- admin users, auth, SSO ----
export type Role = 'viewer' | 'editor' | 'admin';
export type AuthMethod = 'open' | 'token' | 'session';

export interface User {
  id: number;
  email: string;
  name: string;
  role: Role;
  has_password: boolean;
  created_at: string;
  last_login_at: string | null;
}

export interface Principal {
  role: Role;
  method: AuthMethod;
  user: User | null;
}

export interface AuthStatus {
  auth_enabled: boolean;
  token_enabled: boolean;
  users_exist: boolean;
  sso: { enabled: boolean; label: string };
}

export interface SsoConfigView {
  issuer: string;
  client_id: string;
  client_secret_set: boolean;
  label: string;
  auto_provision_role: Role | null;
  allowed_domains: string[];
}

export interface ApplicationSpec {
  name: string;
  domain: string;
  upstream_url: string;
  https: boolean;
  hsts: boolean;
  allowed_cidrs: string[];
}
export interface ApplicationCreated {
  name: string;
  url: string;
  service_id: number;
  target_id: number;
  route_id: number;
  plugin_ids: number[];
}
export interface UpstreamProbe {
  reachable: boolean;
  status: number | null;
  latency_ms: number;
  message: string;
}
