-- Printers live here and only here: never in the config file (ADR-0004).
-- `settings` is the Driver's non-secret fields as JSON. `secrets` is the
-- Driver's credential fields as JSON, encrypted with the secret key
-- (ADR-0005), or NULL when there are none.
CREATE TABLE printers (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL UNIQUE COLLATE NOCASE,
    driver      TEXT NOT NULL,
    settings    TEXT NOT NULL DEFAULT '{}',
    secrets     TEXT,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);
