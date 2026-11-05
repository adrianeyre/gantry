//! Config export and import (PRD §5.4, ADR-0004, ADR-0005).
//!
//! An export is a declarative, Git-friendly TOML file of the Printers (their
//! Driver settings and capability overrides) and Gantry's settings. It is a
//! one-off copy: SQLite stays the source of truth, and nothing watches the
//! file. Printer credentials are left out unless the export asks for them,
//! and then they are written in plain text (so the file can be imported into
//! any Gantry, whatever its secret key), with a warning at the top.
//!
//! An import is planned first, so an Admin sees what will change, and then
//! applied in one transaction: all of it or none of it. Printers are matched
//! by name. Credentials missing from the file are kept for an existing
//! Printer, and left for an Admin to enter for a new one.
use std::collections::{BTreeMap, BTreeSet};

use gantry_core::{Capability, DriverRegistry, FieldKind, Overrides, Secrets};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::crypto::SecretKey;
use crate::store::printers::{capability_name, decrypt_secrets, encrypt_secrets};
use crate::store::{Store, now, rfc3339};

/// The file format's version.
pub const FORMAT: u32 = 1;

/// A whole export.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigFile {
    pub format: u32,
    /// Gantry-wide settings, by key.
    #[serde(default)]
    pub settings: BTreeMap<String, Value>,
    /// Every Tag, including ones on no Printer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default)]
    pub printers: Vec<PrinterEntry>,
}

/// One Printer in an export. `Debug` never shows a credential.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrinterEntry {
    pub name: String,
    pub driver: String,
    /// The Driver's non-secret settings.
    #[serde(default)]
    pub settings: BTreeMap<String, Value>,
    /// Capability overrides: `true` adds one, `false` removes it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub capabilities: BTreeMap<String, bool>,
    /// The Printer's Tags, by name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Plain-text credentials, only when the export asked for them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credentials: Option<BTreeMap<String, String>>,
}

impl std::fmt::Debug for PrinterEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrinterEntry")
            .field("name", &self.name)
            .field("driver", &self.driver)
            .field("settings", &self.settings)
            .field("capabilities", &self.capabilities)
            .field("tags", &self.tags)
            .field(
                "credentials",
                &self
                    .credentials
                    .as_ref()
                    .map(|c| c.keys().collect::<Vec<_>>()),
            )
            .finish()
    }
}

const HEADER: &str = "# Gantry config export (PRD §5.4). Import it under Settings.\n\
# A one-off copy: Gantry's database stays the source of truth.\n";

const CREDENTIALS_WARNING: &str = "# WARNING: this file holds Printer credentials in plain text.\n\
# Keep it private, and don't commit it to a public repository.\n";

/// A stored Printer, everything import and export need.
struct Stored {
    id: String,
    name: String,
    driver: String,
    settings: serde_json::Map<String, Value>,
    secrets: Secrets,
    overrides: Overrides,
    tags: Vec<String>,
}

async fn stored_printers(store: &Store, key: &SecretKey) -> Result<Vec<Stored>, sqlx::Error> {
    let rows: Vec<(String, String, String, String, Option<String>)> = sqlx::query_as(
        "SELECT id, name, driver, settings, secrets FROM printers ORDER BY name COLLATE NOCASE",
    )
    .fetch_all(store.pool())
    .await?;
    let mut out = Vec::new();
    for (id, name, driver, settings, secrets) in rows {
        let overrides = store.capability_overrides(&id).await?;
        let tags = store.printer_tags(&id).await?;
        out.push(Stored {
            tags,
            secrets: decrypt_secrets(key, secrets.as_deref()).unwrap_or_default(),
            settings: serde_json::from_str(&settings).unwrap_or_default(),
            id,
            name,
            driver,
            overrides,
        });
    }
    Ok(out)
}

fn overrides_to_names(overrides: &Overrides) -> BTreeMap<String, bool> {
    overrides
        .iter()
        .map(|(c, on)| (capability_name(*c), *on))
        .collect()
}

/// Settings as TOML can hold them: no nulls.
fn without_nulls(map: impl IntoIterator<Item = (String, Value)>) -> BTreeMap<String, Value> {
    map.into_iter().filter(|(_, v)| !v.is_null()).collect()
}

/// The export file. With `credentials`, Printer credentials in plain text.
pub async fn export(store: &Store, key: &SecretKey, credentials: bool) -> anyhow::Result<String> {
    let printers = stored_printers(store, key)
        .await?
        .into_iter()
        .map(|p| PrinterEntry {
            name: p.name,
            driver: p.driver,
            settings: without_nulls(p.settings),
            capabilities: overrides_to_names(&p.overrides),
            tags: p.tags,
            credentials: credentials.then(|| p.secrets.into_inner()),
        })
        .collect();
    let file = ConfigFile {
        format: FORMAT,
        settings: without_nulls(store.settings().await?),
        tags: store.tags().await?.into_iter().map(|t| t.name).collect(),
        printers,
    };
    let body = toml::to_string_pretty(&file)?;
    Ok(if credentials {
        format!("{HEADER}{CREDENTIALS_WARNING}\n{body}")
    } else {
        format!("{HEADER}\n{body}")
    })
}

/// One thing an import would change.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Change {
    AddPrinter {
        name: String,
        driver: String,
    },
    /// What changed: settings by key, `capabilities`, `credentials`.
    UpdatePrinter {
        name: String,
        changed: Vec<String>,
    },
    RemovePrinter {
        name: String,
    },
    SetSetting {
        key: String,
        from: Option<Value>,
        to: Value,
    },
    RemoveSetting {
        key: String,
    },
    AddTag {
        name: String,
    },
    RemoveTag {
        name: String,
    },
}

/// What an import would do, and anything stopping it.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Plan {
    pub changes: Vec<Change>,
    /// Printers the file has that already match.
    pub unchanged: Vec<String>,
    /// Things worth knowing that don't stop the import.
    pub warnings: Vec<String>,
    /// Problems that stop it: nothing is applied while there are any.
    pub errors: Vec<String>,
    #[serde(skip)]
    work: Vec<Work>,
}

/// What applying does for one Printer.
#[derive(Debug, Clone, PartialEq)]
enum Work {
    Add {
        entry: PrinterEntry,
        secrets: Secrets,
        overrides: Overrides,
    },
    Update {
        id: String,
        entry: PrinterEntry,
        secrets: Secrets,
        overrides: Overrides,
    },
    Remove {
        id: String,
    },
    SetSetting {
        key: String,
        value: Value,
    },
    RemoveSetting {
        key: String,
    },
    AddTag {
        name: String,
    },
    RemoveTag {
        name: String,
    },
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }
}

/// Reads and checks an import, and works out what it would change. Nothing is
/// written. With `prune`, Printers and settings missing from the file are
/// removed.
pub async fn plan(
    store: &Store,
    key: &SecretKey,
    drivers: &DriverRegistry,
    text: &str,
    prune: bool,
) -> anyhow::Result<Plan> {
    let mut plan = Plan::default();
    let file: ConfigFile = match toml::from_str(text) {
        Ok(file) => file,
        Err(e) => {
            plan.errors
                .push(format!("the file isn't a Gantry export: {}", e.message()));
            return Ok(plan);
        }
    };
    if file.format != FORMAT {
        plan.errors.push(format!(
            "the file is format {}, and this Gantry reads format {FORMAT}",
            file.format
        ));
        return Ok(plan);
    }
    let existing = stored_printers(store, key).await?;
    let mut seen = BTreeSet::new();

    // Tags first: the Printers below refer to them by name.
    let existing_tags: Vec<String> = store.tags().await?.into_iter().map(|t| t.name).collect();
    let mut wanted_tags: Vec<String> = Vec::new();
    for name in file
        .tags
        .iter()
        .chain(file.printers.iter().flat_map(|p| &p.tags))
    {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 32 {
            plan.errors.push(format!(
                "a Tag needs a name of 1 to 32 characters, not `{name}`"
            ));
            continue;
        }
        if !wanted_tags.iter().any(|t| t.eq_ignore_ascii_case(name)) {
            wanted_tags.push(name.to_owned());
        }
    }
    for name in &wanted_tags {
        if !existing_tags.iter().any(|t| t.eq_ignore_ascii_case(name)) {
            plan.changes.push(Change::AddTag { name: name.clone() });
            plan.work.push(Work::AddTag { name: name.clone() });
        }
    }
    if prune {
        for name in &existing_tags {
            if !wanted_tags.iter().any(|t| t.eq_ignore_ascii_case(name)) {
                plan.changes.push(Change::RemoveTag { name: name.clone() });
                plan.work.push(Work::RemoveTag { name: name.clone() });
            }
        }
    }

    for entry in &file.printers {
        let name = entry.name.trim().to_owned();
        if name.is_empty() || name.chars().count() > 64 {
            plan.errors.push(format!(
                "a Printer needs a name of 1 to 64 characters, not `{name}`"
            ));
            continue;
        }
        if !seen.insert(name.to_lowercase()) {
            plan.errors
                .push(format!("`{name}` appears more than once in the file"));
            continue;
        }
        let Some(driver) = drivers.get(&entry.driver) else {
            plan.errors.push(format!(
                "`{name}` uses the `{}` Driver, which this Gantry doesn't have",
                entry.driver
            ));
            continue;
        };
        let fields = driver.fields();
        let current = existing.iter().find(|p| p.name.eq_ignore_ascii_case(&name));
        if let Some(current) = current
            && current.driver != entry.driver
        {
            plan.errors.push(format!(
                "`{name}` is a {} Printer here and {} in the file; a Printer's Driver can't change",
                current.driver, entry.driver
            ));
            continue;
        }

        // Only the Driver's own non-secret settings are kept.
        let mut settings = serde_json::Map::new();
        for (k, v) in &entry.settings {
            if fields
                .iter()
                .any(|f| &f.key == k && f.kind != FieldKind::Secret)
            {
                settings.insert(k.clone(), v.clone());
            } else {
                plan.warnings.push(format!(
                    "`{name}`: `{k}` isn't a setting of its Driver, so it's skipped"
                ));
            }
        }
        let secret_keys: Vec<&str> = fields
            .iter()
            .filter(|f| f.kind == FieldKind::Secret)
            .map(|f| f.key.as_str())
            .collect();
        let secrets = match &entry.credentials {
            Some(given) => Secrets::new(
                given
                    .iter()
                    .filter(|(k, v)| secret_keys.contains(&k.as_str()) && !v.is_empty())
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            ),
            None => current.map(|c| c.secrets.clone()).unwrap_or_default(),
        };
        // Missing credentials don't stop an import; an Admin enters them later.
        let mut missing = Vec::new();
        for error in driver.validate(&settings, &secrets) {
            if secret_keys.contains(&error.field.as_str()) {
                missing.push(error.field);
            } else {
                plan.errors
                    .push(format!("`{name}`: {}: {}", error.field, error.message));
            }
        }
        if !missing.is_empty() {
            plan.warnings.push(format!(
                "`{name}` has no {}: enter {} under Printers to connect it",
                missing.join(" or "),
                if missing.len() == 1 { "it" } else { "them" }
            ));
        }
        let mut overrides = Overrides::new();
        for (capability, on) in &entry.capabilities {
            match serde_json::from_value::<Capability>(Value::String(capability.clone())) {
                Ok(c) => {
                    overrides.insert(c, *on);
                }
                Err(_) => plan.warnings.push(format!(
                    "`{name}`: there is no capability `{capability}`, so it's skipped"
                )),
            }
        }
        let mut tags: Vec<String> = Vec::new();
        for tag in entry.tags.iter().map(|t| t.trim()) {
            if let Some(canonical) = wanted_tags.iter().find(|t| t.eq_ignore_ascii_case(tag))
                && !tags.contains(canonical)
            {
                tags.push(canonical.clone());
            }
        }
        tags.sort_by_key(|t| t.to_lowercase());
        let entry = PrinterEntry {
            name: name.clone(),
            driver: entry.driver.clone(),
            settings: settings.clone().into_iter().collect(),
            capabilities: overrides_to_names(&overrides),
            tags,
            // The values travel as `secrets`, never in the entry.
            credentials: None,
        };
        match current {
            None => {
                plan.changes.push(Change::AddPrinter {
                    name: name.clone(),
                    driver: entry.driver.clone(),
                });
                plan.work.push(Work::Add {
                    entry,
                    secrets,
                    overrides,
                });
            }
            Some(current) => {
                let mut changed: Vec<String> = Vec::new();
                if current.name != name {
                    changed.push("name".into());
                }
                let keys: BTreeSet<&String> =
                    current.settings.keys().chain(settings.keys()).collect();
                for k in keys {
                    if current.settings.get(k) != settings.get(k) {
                        changed.push(k.clone());
                    }
                }
                if current.overrides != overrides {
                    changed.push("capabilities".into());
                }
                if current.secrets != secrets {
                    changed.push("credentials".into());
                }
                let lower = |tags: &[String]| -> Vec<String> {
                    tags.iter().map(|t| t.to_lowercase()).collect()
                };
                if lower(&current.tags) != lower(&entry.tags) {
                    changed.push("tags".into());
                }
                if changed.is_empty() {
                    plan.unchanged.push(name);
                } else {
                    plan.changes.push(Change::UpdatePrinter {
                        name: name.clone(),
                        changed,
                    });
                    plan.work.push(Work::Update {
                        id: current.id.clone(),
                        entry,
                        secrets,
                        overrides,
                    });
                }
            }
        }
    }
    if prune {
        for p in &existing {
            if !seen.contains(&p.name.to_lowercase()) {
                plan.changes.push(Change::RemovePrinter {
                    name: p.name.clone(),
                });
                plan.work.push(Work::Remove { id: p.id.clone() });
            }
        }
    }

    let current = store.settings().await?;
    for (k, v) in &file.settings {
        if current.get(k) != Some(v) {
            plan.changes.push(Change::SetSetting {
                key: k.clone(),
                from: current.get(k).cloned(),
                to: v.clone(),
            });
            plan.work.push(Work::SetSetting {
                key: k.clone(),
                value: v.clone(),
            });
        }
    }
    if prune {
        for k in current.keys().filter(|k| !file.settings.contains_key(*k)) {
            plan.changes.push(Change::RemoveSetting { key: k.clone() });
            plan.work.push(Work::RemoveSetting { key: k.clone() });
        }
    }
    Ok(plan)
}

/// What applying a plan did to the running Fleet's Printers.
#[derive(Debug, Default)]
pub struct Applied {
    /// Added or changed: start (or restart) these.
    pub started: Vec<String>,
    /// Removed: stop these.
    pub removed: Vec<String>,
}

/// Applies a plan with no errors, in one transaction.
pub async fn apply(store: &Store, key: &SecretKey, plan: &Plan) -> anyhow::Result<Applied> {
    anyhow::ensure!(plan.errors.is_empty(), "the import has errors");
    let mut applied = Applied::default();
    let at = rfc3339(now());
    let mut tx = store.pool().begin().await?;
    // New Tags before the Printers that use them; removed Tags last.
    let mut ordered: Vec<&Work> = plan.work.iter().collect();
    ordered.sort_by_key(|w| match w {
        Work::AddTag { .. } => 0,
        Work::RemoveTag { .. } => 2,
        _ => 1,
    });
    for work in ordered {
        match work {
            Work::Add {
                entry,
                secrets,
                overrides,
            } => {
                let id = uuid::Uuid::new_v4().to_string();
                sqlx::query(
                    "INSERT INTO printers (id, name, driver, settings, secrets, created_at, updated_at) \
                     VALUES (?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(&id)
                .bind(&entry.name)
                .bind(&entry.driver)
                .bind(serde_json::to_string(&entry.settings)?)
                .bind(encrypt_secrets(key, secrets))
                .bind(&at)
                .bind(&at)
                .execute(&mut *tx)
                .await?;
                write_overrides(&mut tx, &id, overrides).await?;
                write_tags(&mut tx, &id, &entry.tags).await?;
                applied.started.push(id);
            }
            Work::Update {
                id,
                entry,
                secrets,
                overrides,
            } => {
                sqlx::query(
                    "UPDATE printers SET name = ?, settings = ?, secrets = ?, updated_at = ? WHERE id = ?",
                )
                .bind(&entry.name)
                .bind(serde_json::to_string(&entry.settings)?)
                .bind(encrypt_secrets(key, secrets))
                .bind(&at)
                .bind(id)
                .execute(&mut *tx)
                .await?;
                write_overrides(&mut tx, id, overrides).await?;
                write_tags(&mut tx, id, &entry.tags).await?;
                applied.started.push(id.clone());
            }
            Work::Remove { id } => {
                sqlx::query("DELETE FROM printers WHERE id = ?")
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
                applied.removed.push(id.clone());
            }
            Work::SetSetting { key, value } => {
                sqlx::query(
                    "INSERT INTO settings (key, value, updated_at) VALUES (?, ?, ?) \
                     ON CONFLICT (key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
                )
                .bind(key)
                .bind(value.to_string())
                .bind(&at)
                .execute(&mut *tx)
                .await?;
            }
            Work::RemoveSetting { key } => {
                sqlx::query("DELETE FROM settings WHERE key = ?")
                    .bind(key)
                    .execute(&mut *tx)
                    .await?;
            }
            Work::AddTag { name } => {
                sqlx::query("INSERT INTO tags (name, created_at) VALUES (?, ?)")
                    .bind(name)
                    .bind(&at)
                    .execute(&mut *tx)
                    .await?;
            }
            Work::RemoveTag { name } => {
                sqlx::query("DELETE FROM tags WHERE name = ?")
                    .bind(name)
                    .execute(&mut *tx)
                    .await?;
            }
        }
    }
    tx.commit().await?;
    Ok(applied)
}

/// Replaces a Printer's Tags with these names, which exist by now.
async fn write_tags(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    printer: &str,
    tags: &[String],
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM printer_tags WHERE printer_id = ?")
        .bind(printer)
        .execute(&mut **tx)
        .await?;
    for name in tags {
        sqlx::query(
            "INSERT OR IGNORE INTO printer_tags (printer_id, tag_id) \
             SELECT ?, id FROM tags WHERE name = ?",
        )
        .bind(printer)
        .bind(name)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn write_overrides(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    printer: &str,
    overrides: &Overrides,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM capability_overrides WHERE printer_id = ?")
        .bind(printer)
        .execute(&mut **tx)
        .await?;
    for (capability, on) in overrides {
        sqlx::query(
            "INSERT INTO capability_overrides (printer_id, capability, enabled) VALUES (?, ?, ?)",
        )
        .bind(printer)
        .bind(capability_name(*capability))
        .bind(on)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

impl Store {
    /// Every Gantry-wide setting.
    pub async fn settings(&self) -> Result<BTreeMap<String, Value>, sqlx::Error> {
        let rows: Vec<(String, String)> = sqlx::query_as("SELECT key, value FROM settings")
            .fetch_all(self.pool())
            .await?;
        Ok(rows
            .into_iter()
            .filter_map(|(k, v)| serde_json::from_str(&v).ok().map(|v| (k, v)))
            .collect())
    }

    /// Sets one Gantry-wide setting.
    pub async fn set_setting(&self, key: &str, value: &Value) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO settings (key, value, updated_at) VALUES (?, ?, ?) \
             ON CONFLICT (key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        )
        .bind(key)
        .bind(value.to_string())
        .bind(rfc3339(now()))
        .execute(self.pool())
        .await?;
        Ok(())
    }
}
