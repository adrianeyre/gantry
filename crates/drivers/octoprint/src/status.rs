//! OctoPrint's `current` push messages, mapped onto the normalised status
//! model (PRD §6.2). Pure functions, so every mapping is unit-tested.
//!
//! - OctoPrint's state flags: `printing` is Printing (Preparing until the
//!   print's clock starts) and `paused` is Paused. Mid-change (`pausing`,
//!   `resuming`, `cancelling`, `finishing`) still counts as printing: a
//!   Printer is only Paused once OctoPrint will take a resume. Otherwise
//!   `operational` is Idle.
//! - OctoPrint not connected to the printer at all (`Offline`, `Closed`,
//!   `Error`) is a lost Connection: Gantry can't talk to the printer, even
//!   though OctoPrint answers.
use gantry_core::{
    ActiveJob, Capability, Connection, DisconnectReason, PrinterState, PrinterStatus, Temperature,
    Temperatures,
};
use serde_json::Value;

fn flag(current: &Value, name: &str) -> bool {
    current["state"]["flags"][name].as_bool().unwrap_or(false)
}

/// What the printer is doing, by OctoPrint's flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activity {
    /// OctoPrint isn't connected to the printer.
    Offline,
    Idle,
    Printing,
    Paused,
}

pub fn activity(current: &Value) -> Activity {
    let changing = ["pausing", "resuming", "cancelling", "finishing"]
        .iter()
        .any(|f| flag(current, f));
    if flag(current, "paused") && !changing {
        Activity::Paused
    } else if flag(current, "printing") || flag(current, "paused") || changing {
        Activity::Printing
    } else if flag(current, "operational") {
        Activity::Idle
    } else {
        Activity::Offline
    }
}

/// The running print's file, as OctoPrint stores it (its path in `local`).
pub fn file(current: &Value) -> Option<String> {
    let file = &current["job"]["file"];
    file["path"]
        .as_str()
        .or_else(|| file["name"].as_str())
        .filter(|f| !f.is_empty())
        .map(str::to_owned)
}

/// Seconds the print has been running, when OctoPrint says.
pub fn print_time(current: &Value) -> Option<f64> {
    current["progress"]["printTime"].as_f64()
}

/// The print Gantry is following on this Printer.
#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub key: String,
    pub file: String,
    pub started_at: time::OffsetDateTime,
}

fn reading(value: &Value) -> Option<Temperature> {
    let current = value["actual"].as_f64()?;
    Some(Temperature {
        current: (current * 10.0).round() / 10.0,
        target: value["target"].as_f64().filter(|t| *t > 0.0),
    })
}

/// The newest temperatures, from a message's `temps` (oldest first).
pub fn temperatures(current: &Value) -> Option<Temperatures> {
    let latest = current["temps"].as_array()?.last()?;
    Some(Temperatures {
        nozzle: reading(&latest["tool0"]),
        bed: reading(&latest["bed"]),
        chamber: reading(&latest["chamber"]),
    })
}

/// The whole status from the latest `current` message. `temperatures` are
/// the last known ones (not every message carries them).
pub fn status(
    current: &Value,
    temperatures: Temperatures,
    track: Option<&Track>,
    camera: bool,
) -> PrinterStatus {
    let activity = activity(current);
    if activity == Activity::Offline {
        let text = current["state"]["text"].as_str().unwrap_or("Offline");
        let error = current["state"]["error"].as_str().filter(|e| !e.is_empty());
        return PrinterStatus {
            connection: Connection::Disconnected {
                reason: DisconnectReason::Unreachable,
                detail: Some(match error {
                    Some(e) => format!("OctoPrint isn't connected to the printer ({text}): {e}"),
                    None => format!("OctoPrint isn't connected to the printer ({text})"),
                }),
            },
            ..PrinterStatus::disconnected(DisconnectReason::Unreachable)
        };
    }
    let mut status = PrinterStatus::disconnected(DisconnectReason::Stopped);
    status.connection = Connection::Connected;
    let completion = current["progress"]["completion"].as_f64().unwrap_or(0.0);
    let started = print_time(current).unwrap_or(0.0) > 0.0 || completion > 0.0;
    status.state = Some(match activity {
        Activity::Printing if !started => PrinterState::Preparing,
        Activity::Printing => PrinterState::Printing,
        Activity::Paused => PrinterState::Paused,
        Activity::Idle | Activity::Offline => PrinterState::Idle,
    });
    if matches!(activity, Activity::Printing | Activity::Paused)
        && let Some(track) = track
    {
        status.job = Some(ActiveJob {
            key: track.key.clone(),
            print_file: Some(track.file.clone()),
            plate: None,
            progress: (completion.clamp(0.0, 100.0) * 10.0).round() / 10.0,
            layer: None,
            total_layers: None,
            remaining_secs: current["progress"]["printTimeLeft"]
                .as_f64()
                .filter(|s| *s >= 0.0)
                .map(|s| s.round() as u64),
            started_at: Some(track.started_at),
        });
    }
    status.temperatures = temperatures;
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
    if status.temperatures.chamber.is_some() {
        capabilities.push(Capability::ChamberTemperature);
    }
    if camera {
        capabilities.push(Capability::Camera);
    }
    status.capabilities = capabilities.into_iter().collect();
    status
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn current(flags: Value, progress: Value) -> Value {
        json!({
            "state": { "text": "Operational", "error": "", "flags": flags },
            "job": { "file": { "name": "cube.gcode", "path": "gantry/cube.gcode" } },
            "progress": progress,
            "temps": [
                { "time": 1, "tool0": { "actual": 20.0, "target": 0.0 } },
                { "time": 2, "tool0": { "actual": 210.04, "target": 210.0 },
                  "bed": { "actual": 59.96, "target": 60.0 } }
            ]
        })
    }

    fn track() -> Track {
        Track {
            key: "gantry/cube.gcode@1".into(),
            file: "gantry/cube.gcode".into(),
            started_at: time::OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn operational_is_idle_with_the_newest_temperatures() {
        let c = current(json!({ "operational": true, "ready": true }), json!({}));
        let t = temperatures(&c).unwrap();
        assert_eq!(
            t.nozzle,
            Some(Temperature {
                current: 210.0,
                target: Some(210.0)
            })
        );
        assert_eq!(t.bed.unwrap().current, 60.0);
        let s = status(&c, t, None, false);
        assert_eq!(s.state, Some(PrinterState::Idle));
        assert!(s.has(Capability::Pause) && !s.has(Capability::Camera));
        assert!(s.job.is_none());
    }

    #[test]
    fn a_print_prepares_then_prints_with_progress_and_time_left() {
        let starting = current(
            json!({ "operational": true, "printing": true }),
            json!({ "completion": 0.0, "printTime": 0, "printTimeLeft": null }),
        );
        assert_eq!(
            status(&starting, Temperatures::default(), Some(&track()), false).state,
            Some(PrinterState::Preparing)
        );
        let running = current(
            json!({ "operational": true, "printing": true }),
            json!({ "completion": 42.123, "printTime": 120, "printTimeLeft": 300 }),
        );
        let s = status(&running, Temperatures::default(), Some(&track()), true);
        assert_eq!(s.state, Some(PrinterState::Printing));
        let job = s.job.clone().unwrap();
        assert_eq!(job.progress, 42.1);
        assert_eq!(job.remaining_secs, Some(300));
        assert_eq!(job.print_file.as_deref(), Some("gantry/cube.gcode"));
        assert!(s.has(Capability::Camera));
        assert_eq!(file(&running).as_deref(), Some("gantry/cube.gcode"));
        assert_eq!(print_time(&running), Some(120.0));
    }

    #[test]
    fn pausing_and_cancelling_are_still_the_job() {
        let pausing = current(
            json!({ "operational": true, "pausing": true, "printing": true }),
            json!({}),
        );
        assert_eq!(
            activity(&pausing),
            Activity::Printing,
            "not Paused until it will resume"
        );
        let paused = current(json!({ "operational": true, "paused": true }), json!({}));
        assert_eq!(activity(&paused), Activity::Paused);
        let resuming = current(
            json!({ "operational": true, "paused": true, "resuming": true }),
            json!({}),
        );
        assert_eq!(activity(&resuming), Activity::Printing);
        let cancelling = current(
            json!({ "operational": true, "cancelling": true }),
            json!({}),
        );
        assert_eq!(activity(&cancelling), Activity::Printing);
    }

    #[test]
    fn octoprint_without_its_printer_is_a_lost_connection() {
        let offline = json!({
            "state": { "text": "Offline after error", "error": "No more candidates to test, and no working port/baudrate combination detected.",
                       "flags": { "operational": false, "closedOrError": true, "error": true } }
        });
        let s = status(&offline, Temperatures::default(), None, false);
        match s.connection {
            Connection::Disconnected { reason, detail } => {
                assert_eq!(reason, DisconnectReason::Unreachable);
                assert!(detail.unwrap().contains("Offline after error"));
            }
            other => panic!("{other:?}"),
        }
        assert!(s.state.is_none());
    }
}
