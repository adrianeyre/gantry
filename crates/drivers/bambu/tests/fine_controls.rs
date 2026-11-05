//! Fine controls (#23) and firmware-blocked control (#19) on the Bambu
//! Driver, against the mock Bambu broker and a real Fleet.
use std::sync::Arc;
use std::time::Duration;

use gantry_core::{
    Axis, Capability, Command, CommandError, DriverRegistry, FineControl, Fleet, FleetError,
    Heater, JobState, LightId, MemoryJobLog, PrinterConfig, PrinterView, Secrets,
};
use gantry_driver_bambu::BambuDriver;
use gantry_mock_bambu::{Answers, MockBambu};
use serde_json::{Value, json};

const SERIAL: &str = "00MFINESERIAL01";
const CODE: &str = "24681357";

struct Harness {
    mock: MockBambu,
    fleet: Fleet,
}

async fn harness(fixture: &str) -> Harness {
    let mock = MockBambu::start(SERIAL, CODE).await;
    mock.report_fixture(fixture);
    let mut registry = DriverRegistry::new();
    registry.register(BambuDriver);
    let fleet = Fleet::new(registry, Arc::new(MemoryJobLog::new()));
    fleet
        .start(
            "bambu",
            PrinterConfig {
                id: "b".into(),
                name: "Bambu".into(),
                settings: json!({ "host": mock.address().to_string() })
                    .as_object()
                    .unwrap()
                    .clone(),
                secrets: Secrets::new(
                    [
                        ("serial".to_owned(), SERIAL.to_owned()),
                        ("access_code".to_owned(), CODE.to_owned()),
                    ]
                    .into(),
                ),
            },
            JobState::default(),
        )
        .unwrap();
    let h = Harness { mock, fleet };
    h.until(|v| v.status.connection.is_connected() && v.status.state.is_some())
        .await;
    h
}

impl Harness {
    async fn until(&self, pred: impl Fn(&PrinterView) -> bool) -> PrinterView {
        (*self
            .fleet
            .wait_for("b", Duration::from_secs(10), pred)
            .await
            .unwrap())
        .clone()
    }

    async fn fine(&self, control: FineControl) -> Result<(), FleetError> {
        self.fleet.command("b", Command::Fine(control)).await
    }

    /// The newest request in `section` (`print` or `system`) with `command`.
    async fn request(&self, section: &str, command: &str) -> Value {
        let found = self
            .mock
            .wait_for_request(Duration::from_secs(3), |r| r[section]["command"] == command)
            .await
            .unwrap_or_else(|| panic!("no `{command}` request was recorded"));
        let _ = found;
        self.mock
            .requests()
            .into_iter()
            .rev()
            .find(|r| r[section]["command"] == command)
            .unwrap()[section]
            .clone()
    }
}

#[tokio::test]
async fn the_mock_broker_records_the_expected_request_for_each_control() {
    let h = harness("x1c_idle").await;

    h.fine(FineControl::SetSpeedProfile {
        profile: "sport".into(),
    })
    .await
    .unwrap();
    assert_eq!(h.request("print", "print_speed").await["param"], "3");

    h.fine(FineControl::SetTemperature {
        heater: Heater::Nozzle,
        target: 215.0,
    })
    .await
    .unwrap();
    assert_eq!(
        h.request("print", "gcode_line").await["param"],
        "M104 S215 \n"
    );
    h.fine(FineControl::SetTemperature {
        heater: Heater::Bed,
        target: 60.0,
    })
    .await
    .unwrap();
    assert_eq!(
        h.request("print", "gcode_line").await["param"],
        "M140 S60 \n"
    );

    h.fine(FineControl::SetLight {
        light: LightId::Work,
        on: true,
    })
    .await
    .unwrap();
    let light = h.request("system", "ledctrl").await;
    assert_eq!(light["led_node"], "work_light");
    assert_eq!(light["led_mode"], "on");

    h.fine(FineControl::SetFan {
        fan: "auxiliary".into(),
        percent: 50,
    })
    .await
    .unwrap();
    assert_eq!(
        h.request("print", "gcode_line").await["param"],
        "M106 P2 S128 \n"
    );

    h.fine(FineControl::Home).await.unwrap();
    assert_eq!(h.request("print", "gcode_line").await["param"], "G28 \n");

    h.fine(FineControl::Jog {
        axis: Axis::Y,
        distance_mm: -10.0,
    })
    .await
    .unwrap();
    let jog = h.request("print", "gcode_line").await["param"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(jog.contains("G91 \nG1 Y-10.0 F3000\n"), "{jog}");
    assert!(jog.ends_with("M211 R\n"), "{jog}");

    h.fine(FineControl::LoadFilament { slot: "0-2".into() })
        .await
        .unwrap();
    let load = h.request("print", "ams_change_filament").await;
    assert_eq!(load["target"], 2);
    h.fine(FineControl::UnloadFilament).await.unwrap();
    assert_eq!(
        h.request("print", "ams_change_filament").await["target"],
        255
    );

    // Every request carries its own sequence id, which the answer echoes.
    let sequences: Vec<Value> = h
        .mock
        .requests()
        .iter()
        .filter_map(|r| {
            r.get("print")
                .or_else(|| r.get("system"))
                .map(|b| b["sequence_id"].clone())
        })
        .collect();
    let mut unique = sequences.clone();
    unique.sort_by_key(Value::to_string);
    unique.dedup();
    assert_eq!(unique.len(), sequences.len());
}

#[tokio::test]
async fn a_control_this_printer_cannot_take_is_refused_before_sending() {
    let h = harness("x1c_idle").await;
    let before = h.mock.requests().len();
    let refused = h
        .fine(FineControl::SetFan {
            fan: "hotend".into(),
            percent: 10,
        })
        .await;
    assert!(matches!(
        refused,
        Err(FleetError::Command(CommandError::Invalid { .. }))
    ));
    assert_eq!(h.mock.requests().len(), before);
}

#[tokio::test]
async fn capabilities_vary_by_model() {
    let x1 = harness("x1c_idle").await;
    let a1 = harness("a1_idle").await;
    let p1 = harness("p1s_idle").await;
    let has = |v: &PrinterView, c: Capability| v.status.has(c);
    let x1 = x1.until(|_| true).await;
    let a1 = a1.until(|_| true).await;
    let p1 = p1.until(|_| true).await;

    assert!(has(&x1, Capability::WorkLight) && has(&x1, Capability::ChamberLight));
    assert!(!has(&p1, Capability::WorkLight) && has(&p1, Capability::ChamberLight));
    assert!(!has(&a1, Capability::ChamberLight) && !has(&a1, Capability::WorkLight));
    assert!(has(&x1, Capability::ChamberTemperature));
    assert!(!has(&a1, Capability::ChamberTemperature));
    // Every model reports a speed level, temperatures and a part fan.
    for v in [&x1, &a1, &p1] {
        for c in [
            Capability::SpeedProfile,
            Capability::NozzleTemperature,
            Capability::BedTemperature,
            Capability::Fans,
            Capability::Homing,
            Capability::Jogging,
            Capability::FilamentLoad,
        ] {
            assert!(has(v, c), "{c:?}");
        }
    }
    // The A1 has no auxiliary or chamber fan to set.
    let controllable: Vec<&str> = a1
        .status
        .fans
        .iter()
        .filter(|f| f.controllable)
        .map(|f| f.id.as_str())
        .collect();
    assert_eq!(controllable, ["part"]);
    assert_eq!(a1.status.speed_profile.as_deref(), Some("silent"));
    assert_eq!(x1.status.loaded_slot.as_deref(), Some("0-1"));
    assert!(
        x1.status
            .lights
            .iter()
            .any(|l| l.id == LightId::Chamber && l.on)
    );
}

#[tokio::test]
async fn blocked_control_leaves_the_printer_connected_with_control_withdrawn() {
    let h = harness("printing").await;
    h.mock.set_answers(Answers::BlockControl);

    let refused = h.fleet.command("b", Command::Pause).await;
    match refused {
        Err(FleetError::Command(CommandError::Refused { message })) => {
            assert!(message.contains("Developer Mode"), "{message}");
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
    let view = h.until(|v| v.status.control_withdrawn.is_some()).await;
    // Still connected, status still live, every control gone, and why.
    assert!(view.status.connection.is_connected());
    for c in Capability::CONTROL {
        assert!(!view.status.has(c), "{c:?} should be withdrawn");
    }
    assert!(view.status.has(Capability::FilamentSlots));
    assert!(
        view.status
            .control_withdrawn
            .as_deref()
            .unwrap()
            .contains("Developer Mode")
    );
    h.mock.report_fixture("printing_delta");
    h.until(|v| v.status.job.as_ref().is_some_and(|j| j.progress == 44.0))
        .await;
    // Commands are now refused by the Fleet without reaching the printer.
    let before = h.mock.requests().len();
    assert!(matches!(
        h.fleet.command("b", Command::Pause).await,
        Err(FleetError::Command(CommandError::Unsupported))
    ));
    assert_eq!(h.mock.requests().len(), before);

    // Developer Mode on: the printer restarts its broker and allows control.
    h.mock.set_answers(Answers::Accept);
    h.mock.go_down();
    h.until(|v| !v.status.connection.is_connected()).await;
    h.mock.come_back().await;
    let view = h
        .until(|v| v.status.connection.is_connected() && v.status.has(Capability::Pause))
        .await;
    assert_eq!(view.status.control_withdrawn, None);
    h.fleet.command("b", Command::Pause).await.unwrap();
}

#[tokio::test]
async fn an_ordinary_refusal_does_not_withdraw_control() {
    let h = harness("printing").await;
    h.mock.set_answers(Answers::Silent);
    // No answer at all is taken as accepted, as with older firmware.
    h.fleet.command("b", Command::Pause).await.unwrap();
    let view = h.until(|_| true).await;
    assert!(view.status.has(Capability::Pause));
    assert_eq!(view.status.control_withdrawn, None);
}

#[tokio::test]
async fn the_model_decides_the_camera() {
    let h = harness("x1c_idle").await;
    let view = h.until(|_| true).await;
    // The serial says X1C, so the RTSPS camera is offered.
    assert!(view.status.has(Capability::Camera));
    assert!(h.fleet.handle("b").unwrap().camera().is_some());

    // An unknown serial with no model set has no camera Gantry can read.
    let mock = MockBambu::start("ZZZUNKNOWN0001", CODE).await;
    mock.report_fixture("idle");
    let mut registry = DriverRegistry::new();
    registry.register(BambuDriver);
    let fleet = Fleet::new(registry, Arc::new(MemoryJobLog::new()));
    fleet
        .start(
            "bambu",
            PrinterConfig {
                id: "u".into(),
                name: "Unknown".into(),
                settings: json!({ "host": mock.address().to_string() })
                    .as_object()
                    .unwrap()
                    .clone(),
                secrets: Secrets::new(
                    [
                        ("serial".to_owned(), "ZZZUNKNOWN0001".to_owned()),
                        ("access_code".to_owned(), CODE.to_owned()),
                    ]
                    .into(),
                ),
            },
            JobState::default(),
        )
        .unwrap();
    let view = fleet
        .wait_for("u", Duration::from_secs(10), |v| {
            v.status.connection.is_connected() && v.status.state.is_some()
        })
        .await
        .unwrap();
    assert!(!view.status.has(Capability::Camera));
    assert!(fleet.handle("u").unwrap().camera().is_none());
}

#[tokio::test]
async fn moves_and_filament_changes_are_refused_while_printing() {
    let h = harness("printing").await;
    let before = h.mock.requests().len();
    for control in [
        FineControl::Home,
        FineControl::Jog {
            axis: Axis::X,
            distance_mm: 1.0,
        },
        FineControl::LoadFilament { slot: "0-0".into() },
        FineControl::UnloadFilament,
    ] {
        assert!(matches!(
            h.fine(control).await,
            Err(FleetError::Command(CommandError::InvalidState { .. }))
        ));
    }
    assert_eq!(h.mock.requests().len(), before, "nothing was sent");
    // Temperatures can still change mid-print.
    h.fine(FineControl::SetTemperature {
        heater: Heater::Bed,
        target: 65.0,
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn another_apps_refused_command_does_not_withdraw_control() {
    let h = harness("idle").await;
    // Bambu Studio's command (numbered from 1) refused by the firmware.
    let mut refused = gantry_mock_bambu::fixtures::load("command_rejected");
    refused["print"]["sequence_id"] = json!("1");
    h.mock.report(&refused);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let view = h.until(|_| true).await;
    assert!(view.status.has(Capability::Pause));
    assert_eq!(view.status.control_withdrawn, None);
}
