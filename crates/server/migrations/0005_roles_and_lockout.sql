-- Viewer, Operator and Admin roles (PRD §7.6), and what login lockout needs
-- (PRD §9.2). SQLite can't change a CHECK constraint in place, so `users` is
-- rebuilt. Dropping it while `sessions` still pointed at it would cascade and
-- log everyone out, so `sessions` is rebuilt alongside it and every session
-- survives.

CREATE TABLE users_new (
    id            INTEGER PRIMARY KEY,
    username      TEXT NOT NULL UNIQUE COLLATE NOCASE,
    password_hash TEXT NOT NULL,
    role          TEXT NOT NULL CHECK (role IN ('viewer', 'operator', 'admin')),
    created_at    TEXT NOT NULL,
    -- Failed logins in a row; reset by a successful one.
    failed_logins INTEGER NOT NULL DEFAULT 0,
    -- When the latest failed login was, for slowing repeated failures.
    last_failure  TEXT,
    -- Locked out until then (PRD §9.2), or NULL.
    locked_until  TEXT
);

INSERT INTO users_new (id, username, password_hash, role, created_at)
SELECT id, username, password_hash, role, created_at FROM users;

CREATE TABLE sessions_new (
    token_hash  TEXT PRIMARY KEY,
    user_id     INTEGER NOT NULL REFERENCES users_new (id) ON DELETE CASCADE,
    created_at  TEXT NOT NULL,
    expires_at  TEXT NOT NULL
);

INSERT INTO sessions_new (token_hash, user_id, created_at, expires_at)
SELECT token_hash, user_id, created_at, expires_at FROM sessions;

-- Nothing refers to the old tables any more, so dropping them cascades to nothing.
DROP TABLE sessions;
DROP TABLE users;

-- Renaming rewrites `sessions_new`'s reference to point at the new name.
ALTER TABLE users_new RENAME TO users;
ALTER TABLE sessions_new RENAME TO sessions;

CREATE INDEX sessions_user ON sessions (user_id);
