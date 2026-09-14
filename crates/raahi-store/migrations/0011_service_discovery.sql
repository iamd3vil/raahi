-- Service discovery sources and managed target lifecycle.
ALTER TABLE services ADD COLUMN upstream_authority TEXT;

CREATE TABLE discovery_sources (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    service_id        INTEGER NOT NULL REFERENCES services(id) ON DELETE CASCADE,
    name              TEXT NOT NULL,
    provider          TEXT NOT NULL,
    config            TEXT NOT NULL DEFAULT '{}',
    enabled           INTEGER NOT NULL DEFAULT 1,
    stale_after_ms    INTEGER NOT NULL DEFAULT 300000,
    removal_grace_ms  INTEGER NOT NULL DEFAULT 60000,
    created_at        TEXT NOT NULL,
    updated_at        TEXT NOT NULL,
    UNIQUE(service_id, name)
);
CREATE INDEX idx_discovery_sources_service ON discovery_sources(service_id);

CREATE TABLE discovery_source_status (
    source_id         INTEGER PRIMARY KEY REFERENCES discovery_sources(id) ON DELETE CASCADE,
    state             TEXT NOT NULL DEFAULT 'pending',
    last_attempt_at   TEXT,
    last_success_at   TEXT,
    next_refresh_at   TEXT,
    revision          TEXT,
    endpoint_count    INTEGER NOT NULL DEFAULT 0,
    last_error        TEXT
);

ALTER TABLE targets ADD COLUMN priority INTEGER NOT NULL DEFAULT 0;
ALTER TABLE targets ADD COLUMN source_id INTEGER REFERENCES discovery_sources(id) ON DELETE CASCADE;
ALTER TABLE targets ADD COLUMN provider_key TEXT;
ALTER TABLE targets ADD COLUMN state TEXT NOT NULL DEFAULT 'active';
ALTER TABLE targets ADD COLUMN metadata TEXT NOT NULL DEFAULT '{}';
ALTER TABLE targets ADD COLUMN last_seen_at TEXT;
ALTER TABLE targets ADD COLUMN missing_since TEXT;

CREATE UNIQUE INDEX idx_discovered_target_key
ON targets(source_id, provider_key)
WHERE source_id IS NOT NULL;
CREATE INDEX idx_targets_source ON targets(source_id);
CREATE INDEX idx_targets_state ON targets(service_id, state);
