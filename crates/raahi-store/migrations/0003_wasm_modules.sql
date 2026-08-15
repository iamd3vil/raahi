-- User-supplied WASM plugin modules, referenced by name from `wasm` plugins.
CREATE TABLE wasm_modules (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT    NOT NULL UNIQUE,
    description TEXT    NOT NULL DEFAULT '',
    wasm        BLOB    NOT NULL,
    created_at  TEXT    NOT NULL
);
