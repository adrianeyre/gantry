-- Optional two-factor authentication with TOTP (PRD §9.2). The secret is
-- encrypted with the secret key (ADR-0005). `totp_pending` holds a secret
-- being set up, until a code from it proves the app has it. `totp_last_step`
-- is the last 30-second step a code was used for, so no code works twice.
ALTER TABLE users ADD COLUMN totp_secret TEXT;
ALTER TABLE users ADD COLUMN totp_pending TEXT;
ALTER TABLE users ADD COLUMN totp_enabled_at TEXT;
ALTER TABLE users ADD COLUMN totp_last_step INTEGER;

-- Single-use recovery codes, for a lost authenticator. Only a SHA-256 of
-- each is stored.
CREATE TABLE recovery_codes (
    id          INTEGER PRIMARY KEY,
    user_id     INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    code_hash   TEXT NOT NULL,
    used_at     TEXT
);

CREATE INDEX recovery_codes_user ON recovery_codes (user_id);
