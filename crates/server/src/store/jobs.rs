//! The Job log in SQLite, so Jobs and Awaiting Clear survive a restart.
use async_trait::async_trait;
use gantry_core::{
    ActiveJob, JobId, JobLog, JobLogError, JobOutcome, JobRecord, JobStart, JobState, PrinterId,
};

use super::{Store, now, parse_time, rfc3339};

type Row = (
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
);

const COLUMNS: &str = "id, printer_id, external, driver_key, print_file, plate, started_at, \
                       ended_at, outcome, overruled, print_file_id";

fn record(row: Row) -> JobRecord {
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
    ) = row;
    JobRecord {
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
    }
}

fn failed(e: sqlx::Error) -> JobLogError {
    JobLogError(e.to_string())
}

impl Store {
    pub async fn job(&self, id: JobId) -> Result<Option<JobRecord>, sqlx::Error> {
        let row: Option<Row> = sqlx::query_as(&format!("SELECT {COLUMNS} FROM jobs WHERE id = ?"))
            .bind(id)
            .fetch_optional(self.pool())
            .await?;
        Ok(row.map(record))
    }

    /// A Printer's Jobs, newest first.
    pub async fn printer_jobs(
        &self,
        printer: &str,
        limit: i64,
    ) -> Result<Vec<JobRecord>, sqlx::Error> {
        let rows: Vec<Row> = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM jobs WHERE printer_id = ? ORDER BY started_at DESC, id DESC LIMIT ?"
        ))
        .bind(printer)
        .bind(limit)
        .fetch_all(self.pool())
        .await?;
        Ok(rows.into_iter().map(record).collect())
    }

    /// How many Jobs have ended, by Printer and Outcome.
    pub async fn job_counts(&self) -> Result<Vec<(String, JobOutcome, i64)>, sqlx::Error> {
        let rows: Vec<(String, String, i64)> = sqlx::query_as(
            "SELECT printer_id, outcome, COUNT(*) FROM jobs WHERE outcome IS NOT NULL              GROUP BY printer_id, outcome ORDER BY printer_id, outcome",
        )
        .fetch_all(self.pool())
        .await?;
        Ok(rows
            .into_iter()
            .filter_map(|(p, o, n)| JobOutcome::parse(&o).map(|o| (p, o, n)))
            .collect())
    }

    /// What a Printer's Job log says, for starting it: its open Job and the
    /// Job it is Awaiting Clear after.
    pub async fn job_state(&self, printer: &str) -> Result<JobState, sqlx::Error> {
        let open: Option<Row> = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM jobs WHERE printer_id = ? AND ended_at IS NULL \
             ORDER BY started_at DESC, id DESC LIMIT 1"
        ))
        .bind(printer)
        .fetch_optional(self.pool())
        .await?;
        let waiting: Option<i64> =
            sqlx::query_scalar("SELECT awaiting_clear_job FROM printers WHERE id = ?")
                .bind(printer)
                .fetch_optional(self.pool())
                .await?
                .flatten();
        let awaiting_clear = match waiting {
            Some(id) => self.job(id).await?,
            None => None,
        };
        Ok(JobState {
            open: open.map(record),
            awaiting_clear,
        })
    }

    async fn update_job(&self, id: JobId) -> Result<JobRecord, JobLogError> {
        self.job(id)
            .await
            .map_err(failed)?
            .ok_or_else(|| JobLogError(format!("no Job {id}")))
    }
}

/// [`JobLog`] over the [`Store`].
#[derive(Clone, Debug)]
pub struct SqlJobLog(pub Store);

#[async_trait]
impl JobLog for SqlJobLog {
    async fn external_job_started(
        &self,
        printer: &PrinterId,
        job: &ActiveJob,
    ) -> Result<JobRecord, JobLogError> {
        let name: Option<String> = sqlx::query_scalar("SELECT name FROM printers WHERE id = ?")
            .bind(printer)
            .fetch_optional(self.0.pool())
            .await
            .map_err(failed)?;
        let done = sqlx::query(
            "INSERT INTO jobs (printer_id, printer_name, external, driver_key, print_file, plate, started_at) \
             VALUES (?, ?, 1, ?, ?, ?, ?)",
        )
        .bind(printer)
        .bind(name.unwrap_or_else(|| printer.clone()))
        .bind(&job.key)
        .bind(&job.print_file)
        .bind(job.plate.map(i64::from))
        .bind(rfc3339(job.started_at.unwrap_or_else(now)))
        .execute(self.0.pool())
        .await
        .map_err(failed)?;
        self.0.update_job(done.last_insert_rowid()).await
    }

    async fn find(&self, printer: &PrinterId, key: &str) -> Result<Option<JobRecord>, JobLogError> {
        let row: Option<Row> = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM jobs WHERE printer_id = ? AND driver_key = ? \
             ORDER BY id DESC LIMIT 1"
        ))
        .bind(printer)
        .bind(key)
        .fetch_optional(self.0.pool())
        .await
        .map_err(failed)?;
        Ok(row.map(record))
    }

    async fn rekeyed(&self, job: JobId, key: &str) -> Result<JobRecord, JobLogError> {
        sqlx::query("UPDATE jobs SET driver_key = ? WHERE id = ?")
            .bind(key)
            .bind(job)
            .execute(self.0.pool())
            .await
            .map_err(failed)?;
        self.0.update_job(job).await
    }

    async fn job_started(
        &self,
        printer: &PrinterId,
        start: &JobStart,
        key: Option<&str>,
    ) -> Result<JobRecord, JobLogError> {
        let name: Option<String> = sqlx::query_scalar("SELECT name FROM printers WHERE id = ?")
            .bind(printer)
            .fetch_optional(self.0.pool())
            .await
            .map_err(failed)?;
        let done = sqlx::query(
            "INSERT INTO jobs (printer_id, printer_name, external, driver_key, print_file,              print_file_id, plate, started_at) VALUES (?, ?, 0, ?, ?, ?, ?, ?)",
        )
        .bind(printer)
        .bind(name.unwrap_or_else(|| printer.clone()))
        .bind(key.unwrap_or_default())
        .bind(&start.print_file)
        .bind(start.print_file_id)
        .bind(i64::from(start.plate))
        .bind(rfc3339(now()))
        .execute(self.0.pool())
        .await
        .map_err(failed)?;
        self.0.update_job(done.last_insert_rowid()).await
    }

    async fn job_abandoned(&self, job: JobId) -> Result<(), JobLogError> {
        // Only a Job that never ran: one with an ending is History.
        sqlx::query("DELETE FROM jobs WHERE id = ? AND ended_at IS NULL")
            .bind(job)
            .execute(self.0.pool())
            .await
            .map_err(failed)?;
        Ok(())
    }

    async fn job_ended(
        &self,
        printer: &PrinterId,
        job: JobId,
        outcome: JobOutcome,
    ) -> Result<JobRecord, JobLogError> {
        let mut tx = self.0.pool().begin().await.map_err(failed)?;
        sqlx::query("UPDATE jobs SET ended_at = ?, outcome = ? WHERE id = ? AND ended_at IS NULL")
            .bind(rfc3339(now()))
            .bind(outcome.as_str())
            .bind(job)
            .execute(&mut *tx)
            .await
            .map_err(failed)?;
        sqlx::query("UPDATE printers SET awaiting_clear_job = ? WHERE id = ?")
            .bind(job)
            .bind(printer)
            .execute(&mut *tx)
            .await
            .map_err(failed)?;
        tx.commit().await.map_err(failed)?;
        self.0.update_job(job).await
    }

    async fn bed_cleared(
        &self,
        printer: &PrinterId,
        job: JobId,
        overrule: bool,
    ) -> Result<JobRecord, JobLogError> {
        let mut tx = self.0.pool().begin().await.map_err(failed)?;
        if overrule {
            // Only Succeeded can be overruled, and only to Failed.
            sqlx::query(
                "UPDATE jobs SET outcome = 'failed', overruled = 1 \
                 WHERE id = ? AND outcome = 'succeeded'",
            )
            .bind(job)
            .execute(&mut *tx)
            .await
            .map_err(failed)?;
        }
        sqlx::query("UPDATE printers SET awaiting_clear_job = NULL WHERE id = ?")
            .bind(printer)
            .execute(&mut *tx)
            .await
            .map_err(failed)?;
        tx.commit().await.map_err(failed)?;
        self.0.update_job(job).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::SecretKey;
    use crate::store::printers::PrinterInput;

    async fn store_with_printer() -> Store {
        let store = Store::in_memory().await.unwrap();
        let key = SecretKey::from_config("a secret key for tests only").unwrap();
        store
            .insert_printer(
                &key,
                "p",
                &PrinterInput {
                    name: "P".into(),
                    driver: "simulated".into(),
                    settings: Default::default(),
                    secrets: Default::default(),
                },
            )
            .await
            .unwrap();
        store
    }

    fn active(key: &str) -> ActiveJob {
        ActiveJob {
            key: key.into(),
            print_file: Some("cube.gcode".into()),
            plate: Some(1),
            progress: 0.0,
            layer: None,
            total_layers: None,
            remaining_secs: None,
            started_at: None,
        }
    }

    #[tokio::test]
    async fn records_a_job_through_to_clear() {
        let store = store_with_printer().await;
        let log = SqlJobLog(store.clone());
        let p = "p".to_owned();
        let job = log.external_job_started(&p, &active("k")).await.unwrap();
        assert!(job.external);
        assert_eq!(job.plate, Some(1));
        assert_eq!(store.job_state("p").await.unwrap().open.unwrap().id, job.id);

        let ended = log
            .job_ended(&p, job.id, JobOutcome::Succeeded)
            .await
            .unwrap();
        assert_eq!(ended.outcome, Some(JobOutcome::Succeeded));
        let state = store.job_state("p").await.unwrap();
        assert!(state.open.is_none());
        assert_eq!(state.awaiting_clear.unwrap().id, job.id);

        let cleared = log.bed_cleared(&p, job.id, true).await.unwrap();
        assert_eq!(cleared.outcome, Some(JobOutcome::Failed));
        assert!(cleared.overruled);
        assert!(store.job_state("p").await.unwrap().awaiting_clear.is_none());
    }

    #[tokio::test]
    async fn only_succeeded_can_be_overruled() {
        let store = store_with_printer().await;
        let log = SqlJobLog(store.clone());
        let p = "p".to_owned();
        let job = log.external_job_started(&p, &active("k")).await.unwrap();
        log.job_ended(&p, job.id, JobOutcome::Cancelled)
            .await
            .unwrap();
        let cleared = log.bed_cleared(&p, job.id, true).await.unwrap();
        assert_eq!(cleared.outcome, Some(JobOutcome::Cancelled));
        assert!(!cleared.overruled);
    }

    #[tokio::test]
    async fn history_outlives_the_printer() {
        let store = store_with_printer().await;
        let log = SqlJobLog(store.clone());
        let p = "p".to_owned();
        let job = log.external_job_started(&p, &active("k")).await.unwrap();
        log.job_ended(&p, job.id, JobOutcome::Succeeded)
            .await
            .unwrap();
        assert!(store.delete_printer("p").await.unwrap());
        assert_eq!(store.printer_jobs("p", 10).await.unwrap().len(), 1);
    }
}
