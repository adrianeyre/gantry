//! The Queue through the API, with Simulated Printers (#49).
use std::sync::Arc;

use axum::http::StatusCode;
use gantry_core::{Fleet, PrinterState};
use gantry_print_file_fixtures as fixtures;
use serde_json::{Value, json};

use crate::store::Store;
use crate::store::users::Role;
use crate::test_support::{Client, drivers};
use crate::{AppState, Options};

async fn upload_tray(client: &Client) -> Value {
    let (status, file) = client
        .upload(
            "/api/print-files?name=tray.gcode.3mf",
            &fixtures::bambu_3mf(&fixtures::three_plates()),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{file}");
    file
}

async fn auto_start(client: &Client, printer: &str, on: bool) {
    let (status, body) = client
        .put(
            &format!("/api/printers/{printer}/automation"),
            json!({ "auto_start": on }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

fn plates(queue: &Value) -> Vec<u64> {
    queue
        .as_array()
        .unwrap()
        .iter()
        .map(|j| j["plate"].as_u64().unwrap())
        .collect()
}

#[tokio::test]
async fn queueing_a_multi_plate_print_file_creates_one_job_per_plate() {
    let client = Client::new().await;
    let printer = client
        .connected_simulated("X1", json!({ "model": "X1C" }))
        .await;
    let file = upload_tray(&client).await;
    let operator = client.as_role(Role::Operator).await;
    let (status, added) = operator
        .post(
            "/api/queue",
            json!({ "printer_id": printer, "print_file_id": file["id"] }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{added}");
    assert_eq!(plates(&added), [1, 2, 3]);

    // First in, first out: the first Plate is the one suggested.
    let (_, queue) = client.get("/api/queue").await;
    assert_eq!(plates(&queue), [1, 2, 3]);
    let suggested: Vec<bool> = queue
        .as_array()
        .unwrap()
        .iter()
        .map(|j| j["suggested"].as_bool().unwrap())
        .collect();
    assert_eq!(suggested, [true, false, false]);
    assert_eq!(queue[0]["plate_name"], "Cubes");
    assert_eq!(queue[0]["estimated_secs"], 1200);

    // A Viewer can look but not queue.
    let viewer = client.as_role(Role::Viewer).await;
    let (status, _) = viewer.get("/api/queue").await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = viewer
        .post(
            "/api/queue",
            json!({ "printer_id": printer, "print_file_id": file["id"] }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn the_wrong_model_is_never_queued() {
    let client = Client::new().await;
    let printer = client
        .connected_simulated("P1", json!({ "model": "P1S" }))
        .await;
    let file = upload_tray(&client).await;
    let (status, body) = client
        .post(
            "/api/queue",
            json!({ "printer_id": printer, "print_file_id": file["id"] }),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let (_, queue) = client.get("/api/queue").await;
    assert!(queue.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn starting_the_suggested_job_runs_it_and_takes_it_off_the_queue() {
    let client = Client::new().await;
    let printer = client
        .connected_simulated("X1", json!({ "model": "X1C" }))
        .await;
    client
        .simulate(&printer, json!({ "action": "set_speed", "speed": 1000 }))
        .await;
    // Suggested only: with Auto-start on it would be started.
    auto_start(&client, &printer, false).await;
    let file = upload_tray(&client).await;
    client
        .post(
            "/api/queue",
            json!({ "printer_id": printer, "print_file_id": file["id"], "plates": [3, 1] }),
        )
        .await;
    let (_, queue) = client.get("/api/queue").await;
    let first = queue[0]["id"].clone();
    let (status, job) = client
        .post(&format!("/api/queue/{first}/start"), json!({}))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{job}");
    assert_eq!(job["plate"], 3);
    assert_eq!(job["print_file_id"], file["id"]);
    let (_, queue) = client.get("/api/queue").await;
    assert_eq!(plates(&queue), [1]);

    // It finishes, and the Printer is Awaiting Clear: nothing is suggested,
    // and the next Job won't start.
    client
        .until(&printer, |v| {
            v.status.state == Some(PrinterState::AwaitingClear)
        })
        .await;
    let (_, queue) = client.get("/api/queue").await;
    assert_eq!(queue[0]["suggested"], false, "{queue}");
    let next = queue[0]["id"].clone();
    let (status, body) = client
        .post(&format!("/api/queue/{next}/start"), json!({}))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body["error"].as_str().unwrap().contains("Awaiting Clear"));
    let (_, queue) = client.get("/api/queue").await;
    assert_eq!(plates(&queue), [1], "still queued");

    // After Mark Clear it is suggested again.
    client
        .post(&format!("/api/printers/{printer}/mark-clear"), json!({}))
        .await;
    client
        .until(&printer, |v| v.status.state == Some(PrinterState::Idle))
        .await;
    let (_, queue) = client.get("/api/queue").await;
    assert_eq!(queue[0]["suggested"], true, "{queue}");
}

#[tokio::test]
async fn a_printer_awaiting_clear_is_never_suggested_a_job() {
    let client = Client::new().await;
    let printer = client
        .connected_simulated("X1", json!({ "model": "X1C" }))
        .await;
    let file = upload_tray(&client).await;
    // An External Job ends, so the Printer is Awaiting Clear.
    client
        .simulate(
            &printer,
            json!({ "action": "start_print", "duration_secs": 0.1, "layers": 1 }),
        )
        .await;
    client
        .simulate(&printer, json!({ "action": "set_speed", "speed": 1000 }))
        .await;
    client
        .until(&printer, |v| {
            v.status.state == Some(PrinterState::AwaitingClear)
        })
        .await;
    client
        .post(
            "/api/queue",
            json!({ "printer_id": printer, "print_file_id": file["id"] }),
        )
        .await;
    let (_, queue) = client.get("/api/queue").await;
    assert!(
        queue
            .as_array()
            .unwrap()
            .iter()
            .all(|j| j["suggested"] == false),
        "{queue}"
    );
}

async fn on_disk(path: &std::path::Path) -> Client {
    let store = Store::open(path).await.unwrap();
    let key = crate::crypto::SecretKey::from_config("a secret key for tests only").unwrap();
    let log = Arc::new(crate::store::jobs::SqlJobLog(store.clone()));
    let state = AppState::with_options(
        store,
        key,
        Fleet::new(drivers(), log),
        Options {
            data_dir: Some(path.parent().unwrap().to_owned()),
            ..Options::default()
        },
    );
    state.start_printers().await.unwrap();
    Client::with_state(state).await
}

#[tokio::test]
async fn a_reordered_queue_stays_reordered_after_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gantry.db");
    let ids = {
        let client = on_disk(&path).await;
        let printer = client
            .connected_simulated("X1", json!({ "model": "X1C" }))
            .await;
        let file = upload_tray(&client).await;
        client
            .post(
                "/api/queue",
                json!({ "printer_id": printer, "print_file_id": file["id"] }),
            )
            .await;
        let (_, queue) = client.get("/api/queue").await;
        let ids: Vec<Value> = queue
            .as_array()
            .unwrap()
            .iter()
            .map(|j| j["id"].clone())
            .collect();
        // Plate 3 to the front, then 1, then 2.
        let order = json!([ids[2], ids[0], ids[1]]);
        let (status, queue) = client
            .put("/api/queue/order", json!({ "ids": order }))
            .await;
        assert_eq!(status, StatusCode::OK, "{queue}");
        assert_eq!(plates(&queue), [3, 1, 2]);
        // An order that leaves one out is refused.
        let (status, _) = client
            .put("/api/queue/order", json!({ "ids": [ids[0], ids[1]] }))
            .await;
        assert_eq!(status, StatusCode::CONFLICT);
        client.state.fleet().stop_all().await;
        client.state.store().pool().close().await;
        ids
    };
    let client = on_disk(&path).await;
    let (_, queue) = client.get("/api/queue").await;
    assert_eq!(plates(&queue), [3, 1, 2], "{queue}");
    assert_eq!(queue[0]["id"], ids[2]);

    // Removing one keeps the rest in order.
    let (status, _) = client.delete(&format!("/api/queue/{}", ids[0])).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, queue) = client.get("/api/queue").await;
    assert_eq!(plates(&queue), [3, 2]);
}

/// Waits (up to 5 s) for the Queue to satisfy `pred`.
async fn queue_until(client: &Client, pred: impl Fn(&Value) -> bool) -> Value {
    for _ in 0..100 {
        let (_, queue) = client.get("/api/queue").await;
        if pred(&queue) {
            return queue;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!(
        "the Queue never got there: {}",
        client.get("/api/queue").await.1
    );
}

#[tokio::test]
async fn a_job_for_any_compatible_printer_goes_to_one_of_the_right_model() {
    let client = Client::new().await;
    let x1c = client
        .connected_simulated("A X1C", json!({ "model": "X1C" }))
        .await;
    let p1s = client
        .connected_simulated("B P1S", json!({ "model": "P1S" }))
        .await;
    let file = upload_tray(&client).await;
    let (status, added) = client
        .post(
            "/api/queue",
            json!({ "print_file_id": file["id"], "plates": [1, 3] }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{added}");
    assert!(added[0]["printer_id"].is_null(), "{added}");
    let (_, queue) = client.get("/api/queue").await;
    // One free X1C, so Plate 1 takes it and Plate 3 waits. The P1S gets
    // neither.
    assert_eq!(queue[0]["assigned_printer"], x1c.as_str(), "{queue}");
    assert_eq!(queue[0]["suggested"], true);
    assert!(queue[1]["assigned_printer"].is_null(), "{queue}");
    let waiting = queue[1]["waiting"].as_str().unwrap();
    assert!(waiting.contains("sliced for"), "{waiting}");
    // The Simulated Printer's white PLA is #F2F2F2, not #FFFFFF: a warning.
    assert!(
        queue[0]["warnings"][0]
            .as_str()
            .unwrap()
            .contains("#FFFFFF"),
        "{queue}"
    );
    // Started on the wrong model by hand: refused.
    let id = queue[0]["id"].clone();
    let (status, body) = client
        .post(
            &format!("/api/queue/{id}/start"),
            json!({ "printer_id": p1s }),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("sliced for"),
        "{body}"
    );
    // Started where the Queue chose.
    let (status, job) = client
        .post(&format!("/api/queue/{id}/start"), json!({}))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{job}");
    assert_eq!(job["printer_id"], x1c.as_str());
}

#[tokio::test]
async fn a_print_file_without_a_model_cant_be_queued_for_any_printer() {
    let client = Client::new().await;
    let (status, file) = client
        .upload(
            "/api/print-files?name=benchy.gcode",
            &fixtures::cura_gcode(),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{file}");
    let (status, file) = client
        .put(
            &format!("/api/print-files/{}", file["id"]),
            json!({ "printer_model": null }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{file}");
    assert!(file["printer_model"].is_null(), "{file}");
    let (status, body) = client
        .post("/api/queue", json!({ "print_file_id": file["id"] }))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
}

#[tokio::test]
async fn the_next_job_that_can_run_goes_ahead_of_one_that_cant() {
    let client = Client::new().await;
    let printer = client
        .connected_simulated("X1", json!({ "model": "X1C" }))
        .await;
    // Its PETG slot now holds PLA, so Plate 2, which needs PETG, can't run.
    client
        .simulate(
            &printer,
            json!({ "action": "set_filament_slot", "slot": "2",
                    "filament": { "kind": "PLA", "colour": "#D33A2C" } }),
        )
        .await;
    client
        .until(&printer, |v| {
            v.status.filament_slots[2]
                .filament
                .as_ref()
                .is_some_and(|f| f.kind == "PLA")
        })
        .await;
    let file = upload_tray(&client).await;
    client
        .post(
            "/api/queue",
            json!({ "print_file_id": file["id"], "plates": [2, 3] }),
        )
        .await;
    let (_, queue) = client.get("/api/queue").await;
    assert_eq!(plates(&queue), [2, 3]);
    assert_eq!(queue[0]["suggested"], false, "{queue}");
    assert_eq!(queue[0]["waiting"], "X1 has no PETG loaded");
    assert_eq!(queue[1]["suggested"], true, "{queue}");
    assert_eq!(queue[1]["assigned_printer"], printer.as_str());
}

#[tokio::test]
async fn with_auto_start_mark_clear_starts_the_next_job_and_audits_it() {
    let client = Client::new().await;
    let printer = client
        .connected_simulated("X1", json!({ "model": "X1C" }))
        .await;
    client
        .simulate(&printer, json!({ "action": "set_speed", "speed": 1000 }))
        .await;
    let (_, record) = client.get(&format!("/api/printers/{printer}")).await;
    assert_eq!(record["auto_start"], true, "on by default: {record}");
    let file = upload_tray(&client).await;
    client
        .post(
            "/api/queue",
            json!({ "printer_id": printer, "print_file_id": file["id"], "plates": [3, 1] }),
        )
        .await;
    let (_, queue) = client.get("/api/queue").await;
    let first = queue[0]["id"].clone();
    client
        .post(&format!("/api/queue/{first}/start"), json!({}))
        .await;
    client
        .until(&printer, |v| {
            v.status.state == Some(PrinterState::AwaitingClear)
        })
        .await;
    // While it is Awaiting Clear, Auto-start waits.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let (_, queue) = client.get("/api/queue").await;
    assert_eq!(plates(&queue), [1], "{queue}");

    let operator = client.as_role(Role::Operator).await;
    let (status, _) = operator
        .post(&format!("/api/printers/{printer}/mark-clear"), json!({}))
        .await;
    assert_eq!(status, StatusCode::OK);
    let view = client
        .until(&printer, |v| {
            v.job.as_ref().is_some_and(|j| j.plate == Some(1))
        })
        .await;
    assert_eq!(view.job.unwrap().print_file_id, file["id"].as_i64());
    queue_until(&client, |q| q.as_array().unwrap().is_empty()).await;

    let (_, audit) = client.get("/api/audit?action=job.started").await;
    let auto: Vec<&Value> = audit["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["detail"]["auto_start"] == true)
        .collect();
    assert_eq!(auto.len(), 1, "{audit}");
    assert!(auto[0]["username"].is_null(), "Gantry did it: {}", auto[0]);
    assert_eq!(auto[0]["result"], "ok");
    assert_eq!(auto[0]["printer_id"], printer.as_str());
    assert_eq!(auto[0]["detail"]["plate"], 1);
}

#[tokio::test]
async fn with_auto_start_off_the_next_job_is_only_suggested() {
    let client = Client::new().await;
    let printer = client
        .connected_simulated("X1", json!({ "model": "X1C" }))
        .await;
    auto_start(&client, &printer, false).await;
    client
        .simulate(&printer, json!({ "action": "set_speed", "speed": 1000 }))
        .await;
    let file = upload_tray(&client).await;
    client
        .post(
            "/api/queue",
            json!({ "printer_id": printer, "print_file_id": file["id"], "plates": [3, 1] }),
        )
        .await;
    let (_, queue) = client.get("/api/queue").await;
    client
        .post(&format!("/api/queue/{}/start", queue[0]["id"]), json!({}))
        .await;
    client
        .until(&printer, |v| {
            v.status.state == Some(PrinterState::AwaitingClear)
        })
        .await;
    client
        .post(&format!("/api/printers/{printer}/mark-clear"), json!({}))
        .await;
    client
        .until(&printer, |v| v.status.state == Some(PrinterState::Idle))
        .await;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let view = client.state.fleet().view(&printer).unwrap();
    assert!(view.job.is_none(), "nothing started: {:?}", view.job);
    let (_, queue) = client.get("/api/queue").await;
    assert_eq!(plates(&queue), [1]);
    assert_eq!(queue[0]["suggested"], true, "{queue}");
}

#[tokio::test]
async fn auto_clear_skips_awaiting_clear_after_a_success_only() {
    let client = Client::new().await;
    let printer = client.connected_simulated("Ejector", json!({})).await;
    let (_, record) = client.get(&format!("/api/printers/{printer}")).await;
    assert_eq!(record["auto_clear"], false, "off by default: {record}");
    // Only an Admin changes it.
    let operator = client.as_role(Role::Operator).await;
    let (status, _) = operator
        .put(
            &format!("/api/printers/{printer}/automation"),
            json!({ "auto_clear": true }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = client
        .put(
            &format!("/api/printers/{printer}/automation"),
            json!({ "auto_clear": true }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, json!({ "auto_start": true, "auto_clear": true }));
    client.until(&printer, |v| v.auto_clear).await;

    client
        .simulate(&printer, json!({ "action": "set_speed", "speed": 1000 }))
        .await;
    client
        .simulate(
            &printer,
            json!({ "action": "start_print", "duration_secs": 1, "layers": 1 }),
        )
        .await;
    let view = client
        .until(&printer, |v| {
            v.last_ended.is_some() && v.status.state == Some(PrinterState::Idle)
        })
        .await;
    assert!(view.awaiting_clear.is_none());
    assert_eq!(
        view.last_ended.unwrap().outcome,
        Some(gantry_core::JobOutcome::Succeeded)
    );

    // A failed Job still needs Mark Clear.
    client
        .simulate(
            &printer,
            json!({ "action": "start_print", "duration_secs": 600, "layers": 10 }),
        )
        .await;
    client.until(&printer, |v| v.job.is_some()).await;
    client
        .simulate(&printer, json!({ "action": "fail_print" }))
        .await;
    let view = client
        .until(&printer, |v| {
            v.status.state == Some(PrinterState::AwaitingClear)
        })
        .await;
    assert_eq!(
        view.awaiting_clear.unwrap().outcome,
        Some(gantry_core::JobOutcome::Failed)
    );
}

#[tokio::test]
async fn with_auto_clear_and_auto_start_the_queue_runs_itself() {
    let client = Client::new().await;
    let printer = client
        .connected_simulated("Farm", json!({ "model": "X1C" }))
        .await;
    client
        .put(
            &format!("/api/printers/{printer}/automation"),
            json!({ "auto_clear": true }),
        )
        .await;
    client.until(&printer, |v| v.auto_clear).await;
    client
        .simulate(&printer, json!({ "action": "set_speed", "speed": 1000 }))
        .await;
    let file = upload_tray(&client).await;
    client
        .post(
            "/api/queue",
            json!({ "printer_id": printer, "print_file_id": file["id"], "plates": [3, 1] }),
        )
        .await;
    let (_, queue) = client.get("/api/queue").await;
    client
        .post(&format!("/api/queue/{}/start", queue[0]["id"]), json!({}))
        .await;
    // Plate 3 succeeds, Auto-Clear clears it, and Auto-start starts Plate 1.
    queue_until(&client, |q| q.as_array().unwrap().is_empty()).await;
    let jobs = client
        .state
        .store()
        .printer_jobs(&printer, 10)
        .await
        .unwrap();
    let plates: Vec<_> = jobs.iter().rev().map(|j| j.plate).collect();
    assert_eq!(plates, [Some(3), Some(1)], "{jobs:?}");
}
