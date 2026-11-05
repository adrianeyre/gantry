-- Accounts. The first is created from the command line (`gantry user create
-- --admin`); there is no first-run setup page. Until roles arrive in Phase 2,
-- every account is an Admin.
CREATE TABLE users (
    id            INTEGER PRIMARY KEY,
    username      TEXT NOT NULL UNIQUE COLLATE NOCASE,
    password_hash TEXT NOT NULL,
    role          TEXT NOT NULL CHECK (role IN ('admin')),
    created_at    TEXT NOT NULL
);

-- Login sessions. Only a SHA-256 of the cookie's token is stored, so a copy of
-- the database cannot be used to log in. Logout deletes the row.
CREATE TABLE sessions (
    token_hash  TEXT PRIMARY KEY,
    user_id     INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_at  TEXT NOT NULL,
    expires_at  TEXT NOT NULL
);

CREATE INDEX sessions_user ON sessions (user_id);
