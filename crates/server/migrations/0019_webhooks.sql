-- Webhooks (PRD §7.7): an Admin's URL that is sent the same events as
-- Alerts, as signed JSON. `secret` signs each delivery and is encrypted with
-- the secret key (ADR-0005), because it must be read back to sign. `events`,
-- `printers` and `tags` are JSON lists, read as for Alert Rules: no Printers
-- and no Tags mean the whole Fleet.
CREATE TABLE webhooks (
    id          INTEGER PRIMARY KEY,
    name        TEXT NOT NULL UNIQUE COLLATE NOCASE,
    url         TEXT NOT NULL,
    secret      TEXT NOT NULL,
    enabled     INTEGER NOT NULL DEFAULT 1,
    events      TEXT NOT NULL,
    outcomes    TEXT NOT NULL DEFAULT '[]',
    printers    TEXT NOT NULL DEFAULT '[]',
    tags        TEXT NOT NULL DEFAULT '[]',
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);
