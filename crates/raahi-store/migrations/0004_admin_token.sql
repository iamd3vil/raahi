-- Admin API bearer-token auth: SHA-256 hex of the token; NULL = auth disabled.
ALTER TABLE settings ADD COLUMN admin_token_hash TEXT;
