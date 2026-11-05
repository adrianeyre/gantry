//! Which Printers a queued Job may run on, and which Job each free Printer
//! takes next (PRD §7.3).
//!
//! A **Compatible Printer** is one whose model matches the model the Print
//! File was sliced for, and whose Filament Slots hold every filament type the
//! Plate needs. A colour that doesn't match is a warning, never a block. The
//! model is never a warning: a Print File only ever goes to the model it was
//! sliced for.
//!
//! The Queue is first in, first out, but a Job at the front that no free
//! Printer can run doesn't hold up the ones behind it: [`plan`] walks the
//! Queue in order and gives each Job the first free Printer that can take it,
//! so the next Job that can run goes ahead.
use std::collections::{HashMap, HashSet};

use gantry_core::PrinterView;
use serde::Serialize;

use crate::api::print_files::model_mismatch;
use crate::store::print_files::{Plate, PrintFile};
use crate::store::queue::QueuedJob;

/// Whether one Printer can run one Plate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fit {
    /// It can, with these warnings (colours that don't match, filament it
    /// can't see).
    Compatible { warnings: Vec<String> },
    /// It isn't of the model the Print File was sliced for, or the Print
    /// File doesn't say which that is. Nothing overrides this.
    Incompatible(String),
    /// It is of the right model, but a filament type the Plate needs isn't
    /// loaded.
    MissingFilament(String),
}

impl Fit {
    pub fn is_compatible(&self) -> bool {
        matches!(self, Self::Compatible { .. })
    }
}

/// The filament types a Plate needs, once each, in the slicer's order.
fn needed_kinds(plate: &Plate) -> Vec<&str> {
    let mut kinds: Vec<&str> = Vec::new();
    for f in &plate.filaments {
        let kind = f.kind.trim();
        if !kind.is_empty() && !kinds.iter().any(|k| k.eq_ignore_ascii_case(kind)) {
            kinds.push(kind);
        }
    }
    kinds
}

fn same_colour(a: &str, b: &str) -> bool {
    a.trim()
        .trim_start_matches('#')
        .eq_ignore_ascii_case(b.trim().trim_start_matches('#'))
}

/// Whether `printer` is a Compatible Printer for `plate` of `file`.
///
/// A Printer that reports no Filament Slots at all (many Klipper and
/// OctoPrint machines can't say what is loaded) can't be checked, so its
/// filament is a warning rather than a block. The model is always checked.
pub fn fit(file: &PrintFile, plate: &Plate, printer: &PrinterView) -> Fit {
    if let Some(why) = model_mismatch(file, &printer.name, printer.model.as_deref()) {
        return Fit::Incompatible(why);
    }
    let mut warnings = Vec::new();
    let needed = needed_kinds(plate);
    if needed.is_empty() {
        return Fit::Compatible { warnings };
    }
    let slots = &printer.status.filament_slots;
    if slots.is_empty() {
        warnings.push(format!(
            "{} doesn't report its filament: check it has {} loaded",
            printer.name,
            needed.join(" and ")
        ));
        return Fit::Compatible { warnings };
    }
    let loaded: Vec<_> = slots.iter().filter_map(|s| s.filament.as_ref()).collect();
    let missing: Vec<&str> = needed
        .iter()
        .copied()
        .filter(|kind| !loaded.iter().any(|f| f.kind.eq_ignore_ascii_case(kind)))
        .collect();
    if !missing.is_empty() {
        return Fit::MissingFilament(format!(
            "{} has no {} loaded",
            printer.name,
            missing.join(" or ")
        ));
    }
    for wanted in &plate.filaments {
        let Some(colour) = wanted.colour.as_deref().filter(|c| !c.trim().is_empty()) else {
            continue;
        };
        let matched = loaded.iter().any(|f| {
            f.kind.eq_ignore_ascii_case(wanted.kind.trim()) && same_colour(&f.colour, colour)
        });
        if !matched {
            warnings.push(format!(
                "{} has {} loaded, but not in {colour}",
                printer.name, wanted.kind
            ));
        }
    }
    Fit::Compatible { warnings }
}

/// A free Printer the Queue gives a Job to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Assignment {
    pub queued: i64,
    pub printer_id: String,
    /// Colour mismatches and the like, which don't block it.
    pub warnings: Vec<String>,
}

/// What the Queue would do now: for each Job it can start, the Printer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    pub assignments: Vec<Assignment>,
    /// Why each Job left waiting can't start yet.
    pub waiting: HashMap<i64, String>,
}

impl Plan {
    pub fn for_job(&self, queued: i64) -> Option<&Assignment> {
        self.assignments.iter().find(|a| a.queued == queued)
    }

    pub fn for_printer(&self, printer: &str) -> Option<&Assignment> {
        self.assignments.iter().find(|a| a.printer_id == printer)
    }
}

/// Gives each free Printer at most one Job, walking the Queue in order
/// (first in, first out) and skipping a Job no free Printer can run, so the
/// next one that can goes ahead of it. `free` is in order of preference: a
/// Job any of several Printers could run goes to the first of them.
///
/// A Job for a named Printer takes it when it is free and of the right
/// model; its filament is only a warning, since an Operator chose it. A Job
/// for any Compatible Printer needs both the model and the filament types.
/// `skip` holds Jobs already being started, which nothing else may take.
pub fn plan(
    queue: &[QueuedJob],
    files: &HashMap<i64, PrintFile>,
    free: &[&PrinterView],
    skip: &HashSet<i64>,
) -> Plan {
    let mut out = Plan::default();
    let mut taken: HashSet<&str> = HashSet::new();
    for job in queue {
        if skip.contains(&job.id) {
            out.waiting.insert(job.id, "Starting…".into());
            continue;
        }
        let Some((file, plate)) = files
            .get(&job.print_file_id)
            .and_then(|f| f.plate(job.plate).map(|p| (f, p)))
        else {
            out.waiting
                .insert(job.id, "Its Print File or Plate is gone".into());
            continue;
        };
        let available = free.iter().filter(|p| !taken.contains(p.id.as_str()));
        let mut reasons = Vec::new();
        let chosen = match &job.printer_id {
            Some(wanted) => {
                let printer = available.into_iter().find(|p| &p.id == wanted).copied();
                match printer {
                    None => {
                        reasons.push("Waiting for its Printer to be ready".to_owned());
                        None
                    }
                    Some(p) => match fit(file, plate, p) {
                        // Filament on a named Printer is the Operator's call.
                        Fit::MissingFilament(why) => Some((p, vec![why])),
                        Fit::Incompatible(why) => {
                            reasons.push(why);
                            None
                        }
                        Fit::Compatible { warnings } => Some((p, warnings)),
                    },
                }
            }
            None => {
                let mut found = None;
                for p in available {
                    match fit(file, plate, p) {
                        Fit::Compatible { warnings } => {
                            found = Some((*p, warnings));
                            break;
                        }
                        Fit::Incompatible(why) | Fit::MissingFilament(why) => reasons.push(why),
                    }
                }
                if found.is_none() && reasons.is_empty() {
                    reasons.push("Waiting for a free Compatible Printer".to_owned());
                }
                found
            }
        };
        match chosen {
            Some((printer, warnings)) => {
                taken.insert(printer.id.as_str());
                out.assignments.push(Assignment {
                    queued: job.id,
                    printer_id: printer.id.clone(),
                    warnings,
                });
            }
            None => {
                out.waiting.insert(job.id, reasons.join("; "));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use gantry_core::{
        Connection, DisconnectReason, Filament, FilamentSlot, PrinterState, PrinterStatus,
    };

    use super::*;
    use crate::print_files::parse::{Kind, PlateFilament};

    fn printer(id: &str, model: &str, loaded: &[(&str, &str)]) -> PrinterView {
        let mut status = PrinterStatus::disconnected(DisconnectReason::Stopped);
        status.connection = Connection::Connected;
        status.state = Some(PrinterState::Idle);
        status.filament_slots = loaded
            .iter()
            .enumerate()
            .map(|(i, (kind, colour))| FilamentSlot {
                id: i.to_string(),
                label: format!("Slot {}", i + 1),
                filament: Some(Filament {
                    kind: (*kind).into(),
                    colour: (*colour).into(),
                    remaining_percent: Some(80),
                }),
                low: false,
            })
            .collect();
        PrinterView {
            id: id.into(),
            name: id.to_uppercase(),
            driver: "simulated".into(),
            model: Some(model.into()),
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

    fn file(id: i64, model: Option<&str>, filaments: &[(&str, Option<&str>)]) -> PrintFile {
        PrintFile {
            id,
            name: format!("file-{id}.gcode.3mf"),
            kind: Kind::Gcode3mf,
            printer_model: model.map(Into::into),
            model_source: None,
            slicer: None,
            size_bytes: 1,
            uploaded_at: time::OffsetDateTime::UNIX_EPOCH,
            uploaded_by: None,
            plates: vec![Plate {
                number: 1,
                name: None,
                estimated_secs: None,
                layers: None,
                filaments: filaments
                    .iter()
                    .map(|(kind, colour)| PlateFilament {
                        kind: (*kind).into(),
                        colour: colour.map(Into::into),
                        grams: None,
                    })
                    .collect(),
                has_thumbnail: false,
            }],
        }
    }

    fn queued(id: i64, file: i64, printer: Option<&str>) -> QueuedJob {
        QueuedJob {
            id,
            print_file_id: file,
            print_file: format!("file-{file}.gcode.3mf"),
            plate: 1,
            plate_name: None,
            estimated_secs: None,
            printer_id: printer.map(Into::into),
            position: id,
            queued_at: time::OffsetDateTime::UNIX_EPOCH,
            queued_by: None,
        }
    }

    #[test]
    fn the_model_must_match_whatever_the_filament() {
        let x1c = printer("a", "X1C", &[("PLA", "#FFFFFF")]);
        let f = file(1, Some("Bambu Lab X1 Carbon"), &[("PLA", None)]);
        assert!(fit(&f, &f.plates[0], &x1c).is_compatible());

        let p1s = printer("b", "P1S", &[("PLA", "#FFFFFF")]);
        let Fit::Incompatible(why) = fit(&f, &f.plates[0], &p1s) else {
            panic!("a P1S is not an X1C");
        };
        assert!(why.contains("sliced for"), "{why}");

        let unknown = file(2, None, &[("PLA", None)]);
        assert!(!fit(&unknown, &unknown.plates[0], &x1c).is_compatible());
    }

    #[test]
    fn every_filament_type_must_be_loaded() {
        let f = file(1, Some("X1C"), &[("PLA", None), ("PETG", None)]);
        let both = printer("a", "X1C", &[("pla", "#000000"), ("PETG", "#FF0000")]);
        assert_eq!(
            fit(&f, &f.plates[0], &both),
            Fit::Compatible { warnings: vec![] }
        );
        let pla_only = printer("b", "X1C", &[("PLA", "#000000"), ("PLA", "#FFFFFF")]);
        assert_eq!(
            fit(&f, &f.plates[0], &pla_only),
            Fit::MissingFilament("B has no PETG loaded".into())
        );
        let mut empty_slot = printer("c", "X1C", &[("PLA", "#000000")]);
        empty_slot.status.filament_slots[0].filament = None;
        assert!(!fit(&f, &f.plates[0], &empty_slot).is_compatible());
        // A Plate that names no filament fits any Printer of its model.
        let none = file(2, Some("X1C"), &[]);
        assert!(fit(&none, &none.plates[0], &pla_only).is_compatible());
    }

    #[test]
    fn a_colour_mismatch_warns_but_does_not_block() {
        let f = file(1, Some("X1C"), &[("PLA", Some("#D33A2C"))]);
        let white = printer("a", "X1C", &[("PLA", "#FFFFFF")]);
        let Fit::Compatible { warnings } = fit(&f, &f.plates[0], &white) else {
            panic!("a colour mismatch must not block");
        };
        assert_eq!(warnings, ["A has PLA loaded, but not in #D33A2C"]);
        let red = printer("b", "X1C", &[("PLA", "#d33a2c")]);
        assert_eq!(
            fit(&f, &f.plates[0], &red),
            Fit::Compatible { warnings: vec![] }
        );
    }

    #[test]
    fn a_printer_that_reports_no_slots_is_a_warning() {
        let f = file(1, Some("Voron 2.4"), &[("ABS", None)]);
        let voron = printer("v", "Voron 2.4", &[]);
        let Fit::Compatible { warnings } = fit(&f, &f.plates[0], &voron) else {
            panic!("unknown filament can't be checked, so it doesn't block");
        };
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("ABS"), "{warnings:?}");
    }

    #[test]
    fn the_queue_never_sends_a_print_file_to_another_model() {
        let files = HashMap::from([(1, file(1, Some("X1C"), &[("PLA", None)]))]);
        let p1s = printer("p1s", "P1S", &[("PLA", "#FFFFFF")]);
        let a1 = printer("a1", "A1", &[("PLA", "#FFFFFF")]);
        // Neither for any Compatible Printer, nor for one named.
        let queue = [queued(1, 1, None), queued(2, 1, Some("p1s"))];
        let plan = plan(&queue, &files, &[&p1s, &a1], &HashSet::new());
        assert!(plan.assignments.is_empty(), "{plan:?}");
        assert!(plan.waiting[&1].contains("sliced for"), "{plan:?}");
        assert!(plan.waiting[&2].contains("sliced for"), "{plan:?}");
    }

    #[test]
    fn a_job_no_free_printer_can_run_is_skipped_for_the_next() {
        let files = HashMap::from([
            (1, file(1, Some("X1C"), &[("PETG", None)])),
            (2, file(2, Some("X1C"), &[("PLA", None)])),
        ]);
        let pla = printer("a", "X1C", &[("PLA", "#FFFFFF")]);
        // The front Job needs PETG, which no free Printer has: the PLA Job
        // behind it goes ahead.
        let queue = [queued(1, 1, None), queued(2, 2, None)];
        let plan = plan(&queue, &files, &[&pla], &HashSet::new());
        assert_eq!(
            plan.assignments,
            [Assignment {
                queued: 2,
                printer_id: "a".into(),
                warnings: vec![]
            }]
        );
        assert_eq!(plan.waiting[&1], "A has no PETG loaded");
    }

    #[test]
    fn first_in_first_out_one_job_per_printer() {
        let files = HashMap::from([(1, file(1, Some("X1C"), &[("PLA", None)]))]);
        let a = printer("a", "X1C", &[("PLA", "#FFFFFF")]);
        let b = printer("b", "X1C", &[("PLA", "#FFFFFF")]);
        let queue = [queued(1, 1, None), queued(2, 1, None), queued(3, 1, None)];
        let plan = plan(&queue, &files, &[&a, &b], &HashSet::new());
        let pairs: Vec<_> = plan
            .assignments
            .iter()
            .map(|a| (a.queued, a.printer_id.as_str()))
            .collect();
        assert_eq!(pairs, [(1, "a"), (2, "b")]);
        assert_eq!(plan.waiting[&3], "Waiting for a free Compatible Printer");
    }

    #[test]
    fn a_job_for_a_named_printer_waits_for_it_and_does_not_block_others() {
        let files = HashMap::from([(1, file(1, Some("X1C"), &[("PLA", None)]))]);
        let a = printer("a", "X1C", &[("PLA", "#FFFFFF")]);
        // The front Job wants Printer b, which is busy.
        let queue = [queued(1, 1, Some("b")), queued(2, 1, None)];
        let plan = plan(&queue, &files, &[&a], &HashSet::new());
        assert_eq!(plan.for_job(2).unwrap().printer_id, "a");
        assert!(plan.for_job(1).is_none());
        // A named Printer missing a filament type still takes its Job,
        // with a warning: the Operator chose it.
        let petg = HashMap::from([(1, file(1, Some("X1C"), &[("PETG", None)]))]);
        let named = plan_for(&[queued(1, 1, Some("a"))], &petg, &[&a]);
        assert_eq!(named.for_job(1).unwrap().warnings, ["A has no PETG loaded"]);
    }

    fn plan_for(
        queue: &[QueuedJob],
        files: &HashMap<i64, PrintFile>,
        free: &[&PrinterView],
    ) -> Plan {
        plan(queue, files, free, &HashSet::new())
    }

    #[test]
    fn the_preferred_printer_takes_the_job_and_jobs_being_started_are_left_alone() {
        let files = HashMap::from([(1, file(1, Some("X1C"), &[]))]);
        let a = printer("a", "X1C", &[]);
        let b = printer("b", "X1C", &[]);
        let queue = [queued(1, 1, None), queued(2, 1, None)];
        let preferred = plan_for(&queue, &files, &[&b, &a]);
        assert_eq!(preferred.for_printer("b").unwrap().queued, 1);
        let skipping = plan(&queue, &files, &[&a, &b], &HashSet::from([1]));
        assert_eq!(skipping.for_printer("a").unwrap().queued, 2);
        assert!(skipping.for_job(1).is_none());
    }

    #[test]
    fn a_disconnected_printer_does_not_matter_here() {
        // `plan` is only given free Printers; with none, nothing starts.
        let files = HashMap::from([(1, file(1, Some("X1C"), &[]))]);
        let plan = plan_for(&[queued(1, 1, None)], &files, &[]);
        assert!(plan.assignments.is_empty());
    }
}
