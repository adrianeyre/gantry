-- The audit log (PRD §9.2): one row per state-changing action, with who,
-- when, what and the result. Reads are never logged. Entries outlive the
-- users and Printers they name, so there are no foreign keys; names are kept
-- beside ids. `detail` is JSON and never holds a credential or password.
CREATE TABLE audit_log (
    id            INTEGER PRIMARY KEY,
    at            TEXT NOT NULL,
    -- Who: NULL for Gantry itself, or a failed login by an unknown name.
    user_id       INTEGER,
    username      TEXT,
    action        TEXT NOT NULL,
    printer_id    TEXT,
    printer_name  TEXT,
    -- What else it acted on, such as the account a user change was about.
    target        TEXT,
    detail        TEXT,
    -- `skipped` is one part of a bulk action that didn't apply (PRD §6.4).
    result        TEXT NOT NULL CHECK (result IN ('ok', 'refused', 'failed', 'skipped')),
    -- Why it was refused, failed or skipped.
    message       TEXT
);

CREATE INDEX audit_log_at ON audit_log (at);
CREATE INDEX audit_log_username ON audit_log (username COLLATE NOCASE);
CREATE INDEX audit_log_printer ON audit_log (printer_id);
CREATE INDEX audit_log_action ON audit_log (action);
