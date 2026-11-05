//! Prometheus metrics at `/metrics` (PRD §7.7, §9.2, §11), in the text
//! exposition format. Off by default. Turned on by a bearer token from the
//! config (`metrics_token`), which every scrape must bring, or by an address
//! of its own (`metrics_bind`), such as a port only the monitoring network
//! can reach. With an address of its own `/metrics` is not served beside the
//! dashboard at all; a token, if also set, is required there too.
//!
//! Every metric is documented in `docs/metrics.md`. Names follow the
//! Prometheus conventions: a `gantry_` prefix, base units (seconds,
//! celsius), `_total` for counters and `_ratio` for 0-to-1 values.
use std::fmt::Write as _;

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use gantry_core::{JobOutcome, PrinterState, PrinterView};
use sha2::{Digest, Sha256};

use crate::AppState;

/// The Content-Type of the text exposition format.
pub const CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

/// Every Printer State, as metrics name them.
const STATES: [(PrinterState, &str); 6] = [
    (PrinterState::Idle, "idle"),
    (PrinterState::Preparing, "preparing"),
    (PrinterState::Printing, "printing"),
    (PrinterState::Paused, "paused"),
    (PrinterState::AwaitingClear, "awaiting_clear"),
    (PrinterState::Busy, "busy"),
];

const OUTCOMES: [JobOutcome; 4] = [
    JobOutcome::Succeeded,
    JobOutcome::Failed,
    JobOutcome::Cancelled,
    JobOutcome::Lost,
];

/// A label value, escaped as the format requires.
fn escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

fn printer_labels(view: &PrinterView) -> String {
    format!(
        "printer=\"{}\",name=\"{}\"",
        escape(&view.id),
        escape(&view.name)
    )
}

/// One metric family's header.
fn family(out: &mut String, name: &str, kind: &str, help: &str) {
    let _ = writeln!(out, "# HELP {name} {help}");
    let _ = writeln!(out, "# TYPE {name} {kind}");
}

fn number(value: f64) -> String {
    if value.is_finite() {
        format!("{value}")
    } else {
        "NaN".to_owned()
    }
}

/// The exposition text for the Fleet now, and Job counts by Outcome.
pub fn render(
    views: &[std::sync::Arc<PrinterView>],
    jobs: &[(String, JobOutcome, i64)],
    queued: usize,
) -> String {
    let mut out = String::new();
    family(
        &mut out,
        "gantry_build_info",
        "gauge",
        "Always 1, labelled with Gantry's version.",
    );
    let _ = writeln!(
        out,
        "gantry_build_info{{version=\"{}\"}} 1",
        escape(gantry_core::VERSION)
    );

    family(
        &mut out,
        "gantry_printers",
        "gauge",
        "How many Printers the Fleet has.",
    );
    let _ = writeln!(out, "gantry_printers {}", views.len());

    family(
        &mut out,
        "gantry_printer_connected",
        "gauge",
        "1 when Gantry can talk to the Printer, 0 when it is connecting or disconnected.",
    );
    for v in views {
        let _ = writeln!(
            out,
            "gantry_printer_connected{{{},driver=\"{}\"}} {}",
            printer_labels(v),
            escape(&v.driver),
            u8::from(v.status.connection.is_connected())
        );
    }

    family(
        &mut out,
        "gantry_printer_state",
        "gauge",
        "1 for the Printer State the Printer is in, 0 for the others. All 0 while disconnected (unknown).",
    );
    for v in views {
        for (state, name) in STATES {
            let _ = writeln!(
                out,
                "gantry_printer_state{{{},state=\"{name}\"}} {}",
                printer_labels(v),
                u8::from(v.status.state == Some(state))
            );
        }
    }

    family(
        &mut out,
        "gantry_printer_temperature_celsius",
        "gauge",
        "A temperature the Printer reports, by sensor (nozzle, bed, chamber).",
    );
    for v in views {
        let t = &v.status.temperatures;
        for (sensor, reading) in [("nozzle", t.nozzle), ("bed", t.bed), ("chamber", t.chamber)] {
            if let Some(r) = reading {
                let _ = writeln!(
                    out,
                    "gantry_printer_temperature_celsius{{{},sensor=\"{sensor}\"}} {}",
                    printer_labels(v),
                    number(r.current)
                );
            }
        }
    }
    family(
        &mut out,
        "gantry_printer_target_temperature_celsius",
        "gauge",
        "A heater's target temperature, by sensor, while one is set.",
    );
    for v in views {
        let t = &v.status.temperatures;
        for (sensor, reading) in [("nozzle", t.nozzle), ("bed", t.bed), ("chamber", t.chamber)] {
            if let Some(target) = reading.and_then(|r| r.target) {
                let _ = writeln!(
                    out,
                    "gantry_printer_target_temperature_celsius{{{},sensor=\"{sensor}\"}} {}",
                    printer_labels(v),
                    number(target)
                );
            }
        }
    }

    family(
        &mut out,
        "gantry_printer_job_progress_ratio",
        "gauge",
        "How far the running Job has got, 0 to 1. Absent when nothing is running.",
    );
    for v in views {
        if let Some(job) = &v.status.job {
            let _ = writeln!(
                out,
                "gantry_printer_job_progress_ratio{{{}}} {}",
                printer_labels(v),
                number((job.progress / 100.0).clamp(0.0, 1.0))
            );
        }
    }
    family(
        &mut out,
        "gantry_printer_job_remaining_seconds",
        "gauge",
        "How long the running Job is expected to take still, when the Printer says.",
    );
    for v in views {
        if let Some(secs) = v.status.job.as_ref().and_then(|j| j.remaining_secs) {
            let _ = writeln!(
                out,
                "gantry_printer_job_remaining_seconds{{{}}} {secs}",
                printer_labels(v)
            );
        }
    }

    family(
        &mut out,
        "gantry_jobs_total",
        "counter",
        "Jobs that have ended, by Printer and Job Outcome, since History began (overruled Outcomes count as failed).",
    );
    for (printer, outcome, count) in jobs {
        let name = views
            .iter()
            .find(|v| &v.id == printer)
            .map_or_else(String::new, |v| v.name.clone());
        let _ = writeln!(
            out,
            "gantry_jobs_total{{printer=\"{}\",name=\"{}\",outcome=\"{}\"}} {count}",
            escape(printer),
            escape(&name),
            outcome.as_str()
        );
    }
    // Every Outcome appears for the Fleet, so a rate is never missing.
    family(
        &mut out,
        "gantry_fleet_jobs_total",
        "counter",
        "Jobs that have ended across the whole Fleet, by Job Outcome.",
    );
    for outcome in OUTCOMES {
        let count: i64 = jobs
            .iter()
            .filter(|(_, o, _)| *o == outcome)
            .map(|(_, _, c)| c)
            .sum();
        let _ = writeln!(
            out,
            "gantry_fleet_jobs_total{{outcome=\"{}\"}} {count}",
            outcome.as_str()
        );
    }

    family(
        &mut out,
        "gantry_queue_jobs",
        "gauge",
        "Jobs waiting in the Queue.",
    );
    let _ = writeln!(out, "gantry_queue_jobs {queued}");
    out
}

/// Whether `headers` bring `token` as `Authorization: Bearer <token>`,
/// compared in constant time.
pub fn authorised(headers: &HeaderMap, token: &str) -> bool {
    let Some(given) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    else {
        return false;
    };
    let (a, b) = (Sha256::digest(given.trim()), Sha256::digest(token));
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

/// `GET /metrics`.
pub async fn serve(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(token) = state.metrics_token()
        && !authorised(&headers, token)
    {
        return (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer")],
            "a bearer token is needed for /metrics\n",
        )
            .into_response();
    }
    let jobs = match state.store().job_counts().await {
        Ok(jobs) => jobs,
        Err(e) => {
            tracing::error!(error = %e, "could not count Jobs for /metrics");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let queued = state.store().queue().await.map_or(0, |q| q.len());
    let views = state.fleet().snapshot();
    (
        [(header::CONTENT_TYPE, CONTENT_TYPE)],
        render(&views, &jobs, queued),
    )
        .into_response()
}

/// The app for metrics' own address: `/metrics` and nothing else.
pub fn app(state: AppState) -> Router {
    Router::new()
        .route("/metrics", get(serve))
        .with_state(state)
}

/// Serves [`app`] on `listener` until the server stops.
pub async fn serve_on(listener: tokio::net::TcpListener, state: AppState) -> std::io::Result<()> {
    let stopping = state.clone();
    axum::serve(listener, app(state))
        .with_graceful_shutdown(async move { stopping.stopping().await })
        .await
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use serde_json::json;
    use tower::ServiceExt;

    use super::*;
    use crate::test_support::{Client, state_with};

    const TOKEN: &str = "a metrics token for tests";

    async fn scrape(app: Router, token: Option<&str>) -> (StatusCode, String) {
        let mut request = Request::get("/metrics");
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        let response = app
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    #[tokio::test]
    async fn metrics_are_off_by_default() {
        let state = crate::test_support::state().await;
        let (status, _) = scrape(crate::app(state), None).await;
        // Not a metrics endpoint: the frontend's fallback, which needs a login.
        assert_ne!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn with_a_token_requests_without_it_are_refused() {
        let state = state_with(crate::Options {
            metrics_token: Some(TOKEN.into()),
            ..crate::Options::default()
        })
        .await;
        let app = crate::app(state);
        assert_eq!(scrape(app.clone(), None).await.0, StatusCode::UNAUTHORIZED);
        assert_eq!(
            scrape(app.clone(), Some("the wrong token entirely"))
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        let (status, body) = scrape(app, Some(TOKEN)).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("gantry_build_info"), "{body}");
    }

    #[tokio::test]
    async fn on_its_own_address_metrics_need_no_token_unless_one_is_set() {
        let state = crate::test_support::state().await;
        assert_eq!(scrape(app(state), None).await.0, StatusCode::OK);
        let guarded = state_with(crate::Options {
            metrics_token: Some(TOKEN.into()),
            ..crate::Options::default()
        })
        .await;
        assert_eq!(
            scrape(app(guarded.clone()), None).await.0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(scrape(app(guarded), Some(TOKEN)).await.0, StatusCode::OK);
    }

    #[tokio::test]
    async fn the_fleet_jobs_and_queue_are_exposed_by_convention() {
        let client = Client::new().await;
        let printer = client.connected_simulated("Bench \"one\"", json!({})).await;
        client
            .simulate(&printer, json!({ "action": "set_speed", "speed": 1000 }))
            .await;
        client
            .simulate(
                &printer,
                json!({ "action": "start_print", "duration_secs": 1, "layers": 1 }),
            )
            .await;
        client
            .until(&printer, |v| {
                v.status.state == Some(PrinterState::AwaitingClear)
            })
            .await;
        let (status, body) = scrape(app(client.state.clone()), None).await;
        assert_eq!(status, StatusCode::OK);
        let labels = format!("printer=\"{printer}\",name=\"Bench \\\"one\\\"\"");
        for line in [
            format!("gantry_printer_connected{{{labels},driver=\"simulated\"}} 1"),
            format!("gantry_printer_state{{{labels},state=\"awaiting_clear\"}} 1"),
            format!("gantry_printer_state{{{labels},state=\"printing\"}} 0"),
            format!("gantry_jobs_total{{{labels},outcome=\"succeeded\"}} 1"),
            "gantry_fleet_jobs_total{outcome=\"succeeded\"} 1".to_owned(),
            "gantry_fleet_jobs_total{outcome=\"failed\"} 0".to_owned(),
            "gantry_printers 1".to_owned(),
            "gantry_queue_jobs 0".to_owned(),
        ] {
            assert!(
                body.lines().any(|l| l == line),
                "missing `{line}` in:\n{body}"
            );
        }
        assert!(body.contains(&format!(
            "gantry_printer_temperature_celsius{{{labels},sensor=\"nozzle\"}}"
        )));
        // Every family has HELP and TYPE, and every name follows convention.
        for line in body.lines().filter(|l| !l.starts_with('#')) {
            let name = line.split(['{', ' ']).next().unwrap();
            assert!(name.starts_with("gantry_"), "{name}");
            assert!(
                name.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                "{name}"
            );
            assert!(
                body.contains(&format!("# TYPE {name} ")),
                "{name} has no TYPE"
            );
            assert!(
                body.contains(&format!("# HELP {name} ")),
                "{name} has no HELP"
            );
        }
        for line in body.lines().filter(|l| l.starts_with("# TYPE")) {
            let parts: Vec<_> = line.split(' ').collect();
            if parts[3] == "counter" {
                assert!(parts[2].ends_with("_total"), "{line}");
            }
        }
    }
}
