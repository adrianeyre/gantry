//! History and analytics (PRD §7.5), worked out from what is recorded:
//!
//! - [`summarise`]: a Printer's Job Outcomes, success and failure rates,
//!   durations, filament used and its cost. An Outcome overruled at Mark
//!   Clear is already recorded as Failed, and External Jobs count like any
//!   other.
//! - [`filament_used`]: how much of each filament a Job used, and which spool
//!   it came from, so the inventory goes down and the Job has a cost.
//! - [`breakdown`] and [`by_day`]: utilisation (time printing against time
//!   Idle, Busy or Awaiting Clear) and uptime (time connected), from the
//!   recorded stretches of each Printer's Connection and Printer State.
use gantry_core::{JobOutcome, JobRecord, PrinterState};
use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime};

/// A Job as History shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HistoryJob {
    #[serde(flatten)]
    pub job: JobRecord,
    /// From start to end, once it has ended.
    pub duration_secs: Option<i64>,
    /// Grams of filament, when known.
    pub filament_g: Option<f64>,
    /// What that filament cost, when its spool is known.
    pub filament_cost: Option<f64>,
    pub filament_usage: Vec<FilamentUse>,
    /// How many frames the Job's timelapse has, when it has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timelapse_frames: Option<u32>,
}

/// One Printer's History, summed up.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Summary {
    pub jobs: u32,
    pub running: u32,
    pub succeeded: u32,
    /// Includes Succeeded Outcomes an Operator overruled to Failed.
    pub failed: u32,
    pub cancelled: u32,
    pub lost: u32,
    /// Of the Jobs that ended, the share that Succeeded, 0 to 1.
    pub success_rate: Option<f64>,
    /// Of the Jobs that ended, the share that Failed, 0 to 1.
    pub failure_rate: Option<f64>,
    pub total_duration_secs: i64,
    pub average_duration_secs: Option<i64>,
    pub filament_g: f64,
    pub filament_cost: f64,
}

/// Sums up a set of Jobs.
pub fn summarise(jobs: &[HistoryJob]) -> Summary {
    let mut s = Summary {
        jobs: u32::try_from(jobs.len()).unwrap_or(u32::MAX),
        ..Summary::default()
    };
    let mut timed = 0i64;
    for j in jobs {
        match j.job.outcome {
            None => s.running += 1,
            Some(JobOutcome::Succeeded) => s.succeeded += 1,
            Some(JobOutcome::Failed) => s.failed += 1,
            Some(JobOutcome::Cancelled) => s.cancelled += 1,
            Some(JobOutcome::Lost) => s.lost += 1,
        }
        if let Some(d) = j.duration_secs {
            s.total_duration_secs += d;
            timed += 1;
        }
        s.filament_g += j.filament_g.unwrap_or(0.0);
        s.filament_cost += j.filament_cost.unwrap_or(0.0);
    }
    let ended = s.succeeded + s.failed + s.cancelled + s.lost;
    if ended > 0 {
        s.success_rate = Some(f64::from(s.succeeded) / f64::from(ended));
        s.failure_rate = Some(f64::from(s.failed) / f64::from(ended));
    }
    if timed > 0 {
        s.average_duration_secs = Some(s.total_duration_secs / timed);
    }
    s
}

/// How long a Job took, once it has ended.
pub fn duration(job: &JobRecord) -> Option<i64> {
    job.ended_at
        .map(|end| (end - job.started_at).whole_seconds().max(0))
}

/// One filament a Job used.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FilamentUse {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colour: Option<String>,
    pub grams: f64,
    /// The spool it came from, when one was loaded on the Printer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spool_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
}

/// A spool, as far as charging filament to it goes.
#[derive(Debug, Clone, PartialEq)]
pub struct SpoolStock {
    pub id: i64,
    pub kind: String,
    pub colour: String,
    pub weight_g: f64,
    pub cost: f64,
}

impl SpoolStock {
    /// What one gram of it cost.
    pub fn cost_per_gram(&self) -> f64 {
        if self.weight_g > 0.0 {
            self.cost / self.weight_g
        } else {
            0.0
        }
    }
}

/// What a Job used of each filament its Plate needs: `fraction` (0 to 1) of
/// the slicer's grams, charged to the Printer's loaded spool of the same type
/// (one of the same colour first). A filament with no grams known is left
/// out; one with no matching spool has no cost.
pub fn filament_used(
    plate: &[(String, Option<String>, Option<f64>)],
    fraction: f64,
    loaded: &[SpoolStock],
) -> Vec<FilamentUse> {
    let fraction = if fraction.is_finite() {
        fraction.clamp(0.0, 1.0)
    } else {
        0.0
    };
    plate
        .iter()
        .filter_map(|(kind, colour, grams)| {
            let grams = grams.filter(|g| *g > 0.0)? * fraction;
            let same_kind = |s: &&SpoolStock| s.kind.eq_ignore_ascii_case(kind.trim());
            let spool = loaded
                .iter()
                .filter(same_kind)
                .find(|s| {
                    colour
                        .as_deref()
                        .is_some_and(|c| s.colour.eq_ignore_ascii_case(c.trim()))
                })
                .or_else(|| loaded.iter().find(same_kind));
            Some(FilamentUse {
                kind: kind.clone(),
                colour: colour.clone(),
                grams: (grams * 100.0).round() / 100.0,
                spool_id: spool.map(|s| s.id),
                cost: spool.map(|s| ((grams * s.cost_per_gram()) * 100.0).round() / 100.0),
            })
        })
        .collect()
}

/// What a Printer was doing during one recorded stretch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Activity {
    /// A Job running: Preparing, Printing or Paused.
    Printing,
    Idle,
    Busy,
    AwaitingClear,
    /// Not connected (or still connecting).
    Disconnected,
}

impl Activity {
    pub fn of(connected: bool, state: Option<PrinterState>) -> Self {
        if !connected {
            return Self::Disconnected;
        }
        match state {
            Some(s) if s.has_job() => Self::Printing,
            Some(PrinterState::Busy) => Self::Busy,
            Some(PrinterState::AwaitingClear) => Self::AwaitingClear,
            _ => Self::Idle,
        }
    }
}

/// One recorded stretch of a Printer's Connection and Printer State.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Span {
    pub activity: Activity,
    pub start: OffsetDateTime,
    pub end: OffsetDateTime,
}

/// Seconds spent each way over a range. Time no stretch covers is unknown.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct Breakdown {
    pub printing_secs: f64,
    pub idle_secs: f64,
    pub busy_secs: f64,
    pub awaiting_clear_secs: f64,
    pub disconnected_secs: f64,
    pub unknown_secs: f64,
}

impl Breakdown {
    pub fn connected_secs(&self) -> f64 {
        self.printing_secs + self.idle_secs + self.busy_secs + self.awaiting_clear_secs
    }

    /// Of the time connected, the share spent printing, 0 to 1.
    pub fn utilisation(&self) -> Option<f64> {
        let connected = self.connected_secs();
        (connected > 0.0).then(|| self.printing_secs / connected)
    }

    /// Of the time known, the share spent connected, 0 to 1.
    pub fn uptime(&self) -> Option<f64> {
        let known = self.connected_secs() + self.disconnected_secs;
        (known > 0.0).then(|| self.connected_secs() / known)
    }

    fn add(&mut self, activity: Activity, secs: f64) {
        match activity {
            Activity::Printing => self.printing_secs += secs,
            Activity::Idle => self.idle_secs += secs,
            Activity::Busy => self.busy_secs += secs,
            Activity::AwaitingClear => self.awaiting_clear_secs += secs,
            Activity::Disconnected => self.disconnected_secs += secs,
        }
    }

    /// Both together, as for the whole Fleet.
    pub fn plus(&self, other: &Self) -> Self {
        Self {
            printing_secs: self.printing_secs + other.printing_secs,
            idle_secs: self.idle_secs + other.idle_secs,
            busy_secs: self.busy_secs + other.busy_secs,
            awaiting_clear_secs: self.awaiting_clear_secs + other.awaiting_clear_secs,
            disconnected_secs: self.disconnected_secs + other.disconnected_secs,
            unknown_secs: self.unknown_secs + other.unknown_secs,
        }
    }
}

/// How one Printer's time from `from` to `to` was spent. `now` caps the
/// range, so the future is never unknown time.
pub fn breakdown(
    spans: &[Span],
    from: OffsetDateTime,
    to: OffsetDateTime,
    now: OffsetDateTime,
) -> Breakdown {
    let to = to.min(now);
    let mut out = Breakdown::default();
    if to <= from {
        return out;
    }
    let mut covered = 0.0;
    for span in spans {
        let start = span.start.max(from);
        let end = span.end.min(to);
        if end > start {
            let secs = (end - start).as_seconds_f64();
            out.add(span.activity, secs);
            covered += secs;
        }
    }
    out.unknown_secs = ((to - from).as_seconds_f64() - covered).max(0.0);
    out
}

/// One day of a breakdown.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Day {
    /// `YYYY-MM-DD`, in UTC.
    pub date: String,
    #[serde(flatten)]
    pub breakdown: Breakdown,
    pub utilisation: Option<f64>,
    pub uptime: Option<f64>,
}

/// A breakdown for each UTC day from `from` up to `to`.
pub fn by_day(
    spans: &[Span],
    from: OffsetDateTime,
    to: OffsetDateTime,
    now: OffsetDateTime,
) -> Vec<Day> {
    let mut days = Vec::new();
    let mut day = from.replace_time(time::Time::MIDNIGHT);
    while day < to {
        let next = day + Duration::days(1);
        let b = breakdown(spans, day.max(from), next.min(to), now);
        days.push(Day {
            date: day.date().to_string(),
            utilisation: b.utilisation(),
            uptime: b.uptime(),
            breakdown: b,
        });
        day = next;
    }
    days
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::*;

    fn job(id: i64, outcome: Option<JobOutcome>, minutes: Option<i64>) -> HistoryJob {
        let started = datetime!(2026-11-01 10:00 UTC);
        let record = JobRecord {
            id,
            printer_id: "p".into(),
            external: id % 2 == 0,
            driver_key: format!("k{id}"),
            print_file: None,
            print_file_id: None,
            plate: None,
            started_at: started,
            ended_at: minutes.map(|m| started + Duration::minutes(m)),
            outcome,
            overruled: false,
        };
        HistoryJob {
            duration_secs: duration(&record),
            job: record,
            filament_g: Some(10.0),
            filament_cost: Some(0.25),
            filament_usage: vec![],
            timelapse_frames: None,
        }
    }

    #[test]
    fn rates_and_totals_for_a_seeded_set_of_jobs() {
        let mut overruled = job(4, Some(JobOutcome::Failed), Some(20));
        overruled.job.overruled = true;
        let jobs = [
            job(1, Some(JobOutcome::Succeeded), Some(60)),
            job(2, Some(JobOutcome::Succeeded), Some(30)),
            job(3, Some(JobOutcome::Failed), Some(10)),
            overruled,
            job(5, Some(JobOutcome::Cancelled), Some(5)),
            job(6, Some(JobOutcome::Lost), None),
            job(7, None, None),
        ];
        let s = summarise(&jobs);
        assert_eq!(s.jobs, 7);
        assert_eq!(s.running, 1);
        assert_eq!(s.succeeded, 2);
        assert_eq!(s.failed, 2, "the overruled one counts as Failed");
        assert_eq!(s.cancelled, 1);
        assert_eq!(s.lost, 1);
        assert_eq!(s.success_rate, Some(2.0 / 6.0));
        assert_eq!(s.failure_rate, Some(2.0 / 6.0));
        assert_eq!(s.total_duration_secs, (60 + 30 + 10 + 20 + 5) * 60);
        assert_eq!(s.average_duration_secs, Some(25 * 60));
        assert!((s.filament_g - 70.0).abs() < 1e-9);
        assert!((s.filament_cost - 1.75).abs() < 1e-9);
    }

    #[test]
    fn no_jobs_has_no_rates() {
        let s = summarise(&[]);
        assert_eq!(s.jobs, 0);
        assert_eq!(s.success_rate, None);
        assert_eq!(s.average_duration_secs, None);
    }

    fn spool(id: i64, kind: &str, colour: &str) -> SpoolStock {
        SpoolStock {
            id,
            kind: kind.into(),
            colour: colour.into(),
            weight_g: 1000.0,
            cost: 20.0,
        }
    }

    #[test]
    fn filament_is_charged_to_the_loaded_spool_of_its_type_and_colour() {
        let plate = [
            ("PLA".to_owned(), Some("#FFFFFF".to_owned()), Some(40.0)),
            ("PETG".to_owned(), Some("#D33A2C".to_owned()), Some(10.0)),
            ("TPU".to_owned(), None, None),
        ];
        let loaded = [
            spool(1, "PLA", "#000000"),
            spool(2, "pla", "#ffffff"),
            spool(3, "PETG", "#00FF00"),
        ];
        let used = filament_used(&plate, 1.0, &loaded);
        assert_eq!(used.len(), 2, "no grams known for the TPU");
        assert_eq!(used[0].spool_id, Some(2), "the white PLA");
        assert_eq!(used[0].grams, 40.0);
        assert_eq!(used[0].cost, Some(0.8), "40 g at 2p a gram");
        assert_eq!(
            used[1].spool_id,
            Some(3),
            "the only PETG, whatever its colour"
        );
        assert_eq!(used[1].cost, Some(0.2));
    }

    #[test]
    fn a_job_that_stopped_part_way_used_part_of_its_filament() {
        let plate = [("PLA".to_owned(), None, Some(50.0))];
        let used = filament_used(&plate, 0.3, &[]);
        assert_eq!(used[0].grams, 15.0);
        assert_eq!(used[0].spool_id, None);
        assert_eq!(used[0].cost, None, "no spool, no cost");
        assert_eq!(filament_used(&plate, 7.0, &[])[0].grams, 50.0, "capped");
    }

    fn span(activity: Activity, from: (u8, u8), to: (u8, u8)) -> Span {
        let at = |(h, m): (u8, u8)| {
            datetime!(2026-11-01 00:00 UTC)
                + Duration::hours(h.into())
                + Duration::minutes(m.into())
        };
        Span {
            activity,
            start: at(from),
            end: at(to),
        }
    }

    #[test]
    fn utilisation_and_uptime_come_from_recorded_state_changes() {
        let spans = [
            span(Activity::Idle, (8, 0), (9, 0)),
            span(Activity::Printing, (9, 0), (12, 0)),
            span(Activity::AwaitingClear, (12, 0), (12, 30)),
            span(Activity::Busy, (12, 30), (13, 0)),
            span(Activity::Disconnected, (13, 0), (14, 0)),
            // 14:00 to 15:00: Gantry was stopped, so unknown.
            span(Activity::Printing, (15, 0), (16, 0)),
        ];
        let from = datetime!(2026-11-01 08:00 UTC);
        let to = datetime!(2026-11-01 16:00 UTC);
        let b = breakdown(&spans, from, to, to);
        assert_eq!(b.printing_secs, 4.0 * 3600.0);
        assert_eq!(b.idle_secs, 3600.0);
        assert_eq!(b.awaiting_clear_secs, 1800.0);
        assert_eq!(b.busy_secs, 1800.0);
        assert_eq!(b.disconnected_secs, 3600.0);
        assert_eq!(b.unknown_secs, 3600.0);
        assert_eq!(b.utilisation(), Some(4.0 / 6.0));
        assert_eq!(b.uptime(), Some(6.0 / 7.0));
    }

    #[test]
    fn a_range_cuts_stretches_and_never_counts_the_future() {
        let spans = [span(Activity::Printing, (0, 0), (23, 0))];
        let from = datetime!(2026-11-01 10:00 UTC);
        let now = datetime!(2026-11-01 12:00 UTC);
        let b = breakdown(&spans, from, datetime!(2026-11-02 00:00 UTC), now);
        assert_eq!(b.printing_secs, 2.0 * 3600.0);
        assert_eq!(b.unknown_secs, 0.0);
    }

    #[test]
    fn days_split_at_midnight_and_the_fleet_adds_up() {
        let night = Span {
            activity: Activity::Printing,
            start: datetime!(2026-11-01 22:00 UTC),
            end: datetime!(2026-11-02 02:00 UTC),
        };
        let days = by_day(
            &[night],
            datetime!(2026-11-01 00:00 UTC),
            datetime!(2026-11-03 00:00 UTC),
            datetime!(2026-11-03 00:00 UTC),
        );
        assert_eq!(days.len(), 2);
        assert_eq!(days[0].date, "2026-11-01");
        assert_eq!(days[0].breakdown.printing_secs, 7200.0);
        assert_eq!(days[1].breakdown.printing_secs, 7200.0);
        assert_eq!(days[0].utilisation, Some(1.0));
        let fleet = days[0].breakdown.plus(&days[1].breakdown);
        assert_eq!(fleet.printing_secs, 4.0 * 3600.0);
    }

    #[test]
    fn what_a_printer_is_doing_maps_to_an_activity() {
        assert_eq!(
            Activity::of(false, Some(PrinterState::Printing)),
            Activity::Disconnected
        );
        assert_eq!(
            Activity::of(true, Some(PrinterState::Paused)),
            Activity::Printing
        );
        assert_eq!(
            Activity::of(true, Some(PrinterState::Preparing)),
            Activity::Printing
        );
        assert_eq!(
            Activity::of(true, Some(PrinterState::AwaitingClear)),
            Activity::AwaitingClear
        );
        assert_eq!(Activity::of(true, Some(PrinterState::Busy)), Activity::Busy);
        assert_eq!(Activity::of(true, Some(PrinterState::Idle)), Activity::Idle);
    }
}
