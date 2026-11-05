//! The Elegoo SDCP Driver end to end, through the Fleet, against the mock
//! SDCP printer in `mock_sdcp` (PRD §5.6). Runs with plain `cargo test`.
//! Real hardware is unverified: see the crate's documentation.
mod mock_sdcp;

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use gantry_core::{
    Capability, Command, Connection, DisconnectReason, DriverRegistry, FineControl, Fleet, Heater,
    JobOutcome, JobStart, JobState, LightId, MemoryJobLog, PrintUpload, PrinterConfig,
    PrinterState, PrinterView, Secrets,
};
use gantry_driver_elegoo_sdcp::{ElegooSdcpDriver, md5_hex};
use mock_sdcp::{JPEG, MAINBOARD, MockSdcp};
use serde_json::{Value, json};

fn printer(settings: Value) -> PrinterConfig {
    PrinterConfig {
        id: "e".into(),
        name: "Centauri".into(),
        settings: settings.as_object().unwrap().clone(),
        secrets: Secrets::default(),
    }
}

struct Harness {
    fleet: Fleet,
    log: Arc<MemoryJobLog>,
}

fn harness(settings: Value) -> Harness {
    let mut registry = DriverRegistry::new();
    registry.register(ElegooSdcpDriver);
    let log = Arc::new(MemoryJobLog::new());
    let fleet = Fleet::with_command_timeout(registry, log.clone(), Duration::from_secs(15));
    fleet
        .start("elegoo-sdcp", printer(settings), JobState::default())
        .unwrap();
    Harness { fleet, log }
}

fn on(mock: &MockSdcp) -> Harness {
    harness(json!({ "host": "127.0.0.1", "port": mock.port, "model": "Elegoo Centauri Carbon" }))
}

impl Harness {
    async fn until(
        &self,
        secs: u64,
        what: &str,
        pred: impl Fn(&PrinterView) -> bool,
    ) -> PrinterView {
        match self
            .fleet
            .wait_for("e", Duration::from_secs(secs), &pred)
            .await
        {
            Ok(v) => (*v).clone(),
            Err(_) => panic!(
                "timed out waiting for {what}; the Printer is now {:#?}",
                self.fleet.view("e").unwrap().status
            ),
        }
    }

    async fn idle(&self) -> PrinterView {
        self.until(20, "an Idle Printer", |v| {
            v.status.connection.is_connected() && v.status.state == Some(PrinterState::Idle)
        })
        .await
    }

    async fn state(&self, state: PrinterState) -> PrinterView {
        self.until(20, &format!("{state:?}"), |v| v.status.state == Some(state))
            .await
    }
}

async fn eventually(what: &str, check: impl Fn() -> bool) {
    for _ in 0..100 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for {what}");
}

#[tokio::test]
async fn connects_learns_the_mainboard_and_maps_the_status() {
    let mock = MockSdcp::start().await;
    let h = on(&mock);
    let view = h.idle().await;
    assert_eq!(view.model.as_deref(), Some("Elegoo Centauri Carbon"));
    let status = &view.status;
    assert_eq!(status.temperatures.nozzle.unwrap().current, 24.6);
    assert_eq!(status.temperatures.bed.unwrap().current, 23.9);
    assert_eq!(status.temperatures.chamber.unwrap().current, 25.1);
    assert_eq!(status.fans.len(), 3);
    assert_eq!(status.speed_profile.as_deref(), Some("standard"));
    for capability in [
        Capability::Pause,
        Capability::Resume,
        Capability::Cancel,
        Capability::UploadAndStart,
        Capability::NozzleTemperature,
        Capability::BedTemperature,
        Capability::Fans,
        Capability::ChamberLight,
        Capability::SpeedProfile,
    ] {
        assert!(status.has(capability), "{capability:?}");
    }
    assert!(!status.has(Capability::Homing) && !status.has(Capability::Jogging));

    // The camera is the stream Cmd 386 names, once the MainboardID is known.
    let view = h
        .until(20, "the camera", |v| v.status.has(Capability::Camera))
        .await;
    assert!(view.status.has(Capability::Camera));
    let source = h.fleet.handle("e").unwrap().camera().unwrap();
    let frames: Vec<_> =
        tokio::time::timeout(Duration::from_secs(10), source.open().take(2).collect())
            .await
            .unwrap();
    assert!(
        frames
            .iter()
            .all(|f| f.as_ref().is_ok_and(|f| f[..] == JPEG[..]))
    );
    let video = mock.commands().into_iter().find(|c| c.cmd == 386).unwrap();
    assert_eq!(video.data, json!({ "Enable": 1 }));
    assert_eq!(video.mainboard, MAINBOARD, "addressed to the learned id");

    // Fine controls are Cmd 403.
    for control in [
        FineControl::SetTemperature {
            heater: Heater::Nozzle,
            target: 215.0,
        },
        FineControl::SetTemperature {
            heater: Heater::Bed,
            target: 60.0,
        },
        FineControl::SetFan {
            fan: "part".into(),
            percent: 80,
        },
        FineControl::SetLight {
            light: LightId::Chamber,
            on: true,
        },
        FineControl::SetSpeedProfile {
            profile: "sport".into(),
        },
    ] {
        h.fleet.command("e", Command::Fine(control)).await.unwrap();
    }
    let view = h
        .until(10, "the fine controls applied", |v| {
            v.status.temperatures.nozzle.and_then(|t| t.target) == Some(215.0)
                && v.status.temperatures.bed.and_then(|t| t.target) == Some(60.0)
                && v.status.lights.first().is_some_and(|l| l.on)
                && v.status.speed_profile.as_deref() == Some("sport")
        })
        .await;
    assert_eq!(view.status.fans[0].percent, 80);
    let controls: Vec<_> = mock
        .commands()
        .into_iter()
        .filter(|c| c.cmd == 403)
        .map(|c| c.data)
        .collect();
    assert_eq!(
        controls,
        [
            json!({ "TempTargetNozzle": 215 }),
            json!({ "TempTargetHotbed": 60 }),
            json!({ "TargetFanSpeed": { "ModelFan": 80 } }),
            json!({ "LightStatus": { "SecondLight": true } }),
            json!({ "PrintSpeedPct": 130 }),
        ]
    );
    assert!(
        h.fleet.command("e", Command::Pause).await.is_err(),
        "nothing to pause"
    );
    h.fleet.stop_all().await;
}

#[tokio::test]
async fn a_print_from_the_screen_runs_to_a_succeeded_job() {
    let mock = MockSdcp::start().await;
    let h = on(&mock);
    h.idle().await;
    let task = mock.print_from_screen("benchy.gcode");
    let view = h.state(PrinterState::Preparing).await;
    let job = view.job.expect("an External Job");
    assert!(job.external);
    assert_eq!(job.driver_key, task, "keyed by the TaskId");
    mock.print_layer(4);
    let view = h.state(PrinterState::Printing).await;
    let active = view.status.job.unwrap();
    assert_eq!(active.print_file.as_deref(), Some("benchy.gcode"));
    assert_eq!((active.layer, active.total_layers), (Some(4), Some(10)));
    assert_eq!(active.progress, 40.0);
    assert_eq!(active.remaining_secs, Some(360));
    mock.complete();
    let view = h.state(PrinterState::AwaitingClear).await;
    let ended = view.awaiting_clear.unwrap();
    assert_eq!(ended.outcome, Some(JobOutcome::Succeeded));
    assert_eq!(ended.driver_key, task);
    assert_eq!(h.log.jobs().len(), 1);
    h.fleet.stop_all().await;
}

#[tokio::test]
async fn pause_resume_and_cancel_are_the_right_commands() {
    let mock = MockSdcp::start().await;
    let h = on(&mock);
    h.idle().await;
    mock.print_from_screen("cube.gcode");
    mock.print_layer(2);
    h.state(PrinterState::Printing).await;

    h.fleet.command("e", Command::Pause).await.unwrap();
    h.state(PrinterState::Paused).await;
    h.fleet.command("e", Command::Resume).await.unwrap();
    h.state(PrinterState::Printing).await;
    h.fleet.command("e", Command::Cancel).await.unwrap();
    let view = h.state(PrinterState::AwaitingClear).await;
    assert_eq!(
        view.awaiting_clear.unwrap().outcome,
        Some(JobOutcome::Cancelled)
    );
    let controls: Vec<_> = mock
        .commands()
        .into_iter()
        .filter(|c| (129..=131).contains(&c.cmd))
        .collect();
    let cmds: Vec<_> = controls.iter().map(|c| c.cmd).collect();
    assert_eq!(cmds, [129, 131, 130], "pause, resume, then Cancel (stop)");
    assert!(controls.iter().all(|c| c.mainboard == MAINBOARD));
    h.fleet.mark_clear("e", false).await.unwrap();
    h.idle().await;
    h.fleet.stop_all().await;
}

#[tokio::test]
async fn a_print_the_printer_says_failed_is_failed() {
    let mock = MockSdcp::start().await;
    let h = on(&mock);
    h.idle().await;
    mock.print_from_screen("bad.gcode");
    h.state(PrinterState::Preparing).await;
    mock.update(|s| {
        s["CurrentStatus"] = json!([0]);
        s["PrintInfo"]["Status"] = json!(8);
        s["PrintInfo"]["ErrorNumber"] = json!(2);
    });
    let view = h.state(PrinterState::AwaitingClear).await;
    assert_eq!(
        view.awaiting_clear.unwrap().outcome,
        Some(JobOutcome::Failed)
    );
    assert_eq!(view.status.faults[0].code, "sdcp-error-2");
    h.fleet.stop_all().await;
}

/// Bytes that aren't the same from one MiB to the next.
fn gcode(len: usize) -> Vec<u8> {
    let mut out = b"; generated by Gantry's tests\nG28\n".to_vec();
    let mut n = 0u64;
    while out.len() < len {
        out.extend_from_slice(format!("G1 X{} Y{} E0.1\n", n % 220, (n * 7) % 220).as_bytes());
        n += 1;
    }
    out.truncate(len);
    out
}

#[tokio::test]
async fn upload_and_start_sends_the_file_in_chunks_then_cmd_128() {
    let mock = MockSdcp::start().await;
    let h = on(&mock);
    h.idle().await;
    let data = gcode(2 * 1024 * 1024 + 4321);
    let job = h
        .fleet
        .start_job(
            "e",
            JobStart {
                print_file_id: 7,
                print_file: "gantry-upload.gcode".into(),
                plate: 1,
            },
            PrintUpload {
                file_name: "gantry-upload.gcode".into(),
                plate: None,
                data: data.clone().into(),
                estimated_secs: None,
                layers: None,
            },
        )
        .await
        .unwrap();

    {
        let printer = mock.printer();
        let (stored, md5) = printer.files.get("gantry-upload.gcode").expect("uploaded");
        assert_eq!(stored, &data, "the same bytes");
        assert_eq!(md5, &md5_hex(&data));
        let chunks = &printer.chunks;
        assert_eq!(chunks.len(), 3, "1 MiB chunks");
        let offsets: Vec<_> = chunks.iter().map(|c| c.offset).collect();
        assert_eq!(offsets, [0, 1024 * 1024, 2 * 1024 * 1024]);
        assert!(chunks.iter().all(|c| c.total == data.len()
            && c.md5 == md5_hex(&data)
            && c.check == "1"
            && c.uuid == chunks[0].uuid
            && c.filename == "gantry-upload.gcode"));
    }
    let start = mock
        .commands()
        .into_iter()
        .find(|c| c.cmd == 128)
        .expect("Cmd 128");
    assert_eq!(
        start.data,
        json!({ "Filename": "gantry-upload.gcode", "StartLayer": 0 })
    );
    assert_eq!(start.mainboard, MAINBOARD);

    let view = h
        .until(20, "the uploaded print", |v| {
            v.status.job.is_some() && v.job.as_ref().is_some_and(|j| !j.driver_key.is_empty())
        })
        .await;
    let recorded = view.job.unwrap();
    assert_eq!(recorded.id, job.id, "the same Job, now keyed");
    assert!(!recorded.external);
    mock.print_layer(10);
    mock.complete();
    let view = h.state(PrinterState::AwaitingClear).await;
    let ended = view.awaiting_clear.unwrap();
    assert_eq!(ended.id, job.id);
    assert_eq!(ended.outcome, Some(JobOutcome::Succeeded));

    // Not Idle: refused, and nothing is sent.
    let before = mock.printer().chunks.len();
    assert!(
        h.fleet
            .start_job(
                "e",
                JobStart {
                    print_file_id: 7,
                    print_file: "gantry-upload.gcode".into(),
                    plate: 1,
                },
                PrintUpload {
                    file_name: "gantry-upload.gcode".into(),
                    plate: None,
                    data: data.into(),
                    estimated_secs: None,
                    layers: None,
                },
            )
            .await
            .is_err()
    );
    assert_eq!(mock.printer().chunks.len(), before);
    h.fleet.stop_all().await;
}

#[tokio::test]
async fn nothing_listening_is_unreachable() {
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let h = harness(json!({ "host": "127.0.0.1", "port": port }));
    h.until(20, "unreachable", |v| {
        matches!(
            v.status.connection,
            Connection::Disconnected {
                reason: DisconnectReason::Unreachable,
                ..
            }
        )
    })
    .await;
    assert!(h.fleet.command("e", Command::Pause).await.is_err());
    h.fleet.stop_all().await;
}

#[tokio::test]
async fn it_reconnects_after_the_printer_restarts() {
    let mock = MockSdcp::start().await;
    let h = on(&mock);
    h.idle().await;
    mock.print_from_screen("long.gcode");
    mock.print_layer(1);
    let running = h.state(PrinterState::Printing).await.job.unwrap();

    mock.stop().await;
    h.until(20, "the Connection lost", |v| {
        matches!(
            v.status.connection,
            Connection::Disconnected {
                reason: DisconnectReason::Unreachable,
                ..
            }
        )
    })
    .await;
    // The print carries on while Gantry can't see it.
    mock.print_layer(5);
    mock.restart().await;
    let view = h
        .until(30, "connected and printing again", |v| {
            v.status.connection.is_connected()
                && v.status.job.as_ref().is_some_and(|j| j.layer == Some(5))
        })
        .await;
    assert_eq!(view.job.unwrap().id, running.id, "the same Job");
    eventually("Cmd 0 asked again", || {
        mock.cmds().iter().filter(|c| **c == 0).count() >= 2
    })
    .await;
    mock.complete();
    let view = h.state(PrinterState::AwaitingClear).await;
    assert_eq!(
        view.awaiting_clear.unwrap().outcome,
        Some(JobOutcome::Succeeded)
    );
    h.fleet.stop_all().await;
}
