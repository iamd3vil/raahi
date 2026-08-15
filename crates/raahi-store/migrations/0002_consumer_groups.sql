-- Consumer groups for the acl plugin.
ALTER TABLE consumers ADD COLUMN groups TEXT NOT NULL DEFAULT '[]';
