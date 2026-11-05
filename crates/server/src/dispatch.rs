//! Auto-start (PRD §7.3): when a Printer becomes Idle after a Job (through
//! Mark Clear, or straight away with Auto-Clear), the Queue starts the next
//! compatible Job on it without an Operator. With Auto-start off the Queue
//! only suggests that Job.
//!
//! A task follows the Fleet. A Printer is "owed" a Job once a Job of its has
//! ended (it is Awaiting Clear, or Auto-Clear has already cleared it); when it
//! next becomes ready for one, the Queue's plan, with this Printer tried
//! first, says which Job it takes. A start that fails leaves the Printer
//! Idle without owing it another try, so a broken upload can't loop. Every
//! Auto-started Job is audited as done by Gantry itself.
use std::collections::HashMap;
use std::sync::Arc;

use gantry_core::{FleetUpdate, PrinterView};
use tokio::sync::broadcast::error::RecvError;

use crate::AppState;
use crate::api::queue::{current_plan, ready_for_next, start_entry, start_queued};
use crate::audit;

/// What the task remembers about one Printer.
#[derive(Default)]
struct Seen {
    ready: bool,
    last_ended: Option<i64>,
    /// A Job has ended since the Printer last became ready.
    owed: bool,
}

impl Seen {
    /// Takes in a new view; `true` when the Printer has just become ready
    /// for the Job it is owed.
    fn update(&mut self, view: &PrinterView) -> bool {
        let ended = view.last_ended.as_ref().map(|j| j.id);
        if view.awaiting_clear.is_some() || (ended.is_some() && ended != self.last_ended) {
            self.owed = true;
        }
        self.last_ended = ended;
        let ready = ready_for_next(view);
        let due = ready && !self.ready && self.owed;
        if due {
            self.owed = false;
        }
        self.ready = ready;
        due
    }
}

/// Starts the Auto-start task. Must be called within a tokio runtime.
pub fn spawn(state: &AppState) {
    let state = state.clone();
    let mut updates = state.fleet().subscribe();
    tokio::spawn(async move {
        let mut seen: HashMap<String, Seen> = state
            .fleet()
            .snapshot()
            .iter()
            .map(|v| {
                let mut s = Seen::default();
                s.update(v);
                // A Job that ended before this task started is owed only
                // while its Printer still waits to be cleared.
                s.owed = v.awaiting_clear.is_some();
                (v.id.clone(), s)
            })
            .collect();
        loop {
            match updates.recv().await {
                Ok(FleetUpdate::Printer(view)) => {
                    if seen.entry(view.id.clone()).or_default().update(&view) {
                        tokio::spawn(auto_start(state.clone(), view));
                    }
                }
                Ok(FleetUpdate::Removed(id)) => {
                    seen.remove(&id);
                }
                Err(RecvError::Lagged(missed)) => {
                    tracing::warn!(missed, "Auto-start fell behind the Fleet's updates");
                }
                Err(RecvError::Closed) => break,
            }
        }
    });
}

/// Starts the next compatible queued Job on `printer`, if its Auto-start is
/// on and the Queue has one for it.
async fn auto_start(state: AppState, printer: Arc<PrinterView>) {
    match state.store().automation(&printer.id).await {
        Ok(Some(a)) if a.auto_start => {}
        Ok(_) => return,
        Err(e) => {
            tracing::warn!(printer = %printer.name, error = %e, "could not read Auto-start");
            return;
        }
    }
    let queued = match current_plan(&state, Some(&printer.id)).await {
        Ok((_, plan)) => match plan.for_printer(&printer.id) {
            Some(a) => a.queued,
            None => {
                tracing::debug!(printer = %printer.name, "Auto-start: nothing in the Queue for it");
                return;
            }
        },
        Err(e) => {
            tracing::warn!(printer = %printer.name, error = %e, "Auto-start could not read the Queue");
            return;
        }
    };
    let job = state.store().queued_job(queued).await.ok().flatten();
    let result = start_queued(&state, queued, Some(&printer.id)).await;
    match &result {
        Ok((_, job)) => {
            tracing::info!(printer = %printer.name, job = job.id, queued, "Auto-start started the next Job");
        }
        Err(e) => {
            tracing::warn!(printer = %printer.name, queued, error = %e.message(), "Auto-start could not start the next Job");
        }
    }
    audit::record(&state, start_entry(None, queued, job.as_ref(), &result)).await;
}

#[cfg(test)]
mod tests {
    use gantry_core::{
        Connection, DisconnectReason, JobOutcome, JobRecord, PrinterState, PrinterStatus,
    };

    use super::*;

    fn view(state: PrinterState) -> PrinterView {
        let mut status = PrinterStatus::disconnected(DisconnectReason::Stopped);
        status.connection = Connection::Connected;
        status.state = Some(state);
        PrinterView {
            id: "p".into(),
            name: "P".into(),
            driver: "simulated".into(),
            model: None,
            status,
            last_seen: None,
            job: None,
            awaiting_clear: None,
            last_ended: None,
            auto_clear: false,
            reported_capabilities: Default::default(),
            capability_overrides: Default::default(),
        }
    }

    fn ended(id: i64) -> JobRecord {
        JobRecord {
            id,
            printer_id: "p".into(),
            external: false,
            driver_key: "k".into(),
            print_file: None,
            print_file_id: None,
            plate: None,
            started_at: time::OffsetDateTime::UNIX_EPOCH,
            ended_at: Some(time::OffsetDateTime::UNIX_EPOCH),
            outcome: Some(JobOutcome::Succeeded),
            overruled: false,
        }
    }

    #[test]
    fn mark_clear_makes_a_printer_due() {
        let mut seen = Seen::default();
        let mut waiting = view(PrinterState::AwaitingClear);
        waiting.awaiting_clear = Some(ended(1));
        waiting.last_ended = Some(ended(1));
        assert!(!seen.update(&waiting));
        let mut cleared = view(PrinterState::Idle);
        cleared.last_ended = Some(ended(1));
        assert!(seen.update(&cleared), "Mark Clear: due");
        assert!(!seen.update(&cleared), "only once");
    }

    #[test]
    fn auto_clear_makes_a_printer_due() {
        let mut seen = Seen::default();
        let mut printing = view(PrinterState::Printing);
        printing.job = Some(ended(1));
        assert!(!seen.update(&printing));
        // The Job ended and Auto-Clear cleared it; the Driver still says
        // printing for a moment, then Idle.
        printing.job = None;
        printing.last_ended = Some(ended(1));
        assert!(!seen.update(&printing));
        let mut idle = view(PrinterState::Idle);
        idle.last_ended = Some(ended(1));
        assert!(seen.update(&idle));
    }

    #[test]
    fn a_failed_start_or_a_printer_already_idle_is_not_due() {
        let mut seen = Seen::default();
        let idle = view(PrinterState::Idle);
        assert!(!seen.update(&idle), "Idle from the start");
        // A start that failed: a Job appeared, then went again.
        let mut starting = view(PrinterState::Idle);
        starting.job = Some(ended(2));
        assert!(!seen.update(&starting));
        assert!(!seen.update(&idle), "no Job ended, so nothing is owed");
        // Back from Busy (homing) is not after a Job either.
        assert!(!seen.update(&view(PrinterState::Busy)));
        assert!(!seen.update(&idle));
    }
}
