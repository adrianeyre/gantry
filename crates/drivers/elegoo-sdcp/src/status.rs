//! SDCP v3 status messages (`sdcp/status/<MainboardID>`), mapped onto the
//! normalised status model (PRD §6.2). Pure functions, so every mapping is
//! unit-tested without a printer.
//!
//! Two fields decide the Printer State:
//!
//! - `CurrentStatus`, the machine's status codes (a list). The SDCP v3 spec
//!   defines 0 idle, 1 printing, 2 file transferring, 3 exposure testing and
//!   4 devices testing; the Centauri Carbon adds more (5 levelling, 6 input
//!   shaping, 7 stopping, 8 stopped, 9 homing, 10 loading or unloading,
//!   11 PID tuning, 12 recovery), as community clients read them.
//! - `PrintInfo.Status`, the print's own status. The spec's codes (0 to 10)
//!   are a resin printer's; the Centauri Carbon extends them (13 printing,
//!   16 preheating, 20 preheating completed and others). Gantry reads the
//!   FDM table the Home Assistant `elegoo_printer` integration uses, which
//!   says it mirrors Elegoo's own adapter for the Centauri Carbon.
//!
//! A print runs while `PrintInfo.Status` is neither idle (0), stopped (8)
//! nor complete (9), and the machine says it is printing (or stopping).
//! Pausing, resuming and stopping still count as Printing: a Printer is only
//! Paused once it will take a resume. The heating and levelling codes are
//! Preparing, but only until the first layer: the firmware is known to stay
//! on "preheating completed" through a whole print. Complete (9) is a
//! Succeeded Job and stopped (8) a Cancelled one, unless `ErrorNumber` says
//! the print failed. Outside a print, a machine code other than idle is Busy.
//! There is no error Printer State: errors are Faults (PRD §6.2).
use gantry_core::{
    ActiveJob, Capability, Connection, DisconnectReason, Fan, Fault, FaultSeverity, JobOutcome,
    Light, LightId, Position, PrinterState, PrinterStatus, Temperature, Temperatures,
};
use serde_json::{Map, Value};

/// The speed profiles, by the `PrintSpeedPct` the Centauri Carbon uses for
/// each (its Silent, Balanced, Sport and Ludicrous modes).
pub const SPEED_PROFILES: [(&str, u64); 4] = [
    ("silent", 50),
    ("standard", 100),
    ("sport", 130),
    ("ludicrous", 160),
];

/// The fans the Centauri Carbon reports in `CurrentFanSpeed`: SDCP's name,
/// Gantry's id and what the fan is for.
pub const FANS: [(&str, &str, &str); 3] = [
    ("ModelFan", "part", "Part cooling"),
    ("AuxiliaryFan", "auxiliary", "Auxiliary"),
    ("BoxFan", "chamber", "Chamber"),
];

/// Folds a status message (which may carry only what changed) into the
/// status known so far.
pub fn merge(known: &mut Map<String, Value>, update: &Map<String, Value>) {
    for (name, value) in update {
        match (known.get_mut(name), value) {
            (Some(Value::Object(old)), Value::Object(new)) => merge(old, new),
            _ => {
                known.insert(name.clone(), value.clone());
            }
        }
    }
}

fn print_info(status: &Value) -> &Value {
    &status["PrintInfo"]
}

fn number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|s| s.trim().parse().ok()))
}

/// The machine's status codes, when the message has them.
pub fn machine(status: &Value) -> Option<Vec<u64>> {
    match &status["CurrentStatus"] {
        Value::Array(codes) => Some(codes.iter().filter_map(Value::as_u64).collect()),
        Value::Number(n) => n.as_u64().map(|n| vec![n]),
        _ => None,
    }
}

/// `PrintInfo.Status`.
pub fn print_code(status: &Value) -> u64 {
    print_info(status)["Status"].as_u64().unwrap_or(0)
}

/// `PrintInfo.ErrorNumber`: 0 is no error.
pub fn error_number(status: &Value) -> u64 {
    print_info(status)["ErrorNumber"].as_u64().unwrap_or(0)
}

/// What the print is doing, by `PrintInfo.Status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Print {
    /// No print is running.
    None,
    /// A print is running, in this Printer State.
    Running(PrinterState),
    /// The print has ended, like this.
    Ended(JobOutcome),
}

/// The print codes that are a Job getting ready (homing, file and printer
/// checks, levelling, preheating, resonance tests and their "completed"
/// codes).
const PREPARING: [u64; 11] = [1, 10, 11, 15, 16, 17, 18, 19, 20, 21, 22];

/// The print codes that are a paused Job: paused (6), and paused for a
/// filament unload (26).
const PAUSED: [u64; 2] = [6, 26];

/// The print code the Centauri Carbon reports for an error.
const ERROR: u64 = 14;

/// The print code for a filament unload that went wrong.
const UNLOAD_ABNORMAL: u64 = 25;

pub fn print(status: &Value) -> Print {
    let code = print_code(status);
    let failed = error_number(status) != 0;
    match code {
        9 if failed => return Print::Ended(JobOutcome::Failed),
        9 => return Print::Ended(JobOutcome::Succeeded),
        8 if failed => return Print::Ended(JobOutcome::Failed),
        8 => return Print::Ended(JobOutcome::Cancelled),
        0 => return Print::None,
        _ => {}
    }
    // Without the machine's codes, the print's own status decides.
    let printing = machine(status).is_none_or(|codes| codes.iter().any(|c| *c == 1 || *c == 7));
    if !printing {
        return Print::None;
    }
    let started = layer(status).unwrap_or(0) > 0 || ticks(status).0.unwrap_or(0) > 0;
    Print::Running(if PAUSED.contains(&code) || code == ERROR {
        // A print stopped by an error isn't printing; its Fault says why.
        PrinterState::Paused
    } else if PREPARING.contains(&code) && !started {
        PrinterState::Preparing
    } else {
        PrinterState::Printing
    })
}

/// Whether the machine is doing something that isn't a Job.
pub fn busy(status: &Value) -> bool {
    machine(status).is_some_and(|codes| codes.iter().any(|c| !matches!(c, 0 | 1 | 7 | 8)))
}

/// The print's file, as the printer names it.
pub fn filename(status: &Value) -> Option<String> {
    print_info(status)["Filename"]
        .as_str()
        .map(str::trim)
        .filter(|f| !f.is_empty())
        .map(str::to_owned)
}

/// The printer's own id for the print, when it gives one.
pub fn task_id(status: &Value) -> Option<String> {
    print_info(status)["TaskId"]
        .as_str()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
}

fn layer(status: &Value) -> Option<u64> {
    print_info(status)["CurrentLayer"].as_u64()
}

/// The print's `CurrentTicks` and `TotalTicks`. The SDCP v3 spec gives
/// both in milliseconds; whether the Centauri Carbon keeps to that is
/// unverified against real hardware.
pub fn ticks(status: &Value) -> (Option<u64>, Option<u64>) {
    let info = print_info(status);
    (info["CurrentTicks"].as_u64(), info["TotalTicks"].as_u64())
}

/// Seconds the print has been running, from `CurrentTicks`.
pub fn elapsed_secs(status: &Value) -> u64 {
    ticks(status).0.unwrap_or(0) / 1000
}

/// The print Gantry is following on this Printer.
#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub key: String,
    pub task_id: Option<String>,
    pub file: Option<String>,
    pub started_at: time::OffsetDateTime,
    /// Whether the printer reported an error during it, so that its ending
    /// unseen is a Failed Job.
    pub errored: bool,
}

impl Track {
    /// Whether the running print in `status` is this one.
    pub fn is(&self, status: &Value) -> bool {
        match (task_id(status), &self.task_id) {
            (Some(running), Some(mine)) => &running == mine,
            _ => filename(status) == self.file,
        }
    }

    /// A new Track for the print running in `status`. Its key is the
    /// printer's `TaskId`, or else the file and when it started.
    pub fn new(status: &Value, now: time::OffsetDateTime) -> Self {
        let started_at = now - time::Duration::seconds(elapsed_secs(status) as i64);
        let task_id = task_id(status);
        let file = filename(status);
        let key = match &task_id {
            Some(task) => task.clone(),
            None => format!(
                "{}@{}",
                file.as_deref().unwrap_or("print"),
                started_at.unix_timestamp()
            ),
        };
        Self {
            key,
            task_id,
            file,
            started_at,
            errored: false,
        }
    }
}

fn temperature(status: &Value, current: &str, target: &str) -> Option<Temperature> {
    let reading = number(&status[current])?;
    Some(Temperature {
        current: (reading * 10.0).round() / 10.0,
        target: number(&status[target]).filter(|t| *t > 0.0),
    })
}

pub fn temperatures(status: &Value) -> Temperatures {
    Temperatures {
        nozzle: temperature(status, "TempOfNozzle", "TempTargetNozzle"),
        bed: temperature(status, "TempOfHotbed", "TempTargetHotbed"),
        chamber: temperature(status, "TempOfBox", "TempTargetBox"),
    }
}

pub fn fans(status: &Value) -> Vec<Fan> {
    let speeds = &status["CurrentFanSpeed"];
    FANS.iter()
        .filter_map(|(sdcp, id, name)| {
            let percent = number(&speeds[*sdcp])?;
            Some(Fan {
                id: (*id).into(),
                name: (*name).into(),
                percent: percent.clamp(0.0, 100.0).round() as u8,
                controllable: true,
            })
        })
        .collect()
}

/// The chamber light, from `LightStatus.SecondLight` (a bool or 0/1).
pub fn chamber_light(status: &Value) -> Option<bool> {
    match &status["LightStatus"]["SecondLight"] {
        Value::Bool(on) => Some(*on),
        Value::Number(n) => n.as_u64().map(|n| n != 0),
        _ => None,
    }
}

/// The speed profile, from `PrintInfo.PrintSpeedPct`.
pub fn speed_pct(status: &Value) -> Option<u64> {
    print_info(status)["PrintSpeedPct"].as_u64()
}

/// The toolhead's position, from `CurrentCoord` (`"x,y,z"`).
pub fn position(status: &Value) -> Option<Position> {
    let coords: Vec<f64> = status["CurrentCoord"]
        .as_str()?
        .split(',')
        .map(|c| c.trim().parse().ok())
        .collect::<Option<_>>()?;
    match coords[..] {
        [x, y, z] => Some(Position { x, y, z }),
        _ => None,
    }
}

/// What `ErrorNumber` means (SDCP v3).
pub fn error_message(number: u64) -> String {
    match number {
        1 => "The Print File failed its MD5 check".into(),
        2 => "The Print File couldn't be read".into(),
        3 => "The Print File is for another resolution".into(),
        4 => "The Print File's format isn't one the printer knows".into(),
        5 => "The Print File was sliced for another printer model".into(),
        n => format!("The printer reported error {n}"),
    }
}

/// The Faults the status shows.
pub fn faults(status: &Value) -> Vec<Fault> {
    let mut faults = Vec::new();
    let error = error_number(status);
    if error != 0 {
        faults.push(Fault {
            code: format!("sdcp-error-{error}"),
            severity: FaultSeverity::Error,
            message: error_message(error),
        });
    }
    match print_code(status) {
        ERROR => faults.push(Fault {
            code: "sdcp-print-error".into(),
            severity: FaultSeverity::Error,
            message: "The printer stopped the print with an error: see its screen".into(),
        }),
        UNLOAD_ABNORMAL => faults.push(Fault {
            code: "sdcp-unload-abnormal".into(),
            severity: FaultSeverity::Warning,
            message: "Unloading the filament went wrong: see the printer's screen".into(),
        }),
        _ => {}
    }
    faults
}

/// What the connection knows beyond the status.
#[derive(Debug, Clone, Copy, Default)]
pub struct Context<'a> {
    pub track: Option<&'a Track>,
    pub camera: bool,
}

/// The whole status, from the merged status message.
pub fn status(raw: &Value, context: Context<'_>) -> PrinterStatus {
    let mut status = PrinterStatus::disconnected(DisconnectReason::Stopped);
    status.connection = Connection::Connected;
    let print = print(raw);
    status.state = Some(match print {
        Print::Running(state) => state,
        Print::None | Print::Ended(_) if busy(raw) => PrinterState::Busy,
        Print::None | Print::Ended(_) => PrinterState::Idle,
    });
    if let (Print::Running(_), Some(track)) = (print, context.track) {
        let info = print_info(raw);
        let (current, total) = ticks(raw);
        let layer = layer(raw).and_then(|l| u32::try_from(l).ok());
        let total_layers = info["TotalLayer"]
            .as_u64()
            .filter(|t| *t > 0)
            .and_then(|t| u32::try_from(t).ok());
        let progress = number(&info["Progress"])
            .or_else(|| match (current, total) {
                (Some(c), Some(t)) if t > 0 => Some(c as f64 / t as f64 * 100.0),
                _ => None,
            })
            .or_else(|| match (layer, total_layers) {
                (Some(l), Some(t)) => Some(f64::from(l) / f64::from(t) * 100.0),
                _ => None,
            })
            .unwrap_or(0.0);
        status.job = Some(ActiveJob {
            key: track.key.clone(),
            print_file: track.file.clone(),
            plate: None,
            progress: (progress.clamp(0.0, 100.0) * 10.0).round() / 10.0,
            layer,
            total_layers,
            remaining_secs: match (current, total) {
                (Some(c), Some(t)) if t > 0 => Some(t.saturating_sub(c) / 1000),
                _ => None,
            },
            started_at: Some(track.started_at),
        });
    }
    status.temperatures = temperatures(raw);
    status.fans = fans(raw);
    status.faults = faults(raw);
    status.position = position(raw);
    if let Some(pct) = speed_pct(raw) {
        status.speed_profile = SPEED_PROFILES
            .iter()
            .find(|(_, p)| *p == pct)
            .map(|(name, _)| (*name).to_owned());
        status.speed_profiles = SPEED_PROFILES
            .iter()
            .map(|(name, _)| (*name).to_owned())
            .collect();
    }
    let light = chamber_light(raw);
    if let Some(on) = light {
        status.lights.push(Light {
            id: LightId::Chamber,
            on,
        });
    }
    let mut capabilities = vec![
        Capability::Pause,
        Capability::Resume,
        Capability::Cancel,
        Capability::UploadAndStart,
        Capability::NozzleTemperature,
        Capability::BedTemperature,
    ];
    if !status.fans.is_empty() {
        capabilities.push(Capability::Fans);
    }
    if light.is_some() {
        capabilities.push(Capability::ChamberLight);
    }
    if !status.speed_profiles.is_empty() {
        capabilities.push(Capability::SpeedProfile);
    }
    if context.camera {
        capabilities.push(Capability::Camera);
    }
    status.capabilities = capabilities.into_iter().collect();
    status
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn raw(machine: &[u64], print: u64, layer: u64, ticks: u64) -> Value {
        json!({
            "CurrentStatus": machine,
            "TempOfNozzle": 214.96, "TempTargetNozzle": 215,
            "TempOfHotbed": 60.04, "TempTargetHotbed": 60,
            "TempOfBox": 31.2, "TempTargetBox": 0,
            "CurrentFanSpeed": { "ModelFan": 100, "AuxiliaryFan": 0, "BoxFan": 40 },
            "LightStatus": { "SecondLight": true },
            "CurrentCoord": "120.50,98.00,3.20",
            "PrintInfo": {
                "Status": print, "CurrentLayer": layer, "TotalLayer": 100,
                "CurrentTicks": ticks, "TotalTicks": 600_000,
                "Filename": "cube.gcode", "TaskId": "task-1", "ErrorNumber": 0,
                "PrintSpeedPct": 130
            }
        })
    }

    fn track(r: &Value) -> Track {
        Track::new(
            r,
            time::OffsetDateTime::UNIX_EPOCH + time::Duration::hours(1),
        )
    }

    #[test]
    fn idle_with_temperatures_fans_light_speed_and_position() {
        let r = raw(&[0], 0, 0, 0);
        let s = status(&r, Context::default());
        assert_eq!(s.state, Some(PrinterState::Idle));
        assert!(s.job.is_none());
        assert_eq!(
            s.temperatures.nozzle,
            Some(Temperature {
                current: 215.0,
                target: Some(215.0)
            })
        );
        assert_eq!(s.temperatures.bed.unwrap().current, 60.0);
        assert_eq!(
            s.temperatures.chamber,
            Some(Temperature {
                current: 31.2,
                target: None
            })
        );
        let fans: Vec<_> = s.fans.iter().map(|f| (f.id.as_str(), f.percent)).collect();
        assert_eq!(fans, [("part", 100), ("auxiliary", 0), ("chamber", 40)]);
        assert_eq!(
            s.lights,
            [Light {
                id: LightId::Chamber,
                on: true
            }]
        );
        assert_eq!(s.speed_profile.as_deref(), Some("sport"));
        assert_eq!(s.position.unwrap().z, 3.2);
        for c in [
            Capability::Pause,
            Capability::Cancel,
            Capability::UploadAndStart,
            Capability::Fans,
            Capability::ChamberLight,
            Capability::SpeedProfile,
        ] {
            assert!(s.has(c), "{c:?}");
        }
        assert!(!s.has(Capability::Camera) && !s.has(Capability::ChamberTemperature));
        assert!(s.faults.is_empty());
    }

    #[test]
    fn heating_and_levelling_prepare_until_the_first_layer() {
        for code in [1, 16, 20, 15] {
            let r = raw(&[1], code, 0, 0);
            assert_eq!(print(&r), Print::Running(PrinterState::Preparing), "{code}");
        }
        // The firmware stays on "preheating completed" while it prints.
        assert_eq!(
            print(&raw(&[1], 20, 3, 60_000)),
            Print::Running(PrinterState::Printing)
        );
    }

    #[test]
    fn a_running_print_has_progress_layers_and_time_left() {
        let r = raw(&[1], 13, 25, 150_000);
        let t = track(&r);
        assert_eq!(t.key, "task-1");
        assert_eq!(
            t.started_at,
            time::OffsetDateTime::UNIX_EPOCH + time::Duration::hours(1)
                - time::Duration::seconds(150)
        );
        let s = status(
            &r,
            Context {
                track: Some(&t),
                camera: true,
            },
        );
        assert_eq!(s.state, Some(PrinterState::Printing));
        let job = s.job.unwrap();
        assert_eq!(job.progress, 25.0);
        assert_eq!((job.layer, job.total_layers), (Some(25), Some(100)));
        assert_eq!(job.remaining_secs, Some(450));
        assert_eq!(job.print_file.as_deref(), Some("cube.gcode"));
        assert!(s.capabilities.contains(&Capability::Camera));
        // The resin spec's exposing and lifting are printing too.
        assert_eq!(
            print(&raw(&[1], 3, 2, 1)),
            Print::Running(PrinterState::Printing)
        );
    }

    #[test]
    fn pausing_resuming_and_stopping_are_still_printing() {
        for code in [5, 12, 7] {
            assert_eq!(
                print(&raw(&[1], code, 10, 1000)),
                Print::Running(PrinterState::Printing),
                "{code}"
            );
        }
        assert_eq!(
            print(&raw(&[7], 7, 10, 1000)),
            Print::Running(PrinterState::Printing)
        );
        for code in PAUSED {
            assert_eq!(
                print(&raw(&[1], code, 10, 1000)),
                Print::Running(PrinterState::Paused)
            );
        }
    }

    #[test]
    fn complete_stopped_and_errors_end_the_job() {
        assert_eq!(
            print(&raw(&[0], 9, 100, 600_000)),
            Print::Ended(JobOutcome::Succeeded)
        );
        assert_eq!(
            print(&raw(&[0], 8, 10, 1000)),
            Print::Ended(JobOutcome::Cancelled)
        );
        let mut failed = raw(&[0], 8, 10, 1000);
        failed["PrintInfo"]["ErrorNumber"] = json!(2);
        assert_eq!(print(&failed), Print::Ended(JobOutcome::Failed));
        let s = status(&failed, Context::default());
        assert_eq!(s.state, Some(PrinterState::Idle), "no error state");
        assert_eq!(s.faults[0].code, "sdcp-error-2");
        assert_eq!(s.faults[0].severity, FaultSeverity::Error);
    }

    #[test]
    fn an_error_mid_print_is_a_fault_on_a_paused_job() {
        let r = raw(&[1], ERROR, 10, 1000);
        let s = status(
            &r,
            Context {
                track: Some(&track(&r)),
                camera: false,
            },
        );
        assert_eq!(s.state, Some(PrinterState::Paused));
        assert_eq!(s.faults[0].code, "sdcp-print-error");
        assert!(s.job.is_some());
    }

    #[test]
    fn other_machine_work_is_busy_and_a_stale_print_code_is_not_a_job() {
        // Homing (9) or loading filament (10) from the printer's screen.
        for code in [2, 9, 10] {
            assert_eq!(
                status(&raw(&[code], 0, 0, 0), Context::default()).state,
                Some(PrinterState::Busy),
                "{code}"
            );
        }
        // The machine is idle, whatever the print code says.
        assert_eq!(print(&raw(&[0], 13, 10, 1000)), Print::None);
        assert_eq!(
            status(&raw(&[0], 13, 10, 1000), Context::default()).state,
            Some(PrinterState::Idle)
        );
    }

    #[test]
    fn without_a_task_id_the_key_is_the_file_and_its_start() {
        let mut r = raw(&[1], 13, 1, 10_000);
        r["PrintInfo"]["TaskId"] = json!("");
        let t = track(&r);
        assert_eq!(t.key, format!("cube.gcode@{}", 3600 - 10));
        assert!(t.is(&r));
        r["PrintInfo"]["Filename"] = json!("other.gcode");
        assert!(!t.is(&r));
    }

    #[test]
    fn partial_updates_are_merged() {
        let mut known = raw(&[1], 13, 1, 1000).as_object().unwrap().clone();
        let update = json!({ "PrintInfo": { "CurrentLayer": 2 } });
        merge(&mut known, update.as_object().unwrap());
        let merged = Value::Object(known);
        assert_eq!(merged["PrintInfo"]["CurrentLayer"], 2);
        assert_eq!(merged["PrintInfo"]["TaskId"], "task-1");
    }
}
