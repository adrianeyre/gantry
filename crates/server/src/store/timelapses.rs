//! Timelapses (PRD §7.5): each Printer's setting, and each Job's frames.
use serde::{Deserialize, Serialize};

use super::{Store, now, parse_time, rfc3339};

/// When a Printer captures frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Off,
    /// One frame each time the layer changes.
    Layer,
    /// One frame every `interval_secs`.
    Interval,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Layer => "layer",
            Self::Interval => "interval",
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "layer" => Self::Layer,
            "interval" => Self::Interval,
            _ => Self::Off,
        }
    }
}

/// A Printer's timelapse setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimelapseSetting {
    pub mode: Mode,
    pub interval_secs: u32,
}

impl Default for TimelapseSetting {
    fn default() -> Self {
        Self {
            mode: Mode::Off,
            interval_secs: 30,
        }
    }
}

/// One Job's timelapse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Timelapse {
    pub job_id: i64,
    pub printer_id: String,
    pub frames: u32,
    pub bytes: u64,
    pub started_at: String,
    pub updated_at: String,
    pub finished: bool,
}

type Row = (i64, String, i64, i64, String, String, bool);

fn timelapse(row: Row) -> Timelapse {
    let (job_id, printer_id, frames, bytes, started_at, updated_at, finished) = row;
    Timelapse {
        job_id,
        printer_id,
        frames: u32::try_from(frames).unwrap_or(0),
        bytes: u64::try_from(bytes).unwrap_or(0),
        started_at,
        updated_at,
        finished,
    }
}

const SELECT: &str =
    "SELECT job_id, printer_id, frames, bytes, started_at, updated_at, finished FROM timelapses";

impl Store {
    /// A Printer's setting, or `None` if there is no such Printer.
    pub async fn timelapse_setting(
        &self,
        printer: &str,
    ) -> Result<Option<TimelapseSetting>, sqlx::Error> {
        let row: Option<(String, i64)> =
            sqlx::query_as("SELECT timelapse, timelapse_interval_secs FROM printers WHERE id = ?")
                .bind(printer)
                .fetch_optional(self.pool())
                .await?;
        Ok(row.map(|(mode, secs)| TimelapseSetting {
            mode: Mode::parse(&mode),
            interval_secs: u32::try_from(secs).unwrap_or(30),
        }))
    }

    /// Changes a Printer's setting. `false` if there is no such Printer.
    pub async fn set_timelapse_setting(
        &self,
        printer: &str,
        setting: TimelapseSetting,
    ) -> Result<bool, sqlx::Error> {
        let done = sqlx::query(
            "UPDATE printers SET timelapse = ?, timelapse_interval_secs = ? WHERE id = ?",
        )
        .bind(setting.mode.as_str())
        .bind(i64::from(setting.interval_secs))
        .bind(printer)
        .execute(self.pool())
        .await?;
        Ok(done.rows_affected() > 0)
    }

    pub async fn timelapse(&self, job_id: i64) -> Result<Option<Timelapse>, sqlx::Error> {
        let row: Option<Row> = sqlx::query_as(&format!("{SELECT} WHERE job_id = ?"))
            .bind(job_id)
            .fetch_optional(self.pool())
            .await?;
        Ok(row.map(timelapse))
    }

    pub async fn timelapses(&self) -> Result<Vec<Timelapse>, sqlx::Error> {
        let rows: Vec<Row> = sqlx::query_as(&format!("{SELECT} ORDER BY job_id"))
            .fetch_all(self.pool())
            .await?;
        Ok(rows.into_iter().map(timelapse).collect())
    }

    /// Records one more frame of `bytes` for a Job, starting its timelapse
    /// if this is the first. Returns the frame's number, from 1.
    pub async fn add_timelapse_frame(
        &self,
        job_id: i64,
        printer_id: &str,
        bytes: u64,
    ) -> Result<u32, sqlx::Error> {
        let at = rfc3339(now());
        let frames: i64 = sqlx::query_scalar(
            "INSERT INTO timelapses (job_id, printer_id, frames, bytes, started_at, updated_at) \
             VALUES (?, ?, 1, ?, ?, ?) \
             ON CONFLICT (job_id) DO UPDATE SET frames = frames + 1, bytes = bytes + excluded.bytes, \
             updated_at = excluded.updated_at RETURNING frames",
        )
        .bind(job_id)
        .bind(printer_id)
        .bind(i64::try_from(bytes).unwrap_or(i64::MAX))
        .bind(&at)
        .bind(&at)
        .fetch_one(self.pool())
        .await?;
        Ok(u32::try_from(frames).unwrap_or(0))
    }

    pub async fn finish_timelapse(&self, job_id: i64) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE timelapses SET finished = 1 WHERE job_id = ?")
            .bind(job_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// Forgets a timelapse. Its Job stays, untouched.
    pub async fn delete_timelapse(&self, job_id: i64) -> Result<bool, sqlx::Error> {
        let done = sqlx::query("DELETE FROM timelapses WHERE job_id = ?")
            .bind(job_id)
            .execute(self.pool())
            .await?;
        Ok(done.rows_affected() > 0)
    }

    /// Each Job's frame count, for History.
    pub async fn timelapse_frames(
        &self,
        printer_id: &str,
    ) -> Result<std::collections::HashMap<i64, u32>, sqlx::Error> {
        let rows: Vec<(i64, i64)> =
            sqlx::query_as("SELECT job_id, frames FROM timelapses WHERE printer_id = ?")
                .bind(printer_id)
                .fetch_all(self.pool())
                .await?;
        Ok(rows
            .into_iter()
            .map(|(id, frames)| (id, u32::try_from(frames).unwrap_or(0)))
            .collect())
    }
}

impl Timelapse {
    /// When it was started, for cleanup's oldest-first order.
    pub fn started(&self) -> time::OffsetDateTime {
        parse_time(&self.started_at).unwrap_or(time::OffsetDateTime::UNIX_EPOCH)
    }
}
