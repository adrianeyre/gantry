//! Printers, the only source of truth for them (ADR-0004). Credentials are
//! stored encrypted (ADR-0005) and only ever decrypted to hand to a Driver.
use std::collections::BTreeMap;

use gantry_core::{PrinterConfig, Secrets};
use serde::Serialize;

use super::{Store, now, rfc3339};
use crate::crypto::{DecryptError, SecretKey};

/// A Printer as stored, minus its credentials' values.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PrinterRecord {
    pub id: String,
    pub name: String,
    pub driver: String,
    pub settings: serde_json::Map<String, serde_json::Value>,
    /// Which credential fields hold a value. Never the values themselves.
    pub secrets_set: Vec<String>,
    /// The Queue starts the next compatible Job when it becomes Idle.
    pub auto_start: bool,
    /// After a Succeeded Job it skips Awaiting Clear.
    pub auto_clear: bool,
}

/// A Printer's automation settings (PRD §6.3, §7.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct Automation {
    pub auto_start: bool,
    pub auto_clear: bool,
}

impl Default for Automation {
    /// Auto-start on, Auto-Clear off.
    fn default() -> Self {
        Self {
            auto_start: true,
            auto_clear: false,
        }
    }
}

/// A new or changed Printer.
#[derive(Debug, Clone)]
pub struct PrinterInput {
    pub name: String,
    pub driver: String,
    pub settings: serde_json::Map<String, serde_json::Value>,
    pub secrets: Secrets,
}

/// Why a Printer could not be saved.
#[derive(Debug, thiserror::Error)]
pub enum SavePrinterError {
    #[error("a Printer called `{0}` already exists")]
    NameTaken(String),
    #[error("no such Printer")]
    NotFound,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

type Row = (String, String, String, String, Option<String>, bool, bool);

pub(crate) fn encrypt_secrets(key: &SecretKey, secrets: &Secrets) -> Option<String> {
    let values: BTreeMap<String, String> = secrets
        .clone()
        .into_inner()
        .into_iter()
        .filter(|(_, v)| !v.is_empty())
        .collect();
    (!values.is_empty())
        .then(|| key.encrypt(&serde_json::to_string(&values).expect("a map of strings serialises")))
}

pub(crate) fn decrypt_secrets(
    key: &SecretKey,
    stored: Option<&str>,
) -> Result<Secrets, DecryptError> {
    match stored {
        None => Ok(Secrets::default()),
        Some(blob) => {
            let json = key.decrypt(blob)?;
            serde_json::from_str(&json)
                .map(Secrets::new)
                .map_err(|_| DecryptError)
        }
    }
}

fn record(
    key: &SecretKey,
    (id, name, driver, settings, secrets, auto_start, auto_clear): Row,
) -> PrinterRecord {
    let secrets_set = decrypt_secrets(key, secrets.as_deref())
        .map(|s| s.keys().map(str::to_owned).collect())
        .unwrap_or_default();
    PrinterRecord {
        id,
        name,
        driver,
        settings: serde_json::from_str(&settings).unwrap_or_default(),
        secrets_set,
        auto_start,
        auto_clear,
    }
}

/// A capability's stored name, the same as its JSON name.
pub fn capability_name(capability: gantry_core::Capability) -> String {
    match serde_json::to_value(capability) {
        Ok(serde_json::Value::String(name)) => name,
        _ => format!("{capability:?}"),
    }
}

fn unique_violation(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(d) if d.is_unique_violation())
}

const COLUMNS: &str = "id, name, driver, settings, secrets, auto_start, auto_clear";

impl Store {
    /// Every Printer, by name.
    pub async fn printers(&self, key: &SecretKey) -> Result<Vec<PrinterRecord>, sqlx::Error> {
        let rows: Vec<Row> = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM printers ORDER BY name COLLATE NOCASE"
        ))
        .fetch_all(self.pool())
        .await?;
        Ok(rows.into_iter().map(|r| record(key, r)).collect())
    }

    pub async fn printer(
        &self,
        key: &SecretKey,
        id: &str,
    ) -> Result<Option<PrinterRecord>, sqlx::Error> {
        let row: Option<Row> =
            sqlx::query_as(&format!("SELECT {COLUMNS} FROM printers WHERE id = ?"))
                .bind(id)
                .fetch_optional(self.pool())
                .await?;
        Ok(row.map(|r| record(key, r)))
    }

    /// What a Driver needs to connect the Printer, credentials decrypted.
    /// Credentials that can't be decrypted (the secret key changed) are
    /// left out and reported as the error half.
    pub async fn printer_config(
        &self,
        key: &SecretKey,
        id: &str,
    ) -> Result<Option<(String, PrinterConfig, Option<DecryptError>)>, sqlx::Error> {
        let row: Option<Row> =
            sqlx::query_as(&format!("SELECT {COLUMNS} FROM printers WHERE id = ?"))
                .bind(id)
                .fetch_optional(self.pool())
                .await?;
        Ok(row.map(|(id, name, driver, settings, secrets, _, _)| {
            let (secrets, error) = match decrypt_secrets(key, secrets.as_deref()) {
                Ok(s) => (s, None),
                Err(e) => (Secrets::default(), Some(e)),
            };
            let config = PrinterConfig {
                id,
                name,
                settings: serde_json::from_str(&settings).unwrap_or_default(),
                secrets,
            };
            (driver, config, error)
        }))
    }

    /// The decrypted credentials, for merging an edit with what is stored.
    pub async fn printer_secrets(&self, key: &SecretKey, id: &str) -> Result<Secrets, sqlx::Error> {
        let stored: Option<Option<String>> =
            sqlx::query_scalar("SELECT secrets FROM printers WHERE id = ?")
                .bind(id)
                .fetch_optional(self.pool())
                .await?;
        Ok(decrypt_secrets(key, stored.flatten().as_deref()).unwrap_or_default())
    }

    pub async fn insert_printer(
        &self,
        key: &SecretKey,
        id: &str,
        input: &PrinterInput,
    ) -> Result<PrinterRecord, SavePrinterError> {
        let at = rfc3339(now());
        let result = sqlx::query(
            "INSERT INTO printers (id, name, driver, settings, secrets, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(input.name.trim())
        .bind(&input.driver)
        .bind(serde_json::Value::Object(input.settings.clone()).to_string())
        .bind(encrypt_secrets(key, &input.secrets))
        .bind(&at)
        .bind(&at)
        .execute(self.pool())
        .await;
        match result {
            Ok(_) => Ok(self
                .printer(key, id)
                .await?
                .ok_or(SavePrinterError::NotFound)?),
            Err(e) if unique_violation(&e) => {
                Err(SavePrinterError::NameTaken(input.name.trim().to_owned()))
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Replaces a Printer's name, settings and credentials. Its Driver can't
    /// change.
    pub async fn update_printer(
        &self,
        key: &SecretKey,
        id: &str,
        input: &PrinterInput,
    ) -> Result<PrinterRecord, SavePrinterError> {
        let result = sqlx::query(
            "UPDATE printers SET name = ?, settings = ?, secrets = ?, updated_at = ? WHERE id = ?",
        )
        .bind(input.name.trim())
        .bind(serde_json::Value::Object(input.settings.clone()).to_string())
        .bind(encrypt_secrets(key, &input.secrets))
        .bind(rfc3339(now()))
        .bind(id)
        .execute(self.pool())
        .await;
        match result {
            Ok(done) if done.rows_affected() == 0 => Err(SavePrinterError::NotFound),
            Ok(_) => Ok(self
                .printer(key, id)
                .await?
                .ok_or(SavePrinterError::NotFound)?),
            Err(e) if unique_violation(&e) => {
                Err(SavePrinterError::NameTaken(input.name.trim().to_owned()))
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Deletes a Printer. `false` if there was none.
    pub async fn delete_printer(&self, id: &str) -> Result<bool, sqlx::Error> {
        let done = sqlx::query("DELETE FROM printers WHERE id = ?")
            .bind(id)
            .execute(self.pool())
            .await?;
        Ok(done.rows_affected() > 0)
    }

    /// An Admin's capability overrides for a Printer.
    pub async fn capability_overrides(
        &self,
        printer: &str,
    ) -> Result<gantry_core::Overrides, sqlx::Error> {
        let rows: Vec<(String, bool)> = sqlx::query_as(
            "SELECT capability, enabled FROM capability_overrides WHERE printer_id = ?",
        )
        .bind(printer)
        .fetch_all(self.pool())
        .await?;
        Ok(rows
            .into_iter()
            .filter_map(|(capability, enabled)| {
                // One a later Gantry no longer knows is skipped, not an error.
                serde_json::from_value(serde_json::Value::String(capability))
                    .ok()
                    .map(|c| (c, enabled))
            })
            .collect())
    }

    /// Replaces a Printer's capability overrides, all at once.
    pub async fn set_capability_overrides(
        &self,
        printer: &str,
        overrides: &gantry_core::Overrides,
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.pool().begin().await?;
        sqlx::query("DELETE FROM capability_overrides WHERE printer_id = ?")
            .bind(printer)
            .execute(&mut *tx)
            .await?;
        for (capability, enabled) in overrides {
            sqlx::query(
                "INSERT INTO capability_overrides (printer_id, capability, enabled) VALUES (?, ?, ?)",
            )
            .bind(printer)
            .bind(capability_name(*capability))
            .bind(enabled)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await
    }

    /// A Printer's automation settings, or `None` if there is no such Printer.
    pub async fn automation(&self, printer: &str) -> Result<Option<Automation>, sqlx::Error> {
        let row: Option<(bool, bool)> =
            sqlx::query_as("SELECT auto_start, auto_clear FROM printers WHERE id = ?")
                .bind(printer)
                .fetch_optional(self.pool())
                .await?;
        Ok(row.map(|(auto_start, auto_clear)| Automation {
            auto_start,
            auto_clear,
        }))
    }

    /// Changes a Printer's automation settings. `false` if there is no such
    /// Printer.
    pub async fn set_automation(
        &self,
        printer: &str,
        automation: Automation,
    ) -> Result<bool, sqlx::Error> {
        let done = sqlx::query(
            "UPDATE printers SET auto_start = ?, auto_clear = ?, updated_at = ? WHERE id = ?",
        )
        .bind(automation.auto_start)
        .bind(automation.auto_clear)
        .bind(rfc3339(now()))
        .bind(printer)
        .execute(self.pool())
        .await?;
        Ok(done.rows_affected() > 0)
    }

    /// Every Printer's id, for starting them.
    pub async fn printer_ids(&self) -> Result<Vec<String>, sqlx::Error> {
        sqlx::query_scalar("SELECT id FROM printers ORDER BY created_at")
            .fetch_all(self.pool())
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> SecretKey {
        SecretKey::from_config("a secret key for tests only").unwrap()
    }

    fn input(name: &str, code: &str) -> PrinterInput {
        PrinterInput {
            name: name.into(),
            driver: "simulated".into(),
            settings: serde_json::json!({ "model": "X" })
                .as_object()
                .unwrap()
                .clone(),
            secrets: Secrets::new([("access_code".to_owned(), code.to_owned())].into()),
        }
    }

    #[tokio::test]
    async fn credentials_are_encrypted_at_rest() {
        let store = Store::in_memory().await.unwrap();
        store
            .insert_printer(&key(), "p1", &input("One", "SECRET-CODE-123"))
            .await
            .unwrap();
        let raw: String = sqlx::query_scalar("SELECT secrets FROM printers")
            .fetch_one(store.pool())
            .await
            .unwrap();
        assert!(!raw.contains("SECRET-CODE-123"));
        assert!(raw.starts_with("v1:"));
        let (_, config, error) = store.printer_config(&key(), "p1").await.unwrap().unwrap();
        assert!(error.is_none());
        assert_eq!(config.secrets.get("access_code"), Some("SECRET-CODE-123"));
    }

    #[tokio::test]
    async fn records_name_which_credentials_are_set_but_not_their_values() {
        let store = Store::in_memory().await.unwrap();
        let record = store
            .insert_printer(&key(), "p1", &input("One", "SECRET-CODE-123"))
            .await
            .unwrap();
        assert_eq!(record.secrets_set, ["access_code"]);
        let json = serde_json::to_string(&store.printers(&key()).await.unwrap()).unwrap();
        assert!(!json.contains("SECRET-CODE-123"), "{json}");
    }

    #[tokio::test]
    async fn names_are_unique() {
        let store = Store::in_memory().await.unwrap();
        store
            .insert_printer(&key(), "p1", &input("One", "a"))
            .await
            .unwrap();
        assert!(matches!(
            store.insert_printer(&key(), "p2", &input("one", "b")).await,
            Err(SavePrinterError::NameTaken(_))
        ));
    }

    #[tokio::test]
    async fn another_key_cannot_read_the_credentials() {
        let store = Store::in_memory().await.unwrap();
        store
            .insert_printer(&key(), "p1", &input("One", "SECRET-CODE-123"))
            .await
            .unwrap();
        let other = SecretKey::from_config("a different key entirely").unwrap();
        let (_, config, error) = store.printer_config(&other, "p1").await.unwrap().unwrap();
        assert!(error.is_some());
        assert!(config.secrets.get("access_code").is_none());
    }

    #[tokio::test]
    async fn automation_is_auto_start_on_and_auto_clear_off_by_default() {
        let store = Store::in_memory().await.unwrap();
        let record = store
            .insert_printer(&key(), "p1", &input("One", "a"))
            .await
            .unwrap();
        assert!(record.auto_start);
        assert!(!record.auto_clear);
        assert_eq!(
            store.automation("p1").await.unwrap(),
            Some(Automation::default())
        );
        let changed = Automation {
            auto_start: false,
            auto_clear: true,
        };
        assert!(store.set_automation("p1", changed).await.unwrap());
        assert_eq!(store.automation("p1").await.unwrap(), Some(changed));
        assert!(!store.set_automation("nope", changed).await.unwrap());
        assert_eq!(store.automation("nope").await.unwrap(), None);
    }

    #[tokio::test]
    async fn updates_and_deletes() {
        let store = Store::in_memory().await.unwrap();
        store
            .insert_printer(&key(), "p1", &input("One", "a"))
            .await
            .unwrap();
        let updated = store
            .update_printer(&key(), "p1", &input("Renamed", "b"))
            .await
            .unwrap();
        assert_eq!(updated.name, "Renamed");
        assert!(store.delete_printer("p1").await.unwrap());
        assert!(!store.delete_printer("p1").await.unwrap());
        assert!(matches!(
            store.update_printer(&key(), "p1", &input("x", "c")).await,
            Err(SavePrinterError::NotFound)
        ));
    }
}
