//! RepRapFirmware's object model (from `rr_model`), mapped onto the
//! normalised status model (PRD §6.2). Pure functions, so every mapping is
//! unit-tested.
//!
//! `state.status`:
//!
//! - `idle` is Idle.
//! - `processing` (RRF 3's word for printing a file) and `printing` are
//!   Printing, Preparing until the first layer starts. Mid-change
//!   (`pausing`, `resuming`, `cancelling`) still counts as printing: a
//!   Printer is only Paused once it will take a resume. `paused` is Paused.
//! - `busy`, `changingTool`, `starting`, `updating`, `simulating` (a
//!   simulation is not a Job) and anything new are Busy, unless a print
//!   file is open, when they are Printing (a tool change mid-print, say).
//! - `halted` (an emergency stop, waiting for a reset) is Idle with an error
//!   Fault, and ends a running Job as Failed. `off` (the power supply off)
//!   is Idle with a warning Fault. There is no "error" Printer State.
//!
//! A heater in `fault` is an error Fault too.
use gantry_core::{
    ActiveJob, Capability, Connection, DisconnectReason, Fan, Fault, FaultSeverity, JobOutcome,
    PrinterState, PrinterStatus, Temperature, Temperatures,
};
use serde_json::Value;

/// Where a print file's name starts on the SD card.
pub const GCODES: &str = "0:/gcodes/";

/// What the machine is doing, by `state.status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activity {
    Idle,
    Busy,
    Printing,
    Paused,
    Halted,
    Off,
}

/// The open print file, as RepRapFirmware names it (`0:/gcodes/…`).
pub fn file(model: &Value) -> Option<String> {
    model["job"]["file"]["fileName"]
        .as_str()
        .filter(|f| !f.is_empty())
        .map(str::to_owned)
}

pub fn activity(model: &Value) -> Activity {
    let open = file(model).is_some();
    match model["state"]["status"].as_str().unwrap_or_default() {
        "idle" => Activity::Idle,
        "processing" | "printing" | "pausing" | "resuming" | "cancelling" => Activity::Printing,
        "paused" => Activity::Paused,
        "halted" => Activity::Halted,
        "off" => Activity::Off,
        _ if open => Activity::Printing,
        _ => Activity::Busy,
    }
}

/// Whether a Job is running.
pub fn running(model: &Value) -> bool {
    matches!(activity(model), Activity::Printing | Activity::Paused) && file(model).is_some()
}

/// How the print of `file` ended, from the job's `lastFile…` fields, when
/// they are about it.
pub fn outcome(model: &Value, file: &str) -> Option<JobOutcome> {
    let job = &model["job"];
    if job["lastFileName"].as_str() != Some(file) {
        return None;
    }
    Some(if job["lastFileCancelled"].as_bool() == Some(true) {
        JobOutcome::Cancelled
    } else if job["lastFileAborted"].as_bool() == Some(true) {
        JobOutcome::Failed
    } else {
        JobOutcome::Succeeded
    })
}

/// A print file's name as Gantry shows it: without `0:/gcodes/`.
pub fn display_name(file: &str) -> String {
    file.strip_prefix(GCODES).unwrap_or(file).to_owned()
}

/// The print Gantry is following on this Printer.
#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub key: String,
    /// RepRapFirmware's name for the file, `0:/gcodes/…`.
    pub file: String,
    pub started_at: time::OffsetDateTime,
}

impl Track {
    /// A new print, keyed by its file and when it started (RepRapFirmware
    /// gives a print no id of its own).
    pub fn new(file: String, model: &Value) -> Self {
        let duration = model["job"]["duration"].as_f64().unwrap_or(0.0);
        let started_at =
            time::OffsetDateTime::now_utc() - time::Duration::seconds(duration.round() as i64);
        Self {
            key: format!("{}@{}", display_name(&file), started_at.unix_timestamp()),
            file,
            started_at,
        }
    }
}

fn index(value: &Value) -> Option<usize> {
    value
        .as_i64()
        .filter(|i| *i >= 0)
        .and_then(|i| usize::try_from(i).ok())
}

fn reading(model: &Value, heater: Option<usize>) -> Option<Temperature> {
    let heater = &model["heat"]["heaters"][heater?];
    let current = heater["current"].as_f64()?;
    let target = match heater["state"].as_str() {
        Some("active") => heater["active"].as_f64(),
        Some("standby") => heater["standby"].as_f64(),
        _ => None,
    };
    Some(Temperature {
        current: (current * 10.0).round() / 10.0,
        target: target.filter(|t| *t > 0.0),
    })
}

/// The nozzle's heater: the first tool's first heater, else heater 1.
pub fn nozzle_heater(model: &Value) -> usize {
    index(&model["tools"][0]["heaters"][0]).unwrap_or(1)
}

pub fn temperatures(model: &Value) -> Temperatures {
    Temperatures {
        nozzle: reading(model, Some(nozzle_heater(model))),
        bed: reading(model, index(&model["heat"]["bedHeaters"][0])),
        chamber: reading(model, index(&model["heat"]["chamberHeaters"][0])),
    }
}

/// The fans RepRapFirmware has, by their index. A thermostatic fan (one a
/// heater runs) isn't Gantry's to set.
pub fn fans(model: &Value) -> Vec<Fan> {
    let Some(fans) = model["fans"].as_array() else {
        return Vec::new();
    };
    fans.iter()
        .enumerate()
        .filter(|(_, f)| f.is_object())
        .map(|(i, f)| {
            let value = f["actualValue"]
                .as_f64()
                .or_else(|| f["requestedValue"].as_f64())
                .unwrap_or(0.0);
            let thermostatic = f["thermostatic"]["heaters"]
                .as_array()
                .is_some_and(|h| !h.is_empty());
            let name = f["name"].as_str().filter(|n| !n.is_empty());
            Fan {
                id: i.to_string(),
                name: name.map_or_else(
                    || {
                        if i == 0 {
                            "part cooling".to_owned()
                        } else {
                            format!("fan {i}")
                        }
                    },
                    str::to_owned,
                ),
                percent: (value.clamp(0.0, 1.0) * 100.0).round() as u8,
                controllable: !thermostatic,
            }
        })
        .collect()
}

fn faults(model: &Value) -> Vec<Fault> {
    let mut faults = Vec::new();
    match activity(model) {
        Activity::Halted => faults.push(Fault {
            code: "halted".into(),
            severity: FaultSeverity::Error,
            message: "The printer has halted (emergency stop) and needs a reset".into(),
        }),
        Activity::Off => faults.push(Fault {
            code: "power_off".into(),
            severity: FaultSeverity::Warning,
            message: "The printer's power supply is off".into(),
        }),
        _ => {}
    }
    if let Some(heaters) = model["heat"]["heaters"].as_array() {
        for (i, heater) in heaters.iter().enumerate() {
            if heater["state"].as_str() == Some("fault") {
                faults.push(Fault {
                    code: format!("heater_{i}_fault"),
                    severity: FaultSeverity::Error,
                    message: format!("Heater {i} has a fault"),
                });
            }
        }
    }
    faults
}

/// The whole status, with the print Gantry is following, if any.
pub fn status(model: &Value, track: Option<&Track>) -> PrinterStatus {
    let mut out = PrinterStatus::disconnected(DisconnectReason::Stopped);
    out.connection = Connection::Connected;
    let job = &model["job"];
    let layer = job["layer"].as_u64().and_then(|l| u32::try_from(l).ok());
    let position = job["filePosition"].as_f64().unwrap_or(0.0);
    let started = layer.map_or(position > 0.0, |l| l > 0);
    out.state = Some(match activity(model) {
        Activity::Printing if !started => PrinterState::Preparing,
        Activity::Printing => PrinterState::Printing,
        Activity::Paused => PrinterState::Paused,
        Activity::Busy => PrinterState::Busy,
        Activity::Idle | Activity::Halted | Activity::Off => PrinterState::Idle,
    });
    if running(model)
        && let Some(track) = track
    {
        let size = job["file"]["size"].as_f64().unwrap_or(0.0);
        let progress = if size > 0.0 {
            (position / size * 100.0).clamp(0.0, 100.0)
        } else {
            0.0
        };
        let left = &job["timesLeft"];
        out.job = Some(ActiveJob {
            key: track.key.clone(),
            print_file: Some(display_name(&track.file)),
            plate: None,
            progress: (progress * 10.0).round() / 10.0,
            layer,
            total_layers: job["file"]["numLayers"]
                .as_u64()
                .filter(|n| *n > 0)
                .and_then(|n| u32::try_from(n).ok()),
            remaining_secs: ["slicer", "file", "filament"]
                .iter()
                .find_map(|k| left[k].as_f64())
                .filter(|s| *s >= 0.0)
                .map(|s| s.round() as u64),
            started_at: Some(track.started_at),
        });
    }
    out.temperatures = temperatures(model);
    out.fans = fans(model);
    out.faults = faults(model);
    let mut capabilities = vec![
        Capability::Pause,
        Capability::Resume,
        Capability::Cancel,
        Capability::UploadAndStart,
        Capability::NozzleTemperature,
        Capability::BedTemperature,
        Capability::Homing,
        Capability::Jogging,
    ];
    if out.fans.iter().any(|f| f.controllable) {
        capabilities.push(Capability::Fans);
    }
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

    fn model(status: &str, job: Value) -> Value {
        json!({
            "state": { "status": status },
            "job": job,
            "heat": {
                "bedHeaters": [0, -1],
                "chamberHeaters": [-1],
                "heaters": [
                    { "current": 59.96, "active": 60.0, "standby": 0.0, "state": "active" },
                    { "current": 209.94, "active": 210.0, "standby": 150.0, "state": "standby" }
                ]
            },
            "tools": [ { "number": 0, "heaters": [1] } ],
            "fans": [
                { "actualValue": 0.5, "requestedValue": 0.5, "name": "", "thermostatic": { "heaters": [] } },
                null,
                { "actualValue": 1.0, "requestedValue": 1.0, "name": "Hotend", "thermostatic": { "heaters": [1] } }
            ]
        })
    }

    fn printing_job() -> Value {
        json!({
            "file": { "fileName": "0:/gcodes/cube.gcode", "size": 2000, "numLayers": 50 },
            "filePosition": 500, "duration": 120, "layer": 12,
            "timesLeft": { "slicer": 360.4, "file": 400, "filament": null }
        })
    }

    fn track() -> Track {
        Track {
            key: "cube.gcode@1".into(),
            file: "0:/gcodes/cube.gcode".into(),
            started_at: time::OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn idle_with_temperatures_and_fans() {
        let m = model("idle", json!({ "file": { "fileName": null } }));
        let s = status(&m, None);
        assert_eq!(s.state, Some(PrinterState::Idle));
        assert_eq!(
            s.temperatures.bed,
            Some(Temperature {
                current: 60.0,
                target: Some(60.0)
            })
        );
        assert_eq!(
            s.temperatures.nozzle,
            Some(Temperature {
                current: 209.9,
                target: Some(150.0)
            }),
            "a tool on standby holds its standby temperature"
        );
        assert!(s.temperatures.chamber.is_none());
        assert_eq!(s.fans.len(), 2);
        assert_eq!(s.fans[0].percent, 50);
        assert_eq!(s.fans[0].name, "part cooling");
        assert!(s.fans[0].controllable);
        assert_eq!(s.fans[1].id, "2");
        assert!(!s.fans[1].controllable, "thermostatic");
        for c in [
            Capability::Pause,
            Capability::Cancel,
            Capability::UploadAndStart,
            Capability::NozzleTemperature,
            Capability::BedTemperature,
            Capability::Homing,
            Capability::Jogging,
            Capability::Fans,
        ] {
            assert!(s.has(c), "{c:?}");
        }
        assert!(s.job.is_none() && s.faults.is_empty());
    }

    #[test]
    fn a_print_prepares_then_prints_with_layers_and_time_left() {
        let mut job = printing_job();
        job["layer"] = json!(0);
        job["filePosition"] = json!(40);
        let heating = model("processing", job);
        assert_eq!(
            status(&heating, Some(&track())).state,
            Some(PrinterState::Preparing)
        );
        let m = model("processing", printing_job());
        assert!(running(&m));
        let s = status(&m, Some(&track()));
        assert_eq!(s.state, Some(PrinterState::Printing));
        let job = s.job.unwrap();
        assert_eq!(job.progress, 25.0);
        assert_eq!((job.layer, job.total_layers), (Some(12), Some(50)));
        assert_eq!(job.remaining_secs, Some(360));
        assert_eq!(job.print_file.as_deref(), Some("cube.gcode"));
    }

    #[test]
    fn changing_states_are_still_the_job() {
        for (rrf, state) in [
            ("printing", PrinterState::Printing),
            ("pausing", PrinterState::Printing),
            ("paused", PrinterState::Paused),
            ("resuming", PrinterState::Printing),
            ("cancelling", PrinterState::Printing),
            ("changingTool", PrinterState::Printing),
        ] {
            let m = model(rrf, printing_job());
            assert!(running(&m), "{rrf}");
            assert_eq!(status(&m, Some(&track())).state, Some(state), "{rrf}");
        }
    }

    #[test]
    fn other_work_is_busy() {
        for rrf in [
            "busy",
            "changingTool",
            "starting",
            "updating",
            "simulating",
            "new",
        ] {
            let m = model(rrf, json!({ "file": { "fileName": null } }));
            assert!(!running(&m));
            assert_eq!(status(&m, None).state, Some(PrinterState::Busy), "{rrf}");
        }
    }

    #[test]
    fn halted_off_and_heater_faults_are_faults_not_states() {
        let s = status(&model("halted", Value::Null), None);
        assert_eq!(s.state, Some(PrinterState::Idle));
        assert_eq!(s.faults[0].code, "halted");
        assert_eq!(s.faults[0].severity, FaultSeverity::Error);
        let s = status(&model("off", Value::Null), None);
        assert_eq!(s.faults[0].severity, FaultSeverity::Warning);
        let mut m = model("idle", Value::Null);
        m["heat"]["heaters"][1]["state"] = json!("fault");
        let s = status(&m, None);
        assert_eq!(s.faults[0].code, "heater_1_fault");
        assert!(s.temperatures.nozzle.unwrap().target.is_none());
    }

    #[test]
    fn the_last_file_says_how_a_print_ended() {
        let ended = |cancelled: bool, aborted: bool| {
            json!({ "job": {
                "file": { "fileName": null },
                "lastFileName": "0:/gcodes/cube.gcode",
                "lastFileCancelled": cancelled, "lastFileAborted": aborted
            }})
        };
        let file = "0:/gcodes/cube.gcode";
        assert_eq!(
            outcome(&ended(false, false), file),
            Some(JobOutcome::Succeeded)
        );
        assert_eq!(
            outcome(&ended(true, false), file),
            Some(JobOutcome::Cancelled)
        );
        assert_eq!(outcome(&ended(false, true), file), Some(JobOutcome::Failed));
        assert_eq!(outcome(&ended(false, false), "0:/gcodes/other.gcode"), None);
    }

    #[test]
    fn a_track_is_keyed_by_file_and_start() {
        let t = Track::new(
            "0:/gcodes/cube.gcode".into(),
            &json!({ "job": { "duration": 0 } }),
        );
        assert!(t.key.starts_with("cube.gcode@"), "{}", t.key);
        assert_eq!(display_name("0:/gcodes/sub/a.gcode"), "sub/a.gcode");
    }
}
