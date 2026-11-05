-- Alerts (PRD §7.4). A channel is somewhere Alerts go (an ntfy topic, a
-- Discord webhook). `settings` is its non-secret JSON; `secrets` is its
-- credentials (an access token, a webhook URL) as JSON encrypted with the
-- secret key (ADR-0005), or NULL.
CREATE TABLE alert_channels (
    id          INTEGER PRIMARY KEY,
    name        TEXT NOT NULL UNIQUE COLLATE NOCASE,
    kind        TEXT NOT NULL,
    settings    TEXT NOT NULL DEFAULT '{}',
    secrets     TEXT,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

-- An Alert Rule: which events, on which Printers or Tags (or the whole
-- Fleet), go to which channel. `events` is JSON: a list such as
-- ["job_ended", "fault_raised"]; `outcomes` narrows job_ended to some Job
-- Outcomes (empty: all). `printers` and `tags` are JSON lists of ids; both
-- empty means the whole Fleet.
CREATE TABLE alert_rules (
    id          INTEGER PRIMARY KEY,
    name        TEXT NOT NULL,
    enabled     INTEGER NOT NULL DEFAULT 1,
    events      TEXT NOT NULL,
    outcomes    TEXT NOT NULL DEFAULT '[]',
    printers    TEXT NOT NULL DEFAULT '[]',
    tags        TEXT NOT NULL DEFAULT '[]',
    channel_id  INTEGER NOT NULL REFERENCES alert_channels (id) ON DELETE CASCADE,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);
