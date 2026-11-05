//! PrusaLink's `/api/v1/status`, mapped onto the normalised status model
//! (PRD §6.2). Pure functions, so every mapping is unit-tested.
//!
//! PrusaLink's printer states:
//!
//! - `IDLE` and `READY` (the printer set ready for the next print) are Idle.
//! - `BUSY` is Busy: the printer is doing something that isn't a Job.
//! - `PRINTING` is Printing (Preparing until the print's clock starts), and
//!   `PAUSED` is Paused.
//! - `FINISHED` and `STOPPED` are Idle: the Job has ended (Succeeded or
//!   Cancelled), and Gantry, not the Driver, holds the Printer in Awaiting
//!   Clear.
//! - `ERROR` is Idle with an error Fault (there is no "error" Printer State),
//!   and ends a running Job as Failed.
//! - `ATTENTION` (the printer waits for someone at its screen, such as after
//!   a filament runout) is a warning Fault. A Job it interrupts is Paused.
use gantry_core::{
    ActiveJob, Capability, Connection, DisconnectReason, Fault, FaultSeverity, JobOutcome,
    PrinterState, PrinterStatus, Temperature, Temperatures,
};
use serde_json::Value;

/// What a status's `printer.state` says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reported {
    Idle,
    Ready,
    Busy,
    Printing,
    Paused,
    Finished,
    Stopped,
    Error,
    Attention,
    /// A state this Driver doesn't know: taken as Busy.
    Unknown,
}

pub fn reported(status: &Value) -> Reported {
    match status["printer"]["state"]
        .as_str()
        .unwrap_or_default()
        .to_ascii_uppercase()
        .as_str()
    {
        "IDLE" => Reported::Idle,
        "READY" => Reported::Ready,
        "BUSY" => Reported::Busy,
        "PRINTING" => Reported::Printing,
        "PAUSED" => Reported::Paused,
        "FINISHED" => Reported::Finished,
        "STOPPED" => Reported::Stopped,
        "ERROR" => Reported::Error,
        "ATTENTION" => Reported::Attention,
        _ => Reported::Unknown,
    }
}

/// Where a print is, by the printer's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// A Job is running (in any phase).
    Running,
    /// The Job has ended, and how.
    Ended(JobOutcome),
    /// Nothing says a Job is running or how one ended.
    None,
}

/// The phase, from the printer's state and whether the status names a job.
pub fn phase(status: &Value) -> Phase {
    match reported(status) {
        Reported::Printing | Reported::Paused => Phase::Running,
        Reported::Attention if job_id(status).is_some() => Phase::Running,
        Reported::Finished => Phase::Ended(JobOutcome::Succeeded),
        Reported::Stopped => Phase::Ended(JobOutcome::Cancelled),
        Reported::Error => Phase::Ended(JobOutcome::Failed),
        _ => Phase::None,
    }
}

/// How a job PrusaLink describes at `/api/v1/job` ended, by its `state`.
pub fn job_outcome(job: &Value) -> Option<JobOutcome> {
    match job["state"].as_str()?.to_ascii_uppercase().as_str() {
        "FINISHED" => Some(JobOutcome::Succeeded),
        "STOPPED" => Some(JobOutcome::Cancelled),
        "ERROR" => Some(JobOutcome::Failed),
        _ => None,
    }
}

/// The running job's id, which PrusaLink uses in its job commands.
pub fn job_id(status: &Value) -> Option<u64> {
    status["job"]["id"].as_u64()
}

/// The Print File's name at `/api/v1/job`: its long name, which is the one
/// it was uploaded as, before the short name some storage gives it.
pub fn job_file(job: &Value) -> Option<String> {
    let file = &job["file"];
    file["display_name"]
        .as_str()
        .or_else(|| file["name"].as_str())
        .filter(|f| !f.is_empty())
        .map(str::to_owned)
}

/// The print Gantry is following on this Printer.
#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub key: String,
    /// PrusaLink's job id.
    pub id: u64,
    pub file: Option<String>,
    pub started_at: time::OffsetDateTime,
}

impl Track {
    /// A new print: its key is PrusaLink's job id with the Print File's
    /// name, which stays the same for as long as the print runs, and after
    /// Gantry restarts.
    pub fn new(id: u64, file: Option<String>, status: &Value) -> Self {
        let printing = status["job"]["time_printing"].as_f64().unwrap_or(0.0);
        let started_at =
            time::OffsetDateTime::now_utc() - time::Duration::seconds(printing.round() as i64);
        let key = match &file {
            Some(file) => format!("prusalink-{id}@{file}"),
            None => format!("prusalink-{id}"),
        };
        Self {
            key,
            id,
            file,
            started_at,
        }
    }
}

fn reading(printer: &Value, current: &str, target: &str) -> Option<Temperature> {
    let now = printer[current].as_f64()?;
    Some(Temperature {
        current: (now * 10.0).round() / 10.0,
        target: printer[target].as_f64().filter(|t| *t > 0.0),
    })
}

pub fn temperatures(status: &Value) -> Temperatures {
    let printer = &status["printer"];
    Temperatures {
        nozzle: reading(printer, "temp_nozzle", "target_nozzle"),
        bed: reading(printer, "temp_bed", "target_bed"),
        chamber: reading(printer, "temp_chamber", "target_chamber"),
    }
}

fn faults(status: &Value) -> Vec<Fault> {
    let said = status["printer"]["status_printer"]["message"]
        .as_str()
        .map(str::trim)
        .filter(|m| !m.is_empty());
    match reported(status) {
        Reported::Error => vec![Fault {
            code: "prusalink_error".into(),
            severity: FaultSeverity::Error,
            message: said.map_or_else(
                || "The printer reports an error: see its screen".to_owned(),
                str::to_owned,
            ),
        }],
        Reported::Attention => vec![Fault {
            code: "prusalink_attention".into(),
            severity: FaultSeverity::Warning,
            message: said.map_or_else(
                || "The printer needs attention at its screen".to_owned(),
                str::to_owned,
            ),
        }],
        _ => Vec::new(),
    }
}

/// The whole status, with the print Gantry is following, if any.
pub fn status(status: &Value, track: Option<&Track>) -> PrinterStatus {
    let mut out = PrinterStatus::disconnected(DisconnectReason::Stopped);
    out.connection = Connection::Connected;
    let job = &status["job"];
    let progress = job["progress"].as_f64().unwrap_or(0.0);
    let printing = job["time_printing"].as_f64().unwrap_or(0.0);
    let running = phase(status) == Phase::Running;
    out.state = Some(match reported(status) {
        Reported::Printing if progress <= 0.0 && printing <= 0.0 => PrinterState::Preparing,
        Reported::Printing => PrinterState::Printing,
        Reported::Paused => PrinterState::Paused,
        Reported::Attention if running => PrinterState::Paused,
        Reported::Busy | Reported::Unknown => PrinterState::Busy,
        Reported::Idle
        | Reported::Ready
        | Reported::Finished
        | Reported::Stopped
        | Reported::Error
        | Reported::Attention => PrinterState::Idle,
    });
    if running && let Some(track) = track {
        out.job = Some(ActiveJob {
            key: track.key.clone(),
            print_file: track.file.clone(),
            plate: None,
            progress: (progress.clamp(0.0, 100.0) * 10.0).round() / 10.0,
            layer: None,
            total_layers: None,
            remaining_secs: job["time_remaining"]
                .as_f64()
                .filter(|s| *s >= 0.0)
                .map(|s| s.round() as u64),
            started_at: Some(track.started_at),
        });
    }
    out.temperatures = temperatures(status);
    out.faults = faults(status);
    let mut capabilities = vec![
        Capability::Pause,
        Capability::Resume,
        Capability::Cancel,
        Capability::UploadAndStart,
    ];
    if out.temperatures.chamber.is_some() {
        capabilities.push(Capability::ChamberTemperature);
    }
    out.capabilities = capabilities.into_iter().collect();
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn printer(state: &str, job: Value) -> Value {
        json!({
            "job": job,
            "storage": { "path": "/usb/", "name": "usb", "read_only": false },
            "printer": {
                "state": state,
                "temp_nozzle": 214.96, "target_nozzle": 215.0,
                "temp_bed": 59.9, "target_bed": 60.0,
                "axis_z": 1.2, "flow": 100, "speed": 100,
                "fan_hotend": 4800, "fan_print": 3200
            }
        })
    }

    fn track() -> Track {
        Track {
            key: "prusalink-7@cube.gcode".into(),
            id: 7,
            file: Some("cube.gcode".into()),
            started_at: time::OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn idle_and_ready_are_idle_with_temperatures() {
        for state in ["IDLE", "READY"] {
            let s = status(&printer(state, Value::Null), None);
            assert_eq!(s.state, Some(PrinterState::Idle), "{state}");
            assert!(s.connection.is_connected());
            assert_eq!(
                s.temperatures.nozzle,
                Some(Temperature {
                    current: 215.0,
                    target: Some(215.0)
                })
            );
            assert_eq!(s.temperatures.bed.unwrap().current, 59.9);
            assert!(s.temperatures.chamber.is_none());
            assert!(s.faults.is_empty() && s.job.is_none());
            for c in [
                Capability::Pause,
                Capability::Resume,
                Capability::Cancel,
                Capability::UploadAndStart,
            ] {
                assert!(s.has(c), "{c:?}");
            }
            assert!(!s.has(Capability::Camera) && !s.has(Capability::NozzleTemperature));
        }
    }

    #[test]
    fn a_print_prepares_then_prints_with_progress_and_time_left() {
        let starting = printer(
            "PRINTING",
            json!({ "id": 7, "progress": 0, "time_printing": 0, "time_remaining": 600 }),
        );
        assert_eq!(phase(&starting), Phase::Running);
        assert_eq!(
            status(&starting, Some(&track())).state,
            Some(PrinterState::Preparing)
        );
        let running = printer(
            "PRINTING",
            json!({ "id": 7, "progress": 42.17, "time_printing": 120, "time_remaining": 480 }),
        );
        let s = status(&running, Some(&track()));
        assert_eq!(s.state, Some(PrinterState::Printing));
        let job = s.job.unwrap();
        assert_eq!(job.key, "prusalink-7@cube.gcode");
        assert_eq!(job.progress, 42.2);
        assert_eq!(job.remaining_secs, Some(480));
        assert_eq!(job.print_file.as_deref(), Some("cube.gcode"));
        assert_eq!(job_id(&running), Some(7));
        let paused = printer("PAUSED", json!({ "id": 7, "progress": 50 }));
        assert_eq!(
            status(&paused, Some(&track())).state,
            Some(PrinterState::Paused)
        );
    }

    #[test]
    fn busy_and_unknown_states_are_busy() {
        assert_eq!(
            status(&printer("BUSY", Value::Null), None).state,
            Some(PrinterState::Busy)
        );
        assert_eq!(
            status(&printer("SOMETHING_NEW", Value::Null), None).state,
            Some(PrinterState::Busy)
        );
    }

    #[test]
    fn finished_stopped_and_error_end_the_job() {
        for (state, outcome) in [
            ("FINISHED", JobOutcome::Succeeded),
            ("STOPPED", JobOutcome::Cancelled),
            ("ERROR", JobOutcome::Failed),
        ] {
            let s = printer(state, json!({ "id": 7, "progress": 100 }));
            assert_eq!(phase(&s), Phase::Ended(outcome), "{state}");
            let mapped = status(&s, Some(&track()));
            assert_eq!(mapped.state, Some(PrinterState::Idle), "{state}");
            assert!(mapped.job.is_none());
            assert_eq!(job_outcome(&json!({ "state": state })), Some(outcome));
        }
        assert_eq!(job_outcome(&json!({ "state": "PRINTING" })), None);
    }

    #[test]
    fn error_and_attention_are_faults_not_states() {
        let mut error = printer("ERROR", Value::Null);
        error["printer"]["status_printer"] = json!({ "ok": false, "message": "Thermal runaway" });
        let s = status(&error, None);
        assert_eq!(s.state, Some(PrinterState::Idle));
        assert_eq!(s.faults.len(), 1);
        assert_eq!(s.faults[0].severity, FaultSeverity::Error);
        assert_eq!(s.faults[0].message, "Thermal runaway");

        let attention = printer("ATTENTION", json!({ "id": 7, "progress": 30 }));
        assert_eq!(phase(&attention), Phase::Running);
        let s = status(&attention, Some(&track()));
        assert_eq!(s.state, Some(PrinterState::Paused));
        assert_eq!(s.faults[0].severity, FaultSeverity::Warning);
        assert!(s.job.is_some());

        let idle_attention = printer("ATTENTION", Value::Null);
        assert_eq!(phase(&idle_attention), Phase::None);
        assert_eq!(
            status(&idle_attention, None).state,
            Some(PrinterState::Idle)
        );
    }

    #[test]
    fn the_long_file_name_is_preferred() {
        let job = json!({ "file": { "name": "CUBE~1.GCO", "display_name": "cube.gcode" } });
        assert_eq!(job_file(&job).as_deref(), Some("cube.gcode"));
        let short = json!({ "file": { "name": "cube.gcode" } });
        assert_eq!(job_file(&short).as_deref(), Some("cube.gcode"));
        let key = Track::new(9, Some("cube.gcode".into()), &json!({})).key;
        assert_eq!(key, "prusalink-9@cube.gcode");
    }
}
