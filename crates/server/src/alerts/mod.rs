//! Alerts (PRD §7.4): notifications Gantry sends through an outside channel
//! when something happens to a Printer.
//!
//! A task follows every change to the Fleet and compares each Printer's new
//! view with the one before, which gives the events: a Job ended (with its
//! Outcome), a Job paused, a Fault raised, the Connection lost, or a Filament
//! Slot running low. Each event is checked against the Admins' Alert Rules
//! (which events, on which Printers or Tags, or the whole Fleet), and every
//! matching Rule's channel is sent an Alert. Delivery runs in its own task,
//! is retried, and is logged when it fails; it never stops the server.
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use gantry_core::{Connection, FleetUpdate, JobOutcome, PrinterState, PrinterView};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast::error::RecvError;

use crate::AppState;
use crate::store::alerts::Rule;

pub mod channels;

/// What happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    JobEnded,
    JobPaused,
    FaultRaised,
    ConnectionLost,
    LowFilament,
}

impl EventKind {
    pub const ALL: [EventKind; 5] = [
        EventKind::JobEnded,
        EventKind::JobPaused,
        EventKind::FaultRaised,
        EventKind::ConnectionLost,
        EventKind::LowFilament,
    ];
}

/// How bad an Alert is, which channels show in their own way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Good,
    Info,
    Warning,
    Bad,
}

/// One thing that happened to one Printer.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Event {
    pub kind: EventKind,
    pub printer_id: String,
    pub printer_name: String,
    /// For a Job that ended.
    pub outcome: Option<JobOutcome>,
}

/// An Alert, ready for a channel.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Alert {
    pub event: Event,
    pub title: String,
    pub message: String,
    pub level: Level,
}

fn outcome_word(outcome: JobOutcome) -> &'static str {
    match outcome {
        JobOutcome::Succeeded => "succeeded",
        JobOutcome::Failed => "failed",
        JobOutcome::Cancelled => "was cancelled",
        JobOutcome::Lost => "was lost",
    }
}

/// The events between two views of one Printer. Only changes count: a
/// Printer's first view, or one after it reconnects, sets the baseline.
pub fn events(before: &PrinterView, after: &PrinterView) -> Vec<Alert> {
    let mut out = Vec::new();
    let printer = |kind, outcome| Event {
        kind,
        printer_id: after.id.clone(),
        printer_name: after.name.clone(),
        outcome,
    };
    let name = &after.name;
    let was_connected = before.status.connection.is_connected();
    let connected = after.status.connection.is_connected();

    if was_connected
        && let Connection::Disconnected { reason, detail } = &after.status.connection
        && *reason != gantry_core::DisconnectReason::Stopped
    {
        out.push(Alert {
            event: printer(EventKind::ConnectionLost, None),
            title: format!("{name}: Connection lost"),
            message: format!(
                "Gantry lost its Connection to {name} ({}){}.",
                reason.describe(),
                detail
                    .as_deref()
                    .map(|d| format!(": {d}"))
                    .unwrap_or_default()
            ),
            level: Level::Bad,
        });
    }

    // Every ending, whether the Printer now waits in Awaiting Clear or
    // Auto-Clear has already taken it back to Idle.
    if let Some(job) = &after.last_ended
        && let Some(outcome) = job.outcome
        && before.last_ended.as_ref().map(|j| j.id) != Some(job.id)
    {
        let waiting = after
            .awaiting_clear
            .as_ref()
            .is_some_and(|j| j.id == job.id);
        let file = match (&job.print_file, job.plate) {
            (Some(file), Some(plate)) => format!("{file} (Plate {plate})"),
            (Some(file), None) => file.clone(),
            _ => "The Job".to_owned(),
        };
        out.push(Alert {
            event: printer(EventKind::JobEnded, Some(outcome)),
            title: format!("{name}: Job {}", outcome_word(outcome)),
            message: format!(
                "{file} {} on {name}. {}",
                outcome_word(outcome),
                if waiting {
                    "It is Awaiting Clear."
                } else {
                    "Auto-Clear has made it Idle again."
                }
            ),
            level: match outcome {
                JobOutcome::Succeeded => Level::Good,
                JobOutcome::Cancelled => Level::Info,
                JobOutcome::Failed | JobOutcome::Lost => Level::Bad,
            },
        });
    }

    if !(was_connected && connected) {
        return out;
    }

    if after.status.state == Some(PrinterState::Paused)
        && before.status.state != Some(PrinterState::Paused)
    {
        let file = after
            .status
            .job
            .as_ref()
            .and_then(|j| j.print_file.clone())
            .map(|f| format!(" ({f})"))
            .unwrap_or_default();
        out.push(Alert {
            event: printer(EventKind::JobPaused, None),
            title: format!("{name}: Job paused"),
            message: format!("The Job on {name}{file} is paused."),
            level: Level::Warning,
        });
    }

    for fault in &after.status.faults {
        if !before.status.faults.iter().any(|f| f.code == fault.code) {
            out.push(Alert {
                event: printer(EventKind::FaultRaised, None),
                title: format!(
                    "{name}: {}",
                    match fault.severity {
                        gantry_core::FaultSeverity::Error => "Fault",
                        gantry_core::FaultSeverity::Warning => "Warning",
                    }
                ),
                message: fault.message.clone(),
                level: match fault.severity {
                    gantry_core::FaultSeverity::Error => Level::Bad,
                    gantry_core::FaultSeverity::Warning => Level::Warning,
                },
            });
        }
    }

    for slot in after.status.filament_slots.iter().filter(|s| s.low) {
        let was_low = before
            .status
            .filament_slots
            .iter()
            .find(|s| s.id == slot.id)
            .is_none_or(|s| s.low);
        if !was_low {
            let filament = slot.filament.as_ref();
            out.push(Alert {
                event: printer(EventKind::LowFilament, None),
                title: format!("{name}: Low filament"),
                message: format!(
                    "{} on {name} is low: {}{}.",
                    slot.label,
                    filament.map_or("filament", |f| f.kind.as_str()),
                    filament
                        .and_then(|f| f.remaining_percent)
                        .map(|p| format!(", {p}% left"))
                        .unwrap_or_default()
                ),
                level: Level::Warning,
            });
        }
    }
    out
}

/// Whether `rule` wants this event, for a Printer with these Tags.
pub fn matches(rule: &Rule, event: &Event, printer_tags: &[i64]) -> bool {
    if !rule.enabled || !rule.events.contains(&event.kind) {
        return false;
    }
    if event.kind == EventKind::JobEnded
        && !rule.outcomes.is_empty()
        && !event.outcome.is_some_and(|o| rule.outcomes.contains(&o))
    {
        return false;
    }
    let whole_fleet = rule.printers.is_empty() && rule.tags.is_empty();
    whole_fleet
        || rule.printers.contains(&event.printer_id)
        || rule.tags.iter().any(|t| printer_tags.contains(t))
}

/// Where desktop-channel Alerts go: every open desktop app's WebSocket
/// subscribes to it.
pub type DesktopAlerts = tokio::sync::broadcast::Sender<Alert>;

/// How often, and after what waits, a failed delivery is tried again.
#[derive(Debug, Clone)]
pub struct Retry(pub Vec<Duration>);

impl Default for Retry {
    fn default() -> Self {
        Self(vec![
            Duration::from_secs(2),
            Duration::from_secs(10),
            Duration::from_secs(60),
        ])
    }
}

/// Starts following the Fleet and sending Alerts. Runs until the process
/// ends.
pub fn spawn(state: &AppState) {
    let state = state.clone();
    tokio::spawn(async move {
        let mut updates = state.fleet().subscribe();
        let mut last: HashMap<String, Arc<PrinterView>> = state
            .fleet()
            .snapshot()
            .into_iter()
            .map(|v| (v.id.clone(), v))
            .collect();
        loop {
            let update = tokio::select! {
                update = updates.recv() => update,
                () = state.stopping() => return,
            };
            match update {
                Ok(FleetUpdate::Printer(view)) => {
                    if let Some(before) = last.insert(view.id.clone(), view.clone()) {
                        for alert in events(&before, &view) {
                            // Webhooks get the same events (PRD §7.7).
                            crate::webhooks::dispatch(&state, &alert).await;
                            dispatch(&state, alert).await;
                        }
                    }
                }
                Ok(FleetUpdate::Removed(id)) => {
                    last.remove(&id);
                }
                // Too slow to keep up: start again from now, missing some
                // changes rather than inventing any.
                Err(RecvError::Lagged(missed)) => {
                    tracing::warn!(
                        missed,
                        "Alerts fell behind the Fleet; some changes were skipped"
                    );
                    last = state
                        .fleet()
                        .snapshot()
                        .into_iter()
                        .map(|v| (v.id.clone(), v))
                        .collect();
                }
                Err(RecvError::Closed) => return,
            }
        }
    });
}

/// Sends `alert` to the channel of every Rule that wants it.
async fn dispatch(state: &AppState, alert: Alert) {
    let rules = match state.store().alert_rules().await {
        Ok(rules) => rules,
        Err(e) => {
            tracing::error!(error = %e, "could not read the Alert Rules");
            return;
        }
    };
    if rules.is_empty() {
        return;
    }
    let tags = state
        .store()
        .printer_tag_ids(&alert.event.printer_id)
        .await
        .unwrap_or_default();
    let mut channels: Vec<i64> = rules
        .iter()
        .filter(|r| matches(r, &alert.event, &tags))
        .map(|r| r.channel_id)
        .collect();
    // One Alert per channel, however many Rules send it there.
    channels.sort_unstable();
    channels.dedup();
    for id in channels {
        let channel = match state.store().alert_channel(state.key(), id).await {
            Ok(Some(channel)) => channel,
            Ok(None) => continue,
            Err(e) => {
                tracing::error!(error = %e, "could not read an Alert channel");
                continue;
            }
        };
        let sender = match channels::build(&channel, state.desktop_alerts()) {
            Ok(sender) => sender,
            Err(e) => {
                tracing::warn!(channel = %channel.name, error = %e, "an Alert channel isn't set up right");
                continue;
            }
        };
        let alert = alert.clone();
        let retry = state.alert_retry().clone();
        tokio::spawn(async move {
            deliver(&channel.name, sender.as_ref(), &alert, &retry).await;
        });
    }
}

/// Sends one Alert, trying again after each wait in `retry`. Returns whether
/// it got through.
pub async fn deliver(
    channel: &str,
    sender: &dyn channels::Channel,
    alert: &Alert,
    retry: &Retry,
) -> bool {
    let mut waits = retry.0.iter();
    let mut attempt = 1;
    loop {
        match sender.send(alert).await {
            Ok(()) => {
                tracing::info!(channel, printer = %alert.event.printer_name, event = ?alert.event.kind, attempt, "Alert sent");
                return true;
            }
            Err(e) => match waits.next() {
                Some(wait) => {
                    tracing::warn!(channel, attempt, error = %e, "an Alert couldn't be sent; trying again");
                    tokio::time::sleep(*wait).await;
                    attempt += 1;
                }
                None => {
                    tracing::error!(channel, attempts = attempt, error = %e, title = %alert.title, "an Alert couldn't be sent; giving up");
                    return false;
                }
            },
        }
    }
}

#[cfg(test)]
mod tests;
