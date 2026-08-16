-- Optional HTTP health check per service: GET this path on each target; healthy =
-- 2xx/3xx. NULL keeps the plain TCP-connect check.
ALTER TABLE services ADD COLUMN health_path TEXT;
