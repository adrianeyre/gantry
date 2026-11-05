-- Passkeys (WebAuthn), a way to log in instead of a password (PRD §9.2).
-- `credential_id` is the authenticator's id for the credential, base64url.
-- `public_key` is its COSE public key, which can only check signatures, so
-- it isn't a secret. `sign_count` spots a cloned authenticator.
CREATE TABLE passkeys (
    id             INTEGER PRIMARY KEY,
    user_id        INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name           TEXT NOT NULL,
    credential_id  TEXT NOT NULL UNIQUE,
    public_key     BLOB NOT NULL,
    algorithm      INTEGER NOT NULL,
    sign_count     INTEGER NOT NULL DEFAULT 0,
    created_at     TEXT NOT NULL,
    last_used_at   TEXT
);

CREATE INDEX passkeys_user ON passkeys (user_id);

-- The handle passkeys know a user by: random, so it says nothing about the
-- account (WebAuthn §14.6.1).
ALTER TABLE users ADD COLUMN webauthn_handle TEXT;
