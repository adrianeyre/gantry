//! Timelapse capture end to end (#69), from the Simulated Printer's test
//! camera: off by default, set per Printer, attached to the Job in History,
//! and cleaned up by size without touching History.
use std::time::Duration;

use axum::http::StatusCode;
use http_body_util::BodyExt;
use serde_json::{Value, json};

use crate::store::users::Role;
use crate::test_support::Client;

async fn print(client: &Client, printer: &str, secs: u64, layers: u32) -> i64 {
    client
        .simulate(
            printer,
            json!({ "action": "start_print", "print_file": "cube.gcode", "duration_secs": secs, "layers": layers }),
        )
        .await;
    let view = client.until(printer, |v| v.job.is_some()).await;
    let job = view.job.unwrap().id;
    // It runs to its end on its own.
    let printer = printer.to_owned();
    for _ in 0..200 {
        let view = client.state.fleet().view(&printer).unwrap();
        if view.job.is_none() {
            return job;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the simulated print didn't end");
}

async fn timelapse(client: &Client, job: i64) -> Value {
    for _ in 0..100 {
        let (status, body) = client.get(&format!("/api/jobs/{job}/timelapse")).await;
        if status == StatusCode::OK && body["finished"] == true {
            return body;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("Job {job} has no finished timelapse");
}

#[tokio::test]
async fn timelapses_are_off_by_default() {
    let client = Client::new().await;
    let printer = client.connected_simulated("X1", json!({})).await;
    let (status, setting) = client
        .get(&format!("/api/printers/{printer}/timelapse"))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(setting, json!({ "mode": "off", "interval_secs": 30 }));
    let job = print(&client, &printer, 2, 4).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (status, _) = client.get(&format!("/api/jobs/{job}/timelapse")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(client.state.store().timelapses().await.unwrap().is_empty());
}

#[tokio::test]
async fn one_frame_per_layer_from_the_simulated_camera_attached_to_the_job() {
    let client = Client::new().await;
    let printer = client.connected_simulated("X1", json!({})).await;
    let other = client.connected_simulated("P1", json!({})).await;
    let (status, setting) = client
        .put(
            &format!("/api/printers/{printer}/timelapse"),
            json!({ "mode": "layer" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{setting}");
    assert_eq!(setting["mode"], "layer");

    let job = print(&client, &printer, 3, 4).await;
    let untouched = print(&client, &other, 1, 2).await;
    let timelapse = timelapse(&client, job).await;
    let frames = timelapse["frames"].as_u64().unwrap();
    assert!((2..=5).contains(&frames), "one per layer: {timelapse}");
    assert!(timelapse["bytes"].as_u64().unwrap() > 0);

    // Each frame is a JPEG from the camera.
    let response = client
        .raw_get(&format!("/api/jobs/{job}/timelapse/frames/1"))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "image/jpeg");
    let jpeg = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
    let (status, _) = client
        .get(&format!("/api/jobs/{job}/timelapse/frames/{}", frames + 1))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // In History, beside its Job; a Printer with timelapses off has none.
    let (_, history) = client
        .get(&format!("/api/printers/{printer}/history"))
        .await;
    assert_eq!(history["jobs"][0]["id"], job);
    assert_eq!(history["jobs"][0]["timelapse_frames"], frames);
    let (status, _) = client
        .get(&format!("/api/jobs/{untouched}/timelapse"))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // The camera was only pulled for each frame.
    let simulated = client
        .state
        .fleet()
        .handle(&printer)
        .unwrap()
        .as_any()
        .downcast_ref::<gantry_driver_simulated::SimulatedPrinter>()
        .map(gantry_driver_simulated::SimulatedPrinter::open_camera_streams);
    for _ in 0..40 {
        if client.state.cameras().active() == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(client.state.cameras().active(), 0, "{simulated:?}");
}

#[tokio::test]
async fn a_timer_takes_frames_without_waiting_for_layers() {
    let client = Client::new().await;
    let printer = client.connected_simulated("X1", json!({})).await;
    let (status, _) = client
        .put(
            &format!("/api/printers/{printer}/timelapse"),
            json!({ "mode": "interval", "interval_secs": 5 }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    // One layer only, so any frames came from the timer.
    let job = print(&client, &printer, 2, 1).await;
    let timelapse = timelapse(&client, job).await;
    assert!(timelapse["frames"].as_u64().unwrap() >= 1, "{timelapse}");

    for (body, field) in [
        (
            json!({ "mode": "interval", "interval_secs": 1 }),
            "interval_secs",
        ),
        (
            json!({ "mode": "interval", "interval_secs": 4000 }),
            "interval_secs",
        ),
    ] {
        let (status, error) = client
            .put(&format!("/api/printers/{printer}/timelapse"), body)
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(error["fields"][0]["field"], field);
    }
    let (status, _) = client
        .put(
            &format!("/api/printers/{printer}/timelapse"),
            json!({ "mode": "sometimes" }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn timelapses_are_cleaned_up_by_size_without_touching_job_history() {
    let client = Client::new().await;
    let printer = client.connected_simulated("X1", json!({})).await;
    client
        .put(
            &format!("/api/printers/{printer}/timelapse"),
            json!({ "mode": "layer" }),
        )
        .await;
    let first = print(&client, &printer, 1, 2).await;
    timelapse(&client, first).await;
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let second = print(&client, &printer, 1, 2).await;
    timelapse(&client, second).await;
    let (_, before) = client
        .get(&format!("/api/printers/{printer}/history"))
        .await;

    // Pretend each is 600 KB, and allow 1 MB: the older one must go.
    sqlx::query("UPDATE timelapses SET bytes = 600000")
        .execute(client.state.store().pool())
        .await
        .unwrap();
    let (status, settings) = client
        .put("/api/settings", json!({ "timelapse_limit_mb": 1 }))
        .await;
    assert_eq!(status, StatusCode::OK, "{settings}");
    assert_eq!(settings["timelapse_limit_mb"], 1);

    let (status, _) = client.get(&format!("/api/jobs/{first}/timelapse")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(!client.state.timelapses().folder(first).exists());
    let (status, _) = client.get(&format!("/api/jobs/{second}/timelapse")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(client.state.timelapses().folder(second).exists());

    // History is exactly as it was, except that the first Job's timelapse
    // is gone.
    let (_, after) = client
        .get(&format!("/api/printers/{printer}/history"))
        .await;
    assert_eq!(after["summary"], before["summary"]);
    let jobs = after["jobs"].as_array().unwrap();
    assert_eq!(jobs.len(), 2);
    let first_job = jobs.iter().find(|j| j["id"] == first).unwrap();
    assert!(first_job.get("timelapse_frames").is_none());
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM jobs")
        .fetch_one(client.state.store().pool())
        .await
        .unwrap();
    assert_eq!(rows, 2);
    let (_, audit) = client.get("/api/audit").await;
    let removed = audit["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["action"] == "timelapse.removed")
        .unwrap();
    assert!(removed["username"].is_null(), "Gantry did it");
    assert_eq!(removed["target"], format!("Job {first}"));
}

#[tokio::test]
async fn admins_set_it_and_operators_remove_a_timelapse() {
    let client = Client::new().await;
    let operator = client.as_role(Role::Operator).await;
    let viewer = client.as_role(Role::Viewer).await;
    let printer = client.connected_simulated("X1", json!({})).await;
    let (status, _) = operator
        .put(
            &format!("/api/printers/{printer}/timelapse"),
            json!({ "mode": "layer" }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    client
        .put(
            &format!("/api/printers/{printer}/timelapse"),
            json!({ "mode": "layer" }),
        )
        .await;
    let job = print(&client, &printer, 1, 2).await;
    timelapse(&client, job).await;
    let (status, _) = viewer.get(&format!("/api/jobs/{job}/timelapse")).await;
    assert_eq!(status, StatusCode::OK, "Viewers see them");
    let (status, _) = viewer.delete(&format!("/api/jobs/{job}/timelapse")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = operator.delete(&format!("/api/jobs/{job}/timelapse")).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(!client.state.timelapses().folder(job).exists());
    let (_, history) = client
        .get(&format!("/api/printers/{printer}/history"))
        .await;
    assert_eq!(history["jobs"].as_array().unwrap().len(), 1);
}
