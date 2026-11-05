//! Jobs end to end through the API, with Simulated Printers.
use std::time::Duration;

use axum::http::StatusCode;
use gantry_core::{JobOutcome, PrinterState, PrinterView};
use gantry_driver_simulated::SimulatedPrinter;
use serde_json::{Value, json};

use crate::AppState;
use crate::config::Config;
use crate::test_support::{Client, drivers};

async fn connected(client: &Client, name: &str) -> String {
    let id = client.add_simulated(name, json!({})).await;
    until(client, &id, |v| v.status.connection.is_connected()).await;
    id
}

async fn until(client: &Client, id: &str, pred: impl Fn(&PrinterView) -> bool) -> PrinterView {
    (*client
        .state
        .fleet()
        .wait_for(id, Duration::from_secs(5), pred)
        .await
        .unwrap())
    .clone()
}

async fn simulate(client: &Client, id: &str, action: Value) {
    let (status, body) = client
        .post(&format!("/api/printers/{id}/simulation"), action)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
}

async fn jobs(client: &Client, id: &str) -> Vec<Value> {
    let (status, body) = client.get(&format!("/api/printers/{id}/jobs")).await;
    assert_eq!(status, StatusCode::OK);
    body.as_array().unwrap().clone()
}

fn sim(client: &Client, id: &str) -> SimulatedPrinter {
    client
        .state
        .fleet()
        .handle(id)
        .unwrap()
        .as_any()
        .downcast_ref::<SimulatedPrinter>()
        .unwrap()
        .clone()
}

fn start(duration: f64) -> Value {
    json!({ "action": "start_print", "print_file": "cube.gcode", "duration_secs": duration, "layers": 5 })
}

#[tokio::test]
async fn a_simulated_print_is_exactly_one_external_job() {
    let client = Client::new().await;
    let id = connected(&client, "Sim").await;
    simulate(&client, &id, start(600.0)).await;
    let view = until(&client, &id, |v| v.job.is_some()).await;
    // Let several status updates go by.
    tokio::time::sleep(Duration::from_millis(800)).await;
    let jobs = jobs(&client, &id).await;
    assert_eq!(jobs.len(), 1, "{jobs:?}");
    assert_eq!(jobs[0]["external"], true);
    assert_eq!(jobs[0]["print_file"], "cube.gcode");
    assert_eq!(jobs[0]["outcome"], Value::Null);
    assert_eq!(view.job.unwrap().id, jobs[0]["id"].as_i64().unwrap());
}

#[tokio::test]
async fn each_ending_records_its_outcome_then_awaiting_clear() {
    let client = Client::new().await;

    // Succeeded: the print runs to the end.
    let done = connected(&client, "Finishes").await;
    simulate(
        &client,
        &done,
        json!({ "action": "set_speed", "speed": 100 }),
    )
    .await;
    simulate(&client, &done, start(20.0)).await;
    // Failed: it fails on the Printer.
    let fails = connected(&client, "Fails").await;
    simulate(&client, &fails, start(600.0)).await;
    until(&client, &fails, |v| v.job.is_some()).await;
    simulate(&client, &fails, json!({ "action": "fail_print" })).await;
    // Cancelled: Gantry cancels it.
    let cancelled = connected(&client, "Cancelled").await;
    simulate(&client, &cancelled, start(600.0)).await;
    until(&client, &cancelled, |v| v.job.is_some()).await;
    gantry_core::PrinterHandle::cancel(&sim(&client, &cancelled))
        .await
        .unwrap();

    for (id, outcome) in [
        (&done, JobOutcome::Succeeded),
        (&fails, JobOutcome::Failed),
        (&cancelled, JobOutcome::Cancelled),
    ] {
        let view = until(&client, id, |v| {
            v.status.state == Some(PrinterState::AwaitingClear)
        })
        .await;
        assert_eq!(view.awaiting_clear.as_ref().unwrap().outcome, Some(outcome));
        let jobs = jobs(&client, id).await;
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0]["outcome"], outcome.as_str());
        assert!(jobs[0]["ended_at"].is_string());
    }
}

#[tokio::test]
async fn jobs_and_awaiting_clear_survive_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().to_str().unwrap().to_owned();
    let config = Config::load(None, move |k| match k {
        "GANTRY_DATA_DIR" => Some(data.clone()),
        "GANTRY_SECRET_KEY" => Some("a secret key for tests only".into()),
        _ => None,
    })
    .unwrap();

    let first = AppState::start(&config, drivers()).await.unwrap();
    let client = Client::with_state(first.clone()).await;
    let id = connected(&client, "Restarts").await;
    simulate(&client, &id, start(600.0)).await;
    until(&client, &id, |v| v.job.is_some()).await;
    simulate(&client, &id, json!({ "action": "fail_print" })).await;
    until(&client, &id, |v| {
        v.status.state == Some(PrinterState::AwaitingClear)
    })
    .await;
    first.fleet().stop_all().await;
    first.store().pool().close().await;
    drop((client, first));

    let second = AppState::start(&config, drivers()).await.unwrap();
    let client = Client::with_state(second.clone()).await;
    let jobs = jobs(&client, &id).await;
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0]["outcome"], "failed");
    let view = until(&client, &id, |v| {
        v.status.state == Some(PrinterState::AwaitingClear)
    })
    .await;
    assert_eq!(
        view.awaiting_clear.unwrap().id,
        jobs[0]["id"].as_i64().unwrap()
    );
    second.fleet().stop_all().await;
}

#[tokio::test]
async fn an_unknown_printer_has_no_job_list() {
    let client = Client::new().await;
    assert_eq!(
        client.get("/api/printers/nope/jobs").await.0,
        StatusCode::NOT_FOUND
    );
}
