//! Keeps History (PRD §7.5) as the Fleet runs. A task follows every change
//! to the Fleet and:
//!
//! - records each change of a Printer's Connection or Printer State, and
//!   every [`HEARTBEAT`] that the current one still holds, for utilisation
//!   and uptime charts;
//! - when a Job ends, works out the filament it used from its Plate's slicer
//!   figures (all of it for a Succeeded Job, the share it got through
//!   otherwise), charges it to the spools loaded on the Printer, and records
//!   it with the Job, so the inventory goes down and the Job has a cost.
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use gantry_core::{FleetUpdate, JobOutcome, JobRecord, PrinterState, PrinterView};
use tokio::sync::broadcast::error::RecvError;

use crate::AppState;
use crate::analytics::filament_used;
use crate::store::now;

/// How often a Printer's current stretch is marked as still holding. Time
/// after the last mark (Gantry stopped, say) counts as unknown.
pub const HEARTBEAT: Duration = Duration::from_secs(30);

#[derive(Default)]
struct Seen {
    activity: Option<(bool, Option<PrinterState>)>,
    last_ended: Option<i64>,
    /// The running Job and how far it had got, 0 to 100.
    progress: Option<(i64, f64)>,
}

/// Starts the History task. Must be called within a tokio runtime.
pub fn spawn(state: &AppState) {
    let state = state.clone();
    let mut updates = state.fleet().subscribe();
    tokio::spawn(async move {
        let mut seen: HashMap<String, Seen> = HashMap::new();
        for view in state.fleet().snapshot() {
            let s = seen.entry(view.id.clone()).or_default();
            s.last_ended = view.last_ended.as_ref().map(|j| j.id);
            follow_state(&state, s, &view).await;
        }
        let mut heartbeat = tokio::time::interval(HEARTBEAT);
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                update = updates.recv() => match update {
                    Ok(FleetUpdate::Printer(view)) => {
                        let s = seen.entry(view.id.clone()).or_default();
                        follow(&state, s, view).await;
                    }
                    Ok(FleetUpdate::Removed(id)) => {
                        if let Err(e) = state.store().state_seen(&id, now()).await {
                            tracing::warn!(printer = %id, error = %e, "could not record a Printer's last state");
                        }
                        seen.remove(&id);
                    }
                    Err(RecvError::Lagged(missed)) => {
                        tracing::warn!(missed, "History fell behind the Fleet's updates");
                    }
                    Err(RecvError::Closed) => break,
                },
                _ = heartbeat.tick() => {
                    let at = now();
                    for id in seen.keys() {
                        if let Err(e) = state.store().state_seen(id, at).await {
                            tracing::warn!(printer = %id, error = %e, "could not record a Printer's state");
                        }
                    }
                }
            }
        }
    });
}

async fn follow_state(state: &AppState, seen: &mut Seen, view: &PrinterView) {
    let activity = (view.status.connection.is_connected(), view.status.state);
    if seen.activity == Some(activity) {
        return;
    }
    seen.activity = Some(activity);
    if let Err(e) = state
        .store()
        .state_changed(&view.id, activity.0, activity.1, now())
        .await
    {
        tracing::warn!(printer = %view.name, error = %e, "could not record a state change");
    }
}

async fn follow(state: &AppState, seen: &mut Seen, view: Arc<PrinterView>) {
    follow_state(state, seen, &view).await;
    if let (Some(job), Some(active)) = (&view.job, &view.status.job) {
        seen.progress = Some((job.id, active.progress));
    }
    let ended = view.last_ended.as_ref().map(|j| j.id);
    if ended.is_some() && ended != seen.last_ended {
        seen.last_ended = ended;
        if let Some(job) = &view.last_ended {
            let progress = seen
                .progress
                .filter(|(id, _)| *id == job.id)
                .map(|(_, p)| p);
            if let Err(e) = job_ended(state, job, progress).await {
                tracing::warn!(printer = %view.name, job = job.id, error = %e, "could not record a Job's filament");
            }
        }
    }
}

/// Records what an ended Job used, if its Plate says how much filament it
/// needs. `progress` is how far it got, 0 to 100, when Gantry saw it run.
pub async fn job_ended(
    state: &AppState,
    job: &JobRecord,
    progress: Option<f64>,
) -> Result<(), sqlx::Error> {
    let (Some(file), Some(plate)) = (job.print_file_id, job.plate) else {
        return Ok(());
    };
    let Some(file) = state.store().print_file(file).await? else {
        return Ok(());
    };
    let Some(plate) = file.plate(plate) else {
        return Ok(());
    };
    let fraction = match job.outcome {
        Some(JobOutcome::Succeeded) => 1.0,
        _ => progress.unwrap_or(0.0) / 100.0,
    };
    let needs: Vec<_> = plate
        .filaments
        .iter()
        .map(|f| (f.kind.clone(), f.colour.clone(), f.grams))
        .collect();
    let loaded = state.store().loaded_spools(&job.printer_id).await?;
    let usage = filament_used(&needs, fraction, &loaded);
    if usage.is_empty() {
        return Ok(());
    }
    state.store().record_filament(job.id, &usage).await?;
    Ok(())
}
