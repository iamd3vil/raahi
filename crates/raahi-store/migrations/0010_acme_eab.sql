-- Account-registration secrets are separate from public certificate configuration.
CREATE TABLE acme_eab (
    directory_url TEXT PRIMARY KEY,
    key_id TEXT NOT NULL,
    hmac_key TEXT NOT NULL
);
