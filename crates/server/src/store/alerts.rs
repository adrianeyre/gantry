//! Alert channels and Alert Rules (PRD §7.4). A channel's credentials (an
//! ntfy token, a Discord webhook URL) are encrypted with the secret key, like
//! Printer credentials (ADR-0005), and never sent back to the browser.
use gantry_core::{JobOutcome, Secrets};
use serde::{Deserialize, Serialize};

use super::printers::{decrypt_secrets, encrypt_secrets};
use super::{Store, now, rfc3339};
use crate::alerts::EventKind;
use crate::crypto::SecretKey;

/// A channel with its credentials decrypted, for sending.
#[derive(Debug, Clone)]
pub struct StoredChannel {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub settings: serde_json::Map<String, serde_json::Value>,
    pub secrets: Secrets,
}

/// A channel as the browser sees it: which credentials are set, never what
/// they are.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChannelSummary {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub settings: serde_json::Map<String, serde_json::Value>,
    pub secrets_set: Vec<String>,
}

impl From<&StoredChannel> for ChannelSummary {
    fn from(c: &StoredChannel) -> Self {
        Self {
            id: c.id,
            name: c.name.clone(),
            kind: c.kind.clone(),
            settings: c.settings.clone(),
            secrets_set: c.secrets.keys().map(str::to_owned).collect(),
        }
    }
}

/// An Alert Rule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    pub id: i64,
    pub name: String,
    pub enabled: bool,
    pub events: Vec<EventKind>,
    /// Narrows `job_ended` to these Outcomes; empty means every Outcome.
    pub outcomes: Vec<JobOutcome>,
    /// Printer ids; with `tags` empty too, the whole Fleet.
    pub printers: Vec<String>,
    /// Tag ids.
    pub tags: Vec<i64>,
    pub channel_id: i64,
}

/// What a Rule is made from, before it has an id.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RuleInput {
    pub name: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    pub events: Vec<EventKind>,
    #[serde(default)]
    pub outcomes: Vec<JobOutcome>,
    #[serde(default)]
    pub printers: Vec<String>,
    #[serde(default)]
    pub tags: Vec<i64>,
    pub channel_id: i64,
}

fn yes() -> bool {
    true
}

type ChannelRow = (i64, String, String, String, Option<String>);
type RuleRow = (i64, String, bool, String, String, String, String, i64);

fn rule(row: RuleRow) -> Rule {
    let (id, name, enabled, events, outcomes, printers, tags, channel_id) = row;
    Rule {
        id,
        name,
        enabled,
        events: serde_json::from_str(&events).unwrap_or_default(),
        outcomes: serde_json::from_str(&outcomes).unwrap_or_default(),
        printers: serde_json::from_str(&printers).unwrap_or_default(),
        tags: serde_json::from_str(&tags).unwrap_or_default(),
        channel_id,
    }
}

fn json<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "[]".into())
}

impl Store {
    fn channel_from(key: &SecretKey, row: ChannelRow) -> StoredChannel {
        let (id, name, kind, settings, secrets) = row;
        StoredChannel {
            id,
            name,
            kind,
            settings: serde_json::from_str(&settings).unwrap_or_default(),
            secrets: decrypt_secrets(key, secrets.as_deref()).unwrap_or_default(),
        }
    }

    pub async fn alert_channels(&self, key: &SecretKey) -> Result<Vec<StoredChannel>, sqlx::Error> {
        let rows: Vec<ChannelRow> = sqlx::query_as(
            "SELECT id, name, kind, settings, secrets FROM alert_channels ORDER BY name COLLATE NOCASE",
        )
        .fetch_all(self.pool())
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| Self::channel_from(key, r))
            .collect())
    }

    pub async fn alert_channel(
        &self,
        key: &SecretKey,
        id: i64,
    ) -> Result<Option<StoredChannel>, sqlx::Error> {
        let row: Option<ChannelRow> = sqlx::query_as(
            "SELECT id, name, kind, settings, secrets FROM alert_channels WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(self.pool())
        .await?;
        Ok(row.map(|r| Self::channel_from(key, r)))
    }

    /// Adds a channel (with `id` 0) or replaces one. Returns its id.
    pub async fn save_alert_channel(
        &self,
        key: &SecretKey,
        channel: &StoredChannel,
    ) -> Result<i64, sqlx::Error> {
        let at = rfc3339(now());
        let settings = serde_json::to_string(&channel.settings).unwrap_or_else(|_| "{}".into());
        let secrets = encrypt_secrets(key, &channel.secrets);
        if channel.id == 0 {
            let done = sqlx::query(
                "INSERT INTO alert_channels (name, kind, settings, secrets, created_at, updated_at) \
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(&channel.name)
            .bind(&channel.kind)
            .bind(settings)
            .bind(secrets)
            .bind(&at)
            .bind(&at)
            .execute(self.pool())
            .await?;
            return Ok(done.last_insert_rowid());
        }
        sqlx::query(
            "UPDATE alert_channels SET name = ?, settings = ?, secrets = ?, updated_at = ? WHERE id = ?",
        )
        .bind(&channel.name)
        .bind(settings)
        .bind(secrets)
        .bind(&at)
        .bind(channel.id)
        .execute(self.pool())
        .await?;
        Ok(channel.id)
    }

    /// Removes a channel and every Rule that sends to it.
    pub async fn delete_alert_channel(&self, id: i64) -> Result<bool, sqlx::Error> {
        let done = sqlx::query("DELETE FROM alert_channels WHERE id = ?")
            .bind(id)
            .execute(self.pool())
            .await?;
        Ok(done.rows_affected() > 0)
    }

    pub async fn alert_rules(&self) -> Result<Vec<Rule>, sqlx::Error> {
        let rows: Vec<RuleRow> = sqlx::query_as(
            "SELECT id, name, enabled, events, outcomes, printers, tags, channel_id \
             FROM alert_rules ORDER BY name COLLATE NOCASE, id",
        )
        .fetch_all(self.pool())
        .await?;
        Ok(rows.into_iter().map(rule).collect())
    }

    pub async fn alert_rule(&self, id: i64) -> Result<Option<Rule>, sqlx::Error> {
        let row: Option<RuleRow> = sqlx::query_as(
            "SELECT id, name, enabled, events, outcomes, printers, tags, channel_id \
             FROM alert_rules WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(self.pool())
        .await?;
        Ok(row.map(rule))
    }

    /// Adds a Rule (`id` None) or replaces one. Returns its id.
    pub async fn save_alert_rule(
        &self,
        id: Option<i64>,
        input: &RuleInput,
    ) -> Result<i64, sqlx::Error> {
        let at = rfc3339(now());
        match id {
            None => {
                let done = sqlx::query(
                    "INSERT INTO alert_rules (name, enabled, events, outcomes, printers, tags, \
                     channel_id, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(&input.name)
                .bind(input.enabled)
                .bind(json(&input.events))
                .bind(json(&input.outcomes))
                .bind(json(&input.printers))
                .bind(json(&input.tags))
                .bind(input.channel_id)
                .bind(&at)
                .bind(&at)
                .execute(self.pool())
                .await?;
                Ok(done.last_insert_rowid())
            }
            Some(id) => {
                sqlx::query(
                    "UPDATE alert_rules SET name = ?, enabled = ?, events = ?, outcomes = ?, \
                     printers = ?, tags = ?, channel_id = ?, updated_at = ? WHERE id = ?",
                )
                .bind(&input.name)
                .bind(input.enabled)
                .bind(json(&input.events))
                .bind(json(&input.outcomes))
                .bind(json(&input.printers))
                .bind(json(&input.tags))
                .bind(input.channel_id)
                .bind(&at)
                .bind(id)
                .execute(self.pool())
                .await?;
                Ok(id)
            }
        }
    }

    pub async fn delete_alert_rule(&self, id: i64) -> Result<bool, sqlx::Error> {
        let done = sqlx::query("DELETE FROM alert_rules WHERE id = ?")
            .bind(id)
            .execute(self.pool())
            .await?;
        Ok(done.rows_affected() > 0)
    }
}
