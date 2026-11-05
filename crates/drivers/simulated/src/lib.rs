//! The Simulated Driver (PRD §5.6): a Printer imitated in software, with
//! behaviour that code (and, through the server, the simulation panel) can
//! control. It ships in release builds and is chosen like any other Driver.
//!
//! Each Simulated Printer runs in its own tokio task that advances a simulated
//! clock. Tests can instead build a [`SimulatedPrinter`] with
//! [`SimulatedPrinter::manual`] and step the clock with
//! [`SimulatedPrinter::advance`].
pub mod camera;

use std::any::Any;
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use gantry_core::{
    ActiveJob, Axis, Capability, CommandError, Connection, DisconnectReason, Driver, EventSender,
    Fan, Fault, FieldKind, FieldSpec, Filament, FilamentSlot, FineControl, Heater, JobOutcome,
    Light, LightId, Position, PrinterConfig, PrinterHandle, PrinterState, PrinterStatus,
    Temperature, Temperatures,
};
use serde::{Deserialize, Serialize};
use tokio::task::JoinHandle;

/// How often a running Simulated Printer advances and reports.
pub const TICK: Duration = Duration::from_millis(250);

/// The Driver id stored with each Simulated Printer.
pub const DRIVER_ID: &str = "simulated";

/// The Simulated Driver.
#[derive(Debug, Clone, Copy, Default)]
pub struct SimulatedDriver;

impl Driver for SimulatedDriver {
    fn id(&self) -> &'static str {
        DRIVER_ID
    }

    fn name(&self) -> &'static str {
        "Simulated"
    }

    fn fields(&self) -> Vec<FieldSpec> {
        vec![
            FieldSpec::new("model", "Model", FieldKind::Text)
                .help("The printer model this Simulated Printer pretends to be.")
                .default_value("Simulated".into()),
            FieldSpec::new("filament_slots", "Filament Slots", FieldKind::Number)
                .help("How many Filament Slots it has (0 to 16).")
                .default_value(4.into()),
            FieldSpec::new("control", "Can be controlled", FieldKind::Boolean)
                .help("Turn off to imitate a Printer that only reports status.")
                .default_value(true.into()),
            FieldSpec::new("camera", "Has a camera", FieldKind::Boolean)
                .help("A looping test pattern.")
                .default_value(true.into()),
            FieldSpec::new("access_code", "Access code", FieldKind::Secret).help(
                "Ignored by the Simulated Driver. It exists so the encrypted-credential path is exercised.",
            ),
        ]
    }

    fn validate(
        &self,
        settings: &serde_json::Map<String, serde_json::Value>,
        secrets: &gantry_core::Secrets,
    ) -> Vec<gantry_core::FieldError> {
        let mut errors = gantry_core::driver::validate_fields(&self.fields(), settings, secrets);
        if let Some(n) = settings
            .get("filament_slots")
            .and_then(serde_json::Value::as_f64)
            && !(0.0..=16.0).contains(&n)
        {
            errors.push(gantry_core::FieldError {
                field: "filament_slots".into(),
                message: "Must be between 0 and 16".into(),
            });
        }
        errors
    }

    fn connect(&self, printer: PrinterConfig, events: EventSender) -> Box<dyn PrinterHandle> {
        Box::new(SimulatedPrinter::spawn(&printer, events))
    }
}

/// A print started from the simulation panel or a test.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartPrint {
    /// The Print File's name, as the Printer would report it. Simulated
    /// prints are External Jobs, so this is all Gantry learns of the file.
    #[serde(default)]
    pub print_file: Option<String>,
    /// How long the print takes at speed 1, in seconds.
    pub duration_secs: f64,
    pub layers: u32,
    /// The Plate within the Print File, when known.
    #[serde(default)]
    pub plate: Option<u32>,
}

/// How long a Plate with no estimate takes to "print", in seconds.
pub const DEFAULT_UPLOADED_PRINT_SECS: u64 = 600;

/// How many layers a Plate with no layer count is given.
pub const DEFAULT_UPLOADED_LAYERS: u32 = 100;

/// Why a simulation control did nothing.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize)]
pub enum SimulationError {
    #[error("a print is already running")]
    AlreadyPrinting,
    #[error("nothing is printing")]
    NotPrinting,
    #[error("{0}")]
    Invalid(String),
    #[error("no Filament Slot `{0}`")]
    NoSuchSlot(String),
}

#[derive(Debug, Clone)]
struct SimJob {
    key: String,
    print_file: Option<String>,
    plate: Option<u32>,
    started_at: time::OffsetDateTime,
    duration: f64,
    layers: u32,
    preparation: f64,
    prepared: f64,
    elapsed: f64,
    paused: bool,
}

impl SimJob {
    fn state(&self) -> PrinterState {
        if self.paused {
            PrinterState::Paused
        } else if self.prepared < self.preparation {
            PrinterState::Preparing
        } else {
            PrinterState::Printing
        }
    }

    fn progress(&self) -> f64 {
        (self.elapsed / self.duration * 100.0).clamp(0.0, 100.0)
    }
}

/// The speed profiles a Simulated Printer offers, and how much faster than
/// standard each prints.
pub const SPEED_PROFILES: [(&str, f64); 4] = [
    ("silent", 0.5),
    ("standard", 1.0),
    ("sport", 1.25),
    ("ludicrous", 1.66),
];

/// The fans a Simulated Printer has, all of them controllable.
const FANS: [(&str, &str); 2] = [("part", "Part cooling"), ("auxiliary", "Auxiliary")];

/// The build volume, in mm, so a jog can't leave it.
const VOLUME: (f64, f64, f64) = (256.0, 256.0, 250.0);

/// How long homing, and loading or unloading filament, keep a Simulated
/// Printer Busy, in simulated seconds.
const HOMING_SECS: f64 = 3.0;
const FILAMENT_CHANGE_SECS: f64 = 5.0;

const MAX_NOZZLE: f64 = 300.0;
const MAX_BED: f64 = 120.0;

#[derive(Debug)]
struct Sim {
    connection: Connection,
    speed: f64,
    /// An index into [`SPEED_PROFILES`].
    speed_profile: usize,
    chamber_light: bool,
    work_light: bool,
    /// Fan speeds set by a fine control, which win over the automatic ones.
    fan_overrides: std::collections::BTreeMap<String, u8>,
    /// `None` until homed.
    position: Option<(f64, f64, f64)>,
    loaded_slot: Option<String>,
    /// Simulated seconds of Busy left (homing, loading filament).
    busy: f64,
    job: Option<SimJob>,
    jobs_started: u64,
    nozzle: (f64, f64),
    bed: (f64, f64),
    chamber: f64,
    slots: Vec<FilamentSlot>,
    faults: Vec<Fault>,
    capabilities: BTreeSet<Capability>,
    /// Jobs that ended while disconnected, reported on reconnection.
    unreported_ends: Vec<(String, JobOutcome)>,
    id: String,
    /// When this Simulated Printer started, so Job keys never repeat across
    /// restarts of Gantry.
    epoch: u128,
    /// When its clock last moved, so time keeps passing (and a print keeps
    /// going) while no Fleet is connected to it.
    last_tick: Option<tokio::time::Instant>,
}

const AMBIENT: f64 = 22.0;
const NOZZLE_PRINTING: f64 = 220.0;
const BED_PRINTING: f64 = 60.0;
const COLOURS: [(&str, &str); 4] = [
    ("PLA", "#F2F2F2"),
    ("PLA", "#1A1A1A"),
    ("PETG", "#D33A2C"),
    ("PLA", "#2E7D32"),
];

impl Sim {
    fn new(config: &PrinterConfig) -> Self {
        let slots = config
            .number("filament_slots")
            .unwrap_or(4.0)
            .clamp(0.0, 16.0) as usize;
        let mut capabilities = BTreeSet::from([Capability::ChamberTemperature]);
        if config.boolean("control").unwrap_or(true) {
            capabilities.extend([
                Capability::Pause,
                Capability::Resume,
                Capability::Cancel,
                Capability::UploadAndStart,
            ]);
            capabilities.extend(Capability::FINE);
            if slots == 0 {
                capabilities.remove(&Capability::FilamentLoad);
            }
        }
        if slots > 0 {
            capabilities.insert(Capability::FilamentSlots);
        }
        if config.boolean("camera").unwrap_or(true) {
            capabilities.insert(Capability::Camera);
        }
        Self {
            connection: Connection::Connecting,
            speed: 1.0,
            speed_profile: 1,
            chamber_light: true,
            work_light: false,
            fan_overrides: std::collections::BTreeMap::new(),
            position: None,
            loaded_slot: None,
            busy: 0.0,
            job: None,
            jobs_started: 0,
            nozzle: (AMBIENT, 0.0),
            bed: (AMBIENT, 0.0),
            chamber: AMBIENT,
            slots: (0..slots)
                .map(|i| FilamentSlot {
                    id: i.to_string(),
                    label: format!("Slot {}", i + 1),
                    // The last of four starts empty, so both cases show.
                    filament: (i < 3).then(|| {
                        let (kind, colour) = COLOURS[i % COLOURS.len()];
                        Filament {
                            kind: kind.into(),
                            colour: colour.into(),
                            remaining_percent: Some(100 - (i as u8 * 15)),
                        }
                    }),
                    low: false,
                })
                .collect(),
            faults: Vec::new(),
            capabilities,
            unreported_ends: Vec::new(),
            id: config.id.clone(),
            last_tick: None,
            epoch: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis()),
        }
    }

    fn status(&self) -> PrinterStatus {
        let connected = self.connection.is_connected();
        if !connected {
            return PrinterStatus {
                connection: self.connection.clone(),
                ..PrinterStatus::disconnected(DisconnectReason::Stopped)
            };
        }
        let job = self.job.as_ref().map(|j| ActiveJob {
            key: j.key.clone(),
            print_file: j.print_file.clone(),
            plate: j.plate,
            progress: (j.progress() * 10.0).round() / 10.0,
            layer: Some(((j.progress() / 100.0 * f64::from(j.layers)).ceil() as u32).max(1)),
            total_layers: Some(j.layers),
            started_at: Some(j.started_at),
            remaining_secs: Some(
                (((j.duration - j.elapsed).max(0.0) / SPEED_PROFILES[self.speed_profile].1
                    + (j.preparation - j.prepared).max(0.0))
                    / self.speed)
                    .round() as u64,
            ),
        });
        let heating = |(current, target): (f64, f64)| Temperature {
            current: (current * 10.0).round() / 10.0,
            target: (target > 0.0).then_some(target),
        };
        let has = |c: Capability| self.capabilities.contains(&c);
        let automatic = |id: &str| match id {
            "part"
                if self
                    .job
                    .as_ref()
                    .is_some_and(|j| j.state() == PrinterState::Printing) =>
            {
                100
            }
            "auxiliary" if self.job.is_some() => 40,
            _ => 0,
        };
        let state = match &self.job {
            Some(job) => job.state(),
            None if self.busy > 0.0 => PrinterState::Busy,
            None => PrinterState::Idle,
        };
        let mut lights = Vec::new();
        if has(Capability::ChamberLight) {
            lights.push(Light {
                id: LightId::Chamber,
                on: self.chamber_light,
            });
        }
        if has(Capability::WorkLight) {
            lights.push(Light {
                id: LightId::Work,
                on: self.work_light,
            });
        }
        PrinterStatus {
            connection: Connection::Connected,
            state: Some(state),
            job,
            temperatures: Temperatures {
                nozzle: Some(heating(self.nozzle)),
                bed: Some(heating(self.bed)),
                chamber: self
                    .capabilities
                    .contains(&Capability::ChamberTemperature)
                    .then(|| Temperature {
                        current: (self.chamber * 10.0).round() / 10.0,
                        target: None,
                    }),
            },
            fans: FANS
                .iter()
                .map(|(id, name)| Fan {
                    id: (*id).into(),
                    name: (*name).into(),
                    percent: self
                        .fan_overrides
                        .get(*id)
                        .copied()
                        .unwrap_or_else(|| automatic(id)),
                    controllable: has(Capability::Fans),
                })
                .collect(),
            filament_slots: self.slots.clone(),
            faults: self.faults.clone(),
            speed_profile: has(Capability::SpeedProfile)
                .then(|| SPEED_PROFILES[self.speed_profile].0.to_owned()),
            speed_profiles: if has(Capability::SpeedProfile) {
                SPEED_PROFILES
                    .iter()
                    .map(|(p, _)| (*p).to_owned())
                    .collect()
            } else {
                Vec::new()
            },
            lights,
            position: self.position.map(|(x, y, z)| Position { x, y, z }),
            loaded_slot: self.loaded_slot.clone(),
            capabilities: self.capabilities.clone(),
            control_withdrawn: None,
        }
    }

    /// Moves simulated time on by `real` seconds of wall time.
    fn advance(&mut self, real: f64) -> Option<(String, JobOutcome)> {
        let dt = real * self.speed;
        self.busy = (self.busy - dt).max(0.0);
        // The speed profile changes how fast a print goes, not the clock.
        let factor = SPEED_PROFILES[self.speed_profile].1;
        let approach = |value: &mut f64, target: f64| {
            let goal = if target > 0.0 { target } else { AMBIENT };
            *value += (goal - *value) * (1.0 - (-dt / 8.0).exp());
        };
        approach(&mut self.nozzle.0, self.nozzle.1);
        approach(&mut self.bed.0, self.bed.1);
        let chamber_goal = if self.job.is_some() { 35.0 } else { AMBIENT };
        self.chamber += (chamber_goal - self.chamber) * (1.0 - (-dt / 60.0).exp());

        let job = self.job.as_mut()?;
        if job.paused {
            return None;
        }
        // Time left over from preparing carries on into printing.
        let preparing = (job.preparation - job.prepared).clamp(0.0, dt);
        job.prepared += preparing;
        job.elapsed += (dt - preparing) * factor;
        if job.elapsed < job.duration {
            return None;
        }
        Some(self.finish(JobOutcome::Succeeded))
    }

    fn finish(&mut self, outcome: JobOutcome) -> (String, JobOutcome) {
        let job = self.job.take().expect("a job to finish");
        self.nozzle.1 = 0.0;
        self.bed.1 = 0.0;
        (job.key, outcome)
    }
}

struct Shared {
    sim: Mutex<Sim>,
    /// The Fleet it reports to now; replaced when a Fleet connects again.
    events: Mutex<EventSender>,
    /// The settings it was made with: a Printer whose settings changed gets
    /// a new Simulated Printer.
    settings: serde_json::Map<String, serde_json::Value>,
    camera: Option<camera::TestPatternCamera>,
    /// The model it pretends to be.
    model: String,
}

/// Every Simulated Printer made by [`SimulatedPrinter::spawn`], by Printer id.
///
/// A real printer keeps printing while Gantry restarts or reconnects, so a
/// Simulated Printer outlives its connection too: connecting the same Printer
/// again (a restart of the Fleet, or an edit that changes nothing it uses)
/// finds the same machine, still printing. It lives as long as the process.
fn machines() -> &'static Mutex<std::collections::HashMap<String, Arc<Shared>>> {
    static MACHINES: std::sync::OnceLock<Mutex<std::collections::HashMap<String, Arc<Shared>>>> =
        std::sync::OnceLock::new();
    MACHINES.get_or_init(Default::default)
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, Sim> {
        self.sim
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Reports the current status, plus any Job ending, unless disconnected.
    fn report(&self, sim: &mut Sim, ended: Option<(String, JobOutcome)>) {
        if let Some(end) = ended {
            sim.unreported_ends.push(end);
        }
        let events = self.events.lock().expect("events lock").clone();
        if sim.connection.is_connected() {
            for (key, outcome) in sim.unreported_ends.drain(..) {
                events.job_ended(key, outcome);
            }
        }
        events.status(sim.status());
    }
}

/// One Simulated Printer. Cheap to clone; every clone controls the same Printer.
#[derive(Clone)]
pub struct SimulatedPrinter {
    shared: Arc<Shared>,
    task: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl SimulatedPrinter {
    /// A Simulated Printer running in its own task: it connects after a
    /// moment, then advances in real time.
    pub fn spawn(config: &PrinterConfig, events: EventSender) -> Self {
        let existing = (!config.id.is_empty())
            .then(|| {
                machines()
                    .lock()
                    .expect("machines lock")
                    .get(&config.id)
                    .filter(|m| m.settings == config.settings)
                    .cloned()
            })
            .flatten();
        let printer = match existing {
            Some(shared) => {
                *shared.events.lock().expect("events lock") = events;
                Self {
                    shared,
                    task: Arc::new(Mutex::new(None)),
                }
            }
            None => {
                let printer = Self::manual(config, events);
                if !config.id.is_empty() {
                    machines()
                        .lock()
                        .expect("machines lock")
                        .insert(config.id.clone(), printer.shared.clone());
                }
                printer
            }
        };
        {
            // Connecting afresh, unless the simulation has dropped it.
            let mut sim = printer.shared.lock();
            if !matches!(sim.connection, Connection::Disconnected { .. }) {
                sim.connection = Connection::Connecting;
            }
        }
        let shared = printer.shared.clone();
        let task = tokio::spawn(async move {
            shared.report(&mut shared.lock(), None);
            tokio::time::sleep(Duration::from_millis(300)).await;
            {
                let mut sim = shared.lock();
                if sim.connection == Connection::Connecting {
                    sim.connection = Connection::Connected;
                }
                shared.report(&mut sim, None);
            }
            let mut ticker = tokio::time::interval(TICK);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                if shared.events.lock().expect("events lock").is_closed() {
                    break;
                }
                let now = tokio::time::Instant::now();
                let mut sim = shared.lock();
                // Since the last tick, including any time no Fleet was here.
                let real = sim.last_tick.map_or(0.0, |last| (now - last).as_secs_f64());
                sim.last_tick = Some(now);
                let ended = sim.advance(real);
                shared.report(&mut sim, ended);
            }
        });
        *printer.task.lock().expect("task lock") = Some(task);
        printer
    }

    /// A Simulated Printer with no task: already connected, and its clock
    /// moves only when [`advance`](Self::advance) is called.
    pub fn manual(config: &PrinterConfig, events: EventSender) -> Self {
        let mut sim = Sim::new(config);
        sim.connection = Connection::Connected;
        if config.id.is_empty() {
            sim.id = "simulated".into();
        }
        Self {
            shared: Arc::new(Shared {
                camera: config
                    .boolean("camera")
                    .unwrap_or(true)
                    .then(camera::TestPatternCamera::new),
                sim: Mutex::new(sim),
                events: Mutex::new(events),
                settings: config.settings.clone(),
                model: config.text("model").unwrap_or("Simulated").to_owned(),
            }),
            task: Arc::new(Mutex::new(None)),
        }
    }

    fn change<T>(&self, f: impl FnOnce(&mut Sim) -> T) -> T {
        let mut sim = self.shared.lock();
        let result = f(&mut sim);
        self.shared.report(&mut sim, None);
        result
    }

    /// Advances the simulated clock by `real` of wall time (at the current
    /// speed) and reports.
    pub fn advance(&self, real: Duration) {
        let mut sim = self.shared.lock();
        let ended = sim.advance(real.as_secs_f64());
        self.shared.report(&mut sim, ended);
    }

    /// How many camera streams are open on it right now.
    pub fn open_camera_streams(&self) -> usize {
        self.shared
            .camera
            .as_ref()
            .map_or(0, camera::TestPatternCamera::open_streams)
    }

    /// Whether its task is still running (always `false` for a manual one).
    pub fn is_running(&self) -> bool {
        self.task
            .lock()
            .expect("task lock")
            .as_ref()
            .is_some_and(|t| !t.is_finished())
    }

    /// The status it would report now.
    pub fn status(&self) -> PrinterStatus {
        self.shared.lock().status()
    }

    /// Starts a print, as if from the Printer's own screen: Gantry will see an
    /// External Job. Returns the Job's key.
    pub fn start_print(&self, print: StartPrint) -> Result<String, SimulationError> {
        if !(print.duration_secs.is_finite() && print.duration_secs > 0.0) {
            return Err(SimulationError::Invalid(
                "the duration must be more than 0 seconds".into(),
            ));
        }
        if print.layers == 0 {
            return Err(SimulationError::Invalid(
                "there must be at least 1 layer".into(),
            ));
        }
        self.change(|sim| {
            if sim.job.is_some() {
                return Err(SimulationError::AlreadyPrinting);
            }
            sim.jobs_started += 1;
            let key = format!("{}-{}-{}", sim.id, sim.epoch, sim.jobs_started);
            sim.job = Some(SimJob {
                key: key.clone(),
                print_file: print.print_file.filter(|f| !f.trim().is_empty()),
                plate: print.plate,
                started_at: time::OffsetDateTime::now_utc(),
                duration: print.duration_secs,
                layers: print.layers,
                preparation: (print.duration_secs * 0.05).clamp(0.5, 30.0),
                prepared: 0.0,
                elapsed: 0.0,
                paused: false,
            });
            sim.nozzle.1 = NOZZLE_PRINTING;
            sim.bed.1 = BED_PRINTING;
            Ok(key)
        })
    }

    /// Ends the running print as Failed, as if it had failed on the Printer.
    pub fn fail_print(&self) -> Result<(), SimulationError> {
        let mut sim = self.shared.lock();
        if sim.job.is_none() {
            return Err(SimulationError::NotPrinting);
        }
        let ended = sim.finish(JobOutcome::Failed);
        self.shared.report(&mut sim, Some(ended));
        Ok(())
    }

    /// Changes how fast simulated time runs: 1 is real time.
    pub fn set_speed(&self, speed: f64) -> Result<(), SimulationError> {
        if !(speed.is_finite() && (0.1..=1000.0).contains(&speed)) {
            return Err(SimulationError::Invalid(
                "the speed must be between 0.1 and 1000".into(),
            ));
        }
        self.change(|sim| sim.speed = speed);
        Ok(())
    }

    pub fn speed(&self) -> f64 {
        self.shared.lock().speed
    }

    /// Raises a Fault, replacing any with the same code.
    pub fn raise_fault(&self, fault: Fault) {
        self.change(|sim| {
            sim.faults.retain(|f| f.code != fault.code);
            sim.faults.push(fault);
        });
    }

    /// Clears the Fault with this code. Clearing one that isn't raised is fine.
    pub fn clear_fault(&self, code: &str) {
        self.change(|sim| sim.faults.retain(|f| f.code != code));
    }

    /// Drops the Connection, as if the Printer fell off the network. It keeps
    /// printing; Gantry just can't see it.
    pub fn drop_connection(&self, reason: DisconnectReason) {
        self.change(|sim| sim.connection = Connection::disconnected(reason));
    }

    /// Restores a dropped Connection.
    pub fn restore_connection(&self) {
        self.change(|sim| sim.connection = Connection::Connected);
    }

    /// Changes what a Filament Slot holds (`None` empties it).
    pub fn set_filament_slot(
        &self,
        slot: &str,
        filament: Option<Filament>,
    ) -> Result<(), SimulationError> {
        if let Some(f) = &filament
            && !valid_colour(&f.colour)
        {
            return Err(SimulationError::Invalid(
                "the colour must be #RRGGBB".into(),
            ));
        }
        self.change(|sim| {
            let target = sim
                .slots
                .iter_mut()
                .find(|s| s.id == slot)
                .ok_or_else(|| SimulationError::NoSuchSlot(slot.to_owned()))?;
            target.filament = filament;
            Ok(())
        })
    }

    fn command(
        &self,
        f: impl FnOnce(&mut Sim) -> Result<Option<JobOutcome>, CommandError>,
    ) -> Result<(), CommandError> {
        let mut sim = self.shared.lock();
        if let Connection::Disconnected { reason, .. } = sim.connection {
            return Err(CommandError::Disconnected { reason });
        }
        let ended = f(&mut sim)?.map(|outcome| sim.finish(outcome));
        self.shared.report(&mut sim, ended);
        Ok(())
    }
}

/// Refuses a move or a filament change while a Job runs or the Printer is busy.
fn require_idle(sim: &Sim, what: &str) -> Result<(), CommandError> {
    if sim.job.is_some() {
        return Err(CommandError::invalid_state(format!(
            "can't {what} while a Job is running"
        )));
    }
    if sim.busy > 0.0 {
        return Err(CommandError::invalid_state(format!(
            "can't {what} while the Printer is busy"
        )));
    }
    Ok(())
}

/// Carries out one fine control on the simulation.
fn fine(sim: &mut Sim, control: FineControl) -> Result<(), CommandError> {
    require(sim, control.capability())?;
    match control {
        FineControl::SetSpeedProfile { profile } => {
            let index = SPEED_PROFILES
                .iter()
                .position(|(p, _)| *p == profile)
                .ok_or_else(|| CommandError::invalid(format!("no speed profile `{profile}`")))?;
            sim.speed_profile = index;
        }
        FineControl::SetTemperature { heater, target } => {
            let (max, value) = match heater {
                Heater::Nozzle => (MAX_NOZZLE, &mut sim.nozzle.1),
                Heater::Bed => (MAX_BED, &mut sim.bed.1),
            };
            if target > max {
                return Err(CommandError::invalid(format!(
                    "this Printer's {} goes up to {max} °C",
                    if heater == Heater::Nozzle {
                        "nozzle"
                    } else {
                        "bed"
                    }
                )));
            }
            *value = target;
        }
        FineControl::SetLight { light, on } => match light {
            LightId::Chamber => sim.chamber_light = on,
            LightId::Work => sim.work_light = on,
        },
        FineControl::SetFan { fan, percent } => {
            if !FANS.iter().any(|(id, _)| *id == fan) {
                return Err(CommandError::invalid(format!("no fan `{fan}`")));
            }
            sim.fan_overrides.insert(fan, percent);
        }
        FineControl::Home => {
            require_idle(sim, "home")?;
            sim.position = Some((0.0, 0.0, 0.0));
            sim.busy = HOMING_SECS;
        }
        FineControl::Jog { axis, distance_mm } => {
            require_idle(sim, "jog")?;
            let (x, y, z) = sim
                .position
                .ok_or_else(|| CommandError::invalid_state("home the Printer before jogging"))?;
            let (value, limit) = match axis {
                Axis::X => (x + distance_mm, VOLUME.0),
                Axis::Y => (y + distance_mm, VOLUME.1),
                Axis::Z => (z + distance_mm, VOLUME.2),
            };
            if !(0.0..=limit).contains(&value) {
                return Err(CommandError::invalid_state(format!(
                    "that would move {} to {value:.1} mm, outside 0 to {limit} mm",
                    match axis {
                        Axis::X => "X",
                        Axis::Y => "Y",
                        Axis::Z => "Z",
                    }
                )));
            }
            sim.position = Some(match axis {
                Axis::X => (value, y, z),
                Axis::Y => (x, value, z),
                Axis::Z => (x, y, value),
            });
        }
        FineControl::LoadFilament { slot } => {
            require_idle(sim, "load filament")?;
            let found = sim
                .slots
                .iter()
                .find(|s| s.id == slot)
                .ok_or_else(|| CommandError::invalid(format!("no Filament Slot `{slot}`")))?;
            if found.filament.is_none() {
                return Err(CommandError::invalid_state(format!(
                    "{} is empty",
                    found.label
                )));
            }
            sim.loaded_slot = Some(slot);
            sim.busy = FILAMENT_CHANGE_SECS;
        }
        FineControl::UnloadFilament => {
            require_idle(sim, "unload filament")?;
            if sim.loaded_slot.take().is_none() {
                return Err(CommandError::invalid_state("no filament is loaded"));
            }
            sim.busy = FILAMENT_CHANGE_SECS;
        }
    }
    Ok(())
}

fn valid_colour(colour: &str) -> bool {
    colour.len() == 7
        && colour.starts_with('#')
        && colour[1..].chars().all(|c| c.is_ascii_hexdigit())
}

fn require(sim: &Sim, capability: Capability) -> Result<(), CommandError> {
    if sim.capabilities.contains(&capability) {
        Ok(())
    } else {
        Err(CommandError::Unsupported)
    }
}

#[async_trait]
impl PrinterHandle for SimulatedPrinter {
    async fn pause(&self) -> Result<(), CommandError> {
        self.command(|sim| {
            require(sim, Capability::Pause)?;
            match sim.job.as_mut() {
                Some(job) if !job.paused => {
                    job.paused = true;
                    Ok(None)
                }
                Some(_) => Err(CommandError::invalid_state("the print is already paused")),
                None => Err(CommandError::invalid_state("nothing is printing")),
            }
        })
    }

    async fn resume(&self) -> Result<(), CommandError> {
        self.command(|sim| {
            require(sim, Capability::Resume)?;
            match sim.job.as_mut() {
                Some(job) if job.paused => {
                    job.paused = false;
                    Ok(None)
                }
                Some(_) => Err(CommandError::invalid_state("the print is not paused")),
                None => Err(CommandError::invalid_state("nothing is printing")),
            }
        })
    }

    async fn cancel(&self) -> Result<(), CommandError> {
        self.command(|sim| {
            require(sim, Capability::Cancel)?;
            if sim.job.is_none() {
                return Err(CommandError::invalid_state("nothing is printing"));
            }
            Ok(Some(JobOutcome::Cancelled))
        })
    }

    async fn fine(&self, control: FineControl) -> Result<(), CommandError> {
        self.command(|sim| fine(sim, control).map(|()| None))
    }

    /// "Prints" the Plate for its estimated time (PRD §7.3). Nothing is
    /// stored: the Simulated Printer only needs the name and the estimate.
    async fn upload_and_start(
        &self,
        upload: gantry_core::PrintUpload,
    ) -> Result<Option<String>, CommandError> {
        {
            let sim = self.shared.lock();
            if let Connection::Disconnected { reason, .. } = sim.connection {
                return Err(CommandError::Disconnected { reason });
            }
            require(&sim, Capability::UploadAndStart)?;
            require_idle(&sim, "start a Job")?;
        }
        if upload.data.is_empty() {
            return Err(CommandError::invalid("the Print File is empty"));
        }
        let key = self
            .start_print(StartPrint {
                print_file: Some(upload.file_name),
                duration_secs: upload
                    .estimated_secs
                    .filter(|s| *s > 0)
                    .unwrap_or(DEFAULT_UPLOADED_PRINT_SECS) as f64,
                layers: upload
                    .layers
                    .filter(|l| *l > 0)
                    .unwrap_or(DEFAULT_UPLOADED_LAYERS),
                plate: upload.plate,
            })
            .map_err(|e| CommandError::invalid_state(e.to_string()))?;
        Ok(Some(key))
    }

    fn model(&self) -> Option<String> {
        Some(self.shared.model.clone())
    }

    fn camera(&self) -> Option<Arc<dyn gantry_core::CameraSource>> {
        self.shared
            .camera
            .clone()
            .map(|c| Arc::new(c) as Arc<dyn gantry_core::CameraSource>)
    }

    async fn shutdown(&self) {
        if let Some(task) = self.task.lock().expect("task lock").take() {
            task.abort();
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests;
