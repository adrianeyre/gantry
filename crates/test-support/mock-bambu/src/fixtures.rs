//! The hand-written `report` payloads. `fixtures/README.md` records where each
//! field comes from.
use serde_json::Value;

/// Every fixture, by name.
pub const ALL: [(&str, &str); 15] = [
    ("idle", include_str!("../fixtures/idle.json")),
    ("printing", include_str!("../fixtures/printing.json")),
    (
        "printing_delta",
        include_str!("../fixtures/printing_delta.json"),
    ),
    ("paused", include_str!("../fixtures/paused.json")),
    ("finished", include_str!("../fixtures/finished.json")),
    ("failed", include_str!("../fixtures/failed.json")),
    ("cancelled", include_str!("../fixtures/cancelled.json")),
    ("ams_none", include_str!("../fixtures/ams_none.json")),
    (
        "ams_multiple",
        include_str!("../fixtures/ams_multiple.json"),
    ),
    ("hms_faults", include_str!("../fixtures/hms_faults.json")),
    ("hms_cleared", include_str!("../fixtures/hms_cleared.json")),
    (
        "command_rejected",
        include_str!("../fixtures/command_rejected.json"),
    ),
    ("x1c_idle", include_str!("../fixtures/x1c_idle.json")),
    ("p1s_idle", include_str!("../fixtures/p1s_idle.json")),
    ("a1_idle", include_str!("../fixtures/a1_idle.json")),
];

/// One fixture, parsed. Panics on an unknown name, which is a test bug.
pub fn load(name: &str) -> Value {
    let (_, text) = ALL
        .iter()
        .find(|(n, _)| *n == name)
        .unwrap_or_else(|| panic!("no fixture called `{name}`"));
    serde_json::from_str(text).unwrap_or_else(|e| panic!("fixture `{name}` is not JSON: {e}"))
}
