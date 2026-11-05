//! Two-factor authentication with TOTP (PRD §9.2): setting it up, checking
//! codes, and recovery codes. The TOTP secret is encrypted at rest with the
//! secret key (ADR-0005); recovery codes are stored only as hashes.
use super::{Store, now, rfc3339};
use crate::crypto::SecretKey;
use crate::totp;

/// The settings key for an Admin requiring TOTP of every user.
pub const REQUIRE_TOTP: &str = "require_totp";

/// One user's two-factor state, for their account page.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TwoFactor {
    pub totp_enabled: bool,
    pub totp_enabled_at: Option<String>,
    pub recovery_codes_left: i64,
}

/// How a second factor checked out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeCheck {
    /// A current TOTP code.
    Totp,
    /// A recovery code, now used up.
    RecoveryCode,
    Wrong,
}

impl Store {
    /// Whether TOTP is on for `user_id`.
    pub async fn totp_enabled(&self, user_id: i64) -> Result<bool, sqlx::Error> {
        let on: Option<bool> =
            sqlx::query_scalar("SELECT totp_secret IS NOT NULL FROM users WHERE id = ?")
                .bind(user_id)
                .fetch_optional(self.pool())
                .await?;
        Ok(on.unwrap_or(false))
    }

    pub async fn two_factor(&self, user_id: i64) -> Result<TwoFactor, sqlx::Error> {
        let (enabled_at,): (Option<String>,) =
            sqlx::query_as("SELECT totp_enabled_at FROM users WHERE id = ?")
                .bind(user_id)
                .fetch_one(self.pool())
                .await?;
        let left: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM recovery_codes WHERE user_id = ? AND used_at IS NULL",
        )
        .bind(user_id)
        .fetch_one(self.pool())
        .await?;
        Ok(TwoFactor {
            totp_enabled: enabled_at.is_some(),
            totp_enabled_at: enabled_at,
            recovery_codes_left: left,
        })
    }

    /// Starts setting TOTP up: a new secret, kept pending until a code from
    /// it is given. Returns the secret, base32.
    pub async fn start_totp(&self, key: &SecretKey, user_id: i64) -> Result<String, sqlx::Error> {
        let secret = totp::new_secret();
        sqlx::query("UPDATE users SET totp_pending = ? WHERE id = ?")
            .bind(key.encrypt(&secret))
            .bind(user_id)
            .execute(self.pool())
            .await?;
        Ok(secret)
    }

    /// Turns TOTP on if `code` comes from the pending secret, and returns
    /// fresh recovery codes. `None` if the code is wrong or nothing is
    /// pending.
    pub async fn enable_totp(
        &self,
        key: &SecretKey,
        user_id: i64,
        code: &str,
        unix: i64,
    ) -> Result<Option<Vec<String>>, sqlx::Error> {
        let pending: Option<Option<String>> =
            sqlx::query_scalar("SELECT totp_pending FROM users WHERE id = ?")
                .bind(user_id)
                .fetch_optional(self.pool())
                .await?;
        let Some(secret) = pending.flatten().and_then(|p| key.decrypt(&p).ok()) else {
            return Ok(None);
        };
        let Some(step) = totp::check(&secret, code, unix, None) else {
            return Ok(None);
        };
        sqlx::query(
            "UPDATE users SET totp_secret = ?, totp_pending = NULL, totp_enabled_at = ?, \
             totp_last_step = ? WHERE id = ?",
        )
        .bind(key.encrypt(&secret))
        .bind(rfc3339(now()))
        .bind(step)
        .bind(user_id)
        .execute(self.pool())
        .await?;
        self.new_recovery_codes(user_id).await.map(Some)
    }

    /// Replaces a user's recovery codes, returning the new ones.
    pub async fn new_recovery_codes(&self, user_id: i64) -> Result<Vec<String>, sqlx::Error> {
        let codes: Vec<String> = (0..totp::RECOVERY_CODES)
            .map(|_| totp::new_recovery_code())
            .collect();
        let mut tx = self.pool().begin().await?;
        sqlx::query("DELETE FROM recovery_codes WHERE user_id = ?")
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        for code in &codes {
            sqlx::query("INSERT INTO recovery_codes (user_id, code_hash) VALUES (?, ?)")
                .bind(user_id)
                .bind(totp::hash_recovery_code(code))
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(codes)
    }

    /// Turns TOTP off and forgets the recovery codes.
    pub async fn disable_totp(&self, user_id: i64) -> Result<bool, sqlx::Error> {
        let done = sqlx::query(
            "UPDATE users SET totp_secret = NULL, totp_pending = NULL, totp_enabled_at = NULL, \
             totp_last_step = NULL WHERE id = ? AND totp_secret IS NOT NULL",
        )
        .bind(user_id)
        .execute(self.pool())
        .await?;
        sqlx::query("DELETE FROM recovery_codes WHERE user_id = ?")
            .bind(user_id)
            .execute(self.pool())
            .await?;
        Ok(done.rows_affected() > 0)
    }

    /// Checks a second factor: a TOTP code (recording its step, so it can't
    /// be used again) or an unused recovery code (using it up).
    pub async fn check_second_factor(
        &self,
        key: &SecretKey,
        user_id: i64,
        code: &str,
        unix: i64,
    ) -> Result<CodeCheck, sqlx::Error> {
        let row: Option<(Option<String>, Option<i64>)> =
            sqlx::query_as("SELECT totp_secret, totp_last_step FROM users WHERE id = ?")
                .bind(user_id)
                .fetch_optional(self.pool())
                .await?;
        let Some((Some(stored), last_step)) = row else {
            return Ok(CodeCheck::Wrong);
        };
        let Ok(secret) = key.decrypt(&stored) else {
            tracing::warn!(
                user_id,
                "a TOTP secret couldn't be decrypted with this secret key"
            );
            return Ok(CodeCheck::Wrong);
        };
        if let Some(step) = totp::check(&secret, code, unix, last_step) {
            // Only a later step is recorded, so two logins racing can't
            // both use one code.
            let done = sqlx::query(
                "UPDATE users SET totp_last_step = ? WHERE id = ? \
                 AND (totp_last_step IS NULL OR totp_last_step < ?)",
            )
            .bind(step)
            .bind(user_id)
            .bind(step)
            .execute(self.pool())
            .await?;
            return Ok(if done.rows_affected() == 1 {
                CodeCheck::Totp
            } else {
                CodeCheck::Wrong
            });
        }
        let used = sqlx::query(
            "UPDATE recovery_codes SET used_at = ? WHERE id = (SELECT id FROM recovery_codes \
             WHERE user_id = ? AND code_hash = ? AND used_at IS NULL LIMIT 1)",
        )
        .bind(rfc3339(now()))
        .bind(user_id)
        .bind(totp::hash_recovery_code(code))
        .execute(self.pool())
        .await?;
        Ok(if used.rows_affected() == 1 {
            CodeCheck::RecoveryCode
        } else {
            CodeCheck::Wrong
        })
    }

    /// Whether an Admin requires TOTP of everyone.
    pub async fn totp_required(&self) -> Result<bool, sqlx::Error> {
        let value: Option<String> = sqlx::query_scalar("SELECT value FROM settings WHERE key = ?")
            .bind(REQUIRE_TOTP)
            .fetch_optional(self.pool())
            .await?;
        Ok(value.as_deref() == Some("true"))
    }
}
