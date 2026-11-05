//! Turning a printer's `report` payloads into the normalised status model.
//!
//! Printers send a full report now and then and, on some models, partial
//! updates holding only what changed in between. [`Report`] keeps the merged
//! whole, and everything here reads from that. Field meanings are recorded in
//! `crates/test-support/mock-bambu/fixtures/README.md`.
use std::collections::BTreeSet;

use gantry_core::{
    ActiveJob, Capability, Connection, Fan, Light, LightId, PrinterState, PrinterStatus,
    Temperature, Temperatures,
};
use serde_json::Value;

/// A fan a Bambu printer reports, and how it is set.
pub struct BambuFan {
    pub id: &'static str,
    pub name: &'static str,
    /// The `report` field holding its speed, 0 to 15.
    pub report_key: &'static str,
    /// The `P` index `M106` sets it with, if Gantry may set it.
    pub gcode_index: Option<u8>,
}

/// Every fan, in the order the UI lists them.
pub const FANS: [BambuFan; 4] = [
    BambuFan {
        id: "part",
        name: "Part cooling",
        report_key: "cooling_fan_speed",
        gcode_index: Some(1),
    },
    BambuFan {
        id: "auxiliary",
        name: "Auxiliary",
        report_key: "big_fan1_speed",
        gcode_index: Some(2),
    },
    BambuFan {
        id: "chamber",
        name: "Chamber",
        report_key: "big_fan2_speed",
        gcode_index: Some(3),
    },
    BambuFan {
        id: "hotend",
        name: "Hotend",
        report_key: "heatbreak_fan_speed",
        gcode_index: None,
    },
];

/// The printer's state as reported so far.
#[derive(Debug, Clone, Default)]
pub struct Report {
    print: serde_json::Map<String, Value>,
    received: bool,
}

/// What a printer's `gcode_state` says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GcodeState {
    Idle,
    Preparing,
    Running,
    Paused,
    Finished,
    Failed,
    Other,
}

impl GcodeState {
    fn parse(value: &str) -> Self {
        match value {
            "IDLE" | "" => Self::Idle,
            "PREPARE" | "SLICING" | "INIT" => Self::Preparing,
            "RUNNING" => Self::Running,
            "PAUSE" => Self::Paused,
            "FINISH" => Self::Finished,
            "FAILED" => Self::Failed,
            _ => Self::Other,
        }
    }

    /// Whether a Job is under way.
    pub fn active(self) -> bool {
        matches!(self, Self::Preparing | Self::Running | Self::Paused)
    }
}

impl Report {
    /// Folds in one payload. Returns `false` for payloads that aren't status
    /// reports (answers to commands, for example), which leave it unchanged.
    pub fn apply(&mut self, payload: &Value) -> bool {
        let Some(print) = payload.get("print").and_then(Value::as_object) else {
            return false;
        };
        if print.get("command").and_then(Value::as_str) != Some("push_status") {
            return false;
        }
        merge(&mut self.print, print);
        self.received = true;
        true
    }

    /// Whether any status report has arrived.
    pub fn received(&self) -> bool {
        self.received
    }

    fn number(&self, key: &str) -> Option<f64> {
        match self.print.get(key)? {
            Value::Number(n) => n.as_f64(),
            Value::String(s) => s.trim().parse().ok(),
            _ => None,
        }
    }

    fn text(&self, key: &str) -> Option<&str> {
        self.print
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }

    /// Whether any report so far has said what the printer is doing.
    pub fn knows_gcode_state(&self) -> bool {
        self.print.contains_key("gcode_state")
    }

    pub fn gcode_state(&self) -> GcodeState {
        GcodeState::parse(self.text("gcode_state").unwrap_or(""))
    }

    /// `print_error`, the printer's code for why a print ended (0 for none).
    pub fn print_error(&self) -> u64 {
        self.number("print_error").map_or(0, |n| n as u64)
    }

    /// The Printer State, before Gantry's own Awaiting Clear.
    pub fn printer_state(&self) -> PrinterState {
        match self.gcode_state() {
            GcodeState::Preparing => PrinterState::Preparing,
            GcodeState::Running => PrinterState::Printing,
            GcodeState::Paused => PrinterState::Paused,
            // A finished or failed print leaves the printer idle; Gantry puts
            // it in Awaiting Clear.
            GcodeState::Idle | GcodeState::Finished | GcodeState::Failed => PrinterState::Idle,
            GcodeState::Other => PrinterState::Busy,
        }
    }

    /// The printer's identity for the running print, stable for its whole
    /// life. Built from the task ids it reports, which change per print.
    pub fn job_key(&self) -> String {
        // Prints from the SD card or LAN have no task id; their start time
        // still tells one run of the same file from the next.
        let id = self
            .text("task_id")
            .filter(|t| *t != "0")
            .or_else(|| self.text("subtask_id").filter(|t| *t != "0"))
            .or_else(|| self.text("gcode_start_time").filter(|t| *t != "0"))
            .unwrap_or("unknown");
        let name = self
            .text("subtask_name")
            .or_else(|| self.text("gcode_file"))
            .unwrap_or("");
        format!("{id}:{name}")
    }

    /// The Print File's name, as the printer knows it.
    pub fn print_file(&self) -> Option<String> {
        self.text("subtask_name").map(str::to_owned).or_else(|| {
            self.text("gcode_file")
                .and_then(|f| f.rsplit('/').next())
                .map(str::to_owned)
        })
    }

    /// The Plate, read from `gcode_file` (`…/plate_2.gcode`).
    pub fn plate(&self) -> Option<u32> {
        let file = self.text("gcode_file")?;
        let name = file.rsplit('/').next()?;
        name.strip_prefix("plate_")?.split('.').next()?.parse().ok()
    }

    pub fn job(&self) -> Option<ActiveJob> {
        if !self.gcode_state().active() {
            return None;
        }
        let layer = self.number("layer_num").map(|n| n as u32);
        let total = self
            .number("total_layer_num")
            .map(|n| n as u32)
            .filter(|n| *n > 0);
        Some(ActiveJob {
            key: self.job_key(),
            print_file: self.print_file(),
            plate: self.plate(),
            progress: self.number("mc_percent").unwrap_or(0.0).clamp(0.0, 100.0),
            layer: layer.filter(|_| total.is_some()),
            total_layers: total,
            remaining_secs: self.number("mc_remaining_time").map(|m| (m * 60.0) as u64),
            // Unix seconds; 0 (or absent) when the printer doesn't say.
            started_at: self
                .number("gcode_start_time")
                .filter(|t| *t > 0.0)
                .and_then(|t| time::OffsetDateTime::from_unix_timestamp(t as i64).ok()),
        })
    }

    pub fn temperatures(&self) -> Temperatures {
        let reading = |current: &str, target: Option<&str>| {
            self.number(current).map(|current| Temperature {
                current,
                target: target.and_then(|t| self.number(t)).filter(|t| *t > 0.0),
            })
        };
        Temperatures {
            nozzle: reading("nozzle_temper", Some("nozzle_target_temper")),
            bed: reading("bed_temper", Some("bed_target_temper")),
            chamber: reading("chamber_temper", None),
        }
    }

    pub fn fans(&self) -> Vec<Fan> {
        FANS.iter()
            .filter_map(|fan| {
                // 0 to 15, 15 being full speed.
                let level = self.number(fan.report_key)?.clamp(0.0, 15.0);
                Some(Fan {
                    id: fan.id.to_owned(),
                    name: fan.name.to_owned(),
                    percent: (level * 100.0 / 15.0).round() as u8,
                    controllable: fan.gcode_index.is_some(),
                })
            })
            .collect()
    }

    /// What this printer can do, judged from what it reports. Models differ
    /// (an X1 has a work light and a chamber fan, an A1 neither), so each
    /// capability follows the fields that show it.
    pub fn capabilities(&self) -> BTreeSet<Capability> {
        let mut capabilities = BTreeSet::new();
        if !self.received {
            return capabilities;
        }
        if self.number("chamber_temper").is_some() {
            capabilities.insert(Capability::ChamberTemperature);
        }
        if !self.filament_slots().is_empty() {
            capabilities.extend([Capability::FilamentSlots, Capability::FilamentLoad]);
        }
        if self.number("spd_lvl").is_some() {
            capabilities.insert(Capability::SpeedProfile);
        }
        if self.number("nozzle_temper").is_some() {
            capabilities.insert(Capability::NozzleTemperature);
        }
        if self.number("bed_temper").is_some() {
            capabilities.insert(Capability::BedTemperature);
        }
        for light in self.lights() {
            capabilities.insert(match light.id {
                LightId::Chamber => Capability::ChamberLight,
                LightId::Work => Capability::WorkLight,
            });
        }
        if self.fans().iter().any(|f| f.controllable) {
            capabilities.insert(Capability::Fans);
        }
        capabilities.extend([Capability::Homing, Capability::Jogging]);
        capabilities
    }

    /// The speed profile in use, from `spd_lvl` (1 silent to 4 ludicrous).
    pub fn speed_profile(&self) -> Option<String> {
        let level = self.number("spd_lvl")? as usize;
        crate::control::SPEED_PROFILES
            .get(level.checked_sub(1)?)
            .map(|p| (*p).to_owned())
    }

    /// The lights the printer reports, from `lights_report`.
    pub fn lights(&self) -> Vec<Light> {
        let mut lights: Vec<Light> = self
            .print
            .get("lights_report")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|l| {
                let id = match l.get("node").and_then(Value::as_str)? {
                    "chamber_light" => LightId::Chamber,
                    "work_light" => LightId::Work,
                    _ => return None,
                };
                let on = l.get("mode").and_then(Value::as_str) == Some("on");
                Some(Light { id, on })
            })
            .collect();
        lights.sort_by_key(|l| l.id);
        lights.dedup_by_key(|l| l.id);
        lights
    }

    /// The Filament Slot whose filament is loaded, from `ams.tray_now`.
    pub fn loaded_slot(&self) -> Option<String> {
        let tray_now = match self.print.get("ams")?.get("tray_now")? {
            Value::String(s) => s.trim().parse().ok()?,
            Value::Number(n) => u32::try_from(n.as_u64()?).ok()?,
            _ => return None,
        };
        crate::control::loaded_slot(tray_now)
    }

    pub fn filament_slots(&self) -> Vec<gantry_core::FilamentSlot> {
        crate::filament_and_faults::filament_slots(&self.print)
    }

    pub fn faults(&self) -> Vec<gantry_core::Fault> {
        crate::filament_and_faults::faults(&self.print)
    }

    /// The whole normalised status, connected.
    pub fn status(&self) -> PrinterStatus {
        PrinterStatus {
            connection: Connection::Connected,
            state: Some(self.printer_state()),
            job: self.job(),
            temperatures: self.temperatures(),
            fans: self.fans(),
            filament_slots: self.filament_slots(),
            faults: self.faults(),
            capabilities: self.capabilities(),
            control_withdrawn: None,
            speed_profile: self.speed_profile(),
            speed_profiles: if self.number("spd_lvl").is_some() {
                crate::control::SPEED_PROFILES
                    .iter()
                    .map(|p| (*p).to_owned())
                    .collect()
            } else {
                Vec::new()
            },
            lights: self.lights(),
            // Bambu printers don't report the toolhead's position.
            position: None,
            loaded_slot: self.loaded_slot(),
        }
    }
}

/// Folds `update` into `state`: objects merge key by key, anything else
/// (arrays included) replaces.
fn merge(state: &mut serde_json::Map<String, Value>, update: &serde_json::Map<String, Value>) {
    for (key, value) in update {
        match (state.get_mut(key), value) {
            (Some(Value::Object(existing)), Value::Object(incoming)) => merge(existing, incoming),
            _ => {
                state.insert(key.clone(), value.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gantry_mock_bambu::fixtures;

    fn report(names: &[&str]) -> Report {
        let mut report = Report::default();
        for name in names {
            assert!(
                report.apply(&fixtures::load(name)),
                "{name} is a status report"
            );
        }
        report
    }

    #[test]
    fn idle() {
        let status = report(&["idle"]).status();
        assert_eq!(status.state, Some(PrinterState::Idle));
        assert!(status.job.is_none());
        assert_eq!(
            status.temperatures.nozzle,
            Some(Temperature {
                current: 24.5,
                target: None
            })
        );
        assert!(
            status
                .capabilities
                .contains(&Capability::ChamberTemperature)
        );
    }

    #[test]
    fn printing() {
        let status = report(&["idle", "printing"]).status();
        assert_eq!(status.state, Some(PrinterState::Printing));
        let job = status.job.unwrap();
        assert_eq!(job.print_file.as_deref(), Some("benchy"));
        assert_eq!(job.plate, Some(1));
        assert_eq!(job.progress, 43.0);
        assert_eq!(job.layer, Some(12));
        assert_eq!(job.total_layers, Some(40));
        assert_eq!(job.remaining_secs, Some(65 * 60));
        assert_eq!(job.key, "3001:benchy");
        assert_eq!(status.temperatures.nozzle.unwrap().target, Some(220.0));
        assert_eq!(status.temperatures.bed.unwrap().current, 59.9);
        let fans: Vec<(String, u8)> = status
            .fans
            .into_iter()
            .map(|f| (f.name, f.percent))
            .collect();
        assert_eq!(
            fans,
            [
                ("Part cooling".to_owned(), 100),
                ("Auxiliary".to_owned(), 40),
                ("Chamber".to_owned(), 0),
                ("Hotend".to_owned(), 100)
            ]
        );
    }

    #[test]
    fn a_partial_update_merges_into_the_last_full_report() {
        let status = report(&["printing", "printing_delta"]).status();
        let job = status.job.unwrap();
        assert_eq!(job.progress, 44.0);
        assert_eq!(job.layer, Some(13));
        assert_eq!(job.total_layers, Some(40), "kept from the full report");
        assert_eq!(job.key, "3001:benchy", "the same Job");
        assert_eq!(status.temperatures.nozzle.unwrap().current, 220.1);
        assert_eq!(status.temperatures.bed.unwrap().current, 59.9);
    }

    #[test]
    fn paused() {
        assert_eq!(
            report(&["printing", "paused"]).status().state,
            Some(PrinterState::Paused)
        );
    }

    #[test]
    fn finished_failed_and_cancelled_leave_the_printer_idle() {
        for (name, error) in [
            ("finished", 0),
            ("failed", 117_473_282),
            ("cancelled", 50_348_044),
        ] {
            let report = report(&["printing", name]);
            assert_eq!(report.status().state, Some(PrinterState::Idle), "{name}");
            assert!(report.status().job.is_none(), "{name}");
            assert_eq!(report.print_error(), error, "{name}");
        }
    }

    #[test]
    fn every_fixture_maps_without_panicking() {
        for (name, _) in fixtures::ALL {
            let mut r = Report::default();
            r.apply(&fixtures::load(name));
            let _ = r.status();
        }
    }

    #[test]
    fn answers_to_commands_are_not_status() {
        let mut r = report(&["printing"]);
        assert!(!r.apply(&fixtures::load("command_rejected")));
        assert_eq!(r.status().state, Some(PrinterState::Printing));
    }

    #[test]
    fn reprints_without_a_task_id_get_their_own_keys() {
        let run = |start: &str| {
            let mut r = Report::default();
            r.apply(&serde_json::json!({ "print": {
                "command": "push_status", "gcode_state": "RUNNING",
                "task_id": "0", "subtask_id": "0", "subtask_name": "cube",
                "gcode_start_time": start
            } }));
            r.job_key()
        };
        assert_ne!(run("1762250400"), run("1762254000"));
    }

    #[test]
    fn a_partial_update_without_a_state_is_not_taken_for_one() {
        let mut r = Report::default();
        r.apply(&fixtures::load("printing_delta"));
        assert!(!r.knows_gcode_state());
        r.apply(&fixtures::load("finished"));
        assert!(r.knows_gcode_state());
    }
}
