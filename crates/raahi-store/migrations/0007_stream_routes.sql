-- L4 stream routes: raw TCP listeners proxied to a service's targets. Listeners
-- bind at startup; retargeting an existing route applies live via the snapshot.
CREATE TABLE stream_routes (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT    NOT NULL UNIQUE,
    listen_addr TEXT    NOT NULL UNIQUE,
    service_id  INTEGER NOT NULL REFERENCES services(id) ON DELETE CASCADE,
    enabled     INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX idx_stream_routes_service ON stream_routes(service_id);
