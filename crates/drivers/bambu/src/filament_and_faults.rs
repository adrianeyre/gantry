//! Filament Slots (from the printer's filament units and external spool) and
//! Faults (from its HMS codes).
//!
//! The UI and the code speak the glossary's language: a position is a
//! Filament Slot, and a problem the printer reports is a Fault. The printer's
//! own names stay in the protocol.
use gantry_core::{Fault, FaultSeverity, Filament, FilamentSlot};
use serde_json::Value;

fn text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn number(value: &Value, key: &str) -> Option<i64> {
    match value.get(key)? {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// `RRGGBBAA` (or `RRGGBB`) as `#RRGGBB`.
fn colour(raw: &str) -> Option<String> {
    let hex = raw.trim();
    let rgb = hex.get(..6)?;
    rgb.chars()
        .all(|c| c.is_ascii_hexdigit())
        .then(|| format!("#{}", rgb.to_ascii_uppercase()))
}

/// What one slot holds, or `None` when it is empty.
fn filament(slot: &Value) -> Option<Filament> {
    let kind = text(slot, "tray_type")?;
    Some(Filament {
        kind,
        colour: text(slot, "tray_color")
            .and_then(|c| colour(&c))
            .unwrap_or_else(|| "#808080".into()),
        // -1 means the printer doesn't know.
        remaining_percent: number(slot, "remain")
            .filter(|r| (0..=100).contains(r))
            .map(|r| r as u8),
    })
}

/// Every Filament Slot: each filament unit's slots, then the external spool.
pub fn filament_slots(print: &serde_json::Map<String, Value>) -> Vec<FilamentSlot> {
    let mut slots = Vec::new();
    let units = print
        .get("ams")
        .and_then(|a| a.get("ams"))
        .and_then(Value::as_array);
    for (u, unit) in units.into_iter().flatten().enumerate() {
        let unit_id = number(unit, "id").unwrap_or(u as i64);
        let trays = unit.get("tray").and_then(Value::as_array);
        for (t, slot) in trays.into_iter().flatten().enumerate() {
            let slot_id = number(slot, "id").unwrap_or(t as i64);
            slots.push(FilamentSlot {
                id: format!("{unit_id}-{slot_id}"),
                label: format!("Unit {} · Slot {}", unit_id + 1, slot_id + 1),
                filament: filament(slot),
                low: false,
            });
        }
    }
    if let Some(external) = print.get("vt_tray").filter(|v| v.is_object()) {
        slots.push(FilamentSlot {
            id: "ext".into(),
            label: "External spool".into(),
            filament: filament(external),
            low: false,
        });
    }
    slots
}

/// An HMS entry's code, as the printer's screen and Bambu's wiki show it.
fn hms_code(attr: u32, code: u32) -> String {
    format!(
        "HMS_{:04X}_{:04X}_{:04X}_{:04X}",
        attr >> 16,
        attr & 0xFFFF,
        code >> 16,
        code & 0xFFFF
    )
}

/// The level in an HMS code: 1 fatal, 2 serious, 3 common, 4 info.
fn severity(code: u32) -> FaultSeverity {
    match code >> 16 {
        1 | 2 => FaultSeverity::Error,
        _ => FaultSeverity::Warning,
    }
}

/// A readable message for the codes Gantry knows.
fn known_message(attr: u32, code: u32) -> Option<String> {
    // Filament run-out in a filament unit's slot: `0700_2000_0002_0001` is
    // unit 1, slot 1 on Bambu's wiki, and the slot is the second hex digit of
    // the attribute's low half. Other units are assumed to follow in the
    // attribute's high half (`07UU`).
    if attr >> 24 == 0x07 && attr & 0xF0FF == 0x2000 && code == 0x0002_0001 {
        let unit = ((attr >> 16) & 0xFF) + 1;
        let slot = ((attr >> 8) & 0x0F) + 1;
        return Some(format!("Filament has run out in Unit {unit}, Slot {slot}"));
    }
    None
}

/// Every Fault the printer is reporting now.
pub fn faults(print: &serde_json::Map<String, Value>) -> Vec<Fault> {
    let Some(entries) = print.get("hms").and_then(Value::as_array) else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| {
            let attr = u32::try_from(number(entry, "attr")?).ok()?;
            let code = u32::try_from(number(entry, "code")?).ok()?;
            let id = hms_code(attr, code);
            Some(Fault {
                message: known_message(attr, code)
                    .unwrap_or_else(|| format!("The printer reported fault {id}")),
                code: id,
                severity: severity(code),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gantry_mock_bambu::fixtures;

    fn print(name: &str) -> serde_json::Map<String, Value> {
        fixtures::load(name)["print"].as_object().unwrap().clone()
    }

    #[test]
    fn no_filament_unit_leaves_only_the_external_spool() {
        let slots = filament_slots(&print("ams_none"));
        assert_eq!(slots.len(), 1);
        assert_eq!(slots[0].label, "External spool");
        let filament = slots[0].filament.as_ref().unwrap();
        assert_eq!(filament.kind, "PETG");
        assert_eq!(filament.colour, "#1E88E5");
        assert_eq!(filament.remaining_percent, None, "-1 means unknown");
    }

    #[test]
    fn one_filament_unit() {
        let slots = filament_slots(&print("idle"));
        assert_eq!(slots.len(), 5, "four slots and the external spool");
        assert_eq!(slots[0].id, "0-0");
        assert_eq!(slots[0].label, "Unit 1 · Slot 1");
        let first = slots[0].filament.as_ref().unwrap();
        assert_eq!(
            (first.kind.as_str(), first.colour.as_str()),
            ("PLA", "#FFFFFF")
        );
        assert_eq!(first.remaining_percent, Some(85));
        assert_eq!(slots[1].filament.as_ref().unwrap().colour, "#000000");
        assert_eq!(slots[2].filament.as_ref().unwrap().remaining_percent, None);
        assert!(
            slots[3].filament.is_none(),
            "a slot with only an id is empty"
        );
        assert!(slots[4].filament.is_none(), "nothing on the external spool");
    }

    #[test]
    fn several_filament_units() {
        let slots = filament_slots(&print("ams_multiple"));
        assert_eq!(slots.len(), 9);
        assert_eq!(slots[4].id, "1-0");
        assert_eq!(slots[4].label, "Unit 2 · Slot 1");
        assert_eq!(slots[4].filament.as_ref().unwrap().kind, "PLA-S");
        assert_eq!(
            slots[5].filament.as_ref().unwrap().remaining_percent,
            Some(5)
        );
        assert!(slots[7].filament.is_none());
    }

    #[test]
    fn known_hms_codes_get_a_message_and_unknown_ones_keep_their_code() {
        let faults = faults(&print("hms_faults"));
        assert_eq!(faults.len(), 3);

        assert_eq!(faults[0].code, "HMS_0700_2000_0002_0001");
        assert_eq!(faults[0].message, "Filament has run out in Unit 1, Slot 1");
        assert_eq!(faults[0].severity, FaultSeverity::Error, "level 2, serious");

        assert_eq!(faults[1].code, "HMS_0300_0100_0003_0001");
        assert_eq!(
            faults[1].severity,
            FaultSeverity::Warning,
            "level 3, common"
        );
        assert!(faults[1].message.contains("HMS_0300_0100_0003_0001"));

        assert_eq!(faults[2].code, "HMS_0300_8000_0001_0007");
        assert_eq!(faults[2].severity, FaultSeverity::Error, "level 1, fatal");
    }

    #[test]
    fn the_run_out_message_follows_the_unit_and_slot() {
        assert_eq!(
            known_message(0x0701_2300, 0x0002_0001).as_deref(),
            Some("Filament has run out in Unit 2, Slot 4")
        );
        assert_eq!(known_message(0x0700_2000, 0x0002_0002), None);
    }

    #[test]
    fn no_hms_is_no_faults() {
        assert!(faults(&print("idle")).is_empty());
        assert!(faults(&print("hms_cleared")).is_empty());
    }
}
