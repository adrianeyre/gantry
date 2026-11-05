-- API tokens for the public REST API (PRD §7.7). A token acts as its owner,
-- with the owner's role as it is at the time of each call. Only a SHA-256 of
-- the token is stored, as for sessions, so a copy of the database can't be
-- used to call the API. `prefix` is the token's first characters, shown so
-- its owner can tell their tokens apart. Revoking a token deletes its row.
CREATE TABLE api_tokens (
    id            INTEGER PRIMARY KEY,
    user_id       INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name          TEXT NOT NULL,
    token_hash    TEXT NOT NULL UNIQUE,
    prefix        TEXT NOT NULL,
    created_at    TEXT NOT NULL,
    -- NULL: it never expires.
    expires_at    TEXT,
    last_used_at  TEXT,
    UNIQUE (user_id, name)
);

CREATE INDEX api_tokens_user ON api_tokens (user_id);

-- A call made with an API token is audited like a UI action, with the
-- token's name beside the user's.
ALTER TABLE audit_log ADD COLUMN api_token TEXT;
