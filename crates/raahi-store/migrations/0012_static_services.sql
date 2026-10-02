-- Native static hosting uses the existing HTTP/HTTPS listeners, with no targets.
ALTER TABLE services ADD COLUMN kind TEXT NOT NULL DEFAULT 'proxy' CHECK (kind IN ('proxy', 'static'));
ALTER TABLE services ADD COLUMN root TEXT;
ALTER TABLE services ADD COLUMN spa_fallback INTEGER NOT NULL DEFAULT 0;

-- Keep upstream-only resources from silently attaching to static sites. These
-- constraints also cover imports and declarative apply inside their transactions.
CREATE TRIGGER static_service_update BEFORE UPDATE OF kind ON services
WHEN NEW.kind = 'static' AND (
    EXISTS (SELECT 1 FROM targets WHERE service_id = NEW.id) OR
    EXISTS (SELECT 1 FROM discovery_sources WHERE service_id = NEW.id) OR
    EXISTS (SELECT 1 FROM stream_routes WHERE service_id = NEW.id)
)
BEGIN
    SELECT RAISE(ABORT, 'remove targets, discovery sources and stream routes before switching to static');
END;

CREATE TRIGGER static_target_insert BEFORE INSERT ON targets
WHEN (SELECT kind FROM services WHERE id = NEW.service_id) = 'static'
BEGIN
    SELECT RAISE(ABORT, 'static services cannot have upstream targets');
END;

CREATE TRIGGER static_discovery_insert BEFORE INSERT ON discovery_sources
WHEN (SELECT kind FROM services WHERE id = NEW.service_id) = 'static'
BEGIN
    SELECT RAISE(ABORT, 'static services cannot have discovery sources');
END;

CREATE TRIGGER static_stream_insert BEFORE INSERT ON stream_routes
WHEN (SELECT kind FROM services WHERE id = NEW.service_id) = 'static'
BEGIN
    SELECT RAISE(ABORT, 'static services cannot have stream routes');
END;

CREATE TRIGGER static_stream_update BEFORE UPDATE OF service_id ON stream_routes
WHEN (SELECT kind FROM services WHERE id = NEW.service_id) = 'static'
BEGIN
    SELECT RAISE(ABORT, 'static services cannot have stream routes');
END;
