//! The Queue (PRD §7.3): Jobs waiting for a Printer, first in, first out,
//! in an order Operators can change by hand. Kept in SQLite, so the order
//! survives a restart. A queued Job names a Printer, or none for any
//! Compatible Printer.
use serde::Serialize;
use time::OffsetDateTime;

use super::{Store, now, parse_time, rfc3339};

/// One queued Job: one Plate of one Print File, for one Printer or for any
/// Compatible Printer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QueuedJob {
    pub id: i64,
    pub print_file_id: i64,
    pub print_file: String,
    pub plate: u32,
    pub plate_name: Option<String>,
    pub estimated_secs: Option<u64>,
    /// `None` for any Compatible Printer.
    pub printer_id: Option<String>,
    /// Its place in the Queue, from 1.
    pub position: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub queued_at: OffsetDateTime,
    pub queued_by: Option<String>,
}

type Row = (
    i64,
    i64,
    String,
    i64,
    Option<String>,
    Option<i64>,
    Option<String>,
    i64,
    String,
    Option<String>,
);

const SELECT: &str = "SELECT q.id, q.print_file_id, f.name, q.plate, p.name, p.estimated_secs, \
                      q.printer_id, q.position, q.queued_at, q.queued_by \
                      FROM queued_jobs q JOIN print_files f ON f.id = q.print_file_id \
                      LEFT JOIN plates p ON p.print_file_id = q.print_file_id AND p.number = q.plate";

fn queued(row: Row) -> QueuedJob {
    let (id, print_file_id, print_file, plate, plate_name, secs, printer_id, position, at, by) =
        row;
    QueuedJob {
        id,
        print_file_id,
        print_file,
        plate: u32::try_from(plate).unwrap_or(1),
        plate_name,
        estimated_secs: secs.and_then(|s| u64::try_from(s).ok()),
        printer_id,
        position,
        queued_at: parse_time(&at).unwrap_or_else(now),
        queued_by: by,
    }
}

impl Store {
    /// The whole Queue, in order.
    pub async fn queue(&self) -> Result<Vec<QueuedJob>, sqlx::Error> {
        let rows: Vec<Row> = sqlx::query_as(&format!("{SELECT} ORDER BY q.position, q.id"))
            .fetch_all(self.pool())
            .await?;
        Ok(rows.into_iter().map(queued).collect())
    }

    pub async fn queued_job(&self, id: i64) -> Result<Option<QueuedJob>, sqlx::Error> {
        let row: Option<Row> = sqlx::query_as(&format!("{SELECT} WHERE q.id = ?"))
            .bind(id)
            .fetch_optional(self.pool())
            .await?;
        Ok(row.map(queued))
    }

    /// Adds one queued Job per Plate, in Plate order, at the back: for
    /// `printer`, or for any Compatible Printer when it is `None`.
    pub async fn enqueue(
        &self,
        printer: Option<&str>,
        print_file: i64,
        plates: &[u32],
        by: Option<&str>,
    ) -> Result<Vec<i64>, sqlx::Error> {
        let mut tx = self.pool().begin().await?;
        let last: Option<i64> = sqlx::query_scalar("SELECT MAX(position) FROM queued_jobs")
            .fetch_one(&mut *tx)
            .await?;
        let mut position = last.unwrap_or(0);
        let mut ids = Vec::new();
        let at = rfc3339(now());
        for plate in plates {
            position += 1;
            let done = sqlx::query(
                "INSERT INTO queued_jobs (print_file_id, plate, printer_id, position, queued_at, \
                 queued_by) VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(print_file)
            .bind(i64::from(*plate))
            .bind(printer)
            .bind(position)
            .bind(&at)
            .bind(by)
            .execute(&mut *tx)
            .await?;
            ids.push(done.last_insert_rowid());
        }
        tx.commit().await?;
        Ok(ids)
    }

    pub async fn dequeue(&self, id: i64) -> Result<bool, sqlx::Error> {
        let done = sqlx::query("DELETE FROM queued_jobs WHERE id = ?")
            .bind(id)
            .execute(self.pool())
            .await?;
        Ok(done.rows_affected() > 0)
    }

    /// Puts the Queue in this order. `order` must name every queued Job
    /// exactly once; returns `false`, changing nothing, if it doesn't.
    pub async fn reorder_queue(&self, order: &[i64]) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool().begin().await?;
        let mut ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM queued_jobs")
            .fetch_all(&mut *tx)
            .await?;
        let mut wanted = order.to_vec();
        ids.sort_unstable();
        wanted.sort_unstable();
        if ids != wanted {
            return Ok(false);
        }
        for (i, id) in order.iter().enumerate() {
            sqlx::query("UPDATE queued_jobs SET position = ? WHERE id = ?")
                .bind(i as i64 + 1)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(true)
    }
}
