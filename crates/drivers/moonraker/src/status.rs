//! Klipper's printer objects, as Moonraker reports them, mapped onto the
//! normalised status model (PRD §6.2). Pure functions, so every mapping is
//! unit-tested without a printer.
//!
//! - `print_stats.state`: `standby` is Idle; `printing` is Printing, or
//!   Preparing until the print's clock starts (heating, homing, levelling);
//!   `paused` is Paused; `complete`, `cancelled` and `error` mean the Job has
//!   ended (Succeeded, Cancelled, Failed) and the Printer is Idle again.
//! - Outside a print, `idle_timeout.state` = `Printing` means Klipper is
//!   running G-code that isn't a Job (homing, a macro): Busy.
//! - `webhooks.state` other than `ready` (Klipper shut down or in error) is a
//!   Fault, and control is withdrawn until Klipper is restarted.
use serde_json::{Map, Value};

use gantry_core::{
    ActiveJob, Capability, Connection, Fan, Fault, FaultSeverity, JobOutcome, Position,
    PrinterState, PrinterStatus, Temperature, Temperatures,
};

/// Klipper's printer objects by name, as merged from Moonraker's updates.
pub type Objects = Map<String, Value>;

/// Folds an update (only what changed) into the objects.
pub fn merge(objects: &mut Objects, update: &Map<String, Value>) {
    for (name, value) in update {
        match (objects.get_mut(name), value) {
            (Some(Value::Object(old)), Value::Object(new)) => merge(old, new),
            _ => {
                objects.insert(name.clone(), value.clone());
            }
        }
    }
}

fn number(objects: &Objects, object: &str, field: &str) -> Option<f64> {
    objects.get(object)?.get(field)?.as_f64()
}

fn text<'a>(objects: &'a Objects, object: &str, field: &str) -> Option<&'a str> {
    objects.get(object)?.get(field)?.as_str()
}

/// What `print_stats.state` says about the print.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrintState {
    Standby,
    Printing,
    Paused,
    Ended(JobOutcome),
}

pub fn print_state(objects: &Objects) -> PrintState {
    match text(objects, "print_stats", "state") {
        Some("printing") => PrintState::Printing,
        Some("paused") => PrintState::Paused,
        Some("complete") => PrintState::Ended(JobOutcome::Succeeded),
        Some("cancelled") => PrintState::Ended(JobOutcome::Cancelled),
        Some("error") => PrintState::Ended(JobOutcome::Failed),
        _ => PrintState::Standby,
    }
}

/// The print's file name, while one is running.
pub fn filename(objects: &Objects) -> Option<String> {
    text(objects, "print_stats", "filename")
        .filter(|f| !f.is_empty())
        .map(str::to_owned)
}

/// Seconds since the print started, pauses included.
pub fn total_duration(objects: &Objects) -> f64 {
    number(objects, "print_stats", "total_duration").unwrap_or(0.0)
}

/// The Job Gantry is following on this Printer.
#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub key: String,
    pub filename: String,
    pub started_at: time::OffsetDateTime,
}

/// What the connection knows beyond the objects.
#[derive(Debug, Clone, Default)]
pub struct Context {
    /// Klipper's own state (`webhooks.state`) and message.
    pub klippy_ready: bool,
    pub klippy_message: Option<String>,
    pub camera: bool,
    pub track: Option<Track>,
}

/// Which heater or sensor reports the chamber, if any.
pub fn chamber_object(names: &[String]) -> Option<String> {
    names
        .iter()
        .find(|n| {
            let lower = n.to_ascii_lowercase();
            (lower.starts_with("temperature_sensor ") || lower.starts_with("heater_generic "))
                && lower.contains("chamber")
        })
        .cloned()
}

fn temperature(objects: &Objects, object: &str) -> Option<Temperature> {
    let current = number(objects, object, "temperature")?;
    Some(Temperature {
        current: (current * 10.0).round() / 10.0,
        target: number(objects, object, "target").filter(|t| *t > 0.0),
    })
}

/// The whole status, from the objects and what the connection knows.
pub fn status(objects: &Objects, context: &Context) -> PrinterStatus {
    let mut status = PrinterStatus::disconnected(gantry_core::DisconnectReason::Stopped);
    status.connection = Connection::Connected;
    let print = print_state(objects);
    let busy = text(objects, "idle_timeout", "state") == Some("Printing");
    let progress = number(objects, "virtual_sdcard", "progress")
        .or_else(|| number(objects, "display_status", "progress"))
        .unwrap_or(0.0)
        .clamp(0.0, 1.0);
    let print_duration = number(objects, "print_stats", "print_duration").unwrap_or(0.0);
    status.state = Some(match print {
        PrintState::Printing if print_duration <= 0.0 && progress <= 0.0 => PrinterState::Preparing,
        PrintState::Printing => PrinterState::Printing,
        PrintState::Paused => PrinterState::Paused,
        PrintState::Standby | PrintState::Ended(_) if busy => PrinterState::Busy,
        PrintState::Standby | PrintState::Ended(_) => PrinterState::Idle,
    });
    if matches!(print, PrintState::Printing | PrintState::Paused)
        && let Some(track) = &context.track
    {
        let info = objects.get("print_stats").and_then(|p| p.get("info"));
        let layer = |field: &str| {
            info.and_then(|i| i.get(field))
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok())
        };
        status.job = Some(ActiveJob {
            key: track.key.clone(),
            print_file: Some(track.filename.clone()),
            plate: None,
            progress: (progress * 1000.0).round() / 10.0,
            layer: layer("current_layer"),
            total_layers: layer("total_layer"),
            remaining_secs: (progress > 0.005 && print_duration > 0.0).then(|| {
                (print_duration / progress - print_duration)
                    .max(0.0)
                    .round() as u64
            }),
            started_at: Some(track.started_at),
        });
    }
    status.temperatures = Temperatures {
        nozzle: temperature(objects, "extruder"),
        bed: temperature(objects, "heater_bed"),
        chamber: objects
            .keys()
            .find(|k| chamber_object(std::slice::from_ref(*k)).is_some())
            .and_then(|k| temperature(objects, k)),
    };
    if let Some(speed) = number(objects, "fan", "speed") {
        status.fans.push(Fan {
            id: "part".into(),
            name: "Part cooling".into(),
            percent: (speed.clamp(0.0, 1.0) * 100.0).round() as u8,
            controllable: true,
        });
    }
    if let Some(Value::Array(p)) = objects.get("toolhead").and_then(|t| t.get("position"))
        && let (Some(x), Some(y), Some(z)) = (
            p.first().and_then(Value::as_f64),
            p.get(1).and_then(Value::as_f64),
            p.get(2).and_then(Value::as_f64),
        )
        && homed(objects).len() == 3
    {
        status.position = Some(Position { x, y, z });
    }
    // Filament runout sensors.
    for (name, value) in objects {
        if (name.starts_with("filament_switch_sensor ")
            || name.starts_with("filament_motion_sensor "))
            && value.get("enabled").and_then(Value::as_bool) != Some(false)
            && value.get("filament_detected").and_then(Value::as_bool) == Some(false)
        {
            let sensor = name.split_once(' ').map_or(name.as_str(), |(_, n)| n);
            status.faults.push(Fault {
                code: format!("runout:{sensor}"),
                severity: FaultSeverity::Warning,
                message: format!("No filament at the {sensor} sensor"),
            });
        }
    }
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
    if objects.contains_key("fan") {
        capabilities.push(Capability::Fans);
    }
    if status.temperatures.chamber.is_some() {
        capabilities.push(Capability::ChamberTemperature);
    }
    if context.camera {
        capabilities.push(Capability::Camera);
    }
    status.capabilities = capabilities.into_iter().collect();
    if !context.klippy_ready {
        let message = context
            .klippy_message
            .clone()
            .unwrap_or_else(|| "Klipper isn't ready".into());
        status.faults.push(Fault {
            code: "klipper".into(),
            severity: FaultSeverity::Error,
            message: format!("Klipper: {}", message.lines().next().unwrap_or("not ready")),
        });
        status.state = Some(PrinterState::Busy);
        status.job = None;
        status.withdraw_control("Klipper is shut down or in error: restart its firmware (FIRMWARE_RESTART) to control it again");
    }
    status
}

/// The axes Klipper has homed, such as `xyz`.
pub fn homed(objects: &Objects) -> String {
    text(objects, "toolhead", "homed_axes")
        .unwrap_or_default()
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn objects(value: Value) -> Objects {
        value.as_object().unwrap().clone()
    }

    fn ready() -> Context {
        Context {
            klippy_ready: true,
            ..Context::default()
        }
    }

    fn tracked() -> Context {
        Context {
            track: Some(Track {
                key: "moonraker-000002".into(),
                filename: "cube.gcode".into(),
                started_at: time::OffsetDateTime::UNIX_EPOCH,
            }),
            ..ready()
        }
    }

    #[test]
    fn standby_is_idle_with_temperatures_and_fans() {
        let o = objects(json!({
            "print_stats": { "state": "standby", "filename": "", "print_duration": 0.0 },
            "extruder": { "temperature": 24.56, "target": 0.0 },
            "heater_bed": { "temperature": 60.04, "target": 60.0 },
            "temperature_sensor chamber": { "temperature": 31.0 },
            "fan": { "speed": 0.5 },
            "idle_timeout": { "state": "Idle" },
            "toolhead": { "homed_axes": "", "position": [0.0, 0.0, 0.0, 0.0] }
        }));
        let s = status(&o, &ready());
        assert_eq!(s.state, Some(PrinterState::Idle));
        assert!(s.job.is_none());
        assert_eq!(
            s.temperatures.nozzle,
            Some(Temperature {
                current: 24.6,
                target: None
            })
        );
        assert_eq!(s.temperatures.bed.unwrap().target, Some(60.0));
        assert_eq!(s.temperatures.chamber.unwrap().current, 31.0);
        assert_eq!(s.fans[0].percent, 50);
        assert!(s.has(Capability::Fans) && s.has(Capability::ChamberTemperature));
        assert!(!s.has(Capability::Camera));
        assert!(s.position.is_none(), "not homed");
    }

    #[test]
    fn a_print_prepares_then_prints_with_progress_layers_and_time_left() {
        let mut o = objects(json!({
            "print_stats": { "state": "printing", "filename": "cube.gcode", "print_duration": 0.0,
                             "info": { "current_layer": null, "total_layer": null } },
            "virtual_sdcard": { "progress": 0.0 },
            "idle_timeout": { "state": "Printing" }
        }));
        assert_eq!(status(&o, &tracked()).state, Some(PrinterState::Preparing));
        merge(
            &mut o,
            &objects(json!({
                "print_stats": { "print_duration": 300.0, "info": { "current_layer": 12, "total_layer": 40 } },
                "virtual_sdcard": { "progress": 0.25 }
            })),
        );
        let s = status(&o, &tracked());
        assert_eq!(s.state, Some(PrinterState::Printing));
        let job = s.job.unwrap();
        assert_eq!(job.key, "moonraker-000002");
        assert_eq!(job.progress, 25.0);
        assert_eq!(job.layer, Some(12));
        assert_eq!(job.total_layers, Some(40));
        assert_eq!(job.remaining_secs, Some(900));
        // The merge kept what the update didn't mention.
        assert_eq!(o["print_stats"]["filename"], "cube.gcode");
    }

    #[test]
    fn endings_map_to_outcomes() {
        for (state, outcome) in [
            ("complete", JobOutcome::Succeeded),
            ("cancelled", JobOutcome::Cancelled),
            ("error", JobOutcome::Failed),
        ] {
            let o = objects(json!({ "print_stats": { "state": state } }));
            assert_eq!(print_state(&o), PrintState::Ended(outcome));
            assert_eq!(status(&o, &tracked()).state, Some(PrinterState::Idle));
            assert!(status(&o, &tracked()).job.is_none());
        }
        let paused =
            objects(json!({ "print_stats": { "state": "paused", "print_duration": 9.0 } }));
        assert_eq!(
            status(&paused, &tracked()).state,
            Some(PrinterState::Paused)
        );
    }

    #[test]
    fn running_gcode_outside_a_print_is_busy() {
        let o = objects(json!({
            "print_stats": { "state": "standby" },
            "idle_timeout": { "state": "Printing" }
        }));
        assert_eq!(status(&o, &ready()).state, Some(PrinterState::Busy));
    }

    #[test]
    fn klipper_shut_down_is_a_fault_with_control_withdrawn() {
        let o = objects(json!({ "print_stats": { "state": "printing" } }));
        let context = Context {
            klippy_ready: false,
            klippy_message: Some("MCU 'mcu' shutdown: Timer too close\nmore".into()),
            ..tracked()
        };
        let s = status(&o, &context);
        assert!(s.connection.is_connected());
        assert_eq!(s.faults[0].severity, FaultSeverity::Error);
        assert_eq!(
            s.faults[0].message,
            "Klipper: MCU 'mcu' shutdown: Timer too close"
        );
        assert!(!s.has(Capability::Pause));
        assert!(s.control_withdrawn.is_some());
    }

    #[test]
    fn a_runout_sensor_without_filament_is_a_warning() {
        let o = objects(json!({
            "print_stats": { "state": "standby" },
            "filament_switch_sensor runout": { "filament_detected": false, "enabled": true },
            "filament_motion_sensor other": { "filament_detected": true, "enabled": true }
        }));
        let s = status(&o, &ready());
        assert_eq!(s.faults.len(), 1);
        assert_eq!(s.faults[0].code, "runout:runout");
    }

    #[test]
    fn a_homed_toolhead_has_a_position() {
        let o = objects(json!({
            "print_stats": { "state": "standby" },
            "toolhead": { "homed_axes": "xyz", "position": [10.0, 20.0, 5.0, 0.0] }
        }));
        assert_eq!(
            status(&o, &ready()).position,
            Some(Position {
                x: 10.0,
                y: 20.0,
                z: 5.0
            })
        );
        assert_eq!(
            chamber_object(&["heater_generic Chamber".into()]).as_deref(),
            Some("heater_generic Chamber")
        );
    }
}
