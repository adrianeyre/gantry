//! History and analytics (PRD §7.5): a Printer's Jobs with their Outcomes,
//! rates, durations, filament and cost, over a range of dates; and
//! utilisation and uptime, for one Printer or the whole Fleet.
use std::collections::BTreeMap;

use axum::Json;
use axum::extract::{Path, Query, State};
use serde::{Deserialize, Serialize};
use time::{Date, Duration, OffsetDateTime, Time};

use super::ApiError;
use crate::AppState;
use crate::analytics::{self, Breakdown, Day, HistoryJob, Span, Summary};
use crate::store::now;

/// A range of whole UTC days, `YYYY-MM-DD`, both ends included.
#[derive(Debug, Default, Deserialize)]
pub struct Range {
    #[serde(default)]
    from: Option<String>,
    #[serde(default)]
    to: Option<String>,
    /// For utilisation: one Printer, or the whole Fleet when left out.
    #[serde(default)]
    printer: Option<String>,
}

fn date(field: &str, value: Option<&str>) -> Result<Option<Date>, ApiError> {
    let Some(value) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    Date::parse(
        value,
        &time::macros::format_description!("[year]-[month]-[day]"),
    )
    .map(Some)
    .map_err(|_| ApiError::field(field, "Use a date such as 2026-11-04"))
}

/// The start of `from`'s day and the end of `to`'s.
fn bounds(range: &Range) -> Result<(Option<OffsetDateTime>, Option<OffsetDateTime>), ApiError> {
    let from =
        date("from", range.from.as_deref())?.map(|d| d.with_time(Time::MIDNIGHT).assume_utc());
    let to = date("to", range.to.as_deref())?
        .map(|d| d.with_time(Time::MIDNIGHT).assume_utc() + Duration::days(1));
    if let (Some(f), Some(t)) = (from, to)
        && t <= f
    {
        return Err(ApiError::field(
            "to",
            "The end must be on or after the start",
        ));
    }
    Ok((from, to))
}

/// A Printer's History.
#[derive(Debug, Serialize)]
pub struct History {
    pub summary: Summary,
    pub jobs: Vec<HistoryJob>,
}

/// `GET /api/printers/{id}/history?from=&to=`: the Printer's Jobs that
/// started in the range (every one when left out), newest first, and their
/// totals. History is kept forever, even for a removed Printer.
pub async fn history(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(range): Query<Range>,
) -> Result<Json<History>, ApiError> {
    let (from, to) = bounds(&range)?;
    let mut jobs = state.store().job_history(&id, from, to).await?;
    let timelapses = state.store().timelapse_frames(&id).await?;
    for job in &mut jobs {
        job.timelapse_frames = timelapses.get(&job.job.id).copied();
    }
    if jobs.is_empty() && state.store().printer(state.key(), &id).await?.is_none() {
        return Err(ApiError::NotFound("no such Printer".into()));
    }
    Ok(Json(History {
        summary: analytics::summarise(&jobs),
        jobs,
    }))
}

/// One Printer's share of a utilisation report.
#[derive(Debug, Serialize)]
pub struct PrinterUse {
    pub id: String,
    pub name: String,
    #[serde(flatten)]
    pub totals: Breakdown,
    pub utilisation: Option<f64>,
    pub uptime: Option<f64>,
}

/// Utilisation and uptime over a range.
#[derive(Debug, Serialize)]
pub struct Utilisation {
    /// The Printer, or `None` for the whole Fleet.
    pub printer: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub from: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub to: OffsetDateTime,
    pub totals: Breakdown,
    pub utilisation: Option<f64>,
    pub uptime: Option<f64>,
    /// Day by day, summed over the Printers.
    pub days: Vec<Day>,
    /// Each Printer's totals.
    pub printers: Vec<PrinterUse>,
}

/// How many days a utilisation report may cover.
pub const MAX_DAYS: i64 = 366;

/// `GET /api/analytics/utilisation?printer=&from=&to=`: time printing
/// against time Idle, Busy or Awaiting Clear, and time connected, from the
/// recorded state changes. The last 7 days when no range is given.
pub async fn utilisation(
    State(state): State<AppState>,
    Query(range): Query<Range>,
) -> Result<Json<Utilisation>, ApiError> {
    let today = now().replace_time(Time::MIDNIGHT);
    let (from, to) = bounds(&range)?;
    let to = to.unwrap_or(today + Duration::days(1));
    let from = from.unwrap_or(to - Duration::days(7));
    if (to - from).whole_days() > MAX_DAYS {
        return Err(ApiError::field("from", "Choose a range of a year or less"));
    }
    let printer = range.printer.as_deref().filter(|p| !p.is_empty());
    if let Some(id) = printer
        && !state.fleet().contains(id)
        && state.store().printer(state.key(), id).await?.is_none()
    {
        return Err(ApiError::NotFound("no such Printer".into()));
    }
    let mut spans: BTreeMap<String, Vec<Span>> = BTreeMap::new();
    for (id, span) in state.store().state_spans(printer, from, to).await? {
        spans.entry(id).or_default().push(span);
    }
    if let Some(id) = printer {
        spans.entry(id.to_owned()).or_default();
    } else {
        for view in state.fleet().snapshot() {
            spans.entry(view.id.clone()).or_default();
        }
    }
    let at = now();
    // A running Printer's latest stretch still holds now; it is only marked
    // as seen every so often.
    for (id, spans) in &mut spans {
        if state.fleet().contains(id)
            && let Some(last) = spans.last_mut()
        {
            last.end = last.end.max(at);
        }
    }
    let mut totals = Breakdown::default();
    let mut days: Vec<Day> = Vec::new();
    let mut printers = Vec::new();
    for (id, spans) in &spans {
        let b = analytics::breakdown(spans, from, to, at);
        totals = totals.plus(&b);
        for (i, day) in analytics::by_day(spans, from, to, at)
            .into_iter()
            .enumerate()
        {
            match days.get_mut(i) {
                Some(sum) => sum.breakdown = sum.breakdown.plus(&day.breakdown),
                None => days.push(day),
            }
        }
        let name = match state.fleet().view(id) {
            Ok(v) => v.name.clone(),
            Err(_) => state
                .store()
                .printer(state.key(), id)
                .await?
                .map_or_else(|| "A removed Printer".to_owned(), |p| p.name),
        };
        printers.push(PrinterUse {
            id: id.clone(),
            name,
            utilisation: b.utilisation(),
            uptime: b.uptime(),
            totals: b,
        });
    }
    for day in &mut days {
        day.utilisation = day.breakdown.utilisation();
        day.uptime = day.breakdown.uptime();
    }
    printers.sort_by_key(|p| p.name.to_lowercase());
    Ok(Json(Utilisation {
        printer: printer.map(str::to_owned),
        from,
        to,
        utilisation: totals.utilisation(),
        uptime: totals.uptime(),
        totals,
        days,
        printers,
    }))
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use gantry_core::PrinterState;
    use gantry_print_file_fixtures as fixtures;
    use serde_json::json;

    use crate::store::users::Role;
    use crate::test_support::Client;

    #[tokio::test]
    async fn history_has_every_job_with_outcomes_rates_filament_and_cost() {
        let client = Client::new().await;
        let printer = client
            .connected_simulated("X1", json!({ "model": "X1C" }))
            .await;
        client
            .put(
                &format!("/api/printers/{printer}/automation"),
                json!({ "auto_start": false }),
            )
            .await;
        client
            .simulate(&printer, json!({ "action": "set_speed", "speed": 1000 }))
            .await;
        // A white PLA spool on it, 1 kg for 20.
        let operator = client.as_role(Role::Operator).await;
        let (status, spool) = operator
            .post(
                "/api/spools",
                json!({ "kind": "PLA", "colour": "#FFFFFF", "weight_g": 1000, "cost": 20,
                        "printer_id": printer }),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{spool}");
        let (_, file) = client
            .upload(
                "/api/print-files?name=tray.gcode.3mf",
                &fixtures::bambu_3mf(&fixtures::three_plates()),
            )
            .await;

        // A Job Gantry starts: Plate 1, 11.8 g of white PLA.
        let (status, job) = client
            .post(
                &format!("/api/printers/{printer}/start"),
                json!({ "print_file_id": file["id"], "plate": 1 }),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{job}");
        client
            .until(&printer, |v| {
                v.status.state == Some(PrinterState::AwaitingClear)
            })
            .await;
        // Overruled to Failed at Mark Clear.
        client
            .post(
                &format!("/api/printers/{printer}/mark-clear"),
                json!({ "overrule_to_failed": true }),
            )
            .await;
        // An External Job, which Succeeds.
        client
            .simulate(
                &printer,
                json!({ "action": "start_print", "duration_secs": 2, "layers": 2 }),
            )
            .await;
        client
            .until(&printer, |v| {
                v.status.state == Some(PrinterState::AwaitingClear)
            })
            .await;

        // The filament is recorded a moment after the Job ends.
        let mut history = serde_json::Value::Null;
        for _ in 0..50 {
            (_, history) = client
                .get(&format!("/api/printers/{printer}/history"))
                .await;
            if history["jobs"][1]["filament_g"].is_number() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let summary = &history["summary"];
        assert_eq!(summary["jobs"], 2, "{history}");
        assert_eq!(summary["failed"], 1, "the overruled Job counts as Failed");
        assert_eq!(summary["succeeded"], 1, "the External Job is included");
        assert_eq!(summary["success_rate"], 0.5);
        assert_eq!(summary["failure_rate"], 0.5);
        let jobs = history["jobs"].as_array().unwrap();
        assert_eq!(jobs[0]["external"], true, "newest first");
        assert!(
            jobs[0]["filament_g"].is_null(),
            "unknown for the External Job"
        );
        assert_eq!(jobs[1]["overruled"], true);
        // Overruled after it Succeeded, so it used all its filament.
        assert_eq!(jobs[1]["filament_g"], 11.8);
        assert_eq!(jobs[1]["filament_cost"], 0.24, "11.8 g at 2p a gram");
        assert_eq!(jobs[1]["filament_usage"][0]["spool_id"], spool["id"]);
        assert!(jobs[1]["duration_secs"].is_number());
        assert_eq!(summary["filament_g"], 11.8);
        // The spool went down.
        let (_, spools) = client.get("/api/spools").await;
        assert_eq!(spools[0]["remaining_g"], 988.2);

        // Date filtering.
        let (_, none) = client
            .get(&format!(
                "/api/printers/{printer}/history?from=2000-01-01&to=2000-01-31"
            ))
            .await;
        assert_eq!(none["summary"]["jobs"], 0);
        let today = crate::store::now().date().to_string();
        let (_, today) = client
            .get(&format!(
                "/api/printers/{printer}/history?from={today}&to={today}"
            ))
            .await;
        assert_eq!(today["summary"]["jobs"], 2);
        let (status, _) = client
            .get(&format!("/api/printers/{printer}/history?from=yesterday"))
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn utilisation_works_for_one_printer_and_for_the_fleet() {
        let client = Client::new().await;
        let a = client.connected_simulated("A", json!({})).await;
        let b = client.connected_simulated("B", json!({})).await;
        client
            .simulate(
                &a,
                json!({ "action": "start_print", "duration_secs": 600, "layers": 10 }),
            )
            .await;
        client.until(&a, |v| v.job.is_some()).await;
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        let (status, one) = client
            .get(&format!("/api/analytics/utilisation?printer={a}"))
            .await;
        assert_eq!(status, StatusCode::OK, "{one}");
        assert_eq!(one["printers"].as_array().unwrap().len(), 1);
        assert!(
            one["totals"]["printing_secs"].as_f64().unwrap() > 0.0,
            "{one}"
        );
        assert!(one["uptime"].as_f64().unwrap() > 0.5, "{one}");
        assert_eq!(one["days"].as_array().unwrap().len(), 7);
        let (_, fleet) = client.get("/api/analytics/utilisation").await;
        assert!(fleet["printer"].is_null());
        let names: Vec<_> = fleet["printers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["A", "B"]);
        // B has been Idle the whole time it's been connected.
        assert_eq!(fleet["printers"][1]["printing_secs"], 0.0);
        assert!(fleet["printers"][1]["idle_secs"].as_f64().unwrap() > 0.0);
        let _ = b;
        let (status, _) = client.get("/api/analytics/utilisation?printer=nope").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let viewer = client.as_role(Role::Viewer).await;
        let (status, _) = viewer.get("/api/analytics/utilisation").await;
        assert_eq!(status, StatusCode::OK);
    }
}
