//! Jobs and Job Outcomes (PRD §7.5).
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::driver::PrinterId;
use crate::status::ActiveJob;

/// How a Job ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobOutcome {
    Succeeded,
    Failed,
    Cancelled,
    /// Gantry never saw the Job end.
    Lost,
}

impl JobOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Lost => "lost",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "succeeded" => Some(Self::Succeeded),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            "lost" => Some(Self::Lost),
            _ => None,
        }
    }
}

/// A Job's identity in the Job log.
pub type JobId = i64;

/// One attempt to print one Plate on one Printer, as recorded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobRecord {
    pub id: JobId,
    pub printer_id: PrinterId,
    /// Started without Gantry (from the Printer's screen, a vendor app or the
    /// simulation panel).
    pub external: bool,
    /// The Driver's key for the print, used to recognise it again.
    pub driver_key: String,
    /// Unknown for many External Jobs.
    pub print_file: Option<String>,
    /// The Print File in Gantry's library, for a Job Gantry started.
    #[serde(default)]
    pub print_file_id: Option<i64>,
    pub plate: Option<u32>,
    #[serde(with = "time::serde::rfc3339")]
    pub started_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub ended_at: Option<OffsetDateTime>,
    pub outcome: Option<JobOutcome>,
    /// An Operator overruled Succeeded to Failed at Mark Clear.
    pub overruled: bool,
}

/// How close a running print's start must be to a recorded Job's start for
/// them to be the same print, when only the Print File's name matches.
pub const REATTACH_WINDOW: time::Duration = time::Duration::minutes(2);

/// How long after Gantry starts a Job the Printer may take to report it
/// printing (an upload, heating, levelling) before the Job counts as Lost.
pub const START_GRACE: time::Duration = time::Duration::minutes(5);

/// A Job Gantry is starting: which Plate of which Print File.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobStart {
    /// The Print File in the library.
    pub print_file_id: i64,
    /// Its name, as the Printer will report it.
    pub print_file: String,
    /// The Plate, from 1.
    pub plate: u32,
}

impl JobRecord {
    /// A Job Gantry started whose print the Printer hasn't reported yet: it
    /// has no Driver key until then.
    pub fn is_pending(&self) -> bool {
        !self.external && self.driver_key.is_empty() && self.ended_at.is_none()
    }

    /// Whether `active` is this Job still running (PRD §7.5): the Driver's
    /// key matches, or the Print File and start time do. A pending Job takes
    /// the first print its Printer reports, since the Printer was idle when
    /// Gantry started it.
    pub fn matches(&self, active: &ActiveJob) -> bool {
        if self.driver_key == active.key || self.is_pending() {
            return true;
        }
        match (&self.print_file, &active.print_file, active.started_at) {
            (Some(recorded), Some(running), Some(started)) if recorded == running => {
                (self.started_at - started).abs() <= REATTACH_WINDOW
            }
            _ => false,
        }
    }
}

/// Why the Job log couldn't record something.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the Job log failed: {0}")]
pub struct JobLogError(pub String);

/// Where the Fleet records Jobs and the Awaiting Clear state, so both survive
/// a restart. The server stores them in SQLite.
#[async_trait]
pub trait JobLog: Send + Sync + 'static {
    /// Records that a Job Gantry didn't start has begun.
    async fn external_job_started(
        &self,
        printer: &PrinterId,
        job: &ActiveJob,
    ) -> Result<JobRecord, JobLogError>;

    /// Records a Job's Outcome, and that its Printer is now Awaiting Clear.
    async fn job_ended(
        &self,
        printer: &PrinterId,
        job: JobId,
        outcome: JobOutcome,
    ) -> Result<JobRecord, JobLogError>;

    /// Records that the Printer's bed is clear: no longer Awaiting Clear.
    /// With `overrule`, a Succeeded Outcome becomes Failed.
    async fn bed_cleared(
        &self,
        printer: &PrinterId,
        job: JobId,
        overrule: bool,
    ) -> Result<JobRecord, JobLogError>;

    /// The newest Job of `printer` with this Driver key, so an ending reported
    /// twice (after a reconnect, say) is never recorded twice.
    async fn find(&self, printer: &PrinterId, key: &str) -> Result<Option<JobRecord>, JobLogError>;

    /// Records the Driver's new key for an open Job it re-attached to by
    /// Print File and start time.
    async fn rekeyed(&self, job: JobId, key: &str) -> Result<JobRecord, JobLogError>;

    /// Records that Gantry is starting a Job on `printer`. Its Driver key is
    /// `key` when known, otherwise empty until the Printer reports the print.
    async fn job_started(
        &self,
        printer: &PrinterId,
        start: &JobStart,
        key: Option<&str>,
    ) -> Result<JobRecord, JobLogError>;

    /// Forgets a Job Gantry tried to start but couldn't (the upload failed),
    /// so a failed start leaves no Job behind.
    async fn job_abandoned(&self, job: JobId) -> Result<(), JobLogError>;
}

/// What a Printer's Job log said when it started: the Job still open, and the
/// ended Job it is Awaiting Clear after.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct JobState {
    pub open: Option<JobRecord>,
    pub awaiting_clear: Option<JobRecord>,
}

/// A Job log that keeps nothing past the process, for tests and tools.
#[derive(Debug, Default)]
pub struct MemoryJobLog {
    jobs: std::sync::Mutex<Vec<JobRecord>>,
}

impl MemoryJobLog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every Job recorded so far.
    pub fn jobs(&self) -> Vec<JobRecord> {
        self.jobs.lock().expect("job log lock").clone()
    }

    fn update(&self, job: JobId, f: impl FnOnce(&mut JobRecord)) -> Result<JobRecord, JobLogError> {
        let mut jobs = self.jobs.lock().expect("job log lock");
        let record = jobs
            .iter_mut()
            .find(|j| j.id == job)
            .ok_or_else(|| JobLogError(format!("no Job {job}")))?;
        f(record);
        Ok(record.clone())
    }
}

#[async_trait]
impl JobLog for MemoryJobLog {
    async fn external_job_started(
        &self,
        printer: &PrinterId,
        job: &ActiveJob,
    ) -> Result<JobRecord, JobLogError> {
        let mut jobs = self.jobs.lock().expect("job log lock");
        let record = JobRecord {
            id: jobs.iter().map(|j| j.id).max().unwrap_or(0) + 1,
            printer_id: printer.clone(),
            external: true,
            driver_key: job.key.clone(),
            print_file: job.print_file.clone(),
            print_file_id: None,
            plate: job.plate,
            started_at: job.started_at.unwrap_or_else(OffsetDateTime::now_utc),
            ended_at: None,
            outcome: None,
            overruled: false,
        };
        jobs.push(record.clone());
        Ok(record)
    }

    async fn job_ended(
        &self,
        _printer: &PrinterId,
        job: JobId,
        outcome: JobOutcome,
    ) -> Result<JobRecord, JobLogError> {
        self.update(job, |j| {
            j.outcome = Some(outcome);
            j.ended_at = Some(OffsetDateTime::now_utc());
        })
    }

    async fn bed_cleared(
        &self,
        _printer: &PrinterId,
        job: JobId,
        overrule: bool,
    ) -> Result<JobRecord, JobLogError> {
        self.update(job, |j| {
            if overrule && j.outcome == Some(JobOutcome::Succeeded) {
                j.outcome = Some(JobOutcome::Failed);
                j.overruled = true;
            }
        })
    }

    async fn find(&self, printer: &PrinterId, key: &str) -> Result<Option<JobRecord>, JobLogError> {
        Ok(self
            .jobs
            .lock()
            .expect("job log lock")
            .iter()
            .rev()
            .find(|j| &j.printer_id == printer && j.driver_key == key)
            .cloned())
    }

    async fn rekeyed(&self, job: JobId, key: &str) -> Result<JobRecord, JobLogError> {
        self.update(job, |j| j.driver_key = key.to_owned())
    }

    async fn job_started(
        &self,
        printer: &PrinterId,
        start: &JobStart,
        key: Option<&str>,
    ) -> Result<JobRecord, JobLogError> {
        let mut jobs = self.jobs.lock().expect("job log lock");
        let record = JobRecord {
            id: jobs.iter().map(|j| j.id).max().unwrap_or(0) + 1,
            printer_id: printer.clone(),
            external: false,
            driver_key: key.unwrap_or_default().to_owned(),
            print_file: Some(start.print_file.clone()),
            print_file_id: Some(start.print_file_id),
            plate: Some(start.plate),
            started_at: OffsetDateTime::now_utc(),
            ended_at: None,
            outcome: None,
            overruled: false,
        };
        jobs.push(record.clone());
        Ok(record)
    }

    async fn job_abandoned(&self, job: JobId) -> Result<(), JobLogError> {
        self.jobs
            .lock()
            .expect("job log lock")
            .retain(|j| j.id != job);
        Ok(())
    }
}
