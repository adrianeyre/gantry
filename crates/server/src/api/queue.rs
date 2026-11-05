//! The Queue (PRD §7.3): Operators queue Plates for a specific Printer or
//! for any Compatible Printer, reorder them, and start the Job the Queue
//! suggests for each free Printer. A Printer Awaiting Clear is never given a
//! Job, and a Print File never goes to a model it wasn't sliced for. With
//! Auto-start on, [`crate::dispatch`] starts the suggested Job itself.
use std::collections::{HashMap, HashSet};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use gantry_core::{JobRecord, PrinterState, PrinterView};
use serde::{Deserialize, Serialize};

use super::ApiError;
use super::print_files::{model_mismatch, start_plate};
use crate::AppState;
use crate::audit::{self, EntryExt};
use crate::compatibility::{self, Fit, Plan};
use crate::store::audit::{Action, Entry};
use crate::store::print_files::PrintFile;
use crate::store::queue::QueuedJob;
use crate::store::users::User;

/// A queued Job, with what the Queue would do with it now.
#[derive(Debug, Serialize)]
pub struct QueueEntry {
    #[serde(flatten)]
    pub job: QueuedJob,
    /// A free Printer can take it now: the one in `assigned_printer`.
    pub suggested: bool,
    /// The Printer it would start on now. For a Job queued for any
    /// Compatible Printer, the Queue chose it.
    pub assigned_printer: Option<String>,
    /// What doesn't block it but an Operator should know, such as a colour
    /// that doesn't match.
    pub warnings: Vec<String>,
    /// Why it can't start yet, when it can't.
    pub waiting: Option<String>,
}

/// Whether a Printer can take its next queued Job now: connected, Idle,
/// running nothing, and not Awaiting Clear.
pub fn ready_for_next(view: &PrinterView) -> bool {
    view.status.connection.is_connected()
        && view.status.state == Some(PrinterState::Idle)
        && view.awaiting_clear.is_none()
        && view.job.is_none()
}

/// The Print Files the Queue's Jobs print.
async fn files_for(
    state: &AppState,
    queue: &[QueuedJob],
) -> Result<HashMap<i64, PrintFile>, sqlx::Error> {
    let mut files = HashMap::new();
    for id in queue
        .iter()
        .map(|j| j.print_file_id)
        .collect::<HashSet<_>>()
    {
        if let Some(file) = state.store().print_file(id).await? {
            files.insert(id, file);
        }
    }
    Ok(files)
}

/// What the Queue would do now. Free Printers are tried in name order,
/// except that `preferred`, when free, is tried first. Jobs in `skip` are
/// left alone.
async fn plan_now(
    state: &AppState,
    preferred: Option<&str>,
    skip: &HashSet<i64>,
) -> Result<(Vec<QueuedJob>, Plan), sqlx::Error> {
    let queue = state.store().queue().await?;
    let files = files_for(state, &queue).await?;
    let views = state.fleet().snapshot();
    let mut free: Vec<&PrinterView> = views
        .iter()
        .map(AsRef::as_ref)
        .filter(|v| ready_for_next(v))
        .collect();
    if let Some(first) = preferred {
        free.sort_by_key(|v| v.id != first);
    }
    let plan = compatibility::plan(&queue, &files, &free, skip);
    Ok((queue, plan))
}

/// What the Queue would do now, with `preferred` tried first among the free
/// Printers. Jobs being started are left alone.
pub async fn current_plan(
    state: &AppState,
    preferred: Option<&str>,
) -> Result<(Vec<QueuedJob>, Plan), sqlx::Error> {
    plan_now(state, preferred, &state.claimed()).await
}

fn entries(queue: Vec<QueuedJob>, plan: &Plan) -> Vec<QueueEntry> {
    queue
        .into_iter()
        .map(|job| {
            let assigned = plan.for_job(job.id);
            QueueEntry {
                suggested: assigned.is_some(),
                assigned_printer: assigned.map(|a| a.printer_id.clone()),
                warnings: assigned.map(|a| a.warnings.clone()).unwrap_or_default(),
                waiting: plan.waiting.get(&job.id).cloned(),
                job,
            }
        })
        .collect()
}

/// The Queue with what it would do now.
pub async fn with_plan(state: &AppState) -> Result<Vec<QueueEntry>, sqlx::Error> {
    let (queue, plan) = current_plan(state, None).await?;
    Ok(entries(queue, &plan))
}

/// `GET /api/queue`: the Queue in order.
pub async fn list(State(state): State<AppState>) -> Result<Json<Vec<QueueEntry>>, ApiError> {
    Ok(Json(with_plan(&state).await?))
}

#[derive(Debug, Deserialize)]
pub struct QueueForm {
    /// The Printer, or `None` (left out) for any Compatible Printer.
    #[serde(default)]
    printer_id: Option<String>,
    print_file_id: i64,
    /// Which Plates; every Plate when left out.
    #[serde(default)]
    plates: Option<Vec<u32>>,
}

/// `POST /api/queue`: queues one Job per Plate, for a Printer or for any
/// Compatible Printer. Refused for a Printer of another model than the
/// Print File was sliced for, and, for any Compatible Printer, for a Print
/// File that doesn't say which model that is.
pub async fn add(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Json(form): Json<QueueForm>,
) -> Result<(StatusCode, Json<Vec<QueueEntry>>), ApiError> {
    let printer = form.printer_id.as_deref().filter(|p| !p.is_empty());
    let result = async {
        let file = state
            .store()
            .print_file(form.print_file_id)
            .await?
            .ok_or_else(|| ApiError::NotFound("no such Print File".into()))?;
        match printer {
            Some(printer) => {
                let view = state.fleet().view(printer)?;
                if let Some(why) = model_mismatch(&file, &view.name, view.model.as_deref()) {
                    return Err(ApiError::Conflict(why));
                }
            }
            None if file.printer_model.is_none() => {
                return Err(ApiError::Conflict(format!(
                    "{} doesn't say which printer model it was sliced for, so no Printer is \
                     compatible: set the model in the library first",
                    file.name
                )));
            }
            None => {}
        }
        let plates = match form.plates.clone() {
            Some(plates) if !plates.is_empty() => plates,
            _ => file.plates.iter().map(|p| p.number).collect(),
        };
        if let Some(missing) = plates.iter().find(|n| file.plate(**n).is_none()) {
            return Err(ApiError::field(
                "plates",
                &format!("{} has no Plate {missing}", file.name),
            ));
        }
        let ids = state
            .store()
            .enqueue(printer, file.id, &plates, Some(&user.username))
            .await?;
        let added = with_plan(&state)
            .await?
            .into_iter()
            .filter(|e| ids.contains(&e.job.id))
            .collect::<Vec<_>>();
        Ok((file, added))
    }
    .await;
    let mut entry = audit::by(&user, Action::QueueAdded).result(&result);
    if let Some(printer) = printer {
        entry = entry.printer(printer);
    }
    if let Ok((file, added)) = &result {
        entry = entry.target(file.name.clone()).detail(serde_json::json!({
            "print_file_id": file.id,
            "plates": added.iter().map(|e| e.job.plate).collect::<Vec<_>>(),
            "any_compatible_printer": printer.is_none(),
        }));
    }
    audit::record(&state, entry).await;
    result.map(|(_, added)| (StatusCode::CREATED, Json(added)))
}

#[derive(Debug, Deserialize)]
pub struct OrderForm {
    /// Every queued Job's id, in the new order.
    ids: Vec<i64>,
}

/// `PUT /api/queue/order`: puts the Queue in a new order.
pub async fn reorder(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Json(form): Json<OrderForm>,
) -> Result<Json<Vec<QueueEntry>>, ApiError> {
    let result = async {
        if !state.store().reorder_queue(&form.ids).await? {
            return Err(ApiError::Conflict(
                "the Queue has changed: reload it and try again".into(),
            ));
        }
        Ok(with_plan(&state).await?)
    }
    .await;
    audit::record(
        &state,
        audit::by(&user, Action::QueueReordered)
            .detail(serde_json::json!({ "order": form.ids }))
            .result(&result),
    )
    .await;
    result.map(Json)
}

/// `DELETE /api/queue/{id}`: takes a Job out of the Queue.
pub async fn remove(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    let result = async {
        let job = state
            .store()
            .queued_job(id)
            .await?
            .ok_or_else(|| ApiError::NotFound("no such queued Job".into()))?;
        state.store().dequeue(id).await?;
        Ok(job)
    }
    .await;
    let mut entry = audit::by(&user, Action::QueueRemoved).result(&result);
    if let Ok(job) = &result {
        if let Some(printer) = &job.printer_id {
            entry = entry.printer(printer);
        }
        entry = entry
            .target(job.print_file.clone())
            .detail(serde_json::json!({ "plate": job.plate }));
    }
    audit::record(&state, entry).await;
    result.map(|_| StatusCode::NO_CONTENT)
}

/// The body of `POST /api/queue/{id}/start`, which may be left out.
#[derive(Debug, Default, Deserialize)]
pub struct StartForm {
    /// For a Job queued for any Compatible Printer: which one. Left out, the
    /// Queue chooses.
    #[serde(default)]
    printer_id: Option<String>,
}

/// Starts a queued Job on `printer`, or where the Queue would put it, and
/// takes it out of the Queue once it has started. Shared by an Operator's
/// start and Auto-start. While it runs, nothing else can start the same Job.
pub async fn start_queued(
    state: &AppState,
    id: i64,
    printer: Option<&str>,
) -> Result<(QueuedJob, JobRecord), ApiError> {
    let Some(_claim) = state.claim(id) else {
        return Err(ApiError::Conflict(
            "this queued Job is already being started".into(),
        ));
    };
    let queued = state
        .store()
        .queued_job(id)
        .await?
        .ok_or_else(|| ApiError::NotFound("no such queued Job".into()))?;
    let file = state
        .store()
        .print_file(queued.print_file_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("its Print File has been removed".into()))?;
    let plate = file
        .plate(queued.plate)
        .ok_or_else(|| ApiError::NotFound("its Plate is gone".into()))?;
    let target = match (&queued.printer_id, printer) {
        (Some(own), Some(asked)) if own != asked => {
            return Err(ApiError::Conflict(
                "this Job is queued for another Printer".into(),
            ));
        }
        (Some(own), _) => own.clone(),
        (None, Some(asked)) => {
            let view = state.fleet().view(asked)?;
            match compatibility::fit(&file, plate, &view) {
                Fit::Compatible { .. } => asked.to_owned(),
                Fit::Incompatible(why) | Fit::MissingFilament(why) => {
                    return Err(ApiError::Conflict(why));
                }
            }
        }
        (None, None) => {
            // This Job's claim is ours, so the plan must see it as free.
            let mut skip = state.claimed();
            skip.remove(&id);
            let (_, plan) = plan_now(state, None, &skip).await?;
            match plan.for_job(id) {
                Some(a) => a.printer_id.clone(),
                None => {
                    return Err(ApiError::Conflict(
                        plan.waiting
                            .get(&id)
                            .cloned()
                            .unwrap_or_else(|| "no free Compatible Printer can take it".into()),
                    ));
                }
            }
        }
    };
    let view = state.fleet().view(&target)?;
    if view.awaiting_clear.is_some() || view.status.state == Some(PrinterState::AwaitingClear) {
        return Err(ApiError::Conflict(format!(
            "{} is Awaiting Clear: Mark Clear before starting another Job",
            view.name
        )));
    }
    let job = start_plate(state, &target, &file, queued.plate).await?;
    state.store().dequeue(id).await?;
    Ok((queued, job))
}

/// The audit entry for starting a queued Job, by an Operator or by
/// Auto-start (`user` `None`: Gantry did it).
pub fn start_entry(
    user: Option<&User>,
    id: i64,
    queued: Option<&QueuedJob>,
    result: &Result<(QueuedJob, JobRecord), ApiError>,
) -> Entry {
    let mut entry = match user {
        Some(user) => audit::by(user, Action::JobStarted),
        None => Entry::new(Action::JobStarted),
    }
    .result(result);
    if let Some(queued) = queued {
        if let Some(printer) = &queued.printer_id {
            entry = entry.printer(printer);
        }
        entry = entry.target(queued.print_file.clone());
    }
    let mut detail = serde_json::json!({ "queued": id, "auto_start": user.is_none() });
    if let Ok((_, job)) = result {
        entry = entry.printer(&job.printer_id);
        detail["plate"] = serde_json::json!(job.plate);
        detail["job"] = serde_json::json!(job.id);
    }
    entry.detail(detail)
}

/// `POST /api/queue/{id}/start`: an Operator starts a queued Job, on its
/// Printer or, for any Compatible Printer, on the one asked for or the one
/// the Queue chose. It leaves the Queue only once it has started.
pub async fn start(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<i64>,
    form: Option<Json<StartForm>>,
) -> Result<(StatusCode, Json<JobRecord>), ApiError> {
    let form = form.map(|Json(f)| f).unwrap_or_default();
    let queued = state.store().queued_job(id).await?;
    let result = start_queued(&state, id, form.printer_id.as_deref()).await;
    audit::record(
        &state,
        start_entry(Some(&user), id, queued.as_ref(), &result),
    )
    .await;
    result.map(|(_, job)| (StatusCode::CREATED, Json(job)))
}
