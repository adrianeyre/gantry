//! History (PRD §7.5) in SQLite: what each Job used, the filament
//! inventory, and the record of each Printer's state over time.
use gantry_core::{JobOutcome, JobRecord, PrinterState};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use super::{Store, now, parse_time, rfc3339};
use crate::analytics::{Activity, FilamentUse, HistoryJob, Span, SpoolStock, duration};

type JobRow = (
    i64,
    String,
    bool,
    String,
    Option<String>,
    Option<i64>,
    String,
    Option<String>,
    Option<String>,
    bool,
    Option<i64>,
    Option<f64>,
    Option<f64>,
    Option<String>,
);

const JOB_COLUMNS: &str = "id, printer_id, external, driver_key, print_file, plate, started_at, \
                           ended_at, outcome, overruled, print_file_id, filament_g, \
                           filament_cost, filament_usage";

fn history_job(row: JobRow) -> HistoryJob {
    let (
        id,
        printer_id,
        external,
        driver_key,
        print_file,
        plate,
        started,
        ended,
        outcome,
        overruled,
        print_file_id,
        grams,
        cost,
        usage,
    ) = row;
    let job = JobRecord {
        id,
        printer_id,
        external,
        driver_key,
        print_file,
        print_file_id,
        plate: plate.and_then(|p| u32::try_from(p).ok()),
        started_at: parse_time(&started).unwrap_or_else(now),
        ended_at: ended.as_deref().and_then(parse_time),
        outcome: outcome.as_deref().and_then(JobOutcome::parse),
        overruled,
    };
    HistoryJob {
        duration_secs: duration(&job),
        job,
        filament_g: grams,
        filament_cost: cost,
        filament_usage: usage
            .as_deref()
            .and_then(|u| serde_json::from_str(u).ok())
            .unwrap_or_default(),
        timelapse_frames: None,
    }
}

/// A spool in the filament inventory.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Spool {
    pub id: i64,
    pub name: Option<String>,
    pub kind: String,
    pub colour: String,
    /// Its filament when full, in grams.
    pub weight_g: f64,
    pub remaining_g: f64,
    /// What the whole spool cost.
    pub cost: f64,
    /// The Printer it is loaded on, if any.
    pub printer_id: Option<String>,
    /// The Filament Slot it is in on that Printer, if known.
    pub slot_id: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

/// A new or changed spool.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SpoolInput {
    #[serde(default)]
    pub name: Option<String>,
    pub kind: String,
    pub colour: String,
    pub weight_g: f64,
    /// Full when left out.
    #[serde(default)]
    pub remaining_g: Option<f64>,
    pub cost: f64,
    #[serde(default)]
    pub printer_id: Option<String>,
    #[serde(default)]
    pub slot_id: Option<String>,
}

type SpoolRow = (
    i64,
    Option<String>,
    String,
    String,
    f64,
    f64,
    f64,
    Option<String>,
    Option<String>,
    String,
);

const SPOOL_COLUMNS: &str =
    "id, name, kind, colour, weight_g, remaining_g, cost, printer_id, slot_id, updated_at";

fn spool(
    (id, name, kind, colour, weight_g, remaining_g, cost, printer_id, slot_id, at): SpoolRow,
) -> Spool {
    Spool {
        id,
        name,
        kind,
        colour,
        weight_g,
        remaining_g,
        cost,
        printer_id,
        slot_id,
        updated_at: parse_time(&at).unwrap_or_else(now),
    }
}

fn state_name(state: Option<PrinterState>) -> Option<String> {
    state.and_then(|s| match serde_json::to_value(s) {
        Ok(serde_json::Value::String(name)) => Some(name),
        _ => None,
    })
}

fn parse_state(name: Option<&str>) -> Option<PrinterState> {
    name.and_then(|n| serde_json::from_value(serde_json::Value::String(n.to_owned())).ok())
}

impl Store {
    /// A Printer's Jobs that started from `from` up to `to`, newest first.
    pub async fn job_history(
        &self,
        printer: &str,
        from: Option<OffsetDateTime>,
        to: Option<OffsetDateTime>,
    ) -> Result<Vec<HistoryJob>, sqlx::Error> {
        let rows: Vec<JobRow> = sqlx::query_as(&format!(
            "SELECT {JOB_COLUMNS} FROM jobs WHERE printer_id = ? \
             AND (? IS NULL OR started_at >= ?) AND (? IS NULL OR started_at < ?) \
             ORDER BY started_at DESC, id DESC"
        ))
        .bind(printer)
        .bind(from.map(rfc3339))
        .bind(from.map(rfc3339))
        .bind(to.map(rfc3339))
        .bind(to.map(rfc3339))
        .fetch_all(self.pool())
        .await?;
        Ok(rows.into_iter().map(history_job).collect())
    }

    /// Records what a Job used, once, and takes it off the spools it came
    /// from. A Job already recorded is left as it is: `false`.
    pub async fn record_filament(
        &self,
        job: i64,
        usage: &[FilamentUse],
    ) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool().begin().await?;
        let grams: f64 = usage.iter().map(|u| u.grams).sum();
        let costs: Vec<f64> = usage.iter().filter_map(|u| u.cost).collect();
        let done = sqlx::query(
            "UPDATE jobs SET filament_g = ?, filament_cost = ?, filament_usage = ? \
             WHERE id = ? AND filament_usage IS NULL",
        )
        .bind(grams)
        .bind((!costs.is_empty()).then(|| costs.iter().sum::<f64>()))
        .bind(serde_json::to_string(usage).unwrap_or_else(|_| "[]".into()))
        .bind(job)
        .execute(&mut *tx)
        .await?;
        if done.rows_affected() == 0 {
            return Ok(false);
        }
        let at = rfc3339(now());
        for u in usage {
            if let Some(spool) = u.spool_id {
                sqlx::query(
                    "UPDATE spools SET remaining_g = MAX(0, remaining_g - ?), updated_at = ? \
                     WHERE id = ?",
                )
                .bind(u.grams)
                .bind(&at)
                .bind(spool)
                .execute(&mut *tx)
                .await?;
            }
        }
        tx.commit().await?;
        Ok(true)
    }

    /// The whole inventory, by type then colour.
    pub async fn spools(&self) -> Result<Vec<Spool>, sqlx::Error> {
        let rows: Vec<SpoolRow> = sqlx::query_as(&format!(
            "SELECT {SPOOL_COLUMNS} FROM spools ORDER BY kind COLLATE NOCASE, colour, id"
        ))
        .fetch_all(self.pool())
        .await?;
        Ok(rows.into_iter().map(spool).collect())
    }

    pub async fn spool(&self, id: i64) -> Result<Option<Spool>, sqlx::Error> {
        let row: Option<SpoolRow> =
            sqlx::query_as(&format!("SELECT {SPOOL_COLUMNS} FROM spools WHERE id = ?"))
                .bind(id)
                .fetch_optional(self.pool())
                .await?;
        Ok(row.map(spool))
    }

    /// The spools loaded on a Printer, with something left on them.
    pub async fn loaded_spools(&self, printer: &str) -> Result<Vec<SpoolStock>, sqlx::Error> {
        let rows: Vec<(i64, String, String, f64, f64)> = sqlx::query_as(
            "SELECT id, kind, colour, weight_g, cost FROM spools \
             WHERE printer_id = ? AND remaining_g > 0 ORDER BY id",
        )
        .bind(printer)
        .fetch_all(self.pool())
        .await?;
        Ok(rows
            .into_iter()
            .map(|(id, kind, colour, weight_g, cost)| SpoolStock {
                id,
                kind,
                colour,
                weight_g,
                cost,
            })
            .collect())
    }

    pub async fn insert_spool(&self, input: &SpoolInput) -> Result<Spool, sqlx::Error> {
        let at = rfc3339(now());
        let done = sqlx::query(
            "INSERT INTO spools (name, kind, colour, weight_g, remaining_g, cost, printer_id, \
             slot_id, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&input.name)
        .bind(input.kind.trim())
        .bind(input.colour.trim())
        .bind(input.weight_g)
        .bind(input.remaining_g.unwrap_or(input.weight_g))
        .bind(input.cost)
        .bind(&input.printer_id)
        .bind(&input.slot_id)
        .bind(&at)
        .bind(&at)
        .execute(self.pool())
        .await?;
        Ok(self
            .spool(done.last_insert_rowid())
            .await?
            .expect("the spool just added"))
    }

    pub async fn update_spool(
        &self,
        id: i64,
        input: &SpoolInput,
    ) -> Result<Option<Spool>, sqlx::Error> {
        let done = sqlx::query(
            "UPDATE spools SET name = ?, kind = ?, colour = ?, weight_g = ?, \
             remaining_g = COALESCE(?, remaining_g), cost = ?, printer_id = ?, slot_id = ?, \
             updated_at = ? WHERE id = ?",
        )
        .bind(&input.name)
        .bind(input.kind.trim())
        .bind(input.colour.trim())
        .bind(input.weight_g)
        .bind(input.remaining_g)
        .bind(input.cost)
        .bind(&input.printer_id)
        .bind(&input.slot_id)
        .bind(rfc3339(now()))
        .bind(id)
        .execute(self.pool())
        .await?;
        if done.rows_affected() == 0 {
            return Ok(None);
        }
        self.spool(id).await
    }

    pub async fn delete_spool(&self, id: i64) -> Result<bool, sqlx::Error> {
        let done = sqlx::query("DELETE FROM spools WHERE id = ?")
            .bind(id)
            .execute(self.pool())
            .await?;
        Ok(done.rows_affected() > 0)
    }

    /// Records that a Printer's Connection or Printer State changed at `at`:
    /// its last stretch ends there and a new one begins.
    pub async fn state_changed(
        &self,
        printer: &str,
        connected: bool,
        state: Option<PrinterState>,
        at: OffsetDateTime,
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.pool().begin().await?;
        let at = rfc3339(at);
        sqlx::query(
            "UPDATE printer_states SET seen_until = ? WHERE id = \
             (SELECT id FROM printer_states WHERE printer_id = ? ORDER BY id DESC LIMIT 1) \
             AND seen_until < ?",
        )
        .bind(&at)
        .bind(printer)
        .bind(&at)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO printer_states (printer_id, connected, state, started_at, seen_until) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(printer)
        .bind(connected)
        .bind(state_name(state))
        .bind(&at)
        .bind(&at)
        .execute(&mut *tx)
        .await?;
        tx.commit().await
    }

    /// Records that a Printer's current stretch still held at `at`.
    pub async fn state_seen(&self, printer: &str, at: OffsetDateTime) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE printer_states SET seen_until = ? WHERE id = \
             (SELECT id FROM printer_states WHERE printer_id = ? ORDER BY id DESC LIMIT 1)",
        )
        .bind(rfc3339(at))
        .bind(printer)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// The recorded stretches that overlap `from` to `to`, of one Printer or
    /// of every one, with whose they are.
    pub async fn state_spans(
        &self,
        printer: Option<&str>,
        from: OffsetDateTime,
        to: OffsetDateTime,
    ) -> Result<Vec<(String, Span)>, sqlx::Error> {
        let rows: Vec<(String, bool, Option<String>, String, String)> = sqlx::query_as(
            "SELECT printer_id, connected, state, started_at, seen_until FROM printer_states \
             WHERE (? IS NULL OR printer_id = ?) AND started_at < ? AND seen_until > ? \
             ORDER BY printer_id, started_at, id",
        )
        .bind(printer)
        .bind(printer)
        .bind(rfc3339(to))
        .bind(rfc3339(from))
        .fetch_all(self.pool())
        .await?;
        Ok(rows
            .into_iter()
            .filter_map(|(printer, connected, state, start, end)| {
                Some((
                    printer,
                    Span {
                        activity: Activity::of(connected, parse_state(state.as_deref())),
                        start: parse_time(&start)?,
                        end: parse_time(&end)?,
                    },
                ))
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use time::Duration;

    use super::*;
    use crate::analytics::breakdown;

    fn input(kind: &str, colour: &str, printer: Option<&str>) -> SpoolInput {
        SpoolInput {
            name: None,
            kind: kind.into(),
            colour: colour.into(),
            weight_g: 1000.0,
            remaining_g: None,
            cost: 25.0,
            printer_id: printer.map(Into::into),
            slot_id: None,
        }
    }

    #[tokio::test]
    async fn a_jobs_filament_is_recorded_once_and_taken_off_its_spool() {
        let store = Store::in_memory().await.unwrap();
        let spool = store
            .insert_spool(&input("PLA", "#FFFFFF", None))
            .await
            .unwrap();
        assert_eq!(spool.remaining_g, 1000.0, "full when not said");
        sqlx::query(
            "INSERT INTO jobs (id, printer_id, printer_name, external, driver_key, started_at) \
             VALUES (1, 'p', 'P', 0, 'k', '2026-11-01T10:00:00.000Z')",
        )
        .execute(store.pool())
        .await
        .unwrap();
        let usage = [FilamentUse {
            kind: "PLA".into(),
            colour: None,
            grams: 40.0,
            spool_id: Some(spool.id),
            cost: Some(1.0),
        }];
        assert!(store.record_filament(1, &usage).await.unwrap());
        assert!(
            !store.record_filament(1, &usage).await.unwrap(),
            "only once"
        );
        assert_eq!(
            store.spool(spool.id).await.unwrap().unwrap().remaining_g,
            960.0
        );
        let history = store.job_history("p", None, None).await.unwrap();
        assert_eq!(history[0].filament_g, Some(40.0));
        assert_eq!(history[0].filament_cost, Some(1.0));
        assert_eq!(history[0].filament_usage, usage);
    }

    #[tokio::test]
    async fn only_loaded_spools_with_filament_left_are_charged() {
        let store = Store::in_memory().await.unwrap();
        sqlx::query(
            "INSERT INTO printers (id, name, driver, created_at, updated_at) \
             VALUES ('p', 'P', 'simulated', 'x', 'x')",
        )
        .execute(store.pool())
        .await
        .unwrap();
        store
            .insert_spool(&input("PLA", "#FFFFFF", Some("p")))
            .await
            .unwrap();
        let mut empty = input("PETG", "#000000", Some("p"));
        empty.remaining_g = Some(0.0);
        store.insert_spool(&empty).await.unwrap();
        store
            .insert_spool(&input("ABS", "#000000", None))
            .await
            .unwrap();
        let loaded = store.loaded_spools("p").await.unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].kind, "PLA");
        // Removing the Printer unloads its spools but keeps them.
        sqlx::query("DELETE FROM printers")
            .execute(store.pool())
            .await
            .unwrap();
        assert_eq!(store.spools().await.unwrap().len(), 3);
        assert!(
            store
                .spools()
                .await
                .unwrap()
                .iter()
                .all(|s| s.printer_id.is_none())
        );
    }

    #[tokio::test]
    async fn state_changes_become_stretches() {
        let store = Store::in_memory().await.unwrap();
        let t0 = time::macros::datetime!(2026-11-01 10:00 UTC);
        store
            .state_changed("p", true, Some(PrinterState::Idle), t0)
            .await
            .unwrap();
        store
            .state_changed(
                "p",
                true,
                Some(PrinterState::Printing),
                t0 + Duration::hours(1),
            )
            .await
            .unwrap();
        store
            .state_seen("p", t0 + Duration::hours(3))
            .await
            .unwrap();
        let spans: Vec<Span> = store
            .state_spans(Some("p"), t0, t0 + Duration::hours(4))
            .await
            .unwrap()
            .into_iter()
            .map(|(_, s)| s)
            .collect();
        assert_eq!(spans.len(), 2);
        let b = breakdown(&spans, t0, t0 + Duration::hours(4), t0 + Duration::hours(4));
        assert_eq!(b.idle_secs, 3600.0);
        assert_eq!(b.printing_secs, 7200.0);
        assert_eq!(b.unknown_secs, 3600.0, "after the last time it was seen");
        assert!(
            store
                .state_spans(Some("other"), t0, t0 + Duration::hours(4))
                .await
                .unwrap()
                .is_empty()
        );
    }
}
