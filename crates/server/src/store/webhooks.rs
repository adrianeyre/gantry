//! Webhooks (PRD §7.7). The signing secret is encrypted at rest, like
//! Printer and Alert channel credentials (ADR-0005), and only ever shown to
//! the Admin once, when it is made.
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use gantry_core::JobOutcome;
use rand::RngCore;
use serde::Serialize;

use super::{Store, now, rfc3339};
use crate::alerts::EventKind;
use crate::crypto::SecretKey;

/// A webhook, with its secret decrypted, for delivering.
#[derive(Clone, PartialEq)]
pub struct Webhook {
    pub id: i64,
    pub name: String,
    pub url: String,
    pub secret: String,
    pub enabled: bool,
    pub events: Vec<EventKind>,
    pub outcomes: Vec<JobOutcome>,
    pub printers: Vec<String>,
    pub tags: Vec<i64>,
}

impl std::fmt::Debug for Webhook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Webhook")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("enabled", &self.enabled)
            .field("events", &self.events)
            .finish_non_exhaustive()
    }
}

/// A webhook as the browser sees it: never the secret. The URL is shown,
/// because a webhook's URL is where it goes, not how it is authenticated.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WebhookSummary {
    pub id: i64,
    pub name: String,
    pub url: String,
    pub enabled: bool,
    pub events: Vec<EventKind>,
    pub outcomes: Vec<JobOutcome>,
    pub printers: Vec<String>,
    pub tags: Vec<i64>,
}

impl From<&Webhook> for WebhookSummary {
    fn from(w: &Webhook) -> Self {
        Self {
            id: w.id,
            name: w.name.clone(),
            url: w.url.clone(),
            enabled: w.enabled,
            events: w.events.clone(),
            outcomes: w.outcomes.clone(),
            printers: w.printers.clone(),
            tags: w.tags.clone(),
        }
    }
}

/// A fresh signing secret: `whsec_` and 256 random bits.
pub fn new_secret() -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    format!("whsec_{}", URL_SAFE_NO_PAD.encode(bytes))
}

type Row = (
    i64,
    String,
    String,
    String,
    bool,
    String,
    String,
    String,
    String,
);

fn json<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "[]".into())
}

const SELECT: &str =
    "SELECT id, name, url, secret, enabled, events, outcomes, printers, tags FROM webhooks";

impl Store {
    fn webhook_from(key: &SecretKey, row: Row) -> Webhook {
        let (id, name, url, secret, enabled, events, outcomes, printers, tags) = row;
        Webhook {
            id,
            name,
            url,
            // A secret encrypted with another key can't sign: the webhook
            // stays, and its deliveries fail verification until it is
            // given a new secret.
            secret: key.decrypt(&secret).unwrap_or_default(),
            enabled,
            events: serde_json::from_str(&events).unwrap_or_default(),
            outcomes: serde_json::from_str(&outcomes).unwrap_or_default(),
            printers: serde_json::from_str(&printers).unwrap_or_default(),
            tags: serde_json::from_str(&tags).unwrap_or_default(),
        }
    }

    pub async fn webhooks(&self, key: &SecretKey) -> Result<Vec<Webhook>, sqlx::Error> {
        let rows: Vec<Row> = sqlx::query_as(&format!("{SELECT} ORDER BY name COLLATE NOCASE"))
            .fetch_all(self.pool())
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| Self::webhook_from(key, r))
            .collect())
    }

    pub async fn webhook(&self, key: &SecretKey, id: i64) -> Result<Option<Webhook>, sqlx::Error> {
        let row: Option<Row> = sqlx::query_as(&format!("{SELECT} WHERE id = ?"))
            .bind(id)
            .fetch_optional(self.pool())
            .await?;
        Ok(row.map(|r| Self::webhook_from(key, r)))
    }

    /// Adds a webhook (with `id` 0) or replaces one. Returns its id.
    pub async fn save_webhook(&self, key: &SecretKey, hook: &Webhook) -> Result<i64, sqlx::Error> {
        let at = rfc3339(now());
        let secret = key.encrypt(&hook.secret);
        if hook.id == 0 {
            let done = sqlx::query(
                "INSERT INTO webhooks (name, url, secret, enabled, events, outcomes, printers, \
                 tags, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&hook.name)
            .bind(&hook.url)
            .bind(secret)
            .bind(hook.enabled)
            .bind(json(&hook.events))
            .bind(json(&hook.outcomes))
            .bind(json(&hook.printers))
            .bind(json(&hook.tags))
            .bind(&at)
            .bind(&at)
            .execute(self.pool())
            .await?;
            return Ok(done.last_insert_rowid());
        }
        sqlx::query(
            "UPDATE webhooks SET name = ?, url = ?, secret = ?, enabled = ?, events = ?, \
             outcomes = ?, printers = ?, tags = ?, updated_at = ? WHERE id = ?",
        )
        .bind(&hook.name)
        .bind(&hook.url)
        .bind(secret)
        .bind(hook.enabled)
        .bind(json(&hook.events))
        .bind(json(&hook.outcomes))
        .bind(json(&hook.printers))
        .bind(json(&hook.tags))
        .bind(&at)
        .bind(hook.id)
        .execute(self.pool())
        .await?;
        Ok(hook.id)
    }

    pub async fn delete_webhook(&self, id: i64) -> Result<bool, sqlx::Error> {
        let done = sqlx::query("DELETE FROM webhooks WHERE id = ?")
            .bind(id)
            .execute(self.pool())
            .await?;
        Ok(done.rows_affected() > 0)
    }
}
