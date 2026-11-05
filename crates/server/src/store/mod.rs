//! SQLite, the source of truth for all state (ADR-0004), through `sqlx` with
//! embedded migrations and runtime-checked queries (ADR-0006).
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

use time::OffsetDateTime;
use time::macros::format_description;

use crate::crypto::SecretKey;

pub mod alerts;
pub mod audit;
pub mod history;
pub mod jobs;
pub mod passkeys;
pub mod print_files;
pub mod printers;
pub mod queue;
pub mod tags;
pub mod timelapses;
pub mod tokens;
pub mod two_factor;
pub mod users;
pub mod webhooks;

static MIGRATIONS: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// The current time, in UTC.
pub fn now() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}

/// The fixed-width UTC form timestamps are stored in, so they sort as text.
pub fn rfc3339(at: OffsetDateTime) -> String {
    at.to_offset(time::UtcOffset::UTC)
        .format(format_description!(
            "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z"
        ))
        .expect("a UTC timestamp always formats")
}

/// Parses a stored timestamp.
pub fn parse_time(value: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339).ok()
}

/// A handle to the database. Cheap to clone.
#[derive(Clone, Debug)]
pub struct Store {
    pool: SqlitePool,
}

/// Whether the stored credentials can be read with the configured key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyCheck {
    /// The database had no key recorded yet; this key is now recorded.
    Recorded,
    /// The key matches the one credentials were encrypted with.
    Matches,
    /// The key differs: stored Printer credentials must be re-entered.
    Changed,
}

impl Store {
    /// Opens (creating if needed) the database at `path` and runs migrations.
    /// Safe to call on every start.
    pub async fn open(path: &Path) -> Result<Self, sqlx::Error> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(sqlx::Error::Io)?;
        }
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(5));
        Self::connect(options, 8).await
    }

    /// A private in-memory database, for tests.
    pub async fn in_memory() -> Result<Self, sqlx::Error> {
        let options = SqliteConnectOptions::from_str("sqlite::memory:")?.foreign_keys(true);
        // One connection, so every query sees the same in-memory database.
        Self::connect(options, 1).await
    }

    async fn connect(options: SqliteConnectOptions, max: u32) -> Result<Self, sqlx::Error> {
        let pool = SqlitePoolOptions::new()
            .max_connections(max)
            .connect_with(options)
            .await?;
        MIGRATIONS.run(&pool).await?;
        Ok(Self { pool })
    }

    /// The underlying pool.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Records the secret key the first time, and says on later starts
    /// whether it is still the same one.
    pub async fn check_secret_key(&self, key: &SecretKey) -> Result<KeyCheck, sqlx::Error> {
        let stored: Option<String> =
            sqlx::query_scalar("SELECT key_check FROM instance WHERE id = 1")
                .fetch_one(&self.pool)
                .await?;
        match stored {
            None => {
                self.record_secret_key(key).await?;
                Ok(KeyCheck::Recorded)
            }
            Some(value) if key.decrypt(&value).is_ok() => Ok(KeyCheck::Matches),
            Some(_) => Ok(KeyCheck::Changed),
        }
    }

    /// Records `key` as the one credentials are encrypted with.
    pub async fn record_secret_key(&self, key: &SecretKey) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE instance SET key_check = ? WHERE id = 1")
            .bind(key.encrypt("gantry"))
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(secret: &str) -> SecretKey {
        SecretKey::from_config(secret).unwrap()
    }

    #[tokio::test]
    async fn creates_the_database_on_first_start_and_reopens_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("gantry.db");
        assert!(!path.exists());

        let store = Store::open(&path).await.unwrap();
        assert!(path.exists());
        let created: String = sqlx::query_scalar("SELECT created_at FROM instance")
            .fetch_one(store.pool())
            .await
            .unwrap();
        store.pool().close().await;

        // A second start runs the migrations again without error or change.
        let store = Store::open(&path).await.unwrap();
        let again: String = sqlx::query_scalar("SELECT created_at FROM instance")
            .fetch_one(store.pool())
            .await
            .unwrap();
        assert_eq!(created, again);
        let applied: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations")
            .fetch_one(store.pool())
            .await
            .unwrap();
        assert_eq!(applied, MIGRATIONS.iter().count() as i64);
    }

    #[tokio::test]
    async fn notices_when_the_secret_key_changes() {
        let store = Store::in_memory().await.unwrap();
        let first = key("the first secret key!!");
        assert_eq!(
            store.check_secret_key(&first).await.unwrap(),
            KeyCheck::Recorded
        );
        assert_eq!(
            store.check_secret_key(&first).await.unwrap(),
            KeyCheck::Matches
        );
        let second = key("a replacement secret key");
        assert_eq!(
            store.check_secret_key(&second).await.unwrap(),
            KeyCheck::Changed
        );
        store.record_secret_key(&second).await.unwrap();
        assert_eq!(
            store.check_secret_key(&second).await.unwrap(),
            KeyCheck::Matches
        );
    }
}
