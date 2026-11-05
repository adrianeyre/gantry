-- Gantry-wide settings an Admin changes in the UI (ADR-0004), such as later
-- thresholds and retention. Values are JSON. Server settings (bind address,
-- data directory, secret key) stay in the config file.
CREATE TABLE settings (
    key         TEXT PRIMARY KEY,
    value       TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);
