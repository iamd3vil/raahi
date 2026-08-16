-- Header conditions (JSON object: name -> exact value, "*" = any value) and
-- weighted traffic splits (JSON array of {service_id, weight}) per route.
ALTER TABLE routes ADD COLUMN headers TEXT NOT NULL DEFAULT '{}';
ALTER TABLE routes ADD COLUMN splits TEXT NOT NULL DEFAULT '[]';
