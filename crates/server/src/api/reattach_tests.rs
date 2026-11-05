//! Job re-attachment after Gantry restarts (PRD §7.5, §11), with a Simulated
//! Printer that, like a real one, keeps printing while Gantry is down.
use std::time::Duration;

use gantry_core::{JobOutcome, PrinterState, PrinterView};
use serde_json::json;

use crate::AppState;
use crate::config::Config;
use crate::test_support::{Client, drivers};

async fn until(state: &AppState, id: &str, pred: impl Fn(&PrinterView) -> bool) -> PrinterView {
    (*state
        .fleet()
        .wait_for(id, Duration::from_secs(10), pred)
        .await
        .unwrap())
    .clone()
}

fn config(dir: &tempfile::TempDir) -> Config {
    let data = dir.path().to_str().unwrap().to_owned();
    Config::load(None, move |k| match k {
        "GANTRY_DATA_DIR" => Some(data.clone()),
        "GANTRY_SECRET_KEY" => Some("a secret key for tests only".into()),
        _ => None,
    })
    .unwrap()
}

async fn stop(state: AppState) {
    state.fleet().stop_all().await;
    state.store().pool().close().await;
}

#[tokio::test]
async fn restarting_gantry_mid_print_re_attaches_the_job() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(&dir);

    let first = AppState::start(&config, drivers()).await.unwrap();
    let client = Client::with_state(first.clone()).await;
    let id = client.add_simulated("Long print", json!({})).await;
    until(&first, &id, |v| v.status.connection.is_connected()).await;
    client
        .post(
            &format!("/api/printers/{id}/simulation"),
            json!({ "action": "start_print", "print_file": "vase.gcode", "duration_secs": 8, "layers": 4 }),
        )
        .await;
    let job = until(&first, &id, |v| v.job.is_some()).await.job.unwrap();
    drop(client);
    stop(first).await;

    // Gantry comes back; the Printer is still printing the same thing.
    let second = AppState::start(&config, drivers()).await.unwrap();
    let view = until(&second, &id, |v| {
        v.status.connection.is_connected() && v.status.job.is_some()
    })
    .await;
    assert_eq!(view.job.as_ref().unwrap().id, job.id, "re-attached");
    // It finishes, and that one Job gets the Outcome.
    let view = until(&second, &id, |v| {
        v.status.state == Some(PrinterState::AwaitingClear)
    })
    .await;
    assert_eq!(view.awaiting_clear.unwrap().id, job.id);
    let jobs = second.store().printer_jobs(&id, 10).await.unwrap();
    assert_eq!(jobs.len(), 1, "no duplicate: {jobs:?}");
    assert_eq!(jobs[0].outcome, Some(JobOutcome::Succeeded));
    assert_eq!(jobs[0].print_file.as_deref(), Some("vase.gcode"));
    stop(second).await;
}

#[tokio::test]
async fn a_print_that_finished_while_gantry_was_down_keeps_its_outcome() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(&dir);

    let first = AppState::start(&config, drivers()).await.unwrap();
    let client = Client::with_state(first.clone()).await;
    let id = client.add_simulated("Short print", json!({})).await;
    until(&first, &id, |v| v.status.connection.is_connected()).await;
    client
        .post(
            &format!("/api/printers/{id}/simulation"),
            json!({ "action": "start_print", "duration_secs": 2, "layers": 2 }),
        )
        .await;
    let job = until(&first, &id, |v| v.job.is_some()).await.job.unwrap();
    drop(client);
    stop(first).await;
    // Down for longer than the print takes.
    tokio::time::sleep(Duration::from_secs(3)).await;

    let second = AppState::start(&config, drivers()).await.unwrap();
    let view = until(&second, &id, |v| {
        v.status.state == Some(PrinterState::AwaitingClear)
    })
    .await;
    let ended = view.awaiting_clear.unwrap();
    assert_eq!(ended.id, job.id);
    assert_eq!(ended.outcome, Some(JobOutcome::Succeeded), "not Lost");
    assert_eq!(second.store().printer_jobs(&id, 10).await.unwrap().len(), 1);
    stop(second).await;
}
