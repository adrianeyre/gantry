use super::*;
use gantry_core::{DriverEvent, EventReceiver, FaultSeverity, Secrets};

fn config(settings: serde_json::Value) -> PrinterConfig {
    PrinterConfig {
        id: "sim".into(),
        name: "Sim".into(),
        settings: settings.as_object().cloned().unwrap_or_default(),
        secrets: Secrets::default(),
    }
}

fn printer() -> (SimulatedPrinter, EventReceiver) {
    let (events, rx) = EventSender::channel();
    (
        SimulatedPrinter::manual(&config(serde_json::json!({})), events),
        rx,
    )
}

/// Every event sent so far.
fn drain(rx: &mut EventReceiver) -> Vec<DriverEvent> {
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    events
}

mod fine_controls {
    use super::*;
    use gantry_core::{Axis, FineControl, Heater, LightId};

    async fn fine(printer: &SimulatedPrinter, control: FineControl) -> Result<(), CommandError> {
        PrinterHandle::fine(printer, control).await
    }

    #[tokio::test]
    async fn each_control_has_its_own_capability() {
        let (printer, _rx) = printer();
        let status = printer.status();
        for capability in Capability::FINE {
            assert!(status.has(capability), "{capability:?}");
        }
        // A status-only Printer has none of them, and refuses them.
        let (events, _rx) = EventSender::channel();
        let status_only =
            SimulatedPrinter::manual(&config(serde_json::json!({ "control": false })), events);
        assert!(
            Capability::FINE
                .iter()
                .all(|c| !status_only.status().has(*c))
        );
        assert_eq!(
            fine(&status_only, FineControl::Home).await,
            Err(CommandError::Unsupported)
        );
    }

    #[tokio::test]
    async fn the_reported_state_reflects_each_control() {
        let (printer, mut rx) = printer();

        fine(
            &printer,
            FineControl::SetSpeedProfile {
                profile: "sport".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(last_status(&mut rx).speed_profile.as_deref(), Some("sport"));

        fine(
            &printer,
            FineControl::SetTemperature {
                heater: Heater::Nozzle,
                target: 215.0,
            },
        )
        .await
        .unwrap();
        fine(
            &printer,
            FineControl::SetTemperature {
                heater: Heater::Bed,
                target: 55.0,
            },
        )
        .await
        .unwrap();
        let status = last_status(&mut rx);
        assert_eq!(status.temperatures.nozzle.unwrap().target, Some(215.0));
        assert_eq!(status.temperatures.bed.unwrap().target, Some(55.0));

        fine(
            &printer,
            FineControl::SetLight {
                light: LightId::Work,
                on: true,
            },
        )
        .await
        .unwrap();
        fine(
            &printer,
            FineControl::SetLight {
                light: LightId::Chamber,
                on: false,
            },
        )
        .await
        .unwrap();
        let lights = last_status(&mut rx).lights;
        assert!(lights.iter().any(|l| l.id == LightId::Work && l.on));
        assert!(lights.iter().any(|l| l.id == LightId::Chamber && !l.on));

        fine(
            &printer,
            FineControl::SetFan {
                fan: "auxiliary".into(),
                percent: 70,
            },
        )
        .await
        .unwrap();
        let fans = last_status(&mut rx).fans;
        assert_eq!(
            fans.iter().find(|f| f.id == "auxiliary").unwrap().percent,
            70
        );
        assert!(fans.iter().all(|f| f.controllable));

        // Jogging needs homing first; homing makes it Busy for a moment.
        assert!(matches!(
            fine(
                &printer,
                FineControl::Jog {
                    axis: Axis::X,
                    distance_mm: 10.0
                }
            )
            .await,
            Err(CommandError::InvalidState { .. })
        ));
        fine(&printer, FineControl::Home).await.unwrap();
        let status = last_status(&mut rx);
        assert_eq!(status.state, Some(PrinterState::Busy));
        assert_eq!(
            status.position,
            Some(Position {
                x: 0.0,
                y: 0.0,
                z: 0.0
            })
        );
        printer.advance(Duration::from_secs(4));
        assert_eq!(last_status(&mut rx).state, Some(PrinterState::Idle));
        fine(
            &printer,
            FineControl::Jog {
                axis: Axis::X,
                distance_mm: 10.0,
            },
        )
        .await
        .unwrap();
        fine(
            &printer,
            FineControl::Jog {
                axis: Axis::Z,
                distance_mm: 2.5,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            last_status(&mut rx).position,
            Some(Position {
                x: 10.0,
                y: 0.0,
                z: 2.5
            })
        );
        // Not past the end of an axis.
        assert!(
            fine(
                &printer,
                FineControl::Jog {
                    axis: Axis::Y,
                    distance_mm: -1.0
                }
            )
            .await
            .is_err()
        );

        fine(&printer, FineControl::LoadFilament { slot: "1".into() })
            .await
            .unwrap();
        let status = last_status(&mut rx);
        assert_eq!(status.loaded_slot.as_deref(), Some("1"));
        assert_eq!(status.state, Some(PrinterState::Busy));
        printer.advance(Duration::from_secs(6));
        // The empty slot can't be loaded.
        assert!(
            fine(&printer, FineControl::LoadFilament { slot: "3".into() })
                .await
                .is_err()
        );
        fine(&printer, FineControl::UnloadFilament).await.unwrap();
        assert_eq!(last_status(&mut rx).loaded_slot, None);
    }

    #[tokio::test]
    async fn moves_and_filament_changes_wait_for_the_job() {
        let (printer, _rx) = printer();
        printer
            .start_print(StartPrint {
                plate: None,
                print_file: None,
                duration_secs: 600.0,
                layers: 10,
            })
            .unwrap();
        for control in [
            FineControl::Home,
            FineControl::UnloadFilament,
            FineControl::LoadFilament { slot: "0".into() },
        ] {
            assert!(matches!(
                fine(&printer, control).await,
                Err(CommandError::InvalidState { .. })
            ));
        }
        // Temperatures and the speed profile can change mid-print.
        fine(
            &printer,
            FineControl::SetSpeedProfile {
                profile: "silent".into(),
            },
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn a_faster_profile_finishes_sooner() {
        let (printer, _rx) = printer();
        printer
            .start_print(StartPrint {
                plate: None,
                print_file: None,
                duration_secs: 100.0,
                layers: 10,
            })
            .unwrap();
        // Past the preparation, which the profile doesn't speed up.
        printer.advance(Duration::from_secs(10));
        let before = printer.status().job.unwrap().progress;
        printer.advance(Duration::from_secs(10));
        let standard = printer.status().job.unwrap().progress - before;
        fine(
            &printer,
            FineControl::SetSpeedProfile {
                profile: "ludicrous".into(),
            },
        )
        .await
        .unwrap();
        let before = printer.status().job.unwrap().progress;
        printer.advance(Duration::from_secs(10));
        let ludicrous = printer.status().job.unwrap().progress - before;
        assert!(ludicrous > standard * 1.5, "{standard} then {ludicrous}");
    }
}

fn last_status(rx: &mut EventReceiver) -> PrinterStatus {
    drain(rx)
        .into_iter()
        .rev()
        .find_map(|e| match e {
            DriverEvent::Status(s) => Some(*s),
            DriverEvent::JobEnded { .. } => None,
        })
        .expect("a status")
}

fn endings(events: &[DriverEvent]) -> Vec<(String, JobOutcome)> {
    events
        .iter()
        .filter_map(|e| match e {
            DriverEvent::JobEnded { key, outcome } => Some((key.clone(), *outcome)),
            DriverEvent::Status(_) => None,
        })
        .collect()
}

fn secs(n: f64) -> Duration {
    Duration::from_secs_f64(n)
}

fn print(duration: f64, layers: u32) -> StartPrint {
    StartPrint {
        plate: None,
        print_file: Some("benchy.gcode.3mf".into()),
        duration_secs: duration,
        layers,
    }
}

#[tokio::test]
async fn a_full_job_lifecycle_with_a_fault_and_a_dropout() {
    let (sim, mut rx) = printer();

    sim.advance(secs(1.0));
    let status = last_status(&mut rx);
    assert_eq!(status.connection, Connection::Connected);
    assert_eq!(status.state, Some(PrinterState::Idle));
    assert!(status.job.is_none());

    // Start: Preparing (heating) first.
    let key = sim.start_print(print(100.0, 20)).unwrap();
    let status = last_status(&mut rx);
    assert_eq!(status.state, Some(PrinterState::Preparing));
    assert_eq!(status.temperatures.nozzle.unwrap().target, Some(220.0));
    let job = status.job.unwrap();
    assert_eq!(job.key, key);
    assert_eq!(job.print_file.as_deref(), Some("benchy.gcode.3mf"));

    // Then Printing, with progress, layers and time remaining.
    sim.advance(secs(5.0));
    sim.advance(secs(25.0));
    let status = last_status(&mut rx);
    assert_eq!(status.state, Some(PrinterState::Printing));
    let job = status.job.unwrap();
    assert!((24.0..=26.0).contains(&job.progress), "{}", job.progress);
    assert_eq!(job.layer, Some(5));
    assert_eq!(job.total_layers, Some(20));
    assert_eq!(job.remaining_secs, Some(75));
    assert!(status.temperatures.nozzle.unwrap().current > 100.0);

    // A Fault sits alongside Printing; there is no error state.
    sim.raise_fault(Fault {
        code: "door_open".into(),
        severity: FaultSeverity::Warning,
        message: "The door is open".into(),
    });
    let status = last_status(&mut rx);
    assert_eq!(status.state, Some(PrinterState::Printing));
    assert_eq!(status.faults.len(), 1);

    // A dropout: disconnected, state unknown, commands refused.
    sim.drop_connection(DisconnectReason::Unreachable);
    let status = last_status(&mut rx);
    assert_eq!(
        status.connection,
        Connection::disconnected(DisconnectReason::Unreachable)
    );
    assert_eq!(status.state, None);
    assert_eq!(
        sim.pause().await,
        Err(CommandError::Disconnected {
            reason: DisconnectReason::Unreachable
        })
    );
    // The Printer keeps printing while Gantry can't see it.
    sim.advance(secs(25.0));
    sim.restore_connection();
    let status = last_status(&mut rx);
    assert_eq!(status.state, Some(PrinterState::Printing));
    assert!(status.job.unwrap().progress >= 49.0);

    // Pause and resume.
    sim.pause().await.unwrap();
    sim.advance(secs(30.0));
    let status = last_status(&mut rx);
    assert_eq!(status.state, Some(PrinterState::Paused));
    let paused_at = status.job.unwrap().progress;
    sim.resume().await.unwrap();
    assert_eq!(last_status(&mut rx).state, Some(PrinterState::Printing));
    assert!(paused_at < 51.0, "no progress while paused: {paused_at}");

    // Clear the Fault, then finish.
    sim.clear_fault("door_open");
    assert!(last_status(&mut rx).faults.is_empty());
    sim.advance(secs(60.0));
    let events = drain(&mut rx);
    assert_eq!(endings(&events), [(key, JobOutcome::Succeeded)]);
    let status = match events.last() {
        Some(DriverEvent::Status(s)) => s,
        other => panic!("{other:?}"),
    };
    assert_eq!(status.state, Some(PrinterState::Idle));
    assert!(status.job.is_none());
}

#[tokio::test]
async fn cancel_and_fail_end_the_job_with_their_outcome() {
    let (sim, mut rx) = printer();
    let first = sim.start_print(print(60.0, 10)).unwrap();
    sim.cancel().await.unwrap();
    let second = sim.start_print(print(60.0, 10)).unwrap();
    assert_ne!(first, second, "every print has its own key");
    sim.fail_print().unwrap();
    assert_eq!(
        endings(&drain(&mut rx)),
        [(first, JobOutcome::Cancelled), (second, JobOutcome::Failed)]
    );
}

#[tokio::test]
async fn a_job_that_ends_during_a_dropout_is_reported_on_reconnection() {
    let (sim, mut rx) = printer();
    let key = sim.start_print(print(10.0, 2)).unwrap();
    sim.drop_connection(DisconnectReason::Unreachable);
    sim.advance(secs(30.0));
    assert!(endings(&drain(&mut rx)).is_empty());
    sim.restore_connection();
    assert_eq!(endings(&drain(&mut rx)), [(key, JobOutcome::Succeeded)]);
}

#[tokio::test]
async fn commands_need_a_print_and_the_capability() {
    let (sim, _rx) = printer();
    assert!(matches!(
        sim.pause().await,
        Err(CommandError::InvalidState { .. })
    ));
    assert_eq!(
        sim.start_print(print(0.0, 1)),
        Err(SimulationError::Invalid(
            "the duration must be more than 0 seconds".into()
        ))
    );
    sim.start_print(print(10.0, 1)).unwrap();
    assert_eq!(
        sim.start_print(print(10.0, 1)),
        Err(SimulationError::AlreadyPrinting)
    );

    let (events, _rx) = EventSender::channel();
    let status_only =
        SimulatedPrinter::manual(&config(serde_json::json!({ "control": false })), events);
    status_only.start_print(print(10.0, 1)).unwrap();
    assert!(!status_only.status().has(Capability::Pause));
    assert_eq!(status_only.pause().await, Err(CommandError::Unsupported));
}

#[tokio::test]
async fn speed_scales_simulated_time() {
    let (sim, mut rx) = printer();
    sim.set_speed(10.0).unwrap();
    sim.start_print(print(100.0, 10)).unwrap();
    sim.advance(secs(5.5));
    let job = last_status(&mut rx).job.unwrap();
    assert!(job.progress >= 49.0, "{}", job.progress);
    assert!(sim.set_speed(0.0).is_err());
}

#[tokio::test]
async fn filament_slots_can_be_changed() {
    let (sim, mut rx) = printer();
    let status = sim.status();
    assert_eq!(status.filament_slots.len(), 4);
    assert!(status.filament_slots[3].filament.is_none());
    sim.set_filament_slot(
        "3",
        Some(Filament {
            kind: "ASA".into(),
            colour: "#123ABC".into(),
            remaining_percent: Some(50),
        }),
    )
    .unwrap();
    let slot = &last_status(&mut rx).filament_slots[3];
    assert_eq!(slot.filament.as_ref().unwrap().kind, "ASA");
    sim.set_filament_slot("0", None).unwrap();
    assert!(last_status(&mut rx).filament_slots[0].filament.is_none());
    assert_eq!(
        sim.set_filament_slot("9", None),
        Err(SimulationError::NoSuchSlot("9".into()))
    );
    assert!(
        sim.set_filament_slot(
            "0",
            Some(Filament {
                kind: "PLA".into(),
                colour: "red".into(),
                remaining_percent: None
            })
        )
        .is_err()
    );
}

#[tokio::test(start_paused = true)]
async fn a_spawned_printer_connects_and_runs_in_its_own_task() {
    let (events, mut rx) = EventSender::channel();
    let handle = SimulatedDriver.connect(config(serde_json::json!({})), events);
    let sim = handle
        .as_any()
        .downcast_ref::<SimulatedPrinter>()
        .unwrap()
        .clone();
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert_eq!(last_status(&mut rx).connection, Connection::Connecting);
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(last_status(&mut rx).connection, Connection::Connected);

    sim.start_print(print(2.0, 2)).unwrap();
    tokio::time::sleep(Duration::from_secs(5)).await;
    let events = drain(&mut rx);
    assert_eq!(endings(&events).len(), 1);

    handle.shutdown().await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    drain(&mut rx);
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(drain(&mut rx).is_empty(), "the task stopped");
}

#[test]
fn validation_checks_the_slot_count() {
    let errors = SimulatedDriver.validate(
        serde_json::json!({ "filament_slots": 40 })
            .as_object()
            .unwrap(),
        &Secrets::default(),
    );
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].field, "filament_slots");
}
