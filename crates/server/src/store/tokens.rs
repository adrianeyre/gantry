//! API tokens (PRD §7.7). A token is `gantry_` and 43 characters of random
//! base64url (256 bits). Only its SHA-256 is stored, and the token itself is
//! shown once, when it is created.
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore;
use serde::Serialize;
use sha2::{Digest, Sha256};
use time::OffsetDateTime;

use super::users::{Role, User};
use super::{Store, now, parse_time, rfc3339};

/// Every API token starts with this, so it is easy to recognise (and for
/// secret scanners to find).
pub const PREFIX: &str = "gantry_";

/// The most tokens one user may have.
pub const MAX_PER_USER: i64 = 25;

/// A token as its owner sees it later: never the token itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TokenSummary {
    pub id: i64,
    pub user_id: i64,
    pub username: String,
    pub name: String,
    /// The token's first characters, such as `gantry_Ab3d`.
    pub prefix: String,
    pub created_at: String,
    pub expires_at: Option<String>,
    pub last_used_at: Option<String>,
}

/// A new token, which goes to its owner once and is never stored.
pub struct NewToken {
    pub summary: TokenSummary,
    pub token: String,
}

impl std::fmt::Debug for NewToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NewToken")
            .field("summary", &self.summary)
            .finish_non_exhaustive()
    }
}

/// Why a token couldn't be made.
#[derive(Debug, thiserror::Error)]
pub enum CreateTokenError {
    #[error("you already have a token named `{0}`")]
    Exists(String),
    #[error("you can have at most {MAX_PER_USER} tokens; revoke one first")]
    TooMany,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

fn hash(token: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(token.as_bytes()))
}

type Row = (
    i64,
    i64,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
);

fn summary(row: Row) -> TokenSummary {
    let (id, user_id, username, name, prefix, created_at, expires_at, last_used_at) = row;
    TokenSummary {
        id,
        user_id,
        username,
        name,
        prefix,
        created_at,
        expires_at,
        last_used_at,
    }
}

const SELECT: &str = "SELECT api_tokens.id, api_tokens.user_id, users.username, api_tokens.name, \
     api_tokens.prefix, api_tokens.created_at, api_tokens.expires_at, api_tokens.last_used_at \
     FROM api_tokens JOIN users ON users.id = api_tokens.user_id";

impl Store {
    /// Makes a token for `user`, expiring at `expires_at` (or never).
    pub async fn create_api_token(
        &self,
        user: &User,
        name: &str,
        expires_at: Option<OffsetDateTime>,
    ) -> Result<NewToken, CreateTokenError> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM api_tokens WHERE user_id = ?")
            .bind(user.id)
            .fetch_one(self.pool())
            .await?;
        if count >= MAX_PER_USER {
            return Err(CreateTokenError::TooMany);
        }
        let mut bytes = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        let token = format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes));
        let prefix: String = token.chars().take(PREFIX.len() + 4).collect();
        let created = now();
        let inserted = sqlx::query(
            "INSERT INTO api_tokens (user_id, name, token_hash, prefix, created_at, expires_at) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(user.id)
        .bind(name)
        .bind(hash(&token))
        .bind(&prefix)
        .bind(rfc3339(created))
        .bind(expires_at.map(rfc3339))
        .execute(self.pool())
        .await;
        let id = match inserted {
            Ok(done) => done.last_insert_rowid(),
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
                return Err(CreateTokenError::Exists(name.to_owned()));
            }
            Err(e) => return Err(e.into()),
        };
        Ok(NewToken {
            summary: TokenSummary {
                id,
                user_id: user.id,
                username: user.username.clone(),
                name: name.to_owned(),
                prefix,
                created_at: rfc3339(created),
                expires_at: expires_at.map(rfc3339),
                last_used_at: None,
            },
            token,
        })
    }

    /// `user_id`'s tokens, or everyone's when `None`, newest first.
    pub async fn api_tokens(&self, user_id: Option<i64>) -> Result<Vec<TokenSummary>, sqlx::Error> {
        let rows: Vec<Row> = sqlx::query_as(&format!(
            "{SELECT} WHERE (?1 IS NULL OR api_tokens.user_id = ?1) ORDER BY api_tokens.id DESC"
        ))
        .bind(user_id)
        .fetch_all(self.pool())
        .await?;
        Ok(rows.into_iter().map(summary).collect())
    }

    /// One token, by id.
    pub async fn api_token(&self, id: i64) -> Result<Option<TokenSummary>, sqlx::Error> {
        let row: Option<Row> = sqlx::query_as(&format!("{SELECT} WHERE api_tokens.id = ?"))
            .bind(id)
            .fetch_optional(self.pool())
            .await?;
        Ok(row.map(summary))
    }

    /// Revokes a token. `false` if there was no such token.
    pub async fn revoke_api_token(&self, id: i64) -> Result<bool, sqlx::Error> {
        let done = sqlx::query("DELETE FROM api_tokens WHERE id = ?")
            .bind(id)
            .execute(self.pool())
            .await?;
        Ok(done.rows_affected() > 0)
    }

    /// The user a live token acts as, with their role as it is now, and
    /// the token's name. Records when the token was last used.
    pub async fn api_token_user(&self, token: &str) -> Result<Option<User>, sqlx::Error> {
        if !token.starts_with(PREFIX) {
            return Ok(None);
        }
        type Row = (i64, Option<String>, String, i64, String, String, bool);
        let row: Option<Row> = sqlx::query_as(
            "SELECT api_tokens.id, api_tokens.expires_at, api_tokens.name, users.id, \
             users.username, users.role, users.totp_secret IS NULL AND EXISTS (SELECT 1 FROM settings WHERE key = 'require_totp' AND value = 'true') FROM api_tokens \
             JOIN users ON users.id = api_tokens.user_id WHERE api_tokens.token_hash = ?",
        )
        .bind(hash(token))
        .fetch_optional(self.pool())
        .await?;
        let Some((token_id, expires_at, name, id, username, role, must_enrol_totp)) = row else {
            return Ok(None);
        };
        let at = now();
        if expires_at
            .as_deref()
            .and_then(parse_time)
            .is_some_and(|expires| expires <= at)
        {
            return Ok(None);
        }
        sqlx::query("UPDATE api_tokens SET last_used_at = ? WHERE id = ?")
            .bind(rfc3339(at))
            .bind(token_id)
            .execute(self.pool())
            .await?;
        Ok(Some(User {
            id,
            username,
            role: Role::parse(&role).unwrap_or(Role::Viewer),
            api_token: Some(name),
            must_enrol_totp,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn tokens_are_stored_hashed_and_act_with_the_owners_current_role() {
        let store = Store::in_memory().await.unwrap();
        let user = store
            .create_user("adrian", "a long enough password", Role::Operator)
            .await
            .unwrap();
        let new = store
            .create_api_token(&user, "home assistant", None)
            .await
            .unwrap();
        assert!(new.token.starts_with("gantry_"));
        assert_eq!(new.token.len(), PREFIX.len() + 43);
        assert!(new.token.starts_with(&new.summary.prefix));
        let raw: Vec<(String, String)> =
            sqlx::query_as("SELECT token_hash, prefix FROM api_tokens")
                .fetch_all(store.pool())
                .await
                .unwrap();
        assert_eq!(raw.len(), 1);
        assert_ne!(raw[0].0, new.token);
        assert!(!raw[0].0.contains(&new.token[PREFIX.len()..]));

        let caller = store.api_token_user(&new.token).await.unwrap().unwrap();
        assert_eq!(caller.id, user.id);
        assert_eq!(caller.role, Role::Operator);
        assert_eq!(caller.api_token.as_deref(), Some("home assistant"));
        // A change of role applies to the token at once.
        store.set_role(user.id, Role::Viewer).await.unwrap();
        let caller = store.api_token_user(&new.token).await.unwrap().unwrap();
        assert_eq!(caller.role, Role::Viewer);
        let listed = store.api_tokens(Some(user.id)).await.unwrap();
        assert!(listed[0].last_used_at.is_some());

        assert!(
            store
                .api_token_user("gantry_not-a-token")
                .await
                .unwrap()
                .is_none()
        );
        assert!(store.revoke_api_token(new.summary.id).await.unwrap());
        assert!(store.api_token_user(&new.token).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn expired_tokens_and_duplicate_names_are_refused() {
        let store = Store::in_memory().await.unwrap();
        let user = store
            .create_user("adrian", "a long enough password", Role::Admin)
            .await
            .unwrap();
        let old = store
            .create_api_token(&user, "old", Some(now() - time::Duration::minutes(1)))
            .await
            .unwrap();
        assert!(store.api_token_user(&old.token).await.unwrap().is_none());
        assert!(matches!(
            store.create_api_token(&user, "old", None).await,
            Err(CreateTokenError::Exists(_))
        ));
    }
}
