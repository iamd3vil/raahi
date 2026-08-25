ALTER TABLE certificates ADD COLUMN acme_config TEXT;
ALTER TABLE certificates ADD COLUMN acme_status TEXT;

CREATE TABLE acme_accounts (
    directory_url TEXT PRIMARY KEY,
    email TEXT,
    credentials TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

ALTER TABLE settings ADD COLUMN cloudflare_api_token TEXT;
