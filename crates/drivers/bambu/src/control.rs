//! What Gantry sends a Bambu printer for each command, and how it reads the
//! answer.
//!
//! The request payloads follow the community-documented schema (OpenBambuAPI's
//! `mqtt.md`, and what Bambu Studio sends): `print.print_speed` for the speed
//! profile, `print.gcode_line` for temperatures, fans, homing and jogging,
//! `system.ledctrl` for lights, `print.ams_change_filament` for filament, and
//! `print.project_file` or `print.gcode_file` to start an uploaded Print File
//! (as ha-bambulab and OpenBambuAPI document them). None is verified against
//! real firmware yet (#27, #43).
use gantry_core::{Axis, FineControl, Heater, LightId};
use md5::{Digest, Md5};
use serde_json::{Value, json};

use crate::report::FANS;

/// The speed profiles, in `spd_lvl` order (1 to 4).
pub const SPEED_PROFILES: [&str; 4] = ["silent", "standard", "sport", "ludicrous"];

/// The hottest targets Gantry sends a Bambu printer, in °C.
pub const MAX_NOZZLE: f64 = 300.0;
pub const MAX_BED: f64 = 110.0;

/// The jog speed, mm/min, as Bambu Studio uses.
const JOG_FEED: u32 = 3000;

/// `ams_change_filament` targets for the external spool and for unloading.
const TARGET_EXTERNAL: u32 = 254;
const TARGET_UNLOAD: u32 = 255;

/// A `print` request with this command.
pub fn print(sequence: &str, command: &str, extra: Value) -> Value {
    let mut body = json!({ "sequence_id": sequence, "command": command });
    if let (Some(body), Value::Object(extra)) = (body.as_object_mut(), extra) {
        body.extend(extra);
    }
    json!({ "print": body })
}

fn gcode(sequence: &str, lines: &str) -> Value {
    print(sequence, "gcode_line", json!({ "param": lines }))
}

/// The `ams_change_filament` target for a Filament Slot id (`unit-slot` or
/// `ext`), as the printer numbers them: four slots per unit.
pub fn filament_target(slot: &str) -> Option<u32> {
    if slot == "ext" {
        return Some(TARGET_EXTERNAL);
    }
    let (unit, tray) = slot.split_once('-')?;
    let (unit, tray): (u32, u32) = (unit.parse().ok()?, tray.parse().ok()?);
    (tray < 4 && unit < 16).then_some(unit * 4 + tray)
}

/// The Filament Slot id for the printer's `tray_now` (the loaded slot).
pub fn loaded_slot(tray_now: u32) -> Option<String> {
    match tray_now {
        TARGET_UNLOAD => None,
        TARGET_EXTERNAL => Some("ext".into()),
        n if n < 64 => Some(format!("{}-{}", n / 4, n % 4)),
        _ => None,
    }
}

/// The request for a fine control, or why this printer can't take it.
pub fn fine(sequence: &str, control: &FineControl) -> Result<Value, String> {
    Ok(match control {
        FineControl::SetSpeedProfile { profile } => {
            let level = SPEED_PROFILES
                .iter()
                .position(|p| p == profile)
                .ok_or_else(|| format!("no speed profile `{profile}`"))?
                + 1;
            print(
                sequence,
                "print_speed",
                json!({ "param": level.to_string() }),
            )
        }
        FineControl::SetTemperature { heater, target } => {
            let (code, max, name) = match heater {
                Heater::Nozzle => ("M104", MAX_NOZZLE, "nozzle"),
                Heater::Bed => ("M140", MAX_BED, "bed"),
            };
            if *target > max {
                return Err(format!("a Bambu printer's {name} goes up to {max} °C"));
            }
            gcode(sequence, &format!("{code} S{} \n", target.round() as u32))
        }
        FineControl::SetLight { light, on } => {
            let node = match light {
                LightId::Chamber => "chamber_light",
                LightId::Work => "work_light",
            };
            json!({ "system": {
                "sequence_id": sequence,
                "command": "ledctrl",
                "led_node": node,
                "led_mode": if *on { "on" } else { "off" },
                "led_on_time": 500,
                "led_off_time": 500,
                "loop_times": 0,
                "interval_time": 0,
            } })
        }
        FineControl::SetFan { fan, percent } => {
            let index = FANS
                .iter()
                .find(|f| f.id == fan)
                .and_then(|f| f.gcode_index)
                .ok_or_else(|| format!("the `{fan}` fan can't be set"))?;
            let speed = (f64::from(*percent) * 255.0 / 100.0).round() as u32;
            gcode(sequence, &format!("M106 P{index} S{speed} \n"))
        }
        FineControl::Home => gcode(sequence, "G28 \n"),
        FineControl::Jog { axis, distance_mm } => {
            let axis = match axis {
                Axis::X => "X",
                Axis::Y => "Y",
                Axis::Z => "Z",
            };
            // Relative for the one move, then back to how it was.
            gcode(
                sequence,
                &format!(
                    "M211 S \nM211 X1 Y1 Z1\nM1002 push_ref_mode\nG91 \nG1 {axis}{distance_mm:.1} F{JOG_FEED}\nM1002 pop_ref_mode\nM211 R\n"
                ),
            )
        }
        FineControl::LoadFilament { slot } => {
            let target =
                filament_target(slot).ok_or_else(|| format!("no Filament Slot `{slot}`"))?;
            print(
                sequence,
                "ams_change_filament",
                json!({ "target": target, "curr_temp": 220, "tar_temp": 220 }),
            )
        }
        FineControl::UnloadFilament => print(
            sequence,
            "ams_change_filament",
            json!({ "target": TARGET_UNLOAD, "curr_temp": 220, "tar_temp": 220 }),
        ),
    })
}

/// The request that starts a Print File already uploaded to the root of the
/// SD card, or why it can't be started.
///
/// A `.3mf` (Bambu's `.gcode.3mf`) starts with `project_file`, naming the
/// Plate's G-code inside it; a plain `.gcode` with `gcode_file`. The MD5 is
/// of the whole file, in upper case as Bambu Studio sends it. `use_ams` is
/// whether the printer has filament units: without an `ams_mapping` the
/// printer is reported to map the Plate's filaments to slots in order.
pub fn start(
    sequence: &str,
    file_name: &str,
    plate: Option<u32>,
    data: &[u8],
    use_ams: bool,
) -> Result<Value, String> {
    let lower = file_name.to_ascii_lowercase();
    if lower.ends_with(".3mf") {
        let plate = plate.unwrap_or(1);
        if plate == 0 {
            return Err("Plates are numbered from 1".into());
        }
        let subtask = strip_suffix_ignore_case(file_name, ".gcode.3mf")
            .or_else(|| strip_suffix_ignore_case(file_name, ".3mf"))
            .unwrap_or(file_name);
        let md5: String = Md5::digest(data)
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect();
        Ok(print(
            sequence,
            "project_file",
            json!({
                "param": format!("Metadata/plate_{plate}.gcode"),
                "url": format!("file:///sdcard/{file_name}"),
                "subtask_name": subtask,
                "md5": md5,
                "bed_type": "auto",
                "timelapse": false,
                "bed_leveling": true,
                "flow_cali": false,
                "vibration_cali": true,
                "layer_inspect": false,
                "use_ams": use_ams,
                "profile_id": "0",
                "project_id": "0",
                "subtask_id": "0",
                "task_id": "0",
            }),
        ))
    } else if lower.ends_with(".gcode") {
        Ok(print(
            sequence,
            "gcode_file",
            json!({ "param": format!("/sdcard/{file_name}") }),
        ))
    } else {
        Err("a Bambu printer prints .gcode.3mf, .3mf or .gcode files".into())
    }
}

fn strip_suffix_ignore_case<'a>(name: &'a str, suffix: &str) -> Option<&'a str> {
    let at = name.len().checked_sub(suffix.len())?;
    (name.is_char_boundary(at) && name[at..].eq_ignore_ascii_case(suffix)).then(|| &name[..at])
}

/// What a printer said about one command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    Accepted,
    /// Firmware won't take local commands at all (PRD §6.1, §10.3): control
    /// is withdrawn, with how to fix it.
    ControlBlocked(String),
    /// This one command was refused, with the printer's reason.
    Refused(String),
}

/// `err_code`s reported for a local command refused by Authorization Control.
/// The fixture's value is a placeholder until checked on a real printer.
const AUTHORISATION_ERRORS: [i64; 1] = [84_033_543];

/// What the card says when firmware blocks control, and how to fix it.
pub const BLOCKED_HELP: &str = "the printer's firmware refused local control. On the printer, \
     turn on LAN Only Mode and then Developer Mode, and Gantry gets control back when it reconnects.";

/// The sequence id and verdict of an answer to a command, if `payload` is one.
pub fn answer(payload: &Value) -> Option<(String, Answer)> {
    let body = ["print", "system"]
        .into_iter()
        .find_map(|s| payload.get(s))?;
    if body.get("command").and_then(Value::as_str) == Some("push_status") {
        return None;
    }
    let result = body.get("result").and_then(Value::as_str)?;
    let sequence = match body.get("sequence_id")? {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    if result.eq_ignore_ascii_case("success") {
        return Some((sequence, Answer::Accepted));
    }
    let reason = body
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_owned();
    let code = body.get("err_code").and_then(Value::as_i64);
    let blocked = reason.to_ascii_lowercase().contains("verify failed")
        || code.is_some_and(|c| AUTHORISATION_ERRORS.contains(&c));
    Some((
        sequence,
        if blocked {
            Answer::ControlBlocked(BLOCKED_HELP.into())
        } else if reason.is_empty() {
            Answer::Refused(format!("the printer answered `{result}`"))
        } else {
            Answer::Refused(reason)
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_filament_slots_as_the_printer_does() {
        assert_eq!(filament_target("0-0"), Some(0));
        assert_eq!(filament_target("1-2"), Some(6));
        assert_eq!(filament_target("ext"), Some(254));
        assert_eq!(filament_target("0-4"), None);
        assert_eq!(filament_target("nope"), None);
        assert_eq!(loaded_slot(6), Some("1-2".into()));
        assert_eq!(loaded_slot(254), Some("ext".into()));
        assert_eq!(loaded_slot(255), None);
    }

    #[test]
    fn reads_answers() {
        let ok =
            json!({ "print": { "command": "pause", "sequence_id": "7", "result": "success" } });
        assert_eq!(answer(&ok), Some(("7".into(), Answer::Accepted)));
        let blocked = gantry_mock_bambu::fixtures::load("command_rejected");
        assert!(matches!(
            answer(&blocked),
            Some((s, Answer::ControlBlocked(_))) if s == "2000"
        ));
        let other = json!({ "system": { "command": "ledctrl", "sequence_id": 3, "result": "fail", "reason": "busy" } });
        assert_eq!(
            answer(&other),
            Some(("3".into(), Answer::Refused("busy".into())))
        );
        let status = gantry_mock_bambu::fixtures::load("idle");
        assert_eq!(answer(&status), None);
    }

    #[test]
    fn starts_a_plate_of_a_project_file() {
        let request = start("9", "Cube.GCODE.3mf", Some(2), b"hello", true).unwrap();
        let body = &request["print"];
        assert_eq!(body["sequence_id"], "9");
        assert_eq!(body["command"], "project_file");
        assert_eq!(body["param"], "Metadata/plate_2.gcode");
        assert_eq!(body["url"], "file:///sdcard/Cube.GCODE.3mf");
        assert_eq!(body["subtask_name"], "Cube");
        // The MD5 of "hello", upper case.
        assert_eq!(body["md5"], "5D41402ABC4B2A76B9719D911017C592");
        assert_eq!(body["use_ams"], true);
        assert_eq!(body["bed_leveling"], true);
        assert_eq!(body["timelapse"], false);
        assert_eq!(body["task_id"], "0");

        let first = start("1", "cube.3mf", None, b"", false).unwrap();
        assert_eq!(first["print"]["param"], "Metadata/plate_1.gcode");
        assert_eq!(first["print"]["subtask_name"], "cube");
        assert_eq!(first["print"]["use_ams"], false);
        assert!(start("1", "cube.3mf", Some(0), b"", false).is_err());
    }

    #[test]
    fn starts_plain_gcode_and_nothing_else() {
        let request = start("4", "cube.gcode", None, b"G28", false).unwrap();
        assert_eq!(
            request,
            json!({ "print": {
                "sequence_id": "4", "command": "gcode_file", "param": "/sdcard/cube.gcode"
            } })
        );
        assert!(start("4", "cube.stl", None, b"", false).is_err());
    }

    #[test]
    fn refuses_what_a_bambu_printer_cannot_take() {
        assert!(
            fine(
                "1",
                &FineControl::SetTemperature {
                    heater: Heater::Bed,
                    target: 150.0
                }
            )
            .is_err()
        );
        assert!(
            fine(
                "1",
                &FineControl::SetFan {
                    fan: "hotend".into(),
                    percent: 50
                }
            )
            .is_err()
        );
        assert!(fine("1", &FineControl::LoadFilament { slot: "9".into() }).is_err());
    }
}
