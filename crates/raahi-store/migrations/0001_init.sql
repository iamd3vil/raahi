-- Raahi config schema. All routing/config state lives here; the proxy reads a
-- compiled in-memory snapshot built from these tables.

CREATE TABLE services (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    name               TEXT    NOT NULL UNIQUE,
    protocol           TEXT    NOT NULL DEFAULT 'http',
    connect_timeout_ms INTEGER NOT NULL DEFAULT 5000,
    read_timeout_ms    INTEGER NOT NULL DEFAULT 60000,
    write_timeout_ms   INTEGER NOT NULL DEFAULT 60000,
    retries            INTEGER NOT NULL DEFAULT 1,
    lb_algorithm       TEXT    NOT NULL DEFAULT 'round_robin',
    tls_sni            TEXT,
    created_at         TEXT    NOT NULL,
    updated_at         TEXT    NOT NULL
);

CREATE TABLE targets (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    service_id INTEGER NOT NULL REFERENCES services(id) ON DELETE CASCADE,
    host       TEXT    NOT NULL,
    port       INTEGER NOT NULL,
    weight     INTEGER NOT NULL DEFAULT 100,
    enabled    INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX idx_targets_service ON targets(service_id);

CREATE TABLE routes (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    name          TEXT    NOT NULL UNIQUE,
    service_id    INTEGER NOT NULL REFERENCES services(id) ON DELETE CASCADE,
    priority      INTEGER NOT NULL DEFAULT 0,
    hosts         TEXT    NOT NULL DEFAULT '[]',
    paths         TEXT    NOT NULL DEFAULT '[]',
    methods       TEXT    NOT NULL DEFAULT '[]',
    strip_path    INTEGER NOT NULL DEFAULT 0,
    preserve_host INTEGER NOT NULL DEFAULT 0,
    enabled       INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX idx_routes_service ON routes(service_id);

CREATE TABLE plugins (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    type       TEXT    NOT NULL,
    scope      TEXT    NOT NULL,
    service_id INTEGER REFERENCES services(id) ON DELETE CASCADE,
    route_id   INTEGER REFERENCES routes(id) ON DELETE CASCADE,
    config     TEXT    NOT NULL DEFAULT '{}',
    ordering   INTEGER NOT NULL DEFAULT 0,
    enabled    INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX idx_plugins_service ON plugins(service_id);
CREATE INDEX idx_plugins_route ON plugins(route_id);

CREATE TABLE consumers (
    id       INTEGER PRIMARY KEY AUTOINCREMENT,
    username TEXT    NOT NULL UNIQUE
);

CREATE TABLE consumer_credentials (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    consumer_id INTEGER NOT NULL REFERENCES consumers(id) ON DELETE CASCADE,
    type        TEXT    NOT NULL,
    identifier  TEXT    NOT NULL,
    secret      TEXT,
    UNIQUE(type, identifier)
);
CREATE INDEX idx_cred_consumer ON consumer_credentials(consumer_id);

CREATE TABLE certificates (
    id       INTEGER PRIMARY KEY AUTOINCREMENT,
    name     TEXT    NOT NULL UNIQUE,
    sni      TEXT    NOT NULL DEFAULT '[]',
    cert_pem TEXT    NOT NULL,
    key_pem  TEXT    NOT NULL
);

CREATE TABLE settings (
    id                    INTEGER PRIMARY KEY CHECK (id = 1),
    proxy_http_addr       TEXT    NOT NULL,
    proxy_https_addr      TEXT,
    admin_addr            TEXT    NOT NULL,
    default_lb            TEXT    NOT NULL DEFAULT 'round_robin',
    active_certificate_id INTEGER REFERENCES certificates(id) ON DELETE SET NULL
);
