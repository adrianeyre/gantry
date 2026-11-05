//! Passkeys (PRD §9.2): the public keys of users' WebAuthn credentials.
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore;
use serde::Serialize;

use super::users::{Role, User};
use super::{Store, now, rfc3339};

/// A passkey as its owner (or an Admin) sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PasskeySummary {
    pub id: i64,
    pub name: String,
    pub created_at: String,
    pub last_used_at: Option<String>,
}

/// A passkey, for checking a login.
#[derive(Debug, Clone)]
pub struct StoredPasskey {
    pub id: i64,
    pub user_id: i64,
    pub public_key: Vec<u8>,
    pub algorithm: i64,
    pub sign_count: u32,
}

impl Store {
    /// The user's WebAuthn handle, made the first time it's needed.
    pub async fn webauthn_handle(&self, user_id: i64) -> Result<String, sqlx::Error> {
        let handle: Option<Option<String>> =
            sqlx::query_scalar("SELECT webauthn_handle FROM users WHERE id = ?")
                .bind(user_id)
                .fetch_optional(self.pool())
                .await?;
        if let Some(Some(handle)) = handle {
            return Ok(handle);
        }
        let mut bytes = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        let handle = URL_SAFE_NO_PAD.encode(bytes);
        sqlx::query(
            "UPDATE users SET webauthn_handle = ? WHERE id = ? AND webauthn_handle IS NULL",
        )
        .bind(&handle)
        .bind(user_id)
        .execute(self.pool())
        .await?;
        // Another request may have made one first.
        sqlx::query_scalar("SELECT webauthn_handle FROM users WHERE id = ?")
            .bind(user_id)
            .fetch_one(self.pool())
            .await
    }

    pub async fn passkeys(&self, user_id: i64) -> Result<Vec<PasskeySummary>, sqlx::Error> {
        let rows: Vec<(i64, String, String, Option<String>)> = sqlx::query_as(
            "SELECT id, name, created_at, last_used_at FROM passkeys WHERE user_id = ? ORDER BY id",
        )
        .bind(user_id)
        .fetch_all(self.pool())
        .await?;
        Ok(rows
            .into_iter()
            .map(|(id, name, created_at, last_used_at)| PasskeySummary {
                id,
                name,
                created_at,
                last_used_at,
            })
            .collect())
    }

    /// The credential ids a user already has, so an authenticator isn't
    /// registered twice.
    pub async fn passkey_credential_ids(&self, user_id: i64) -> Result<Vec<String>, sqlx::Error> {
        sqlx::query_scalar("SELECT credential_id FROM passkeys WHERE user_id = ?")
            .bind(user_id)
            .fetch_all(self.pool())
            .await
    }

    pub async fn add_passkey(
        &self,
        user_id: i64,
        name: &str,
        credential_id: &str,
        public_key: &[u8],
        algorithm: i64,
        sign_count: u32,
    ) -> Result<PasskeySummary, sqlx::Error> {
        let at = rfc3339(now());
        let done = sqlx::query(
            "INSERT INTO passkeys (user_id, name, credential_id, public_key, algorithm, \
             sign_count, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(user_id)
        .bind(name)
        .bind(credential_id)
        .bind(public_key)
        .bind(algorithm)
        .bind(i64::from(sign_count))
        .bind(&at)
        .execute(self.pool())
        .await?;
        Ok(PasskeySummary {
            id: done.last_insert_rowid(),
            name: name.to_owned(),
            created_at: at,
            last_used_at: None,
        })
    }

    /// The passkey with this credential id, and the user it belongs to.
    pub async fn passkey_by_credential(
        &self,
        credential_id: &str,
    ) -> Result<Option<(StoredPasskey, User)>, sqlx::Error> {
        type Row = (i64, i64, Vec<u8>, i64, i64, String, String);
        let row: Option<Row> = sqlx::query_as(
            "SELECT passkeys.id, users.id, passkeys.public_key, passkeys.algorithm, \
             passkeys.sign_count, users.username, users.role FROM passkeys \
             JOIN users ON users.id = passkeys.user_id WHERE passkeys.credential_id = ?",
        )
        .bind(credential_id)
        .fetch_optional(self.pool())
        .await?;
        Ok(row.map(
            |(id, user_id, public_key, algorithm, sign_count, username, role)| {
                (
                    StoredPasskey {
                        id,
                        user_id,
                        public_key,
                        algorithm,
                        sign_count: u32::try_from(sign_count).unwrap_or(u32::MAX),
                    },
                    User {
                        id: user_id,
                        username,
                        role: Role::parse(&role).unwrap_or(Role::Viewer),
                        api_token: None,
                        must_enrol_totp: false,
                    },
                )
            },
        ))
    }

    pub async fn passkey_used(&self, id: i64, sign_count: u32) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE passkeys SET sign_count = ?, last_used_at = ? WHERE id = ?")
            .bind(i64::from(sign_count))
            .bind(rfc3339(now()))
            .bind(id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// Removes one of `user_id`'s passkeys, returning its name.
    pub async fn remove_passkey(
        &self,
        user_id: i64,
        id: i64,
    ) -> Result<Option<String>, sqlx::Error> {
        let name: Option<String> =
            sqlx::query_scalar("DELETE FROM passkeys WHERE id = ? AND user_id = ? RETURNING name")
                .bind(id)
                .bind(user_id)
                .fetch_optional(self.pool())
                .await?;
        Ok(name)
    }
}
