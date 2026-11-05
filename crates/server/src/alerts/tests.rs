//! Which changes are Alert events, and which Rules want them.
use gantry_core::{
    Connection, DisconnectReason, Fault, FaultSeverity, Filament, FilamentSlot, JobOutcome,
    JobRecord, PrinterState, PrinterStatus, PrinterView,
};

use super::*;

fn view(state: PrinterState) -> PrinterView {
    let mut status = PrinterStatus::disconnected(DisconnectReason::Stopped);
    status.connection = Connection::Connected;
    status.state = Some(state);
    PrinterView {
        id: "p1".into(),
        name: "X1".into(),
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

fn ended(outcome: JobOutcome) -> JobRecord {
    JobRecord {
        id: 7,
        printer_id: "p1".into(),
        external: false,
        driver_key: "k".into(),
        print_file: Some("tray.gcode.3mf".into()),
        print_file_id: Some(1),
        plate: Some(2),
        started_at: time::OffsetDateTime::now_utc(),
        ended_at: Some(time::OffsetDateTime::now_utc()),
        outcome: Some(outcome),
        overruled: false,
    }
}

fn kinds(alerts: &[Alert]) -> Vec<EventKind> {
    alerts.iter().map(|a| a.event.kind).collect()
}

#[test]
fn a_job_ending_is_one_event_with_its_outcome() {
    let before = view(PrinterState::Printing);
    let mut after = view(PrinterState::AwaitingClear);
    after.awaiting_clear = Some(ended(JobOutcome::Failed));
    after.last_ended = after.awaiting_clear.clone();
    let alerts = events(&before, &after);
    assert_eq!(kinds(&alerts), [EventKind::JobEnded]);
    assert!(alerts[0].message.ends_with("It is Awaiting Clear."));
    assert_eq!(alerts[0].event.outcome, Some(JobOutcome::Failed));
    assert_eq!(alerts[0].title, "X1: Job failed");
    assert!(alerts[0].message.contains("tray.gcode.3mf (Plate 2)"));
    // The same view again is no new event.
    assert!(events(&after, &after).is_empty());
}

#[test]
fn an_auto_cleared_ending_is_still_an_event() {
    let before = view(PrinterState::Printing);
    let mut after = view(PrinterState::Idle);
    after.last_ended = Some(ended(JobOutcome::Succeeded));
    let alerts = events(&before, &after);
    assert_eq!(kinds(&alerts), [EventKind::JobEnded]);
    assert!(
        alerts[0]
            .message
            .ends_with("Auto-Clear has made it Idle again.")
    );
    assert!(events(&after, &after).is_empty());
}

#[test]
fn pausing_faults_and_low_filament_are_events_once_each() {
    let mut before = view(PrinterState::Printing);
    before.status.filament_slots = vec![FilamentSlot {
        id: "0".into(),
        label: "Slot 1".into(),
        filament: Some(Filament {
            kind: "PLA".into(),
            colour: "#FFFFFF".into(),
            remaining_percent: Some(20),
        }),
        low: false,
    }];
    let mut after = before.clone();
    after.status.state = Some(PrinterState::Paused);
    after.status.faults.push(Fault {
        code: "runout".into(),
        severity: FaultSeverity::Error,
        message: "Filament ran out".into(),
    });
    after.status.filament_slots[0].low = true;
    let alerts = events(&before, &after);
    assert_eq!(
        kinds(&alerts),
        [
            EventKind::JobPaused,
            EventKind::FaultRaised,
            EventKind::LowFilament
        ]
    );
    assert_eq!(alerts[1].message, "Filament ran out");
    assert!(
        alerts[2].message.contains("Slot 1"),
        "{}",
        alerts[2].message
    );
    assert!(events(&after, &after).is_empty());
}

#[test]
fn losing_the_connection_is_an_event_but_stopping_is_not() {
    let before = view(PrinterState::Idle);
    let mut after = view(PrinterState::Idle);
    after.status = PrinterStatus::disconnected(DisconnectReason::Unreachable);
    assert_eq!(kinds(&events(&before, &after)), [EventKind::ConnectionLost]);
    after.status = PrinterStatus::disconnected(DisconnectReason::Stopped);
    assert!(events(&before, &after).is_empty());
    // Faults already there when it connects are the baseline, not news.
    let mut connected = view(PrinterState::Idle);
    connected.status.faults.push(Fault {
        code: "door".into(),
        severity: FaultSeverity::Warning,
        message: "Door open".into(),
    });
    let mut connecting = view(PrinterState::Idle);
    connecting.status = PrinterStatus::connecting();
    assert!(events(&connecting, &connected).is_empty());
}

fn rule(events: Vec<EventKind>) -> Rule {
    Rule {
        id: 1,
        name: "r".into(),
        enabled: true,
        events,
        outcomes: vec![],
        printers: vec![],
        tags: vec![],
        channel_id: 1,
    }
}

fn event(kind: EventKind, outcome: Option<JobOutcome>) -> Event {
    Event {
        kind,
        printer_id: "p1".into(),
        printer_name: "X1".into(),
        outcome,
    }
}

#[test]
fn rules_match_events_outcomes_printers_and_tags() {
    let fleet = rule(vec![EventKind::FaultRaised]);
    assert!(matches(&fleet, &event(EventKind::FaultRaised, None), &[]));
    assert!(!matches(&fleet, &event(EventKind::JobPaused, None), &[]));

    let mut failures = rule(vec![EventKind::JobEnded]);
    failures.outcomes = vec![JobOutcome::Failed, JobOutcome::Lost];
    assert!(matches(
        &failures,
        &event(EventKind::JobEnded, Some(JobOutcome::Failed)),
        &[]
    ));
    assert!(!matches(
        &failures,
        &event(EventKind::JobEnded, Some(JobOutcome::Succeeded)),
        &[]
    ));

    let mut one = rule(vec![EventKind::FaultRaised]);
    one.printers = vec!["p2".into()];
    assert!(!matches(&one, &event(EventKind::FaultRaised, None), &[]));
    one.printers = vec!["p1".into()];
    assert!(matches(&one, &event(EventKind::FaultRaised, None), &[]));

    let mut tagged = rule(vec![EventKind::FaultRaised]);
    tagged.tags = vec![5];
    assert!(!matches(
        &tagged,
        &event(EventKind::FaultRaised, None),
        &[4]
    ));
    assert!(matches(
        &tagged,
        &event(EventKind::FaultRaised, None),
        &[4, 5]
    ));

    let mut off = rule(vec![EventKind::FaultRaised]);
    off.enabled = false;
    assert!(!matches(&off, &event(EventKind::FaultRaised, None), &[]));
}
