//! Accounts and login sessions.
use argon2::Argon2;
use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use std::time::Duration;

use time::OffsetDateTime;

use super::two_factor::CodeCheck;
use crate::crypto::SecretKey;

use super::{Store, now, parse_time, rfc3339};

/// How long a session lasts without logging in again.
pub const SESSION_DAYS: i64 = 30;

/// The shortest password accepted.
pub const MIN_PASSWORD_LEN: usize = 8;

/// What a user may do (PRD §7.6). Each role can do everything the one before
/// it can, so roles compare: `Viewer < Operator < Admin`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Sees everything and changes nothing.
    Viewer,
    /// Also controls Printers and manages Jobs and the Queue.
    Operator,
    /// Also manages Printers, Tags, users, Alert Rules and settings.
    Admin,
}

impl Role {
    pub const ALL: [Role; 3] = [Role::Viewer, Role::Operator, Role::Admin];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Viewer => "viewer",
            Self::Operator => "operator",
            Self::Admin => "admin",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "viewer" => Some(Self::Viewer),
            "operator" => Some(Self::Operator),
            "admin" => Some(Self::Admin),
            _ => None,
        }
    }

    /// How the UI names it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Viewer => "Viewer",
            Self::Operator => "Operator",
            Self::Admin => "Admin",
        }
    }
}

impl std::str::FromStr for Role {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value).ok_or_else(|| format!("`{value}` is not viewer, operator or admin"))
    }
}

/// A logged-in user, as handlers see them. Holds no password material.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub role: Role,
    /// The name of the API token this request was made with, when it was
    /// (PRD §7.7). The audit log records it beside the user.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_token: Option<String>,
    /// Admins require two-factor authentication and this user hasn't set it
    /// up yet: until they do, they may only enrol (PRD §9.2).
    pub must_enrol_totp: bool,
}

/// Why an account could not be created.
#[derive(Debug, thiserror::Error)]
pub enum CreateUserError {
    #[error("a user named `{0}` already exists")]
    Exists(String),
    #[error("the username must be 1 to 64 letters, digits, `.`, `-` or `_`")]
    InvalidUsername,
    #[error("the password must be at least {MIN_PASSWORD_LEN} characters")]
    WeakPassword,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// A new session's token, which goes in the cookie and nowhere else.
pub struct SessionToken(pub String);

impl std::fmt::Debug for SessionToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SessionToken(..)")
    }
}

fn valid_username(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

fn hash_token(token: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(token.as_bytes()))
}

/// Hashes a password with Argon2id and a random salt.
pub fn hash_password(password: &str) -> String {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .expect("Argon2 hashing with default parameters cannot fail")
        .to_string()
}

fn verify_password(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash)
        .map(|parsed| {
            Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .is_ok()
        })
        .unwrap_or(false)
}

/// A stored role. The database's CHECK constraint allows only the three, so
/// anything else is treated as the least privileged.
fn parse_role(value: &str) -> Role {
    Role::parse(value).unwrap_or(Role::Viewer)
}

/// A hash of a password nobody has, so a login for an unknown username costs
/// the same as one for a known username.
fn dummy_hash() -> &'static str {
    static HASH: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    HASH.get_or_init(|| hash_password("gantry-dummy-password"))
}

impl Store {
    /// Creates an account. Fails if the username is taken.
    pub async fn create_user(
        &self,
        username: &str,
        password: &str,
        role: Role,
    ) -> Result<User, CreateUserError> {
        let username = username.trim();
        if !valid_username(username) {
            return Err(CreateUserError::InvalidUsername);
        }
        if password.chars().count() < MIN_PASSWORD_LEN {
            return Err(CreateUserError::WeakPassword);
        }
        let hash = hash_password(password);
        let result = sqlx::query(
            "INSERT INTO users (username, password_hash, role, created_at) VALUES (?, ?, ?, ?)",
        )
        .bind(username)
        .bind(&hash)
        .bind(role.as_str())
        .bind(rfc3339(now()))
        .execute(self.pool())
        .await;
        match result {
            Ok(done) => Ok(User {
                id: done.last_insert_rowid(),
                username: username.to_owned(),
                role,
                api_token: None,
                must_enrol_totp: false,
            }),
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
                Err(CreateUserError::Exists(username.to_owned()))
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Checks a username and password. `None` for either being wrong.
    pub async fn authenticate(
        &self,
        username: &str,
        password: &str,
    ) -> Result<Option<User>, sqlx::Error> {
        let row: Option<(i64, String, String, String)> = sqlx::query_as(
            "SELECT id, username, password_hash, role FROM users WHERE username = ?",
        )
        .bind(username.trim())
        .fetch_optional(self.pool())
        .await?;
        let Some((id, username, hash, role)) = row else {
            verify_password(password, dummy_hash());
            return Ok(None);
        };
        Ok(verify_password(password, &hash).then_some(User {
            id,
            username,
            role: parse_role(&role),
            api_token: None,
            must_enrol_totp: false,
        }))
    }

    /// A login attempt, with lockout and slowing (PRD §9.2): a locked account
    /// is refused even with the right password, and after `slow_after`
    /// failures in a row each attempt must wait a little longer.
    ///
    /// When the account has TOTP on, the right password alone isn't enough:
    /// without a code the answer is [`LoginCheck::NeedsCode`] (which changes
    /// nothing), and a wrong code counts as a failed login, so the same
    /// lockout limits guessing codes.
    pub async fn login(
        &self,
        username: &str,
        password: &str,
        second: SecondFactor<'_>,
        limits: &LoginLimits,
        at: OffsetDateTime,
    ) -> Result<LoginCheck, sqlx::Error> {
        type Row = (
            i64,
            String,
            String,
            String,
            i64,
            Option<String>,
            Option<String>,
        );
        let row: Option<Row> = sqlx::query_as(
            "SELECT id, username, password_hash, role, failed_logins, last_failure, locked_until \
             FROM users WHERE username = ?",
        )
        .bind(username.trim())
        .fetch_optional(self.pool())
        .await?;
        let Some((id, username, hash, role, failures, last_failure, locked_until)) = row else {
            verify_password(password, dummy_hash());
            return Ok(LoginCheck::Wrong {
                account: None,
                locked: false,
            });
        };
        let account = User {
            id,
            username,
            role: parse_role(&role),
            api_token: None,
            must_enrol_totp: false,
        };
        if let Some(until) = locked_until.as_deref().and_then(parse_time)
            && until > at
        {
            // The same cost as checking a password, so timing says nothing.
            verify_password(password, dummy_hash());
            return Ok(LoginCheck::Locked {
                account: Some(account),
                until,
            });
        }
        let failures = u32::try_from(failures).unwrap_or(u32::MAX);
        if let Some(last) = last_failure.as_deref().and_then(parse_time) {
            let ready = last + limits.wait_after(failures);
            if ready > at {
                let wait = (ready - at).unsigned_abs();
                verify_password(password, dummy_hash());
                return Ok(LoginCheck::TooSoon {
                    account: Some(account),
                    wait,
                });
            }
        }
        let mut wrong_code = false;
        if verify_password(password, &hash) {
            let mut recovery = false;
            if self.totp_enabled(id).await? {
                let Some(code) = second.code.filter(|c| !c.trim().is_empty()) else {
                    return Ok(LoginCheck::NeedsCode { account });
                };
                let check = match second.key {
                    Some(key) => {
                        self.check_second_factor(key, id, code, at.unix_timestamp())
                            .await?
                    }
                    None => CodeCheck::Wrong,
                };
                match check {
                    CodeCheck::Totp => {}
                    CodeCheck::RecoveryCode => recovery = true,
                    CodeCheck::Wrong => wrong_code = true,
                }
            }
            if !wrong_code {
                sqlx::query(
                    "UPDATE users SET failed_logins = 0, last_failure = NULL, locked_until = NULL \
                     WHERE id = ?",
                )
                .bind(id)
                .execute(self.pool())
                .await?;
                return Ok(if recovery {
                    LoginCheck::RecoveryCode(account)
                } else {
                    LoginCheck::Ok(account)
                });
            }
        }
        let failures = failures.saturating_add(1);
        let locked = failures >= limits.max_failures.max(1);
        if locked {
            // The count starts again once the lock ends.
            sqlx::query(
                "UPDATE users SET failed_logins = 0, last_failure = NULL, locked_until = ? \
                 WHERE id = ?",
            )
            .bind(rfc3339(at + limits.lockout))
            .bind(id)
            .execute(self.pool())
            .await?;
        } else {
            sqlx::query("UPDATE users SET failed_logins = ?, last_failure = ? WHERE id = ?")
                .bind(i64::from(failures))
                .bind(rfc3339(at))
                .bind(id)
                .execute(self.pool())
                .await?;
        }
        if wrong_code {
            return Ok(LoginCheck::WrongCode { account, locked });
        }
        Ok(LoginCheck::Wrong {
            account: Some(account),
            locked,
        })
    }

    /// When the account's lockout ends, if it is locked now.
    pub async fn locked_until(&self, id: i64) -> Result<Option<OffsetDateTime>, sqlx::Error> {
        let until: Option<Option<String>> =
            sqlx::query_scalar("SELECT locked_until FROM users WHERE id = ?")
                .bind(id)
                .fetch_optional(self.pool())
                .await?;
        Ok(until
            .flatten()
            .as_deref()
            .and_then(parse_time)
            .filter(|t| *t > now()))
    }

    /// Ends a lockout early and forgets the failures. `false` if no such account.
    pub async fn unlock(&self, id: i64) -> Result<bool, sqlx::Error> {
        let done = sqlx::query(
            "UPDATE users SET failed_logins = 0, last_failure = NULL, locked_until = NULL \
             WHERE id = ?",
        )
        .bind(id)
        .execute(self.pool())
        .await?;
        Ok(done.rows_affected() > 0)
    }

    /// Starts a session for `user`, returning the cookie's token.
    pub async fn create_session(&self, user: &User) -> Result<SessionToken, sqlx::Error> {
        let mut bytes = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        let token = URL_SAFE_NO_PAD.encode(bytes);
        let created = now();
        sqlx::query(
            "INSERT INTO sessions (token_hash, user_id, created_at, expires_at) VALUES (?, ?, ?, ?)",
        )
        .bind(hash_token(&token))
        .bind(user.id)
        .bind(rfc3339(created))
        .bind(rfc3339(created + time::Duration::days(SESSION_DAYS)))
        .execute(self.pool())
        .await?;
        Ok(SessionToken(token))
    }

    /// The user a session token belongs to, if the session is live.
    pub async fn session_user(&self, token: &str) -> Result<Option<User>, sqlx::Error> {
        let row: Option<(i64, String, String, bool)> = sqlx::query_as(
            "SELECT users.id, users.username, users.role, users.totp_secret IS NULL AND EXISTS (SELECT 1 FROM settings WHERE key = 'require_totp' AND value = 'true') FROM sessions \
             JOIN users ON users.id = sessions.user_id \
             WHERE sessions.token_hash = ? AND sessions.expires_at > ?",
        )
        .bind(hash_token(token))
        .bind(rfc3339(now()))
        .fetch_optional(self.pool())
        .await?;
        // The role is read on every request, so a change applies at once.
        Ok(row.map(|(id, username, role, must_enrol_totp)| User {
            id,
            username,
            role: parse_role(&role),
            api_token: None,
            must_enrol_totp,
        }))
    }

    /// Ends a session. Ending one that doesn't exist is not an error.
    pub async fn delete_session(&self, token: &str) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM sessions WHERE token_hash = ?")
            .bind(hash_token(token))
            .execute(self.pool())
            .await?;
        Ok(())
    }
}

/// How hard Gantry pushes back on failed logins (PRD §9.2). Set in the
/// server config.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoginLimits {
    /// Failed logins in a row that lock an account.
    pub max_failures: u32,
    /// How long a lockout lasts.
    pub lockout: Duration,
    /// Failed logins in a row after which each further attempt must wait,
    /// one second at first and doubling each time.
    pub slow_after: u32,
    /// Failed logins one client may make per minute, whatever the accounts.
    pub client_attempts_per_minute: u32,
}

impl Default for LoginLimits {
    fn default() -> Self {
        Self {
            max_failures: 5,
            lockout: Duration::from_secs(15 * 60),
            slow_after: 3,
            client_attempts_per_minute: 20,
        }
    }
}

impl LoginLimits {
    /// How long to wait after `failures` failures in a row before trying again.
    pub fn wait_after(&self, failures: u32) -> Duration {
        if failures < self.slow_after.max(1) {
            return Duration::ZERO;
        }
        let doublings = (failures - self.slow_after.max(1)).min(6);
        Duration::from_secs(1u64 << doublings)
    }
}

/// The second factor offered with a login, if any.
#[derive(Debug, Clone, Copy, Default)]
pub struct SecondFactor<'a> {
    /// Decrypts the TOTP secret.
    pub key: Option<&'a SecretKey>,
    /// A TOTP code or a recovery code.
    pub code: Option<&'a str>,
}

impl SecondFactor<'_> {
    /// No second factor offered.
    pub const NONE: SecondFactor<'static> = SecondFactor {
        key: None,
        code: None,
    };
}

/// What a login attempt came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginCheck {
    /// The right password (and code, with TOTP on): the failure count is
    /// reset.
    Ok(User),
    /// The right password and an unused recovery code, which is now used.
    RecoveryCode(User),
    /// The right password, but TOTP is on and no code was given. Nothing
    /// is counted either way.
    NeedsCode { account: User },
    /// The right password but the wrong code: a failed login.
    WrongCode { account: User, locked: bool },
    /// The wrong password, or no such account. `locked` when this failure
    /// locked the account.
    Wrong { account: Option<User>, locked: bool },
    /// The account is locked out until then. `account` is `None` for a name
    /// that isn't an account, which is treated the same so a reply never
    /// says which names exist.
    Locked {
        account: Option<User>,
        until: OffsetDateTime,
    },
    /// Too soon after the last failure.
    TooSoon {
        account: Option<User>,
        wait: Duration,
    },
}

/// An account as Admins manage it. Holds no password material.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Account {
    pub id: i64,
    pub username: String,
    pub role: Role,
    pub created_at: String,
    /// When a lockout ends, while the account is locked out.
    pub locked_until: Option<String>,
}

/// Why an account could not be changed.
#[derive(Debug, thiserror::Error)]
pub enum ChangeUserError {
    #[error("no such user")]
    NotFound,
    #[error("the last Admin can't be removed or demoted; make another Admin first")]
    LastAdmin,
    #[error("the password must be at least {MIN_PASSWORD_LEN} characters")]
    WeakPassword,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// Keeps a change only if at least one Admin would be left, in one statement
/// so two Admins demoting each other at once can't both succeed.
const ANOTHER_ADMIN_REMAINS: &str =
    "(role != 'admin' OR (SELECT COUNT(*) FROM users WHERE role = 'admin') > 1)";

impl Store {
    /// Every account, by username.
    pub async fn users(&self) -> Result<Vec<Account>, sqlx::Error> {
        let rows: Vec<(i64, String, String, String, Option<String>)> = sqlx::query_as(
            "SELECT id, username, role, created_at, locked_until FROM users              ORDER BY username COLLATE NOCASE",
        )
        .fetch_all(self.pool())
        .await?;
        let now = rfc3339(now());
        Ok(rows
            .into_iter()
            .map(|(id, username, role, created_at, locked_until)| Account {
                id,
                username,
                role: parse_role(&role),
                created_at,
                locked_until: locked_until.filter(|until| until.as_str() > now.as_str()),
            })
            .collect())
    }

    /// One account.
    pub async fn user(&self, id: i64) -> Result<Option<Account>, sqlx::Error> {
        Ok(self.users().await?.into_iter().find(|u| u.id == id))
    }

    /// Changes an account's role. The last Admin can't be demoted.
    pub async fn set_role(&self, id: i64, role: Role) -> Result<Account, ChangeUserError> {
        let done = sqlx::query(&format!(
            "UPDATE users SET role = ? WHERE id = ? AND (? = 'admin' OR {ANOTHER_ADMIN_REMAINS})"
        ))
        .bind(role.as_str())
        .bind(id)
        .bind(role.as_str())
        .execute(self.pool())
        .await?;
        let account = self.user(id).await?.ok_or(ChangeUserError::NotFound)?;
        if done.rows_affected() == 0 {
            return Err(ChangeUserError::LastAdmin);
        }
        Ok(account)
    }

    /// Sets a new password and ends the account's sessions, so a leaked
    /// session dies with the old password.
    pub async fn set_password(&self, id: i64, password: &str) -> Result<(), ChangeUserError> {
        if password.chars().count() < MIN_PASSWORD_LEN {
            return Err(ChangeUserError::WeakPassword);
        }
        let mut tx = self.pool().begin().await?;
        let done = sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
            .bind(hash_password(password))
            .bind(id)
            .execute(&mut *tx)
            .await?;
        if done.rows_affected() == 0 {
            return Err(ChangeUserError::NotFound);
        }
        sqlx::query("DELETE FROM sessions WHERE user_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Removes an account and its sessions. The last Admin can't be removed.
    pub async fn delete_user(&self, id: i64) -> Result<Account, ChangeUserError> {
        let account = self.user(id).await?.ok_or(ChangeUserError::NotFound)?;
        let done = sqlx::query(&format!(
            "DELETE FROM users WHERE id = ? AND {ANOTHER_ADMIN_REMAINS}"
        ))
        .bind(id)
        .execute(self.pool())
        .await?;
        if done.rows_affected() == 0 {
            return Err(ChangeUserError::LastAdmin);
        }
        Ok(account)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn creates_a_user_once() {
        let store = Store::in_memory().await.unwrap();
        store
            .create_user("adrian", "a long password", Role::Admin)
            .await
            .unwrap();
        let again = store
            .create_user("Adrian", "another password", Role::Admin)
            .await;
        assert!(matches!(again, Err(CreateUserError::Exists(_))));
    }

    #[tokio::test]
    async fn refuses_weak_passwords_and_odd_usernames() {
        let store = Store::in_memory().await.unwrap();
        assert!(matches!(
            store.create_user("adrian", "short", Role::Admin).await,
            Err(CreateUserError::WeakPassword)
        ));
        assert!(matches!(
            store
                .create_user("bad name", "a long password", Role::Admin)
                .await,
            Err(CreateUserError::InvalidUsername)
        ));
    }

    #[tokio::test]
    async fn stores_only_an_argon2_hash() {
        let store = Store::in_memory().await.unwrap();
        store
            .create_user("adrian", "a long password", Role::Admin)
            .await
            .unwrap();
        let hash: String = sqlx::query_scalar("SELECT password_hash FROM users")
            .fetch_one(store.pool())
            .await
            .unwrap();
        assert!(hash.starts_with("$argon2id$"));
        assert!(!hash.contains("a long password"));
    }

    #[tokio::test]
    async fn authenticates_only_the_right_password() {
        let store = Store::in_memory().await.unwrap();
        store
            .create_user("adrian", "a long password", Role::Admin)
            .await
            .unwrap();
        assert!(
            store
                .authenticate("adrian", "a long password")
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            store
                .authenticate("adrian", "wrong password")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .authenticate("nobody", "a long password")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn sessions_end_when_deleted() {
        let store = Store::in_memory().await.unwrap();
        let user = store
            .create_user("adrian", "a long password", Role::Admin)
            .await
            .unwrap();
        let token = store.create_session(&user).await.unwrap();
        assert_eq!(store.session_user(&token.0).await.unwrap(), Some(user));
        store.delete_session(&token.0).await.unwrap();
        assert_eq!(store.session_user(&token.0).await.unwrap(), None);
    }

    #[tokio::test]
    async fn the_database_holds_no_session_token() {
        let store = Store::in_memory().await.unwrap();
        let user = store
            .create_user("adrian", "a long password", Role::Admin)
            .await
            .unwrap();
        let token = store.create_session(&user).await.unwrap();
        let stored: String = sqlx::query_scalar("SELECT token_hash FROM sessions")
            .fetch_one(store.pool())
            .await
            .unwrap();
        assert_ne!(stored, token.0);
    }

    #[tokio::test]
    async fn expired_sessions_are_refused() {
        let store = Store::in_memory().await.unwrap();
        let user = store
            .create_user("adrian", "a long password", Role::Admin)
            .await
            .unwrap();
        let token = store.create_session(&user).await.unwrap();
        sqlx::query("UPDATE sessions SET expires_at = '2000-01-01T00:00:00Z'")
            .execute(store.pool())
            .await
            .unwrap();
        assert_eq!(store.session_user(&token.0).await.unwrap(), None);
    }
    #[tokio::test]
    async fn roles_are_stored_and_read_back() {
        let store = Store::in_memory().await.unwrap();
        for (name, role) in [
            ("v", Role::Viewer),
            ("o", Role::Operator),
            ("a", Role::Admin),
        ] {
            store
                .create_user(name, "a long password", role)
                .await
                .unwrap();
            let user = store
                .authenticate(name, "a long password")
                .await
                .unwrap()
                .unwrap();
            assert_eq!(user.role, role);
            let token = store.create_session(&user).await.unwrap();
            assert_eq!(
                store.session_user(&token.0).await.unwrap().unwrap().role,
                role
            );
        }
        assert!(Role::Viewer < Role::Operator && Role::Operator < Role::Admin);
    }

    #[tokio::test]
    async fn the_last_admin_cannot_be_demoted_or_removed() {
        let store = Store::in_memory().await.unwrap();
        let first = store
            .create_user("first", "a long password", Role::Admin)
            .await
            .unwrap();
        assert!(matches!(
            store.set_role(first.id, Role::Operator).await,
            Err(ChangeUserError::LastAdmin)
        ));
        assert!(matches!(
            store.delete_user(first.id).await,
            Err(ChangeUserError::LastAdmin)
        ));
        // With a second Admin, either can go.
        let second = store
            .create_user("second", "a long password", Role::Admin)
            .await
            .unwrap();
        assert_eq!(
            store.set_role(first.id, Role::Viewer).await.unwrap().role,
            Role::Viewer
        );
        // Now `second` is the last Admin again.
        assert!(matches!(
            store.delete_user(second.id).await,
            Err(ChangeUserError::LastAdmin)
        ));
        store.delete_user(first.id).await.unwrap();
        assert!(matches!(
            store.set_role(first.id, Role::Admin).await,
            Err(ChangeUserError::NotFound)
        ));
    }

    #[tokio::test]
    async fn repeated_failures_are_slowed_then_locked_out() {
        let store = Store::in_memory().await.unwrap();
        let user = store
            .create_user("adrian", "a long password", Role::Operator)
            .await
            .unwrap();
        let limits = LoginLimits {
            max_failures: 4,
            lockout: Duration::from_secs(600),
            slow_after: 2,
            client_attempts_per_minute: 100,
        };
        // Whole seconds: timestamps are stored to the millisecond.
        let t0 = OffsetDateTime::now_utc().replace_nanosecond(0).unwrap();
        let at = |secs: i64| t0 + time::Duration::seconds(secs);
        let wrong = |r: LoginCheck| matches!(r, LoginCheck::Wrong { locked: false, .. });

        // Two failures: no wait yet.
        assert!(wrong(
            store
                .login("adrian", "nope", SecondFactor::NONE, &limits, at(0))
                .await
                .unwrap()
        ));
        assert!(wrong(
            store
                .login("adrian", "nope", SecondFactor::NONE, &limits, at(0))
                .await
                .unwrap()
        ));
        // After the second, the next try must wait a second, even with the
        // right password.
        assert!(matches!(
            store.login("adrian", "a long password", SecondFactor::NONE, &limits, at(0)).await.unwrap(),
            LoginCheck::TooSoon { wait, .. } if wait == Duration::from_secs(1)
        ));
        assert!(wrong(
            store
                .login("adrian", "nope", SecondFactor::NONE, &limits, at(1))
                .await
                .unwrap()
        ));
        // Then two seconds.
        assert!(matches!(
            store
                .login("adrian", "nope", SecondFactor::NONE, &limits, at(2))
                .await
                .unwrap(),
            LoginCheck::TooSoon { .. }
        ));
        // The fourth failure locks the account.
        assert!(matches!(
            store
                .login("adrian", "nope", SecondFactor::NONE, &limits, at(3))
                .await
                .unwrap(),
            LoginCheck::Wrong { locked: true, .. }
        ));
        // Locked: even the right password is refused, until the lock ends.
        assert!(matches!(
            store
                .login(
                    "adrian",
                    "a long password",
                    SecondFactor::NONE,
                    &limits,
                    at(300)
                )
                .await
                .unwrap(),
            LoginCheck::Locked { .. }
        ));
        assert!(store.users().await.unwrap()[0].locked_until.is_some());
        assert!(matches!(
            store
                .login(
                    "adrian",
                    "a long password",
                    SecondFactor::NONE,
                    &limits,
                    at(604)
                )
                .await
                .unwrap(),
            LoginCheck::Ok(_)
        ));

        // An Admin can end a lock early.
        for i in 0..4 {
            store
                .login(
                    "adrian",
                    "nope",
                    SecondFactor::NONE,
                    &limits,
                    at(700 + i * 100),
                )
                .await
                .unwrap();
        }
        assert!(matches!(
            store
                .login(
                    "adrian",
                    "a long password",
                    SecondFactor::NONE,
                    &limits,
                    at(1100)
                )
                .await
                .unwrap(),
            LoginCheck::Locked { .. }
        ));
        assert!(store.unlock(user.id).await.unwrap());
        assert!(matches!(
            store
                .login(
                    "adrian",
                    "a long password",
                    SecondFactor::NONE,
                    &limits,
                    at(1100)
                )
                .await
                .unwrap(),
            LoginCheck::Ok(_)
        ));
    }

    #[tokio::test]
    async fn a_success_resets_the_count() {
        let store = Store::in_memory().await.unwrap();
        store
            .create_user("adrian", "a long password", Role::Operator)
            .await
            .unwrap();
        let limits = LoginLimits {
            max_failures: 3,
            slow_after: 3,
            ..LoginLimits::default()
        };
        let now = OffsetDateTime::now_utc();
        for _ in 0..2 {
            store
                .login("adrian", "nope", SecondFactor::NONE, &limits, now)
                .await
                .unwrap();
        }
        assert!(matches!(
            store
                .login(
                    "adrian",
                    "a long password",
                    SecondFactor::NONE,
                    &limits,
                    now
                )
                .await
                .unwrap(),
            LoginCheck::Ok(_)
        ));
        for _ in 0..2 {
            assert!(matches!(
                store
                    .login("adrian", "nope", SecondFactor::NONE, &limits, now)
                    .await
                    .unwrap(),
                LoginCheck::Wrong { locked: false, .. }
            ));
        }
    }

    #[tokio::test]
    async fn a_new_password_ends_the_old_sessions() {
        let store = Store::in_memory().await.unwrap();
        let user = store
            .create_user("adrian", "a long password", Role::Operator)
            .await
            .unwrap();
        let token = store.create_session(&user).await.unwrap();
        store
            .set_password(user.id, "another long one")
            .await
            .unwrap();
        assert_eq!(store.session_user(&token.0).await.unwrap(), None);
        assert!(
            store
                .authenticate("adrian", "another long one")
                .await
                .unwrap()
                .is_some()
        );
    }
}
