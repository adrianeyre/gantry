//! The running Fleet: one connection task (owned by its Driver) and one
//! supervisor task per Printer.
//!
//! The supervisor folds a Printer's events into a [`PrinterView`] and
//! broadcasts each change. Nothing here holds a lock across an `.await`, and
//! every command is bounded by a timeout, so one misbehaving Printer can't
//! stall another.
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use serde::Serialize;
use time::OffsetDateTime;
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::task::JoinHandle;

use crate::driver::{
    CommandError, Driver, DriverEvent, EventReceiver, EventSender, FineControl, PrintUpload,
    PrinterConfig, PrinterHandle, PrinterId, UPLOAD_TIMEOUT,
};
use crate::job::{JobId, JobLog, JobOutcome, JobRecord, JobStart, JobState, START_GRACE};
use crate::registry::DriverRegistry;
use crate::status::{ActiveJob, Capability, Connection, Fault, PrinterState, PrinterStatus};

/// How long a command may take before the Fleet gives up on it.
pub const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);

/// The default low-filament threshold, in percent remaining (PRD §7.1).
pub const DEFAULT_LOW_FILAMENT_PERCENT: u8 = 15;

/// What the UI was last told about a Printer while it was connected.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LastSeen {
    pub state: PrinterState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<f64>,
    /// The Faults it was reporting then.
    pub faults: Vec<Fault>,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
}

/// A Printer as the UI sees it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PrinterView {
    pub id: PrinterId,
    pub name: String,
    pub driver: String,
    /// The printer model, when the Driver knows it, such as `X1C`.
    pub model: Option<String>,
    pub status: PrinterStatus,
    /// The last state seen while connected, shown while disconnected
    /// ("Disconnected (last seen: printing at 43%)").
    pub last_seen: Option<LastSeen>,
    /// The Job running now, as recorded.
    pub job: Option<JobRecord>,
    /// The ended Job the Printer is Awaiting Clear after.
    pub awaiting_clear: Option<JobRecord>,
    /// The Job that ended most recently, cleared or not. With Auto-Clear a
    /// Succeeded Job never waits in [`awaiting_clear`](Self::awaiting_clear),
    /// so this is how anything following the Fleet sees every ending.
    pub last_ended: Option<JobRecord>,
    /// Auto-Clear (PRD §6.3): after a Succeeded Job the Printer goes straight
    /// to Idle. Failed, Cancelled and Lost Jobs still need Mark Clear.
    pub auto_clear: bool,
    /// The capabilities the Driver reports, before any Admin override.
    pub reported_capabilities: BTreeSet<Capability>,
    /// An Admin's overrides (PRD §6.1): `true` adds a capability, `false`
    /// removes it. `status.capabilities` already has them applied.
    pub capability_overrides: Overrides,
}

/// An Admin's capability overrides for one Printer.
pub type Overrides = BTreeMap<Capability, bool>;

/// A Printer's Gantry-side settings, applied from its first status on.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StartOptions {
    pub overrides: Overrides,
    /// Off by default.
    pub auto_clear: bool,
}

/// A change to the Fleet.
#[derive(Debug, Clone)]
pub enum FleetUpdate {
    Printer(Arc<PrinterView>),
    Removed(PrinterId),
}

/// A command to one Printer: `{ "command": "pause" }`, or a fine control
/// such as `{ "command": "set_temperature", "heater": "nozzle", "target": 210 }`.
#[derive(Debug, Clone, PartialEq, serde::Deserialize, Serialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Command {
    Pause,
    Resume,
    Cancel,
    #[serde(untagged)]
    Fine(FineControl),
}

impl Command {
    /// The capability the Printer must have for this command.
    pub fn capability(&self) -> Capability {
        match self {
            Self::Pause => Capability::Pause,
            Self::Resume => Capability::Resume,
            Self::Cancel => Capability::Cancel,
            Self::Fine(control) => control.capability(),
        }
    }
}

/// Why the Fleet couldn't do something.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FleetError {
    #[error("no Printer with id {0}")]
    UnknownPrinter(PrinterId),
    #[error("no Driver called `{0}` is compiled into this Gantry")]
    UnknownDriver(String),
    #[error("a Printer with id {0} is already running")]
    AlreadyRunning(PrinterId),
    #[error(transparent)]
    Command(#[from] CommandError),
}

/// A request to a Printer's supervisor, answered on `reply`.
enum Control {
    MarkClear {
        overrule: bool,
        reply: oneshot::Sender<Result<JobRecord, FleetError>>,
    },
    Overrides(Overrides),
    AutoClear(bool),
    /// Gantry is starting a Job: record it, if the Printer can take one.
    Begin {
        start: JobStart,
        reply: oneshot::Sender<Result<JobRecord, FleetError>>,
    },
    /// The start failed: forget the Job, then answer.
    Abandon {
        job: JobId,
        reply: oneshot::Sender<()>,
    },
    /// The Driver said at once what it calls the new print.
    Keyed {
        job: JobId,
        key: String,
    },
    /// Something the view is built from changed (the low-filament threshold).
    Refresh,
}

struct Entry {
    driver: Arc<dyn Driver>,
    handle: Arc<dyn PrinterHandle>,
    control: mpsc::UnboundedSender<Control>,
    view: watch::Receiver<Arc<PrinterView>>,
    supervisor: JoinHandle<()>,
}

struct Inner {
    registry: DriverRegistry,
    log: Arc<dyn JobLog>,
    printers: RwLock<HashMap<PrinterId, Arc<Entry>>>,
    updates: broadcast::Sender<FleetUpdate>,
    command_timeout: Duration,
    low_filament: Arc<AtomicU8>,
}

/// The running Fleet. Cheap to clone.
#[derive(Clone)]
pub struct Fleet {
    inner: Arc<Inner>,
}

impl Fleet {
    /// A Fleet that records Jobs in `log`.
    pub fn new(registry: DriverRegistry, log: Arc<dyn JobLog>) -> Self {
        Self::with_command_timeout(registry, log, DEFAULT_COMMAND_TIMEOUT)
    }

    pub fn with_command_timeout(
        registry: DriverRegistry,
        log: Arc<dyn JobLog>,
        command_timeout: Duration,
    ) -> Self {
        let (updates, _) = broadcast::channel(1024);
        Self {
            inner: Arc::new(Inner {
                registry,
                log,
                printers: RwLock::new(HashMap::new()),
                updates,
                command_timeout,
                low_filament: Arc::new(AtomicU8::new(DEFAULT_LOW_FILAMENT_PERCENT)),
            }),
        }
    }

    pub fn registry(&self) -> &DriverRegistry {
        &self.inner.registry
    }

    /// Starts a Printer: its Driver connects in its own task, and a supervisor
    /// task follows its events. `jobs` is what the Job log says about it, so
    /// an open Job and Awaiting Clear survive a restart.
    pub fn start(
        &self,
        driver_id: &str,
        config: PrinterConfig,
        jobs: JobState,
    ) -> Result<(), FleetError> {
        self.start_with_overrides(driver_id, config, jobs, Overrides::new())
    }

    /// [`start`](Self::start), with an Admin's capability overrides applied
    /// from the first status on.
    pub fn start_with_overrides(
        &self,
        driver_id: &str,
        config: PrinterConfig,
        jobs: JobState,
        overrides: Overrides,
    ) -> Result<(), FleetError> {
        self.start_with(
            driver_id,
            config,
            jobs,
            StartOptions {
                overrides,
                ..StartOptions::default()
            },
        )
    }

    /// [`start`](Self::start), with the Printer's Gantry-side settings.
    pub fn start_with(
        &self,
        driver_id: &str,
        config: PrinterConfig,
        jobs: JobState,
        options: StartOptions,
    ) -> Result<(), FleetError> {
        let driver = self
            .inner
            .registry
            .get(driver_id)
            .ok_or_else(|| FleetError::UnknownDriver(driver_id.to_owned()))?;
        let id = config.id.clone();
        let mut printers = self.inner.printers.write().expect("fleet lock");
        if printers.contains_key(&id) {
            return Err(FleetError::AlreadyRunning(id));
        }
        let (events, receiver) = EventSender::channel();
        let name = config.name.clone();
        let handle: Arc<dyn PrinterHandle> = Arc::from(driver.connect(config, events));
        let initial = Arc::new(PrinterView {
            id: id.clone(),
            name,
            driver: driver_id.to_owned(),
            model: handle.model(),
            status: PrinterStatus::connecting(),
            last_seen: None,
            job: jobs.open,
            last_ended: jobs.awaiting_clear.clone(),
            awaiting_clear: jobs.awaiting_clear,
            auto_clear: options.auto_clear,
            reported_capabilities: BTreeSet::new(),
            capability_overrides: options.overrides,
        });
        let (view_tx, view_rx) = watch::channel(initial.clone());
        let (control, control_rx) = mpsc::unbounded_channel();
        let supervisor = tokio::spawn(
            Supervisor {
                reported: initial.status.clone(),
                view: (*initial).clone(),
                log: self.inner.log.clone(),
                view_tx,
                updates: self.inner.updates.clone(),
                low_filament: self.inner.low_filament.clone(),
            }
            .run(receiver, control_rx),
        );
        printers.insert(
            id,
            Arc::new(Entry {
                driver,
                handle,
                control,
                view: view_rx,
                supervisor,
            }),
        );
        drop(printers);
        let _ = self.inner.updates.send(FleetUpdate::Printer(initial));
        Ok(())
    }

    /// Stops a Printer's tasks and forgets it.
    pub async fn stop(&self, id: &str) -> Result<(), FleetError> {
        let entry = self
            .inner
            .printers
            .write()
            .expect("fleet lock")
            .remove(id)
            .ok_or_else(|| FleetError::UnknownPrinter(id.to_owned()))?;
        // Bounded, so a Driver that won't shut down can't hold up removal.
        let _ = tokio::time::timeout(self.inner.command_timeout, entry.handle.shutdown()).await;
        entry.supervisor.abort();
        let _ = self.inner.updates.send(FleetUpdate::Removed(id.to_owned()));
        Ok(())
    }

    /// Stops every Printer.
    pub async fn stop_all(&self) {
        for id in self.ids() {
            let _ = self.stop(&id).await;
        }
    }

    /// Every Printer's id.
    pub fn ids(&self) -> Vec<PrinterId> {
        self.inner
            .printers
            .read()
            .expect("fleet lock")
            .keys()
            .cloned()
            .collect()
    }

    pub fn contains(&self, id: &str) -> bool {
        self.inner
            .printers
            .read()
            .expect("fleet lock")
            .contains_key(id)
    }

    fn entry(&self, id: &str) -> Result<Arc<Entry>, FleetError> {
        self.inner
            .printers
            .read()
            .expect("fleet lock")
            .get(id)
            .cloned()
            .ok_or_else(|| FleetError::UnknownPrinter(id.to_owned()))
    }

    /// One Printer's current view.
    pub fn view(&self, id: &str) -> Result<Arc<PrinterView>, FleetError> {
        Ok(self.entry(id)?.view.borrow().clone())
    }

    /// Every Printer's current view, sorted by name.
    pub fn snapshot(&self) -> Vec<Arc<PrinterView>> {
        let mut views: Vec<_> = self
            .inner
            .printers
            .read()
            .expect("fleet lock")
            .values()
            .map(|e| e.view.borrow().clone())
            .collect();
        views.sort_by_key(|v| v.name.to_lowercase());
        views
    }

    /// Follows every change. Subscribe before taking a [`snapshot`](Self::snapshot)
    /// so nothing is missed between the two.
    pub fn subscribe(&self) -> broadcast::Receiver<FleetUpdate> {
        self.inner.updates.subscribe()
    }

    /// Waits until `id`'s view satisfies `pred`, or `timeout` passes.
    pub async fn wait_for(
        &self,
        id: &str,
        timeout: Duration,
        pred: impl Fn(&PrinterView) -> bool,
    ) -> Result<Arc<PrinterView>, FleetError> {
        let mut rx = self.entry(id)?.view.clone();
        let wait = async {
            loop {
                let view = rx.borrow_and_update().clone();
                if pred(&view) {
                    return Ok(view);
                }
                if rx.changed().await.is_err() {
                    return Err(FleetError::UnknownPrinter(id.to_owned()));
                }
            }
        };
        tokio::time::timeout(timeout, wait)
            .await
            .map_err(|_| FleetError::Command(CommandError::TimedOut))?
    }

    /// The Printer's connection handle, for Driver-specific extras.
    pub fn handle(&self, id: &str) -> Result<Arc<dyn PrinterHandle>, FleetError> {
        Ok(self.entry(id)?.handle.clone())
    }

    /// The Printer's Driver.
    pub fn driver(&self, id: &str) -> Result<Arc<dyn Driver>, FleetError> {
        Ok(self.entry(id)?.driver.clone())
    }

    /// Mark Clear: the Operator confirms the bed is empty, and the Printer
    /// leaves Awaiting Clear. Only offered in Awaiting Clear. With
    /// `overrule_to_failed`, a Succeeded Job is recorded as Failed instead;
    /// no other Outcome can be overruled (PRD §6.3).
    pub async fn mark_clear(
        &self,
        id: &str,
        overrule_to_failed: bool,
    ) -> Result<JobRecord, FleetError> {
        let entry = self.entry(id)?;
        let (reply, answer) = oneshot::channel();
        entry
            .control
            .send(Control::MarkClear {
                overrule: overrule_to_failed,
                reply,
            })
            .map_err(|_| FleetError::UnknownPrinter(id.to_owned()))?;
        tokio::time::timeout(self.inner.command_timeout, answer)
            .await
            .map_err(|_| CommandError::TimedOut)?
            .map_err(|_| FleetError::UnknownPrinter(id.to_owned()))?
    }

    /// Replaces a Printer's capability overrides. They apply at once, and to
    /// every status the Driver reports from now on.
    pub fn set_overrides(&self, id: &str, overrides: Overrides) -> Result<(), FleetError> {
        self.entry(id)?
            .control
            .send(Control::Overrides(overrides))
            .map_err(|_| FleetError::UnknownPrinter(id.to_owned()))
    }

    /// Turns Auto-Clear on or off for a Printer (PRD §6.3). It applies to
    /// the next Job that ends; a Printer already Awaiting Clear stays so.
    pub fn set_auto_clear(&self, id: &str, on: bool) -> Result<(), FleetError> {
        self.entry(id)?
            .control
            .send(Control::AutoClear(on))
            .map_err(|_| FleetError::UnknownPrinter(id.to_owned()))
    }

    /// The low-filament threshold, in percent remaining.
    pub fn low_filament_threshold(&self) -> u8 {
        self.inner.low_filament.load(Ordering::Relaxed)
    }

    /// Changes the low-filament threshold: a Filament Slot at or below it is
    /// marked low. 0 marks none. Applies at once to every Printer.
    pub fn set_low_filament_threshold(&self, percent: u8) {
        self.inner
            .low_filament
            .store(percent.min(100), Ordering::Relaxed);
        for entry in self.inner.printers.read().expect("fleet lock").values() {
            let _ = entry.control.send(Control::Refresh);
        }
    }

    /// Sends a Plate to a Printer and starts the Job (PRD §7.3): records the
    /// Job linked to its Print File and Plate, then has the Driver upload and
    /// start it. Refused unless the Printer is connected, Idle (not Awaiting
    /// Clear) and can upload. If the upload fails, the Job is forgotten and
    /// the error returned: a failed start leaves no Job.
    pub async fn start_job(
        &self,
        id: &str,
        start: JobStart,
        upload: PrintUpload,
    ) -> Result<JobRecord, FleetError> {
        let entry = self.entry(id)?;
        let status = entry.view.borrow().status.clone();
        check_command(&status, Capability::UploadAndStart)?;
        let (reply, answer) = oneshot::channel();
        entry
            .control
            .send(Control::Begin { start, reply })
            .map_err(|_| FleetError::UnknownPrinter(id.to_owned()))?;
        let job = tokio::time::timeout(self.inner.command_timeout, answer)
            .await
            .map_err(|_| CommandError::TimedOut)?
            .map_err(|_| FleetError::UnknownPrinter(id.to_owned()))??;
        let handle = entry.handle.clone();
        let started = tokio::time::timeout(UPLOAD_TIMEOUT, handle.upload_and_start(upload))
            .await
            .map_err(|_| CommandError::TimedOut)
            .and_then(|r| r);
        match started {
            Ok(key) => {
                if let Some(key) = key {
                    let _ = entry.control.send(Control::Keyed { job: job.id, key });
                }
                Ok(job)
            }
            Err(e) => {
                // Answered once the Job is gone, so the caller never sees it.
                let (reply, done) = oneshot::channel();
                if entry
                    .control
                    .send(Control::Abandon { job: job.id, reply })
                    .is_ok()
                {
                    let _ = tokio::time::timeout(self.inner.command_timeout, done).await;
                }
                Err(e.into())
            }
        }
    }

    /// Sends a command. Refused at once, and never held for later, when the
    /// Printer is not connected or lacks the capability (PRD §6.4).
    pub async fn command(&self, id: &str, command: Command) -> Result<(), FleetError> {
        let entry = self.entry(id)?;
        let status = entry.view.borrow().status.clone();
        check_command(&status, command.capability())?;
        if let Command::Fine(control) = &command {
            control.validate()?;
        }
        let handle = entry.handle.clone();
        let run = async {
            match command {
                Command::Pause => handle.pause().await,
                Command::Resume => handle.resume().await,
                Command::Cancel => handle.cancel().await,
                Command::Fine(control) => handle.fine(control).await,
            }
        };
        tokio::time::timeout(self.inner.command_timeout, run)
            .await
            .map_err(|_| CommandError::TimedOut)??;
        Ok(())
    }
}

/// Refuses a command a Printer can't take right now.
pub fn check_command(status: &PrinterStatus, capability: Capability) -> Result<(), CommandError> {
    match &status.connection {
        Connection::Connected => {}
        Connection::Connecting => {
            return Err(CommandError::invalid_state(
                "the Printer is still connecting",
            ));
        }
        Connection::Disconnected { reason, .. } => {
            return Err(CommandError::Disconnected { reason: *reason });
        }
    }
    if !status.has(capability) {
        return Err(CommandError::Unsupported);
    }
    Ok(())
}

/// Follows one Printer's events: keeps its view, records its Jobs, and puts
/// it in Awaiting Clear after each one.
struct Supervisor {
    view: PrinterView,
    /// The status as the Driver reported it, before Gantry's own overlay.
    reported: PrinterStatus,
    log: Arc<dyn JobLog>,
    view_tx: watch::Sender<Arc<PrinterView>>,
    updates: broadcast::Sender<FleetUpdate>,
    low_filament: Arc<AtomicU8>,
}

impl Supervisor {
    async fn run(
        mut self,
        mut events: EventReceiver,
        mut control: mpsc::UnboundedReceiver<Control>,
    ) {
        // A Driver whose events end (its task finished or panicked) still
        // has a Printer the UI shows, so Mark Clear and overrides keep working.
        let mut driver_gone = false;
        loop {
            tokio::select! {
                event = events.recv(), if !driver_gone => match event {
                    Some(DriverEvent::Status(status)) => self.on_status(*status).await,
                    Some(DriverEvent::JobEnded { key, outcome }) => {
                        self.on_job_ended(key, outcome).await;
                    }
                    None => {
                        driver_gone = true;
                        continue;
                    }
                },
                request = control.recv() => match request {
                    None => break,
                    Some(Control::MarkClear { overrule, reply }) => {
                        let _ = reply.send(self.mark_clear(overrule).await);
                    }
                    Some(Control::Overrides(overrides)) => {
                        self.view.capability_overrides = overrides;
                    }
                    Some(Control::AutoClear(on)) => self.view.auto_clear = on,
                    Some(Control::Begin { start, reply }) => {
                        let _ = reply.send(self.begin(start).await);
                    }
                    Some(Control::Abandon { job, reply }) => {
                        self.abandon(job).await;
                        let _ = reply.send(());
                    }
                    Some(Control::Keyed { job, key }) => self.keyed(job, &key).await,
                    Some(Control::Refresh) => {}
                },
            }
            self.publish();
        }
    }

    async fn mark_clear(&mut self, overrule: bool) -> Result<JobRecord, FleetError> {
        let job = match (&self.view.awaiting_clear, self.view.status.state) {
            (Some(job), Some(PrinterState::AwaitingClear)) => job.clone(),
            _ => {
                return Err(CommandError::invalid_state(
                    "Mark Clear is only for a Printer that is Awaiting Clear",
                )
                .into());
            }
        };
        if overrule && job.outcome != Some(JobOutcome::Succeeded) {
            return Err(CommandError::invalid_state(
                "only a Succeeded Job can be overruled, and only to Failed",
            )
            .into());
        }
        let record = self
            .log
            .bed_cleared(&self.view.id, job.id, overrule)
            .await
            .map_err(|e| CommandError::Refused {
                message: e.to_string(),
            })?;
        self.view.awaiting_clear = None;
        if self
            .view
            .last_ended
            .as_ref()
            .is_some_and(|j| j.id == record.id)
        {
            self.view.last_ended = Some(record.clone());
        }
        Ok(record)
    }

    /// Records a Job Gantry is starting, if the Printer can take one now.
    async fn begin(&mut self, start: JobStart) -> Result<JobRecord, FleetError> {
        let state = self.view.status.state;
        if self.view.awaiting_clear.is_some() || state == Some(PrinterState::AwaitingClear) {
            return Err(CommandError::invalid_state(
                "the Printer is Awaiting Clear: Mark Clear before starting another Job",
            )
            .into());
        }
        if self.view.job.is_some() || state != Some(PrinterState::Idle) {
            return Err(CommandError::invalid_state(
                "the Printer isn't Idle, so it can't start a Job",
            )
            .into());
        }
        let record = self
            .log
            .job_started(&self.view.id, &start, None)
            .await
            .map_err(|e| CommandError::Refused {
                message: e.to_string(),
            })?;
        self.view.job = Some(record.clone());
        Ok(record)
    }

    async fn abandon(&mut self, job: JobId) {
        if self.view.job.as_ref().is_some_and(|j| j.id == job) {
            self.view.job = None;
        }
        if let Err(e) = self.log.job_abandoned(job).await {
            tracing::warn!(printer = %self.view.id, error = %e, "could not forget a Job that didn't start");
        }
    }

    async fn keyed(&mut self, job: JobId, key: &str) {
        let pending = self
            .view
            .job
            .as_ref()
            .is_some_and(|j| j.id == job && j.is_pending());
        if !pending {
            return;
        }
        match self.log.rekeyed(job, key).await {
            Ok(record) => self.view.job = Some(record),
            Err(e) => tracing::warn!(printer = %self.view.id, error = %e, "could not key a Job"),
        }
    }

    fn publish(&mut self) {
        let mut status = self.reported.clone();
        let threshold = self.low_filament.load(Ordering::Relaxed);
        for slot in &mut status.filament_slots {
            slot.low = threshold > 0
                && slot
                    .filament
                    .as_ref()
                    .and_then(|f| f.remaining_percent)
                    .is_some_and(|left| left <= threshold);
        }
        self.view.reported_capabilities = status.capabilities.clone();
        // An Admin's word beats the Driver's, but only while connected: a
        // disconnected Printer can do nothing.
        if status.connection.is_connected() {
            for (capability, enabled) in &self.view.capability_overrides {
                if *enabled {
                    status.capabilities.insert(*capability);
                } else {
                    status.capabilities.remove(capability);
                }
            }
        }
        // Awaiting Clear is Gantry's, not the Driver's: an idle Printer whose
        // last Job hasn't been cleared is Awaiting Clear.
        if self.view.awaiting_clear.is_some()
            && status.connection.is_connected()
            && status.state == Some(PrinterState::Idle)
        {
            status.state = Some(PrinterState::AwaitingClear);
        }
        if let (Connection::Connected, Some(state)) = (&status.connection, status.state) {
            self.view.last_seen = Some(LastSeen {
                state,
                progress: status.job.as_ref().map(|j| j.progress),
                faults: status.faults.clone(),
                at: OffsetDateTime::now_utc(),
            });
        }
        self.view.status = status;
        let published = Arc::new(self.view.clone());
        let _ = self.view_tx.send(published.clone());
        let _ = self.updates.send(FleetUpdate::Printer(published));
    }

    async fn on_status(&mut self, status: PrinterStatus) {
        if status.connection.is_connected() {
            match &status.job {
                Some(active) => self.job_running(active).await,
                // Connected, idle, and the open Job isn't running: it ended
                // without Gantry seeing how (PRD §7.5). A Driver that knows
                // how reports the ending first, which closes the Job before
                // this status arrives.
                None if status.state == Some(PrinterState::Idle) => {
                    // A Job Gantry has just started may not show yet: the
                    // Printer is still taking the upload, or starting.
                    let starting = self.view.job.as_ref().is_some_and(|j| {
                        j.is_pending() && OffsetDateTime::now_utc() - j.started_at < START_GRACE
                    });
                    if starting {
                        tracing::trace!(printer = %self.view.id, "waiting for a started Job to show");
                    } else if let Some(open) = self.view.job.take() {
                        tracing::info!(printer = %self.view.id, job = open.id, "an open Job ended unseen: Lost");
                        self.end(open, JobOutcome::Lost).await;
                    }
                }
                None => {}
            }
        }
        self.reported = status;
    }

    /// A print is running: the open Job (re-attached after a restart or a
    /// reconnect when it matches), or a new External Job.
    async fn job_running(&mut self, active: &ActiveJob) {
        let Some(open) = &self.view.job else {
            self.job_appeared(active).await;
            return;
        };
        if !open.matches(active) {
            self.job_appeared(active).await;
            return;
        }
        if open.driver_key != active.key {
            // The same print under a new key: remember the new one, so its
            // ending is recognised.
            match self.log.rekeyed(open.id, &active.key).await {
                Ok(record) => self.view.job = Some(record),
                Err(e) => {
                    tracing::warn!(printer = %self.view.id, error = %e, "could not re-key a Job")
                }
            }
        }
    }

    /// A print Gantry didn't start is running: it is an External Job.
    async fn job_appeared(&mut self, active: &ActiveJob) {
        if let Some(stale) = self.view.job.take() {
            // Another print is running, so the open one ended unseen.
            self.end(stale, JobOutcome::Lost).await;
        }
        // A new print means the bed was cleared, whoever did it.
        if let Some(waiting) = self.view.awaiting_clear.take()
            && let Err(e) = self.log.bed_cleared(&self.view.id, waiting.id, false).await
        {
            tracing::warn!(printer = %self.view.id, error = %e, "could not record the bed as clear");
        }
        match self.log.external_job_started(&self.view.id, active).await {
            Ok(record) => self.view.job = Some(record),
            Err(e) => tracing::warn!(printer = %self.view.id, error = %e, "could not record a Job"),
        }
    }

    async fn on_job_ended(&mut self, key: String, outcome: JobOutcome) {
        let already = self.view.job.is_none()
            && self
                .view
                .awaiting_clear
                .as_ref()
                .is_some_and(|j| j.driver_key == key);
        if already {
            return;
        }
        let open_matches = self.view.job.as_ref().is_some_and(|j| j.driver_key == key);
        if !open_matches {
            // An ending already recorded (reported again after a reconnect
            // or a restart) is not a second Job.
            match self.log.find(&self.view.id, &key).await {
                Ok(Some(recorded)) if recorded.ended_at.is_some() => return,
                Ok(Some(recorded)) => {
                    // Open but not the one we were following: close it.
                    self.end(recorded, outcome).await;
                    return;
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(printer = %self.view.id, error = %e, "could not look up a Job")
                }
            }
        }
        let job = match self.view.job.take() {
            Some(job) if job.driver_key == key => Some(job),
            other => {
                // It started and ended without Gantry seeing it run.
                self.view.job = other;
                let active = ActiveJob {
                    key,
                    print_file: None,
                    plate: None,
                    progress: 0.0,
                    layer: None,
                    total_layers: None,
                    remaining_secs: None,
                    started_at: None,
                };
                self.log
                    .external_job_started(&self.view.id, &active)
                    .await
                    .ok()
            }
        };
        if let Some(job) = job {
            self.end(job, outcome).await;
        }
    }

    /// Records the Outcome and puts the Printer in Awaiting Clear, unless
    /// Auto-Clear is on and the Job Succeeded (PRD §6.3).
    async fn end(&mut self, job: JobRecord, outcome: JobOutcome) {
        let record = match self.log.job_ended(&self.view.id, job.id, outcome).await {
            Ok(record) => record,
            Err(e) => {
                tracing::warn!(printer = %self.view.id, error = %e, "could not record a Job Outcome");
                return;
            }
        };
        if self.view.auto_clear && outcome == JobOutcome::Succeeded {
            match self.log.bed_cleared(&self.view.id, record.id, false).await {
                Ok(cleared) => {
                    tracing::info!(printer = %self.view.id, job = cleared.id, "Auto-Clear: straight to Idle");
                    self.view.last_ended = Some(cleared);
                    return;
                }
                Err(e) => {
                    tracing::warn!(printer = %self.view.id, error = %e, "Auto-Clear could not record the bed as clear");
                }
            }
        }
        self.view.last_ended = Some(record.clone());
        self.view.awaiting_clear = Some(record);
    }
}

#[cfg(test)]
mod tests {
    use std::any::Any;
    use std::collections::BTreeSet;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;

    use super::*;
    use crate::driver::{FieldSpec, Secrets};
    use crate::status::{DisconnectReason, Temperatures};

    /// A test double: a Driver whose behaviour each test chooses.
    #[derive(Clone, Copy)]
    enum Behaviour {
        /// Reports connected and idle, then updates every 10 ms.
        Healthy,
        /// Never answers a command.
        HangsOnCommands,
        /// Floods its event stream as fast as it can.
        Floods,
        /// Its connection task panics straight away.
        Panics,
    }

    struct MockDriver(Behaviour);

    struct MockHandle {
        behaviour: Behaviour,
        task: JoinHandle<()>,
        commands: Arc<AtomicUsize>,
    }

    fn connected() -> PrinterStatus {
        PrinterStatus {
            connection: Connection::Connected,
            state: Some(PrinterState::Idle),
            job: None,
            temperatures: Temperatures::default(),
            fans: vec![],
            filament_slots: vec![],
            faults: vec![],
            capabilities: BTreeSet::from(Capability::CONTROL),
            control_withdrawn: None,
            speed_profile: None,
            speed_profiles: Vec::new(),
            lights: Vec::new(),
            position: None,
            loaded_slot: None,
        }
    }

    impl Driver for MockDriver {
        fn id(&self) -> &'static str {
            match self.0 {
                Behaviour::Healthy => "healthy",
                Behaviour::HangsOnCommands => "hangs",
                Behaviour::Floods => "floods",
                Behaviour::Panics => "panics",
            }
        }
        fn name(&self) -> &'static str {
            "Mock"
        }
        fn fields(&self) -> Vec<FieldSpec> {
            vec![]
        }
        fn connect(&self, _: PrinterConfig, events: EventSender) -> Box<dyn PrinterHandle> {
            let behaviour = self.0;
            let task = tokio::spawn(async move {
                match behaviour {
                    Behaviour::Panics => panic!("this Printer's task misbehaves"),
                    Behaviour::Floods => loop {
                        events.status(connected());
                        tokio::task::yield_now().await;
                    },
                    Behaviour::Healthy | Behaviour::HangsOnCommands => loop {
                        events.status(connected());
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    },
                }
            });
            Box::new(MockHandle {
                behaviour,
                task,
                commands: Arc::new(AtomicUsize::new(0)),
            })
        }
    }

    #[async_trait]
    impl PrinterHandle for MockHandle {
        async fn pause(&self) -> Result<(), CommandError> {
            self.commands.fetch_add(1, Ordering::SeqCst);
            if matches!(self.behaviour, Behaviour::HangsOnCommands) {
                std::future::pending::<()>().await;
            }
            Ok(())
        }
        async fn resume(&self) -> Result<(), CommandError> {
            Ok(())
        }
        async fn cancel(&self) -> Result<(), CommandError> {
            Ok(())
        }
        async fn shutdown(&self) {
            self.task.abort();
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    fn config(id: &str) -> PrinterConfig {
        PrinterConfig {
            id: id.to_owned(),
            name: id.to_owned(),
            settings: serde_json::Map::new(),
            secrets: Secrets::default(),
        }
    }

    fn fleet() -> Fleet {
        let mut registry = DriverRegistry::new();
        registry
            .register(MockDriver(Behaviour::Healthy))
            .register(MockDriver(Behaviour::HangsOnCommands))
            .register(MockDriver(Behaviour::Floods))
            .register(MockDriver(Behaviour::Panics));
        Fleet::with_command_timeout(
            registry,
            Arc::new(crate::job::MemoryJobLog::new()),
            Duration::from_millis(300),
        )
    }

    async fn connected_within(fleet: &Fleet, id: &str) {
        fleet
            .wait_for(id, Duration::from_secs(2), |v| {
                v.status.connection.is_connected()
            })
            .await
            .unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_hung_command_does_not_stall_another_printer() {
        let fleet = fleet();
        fleet
            .start("hangs", config("stuck"), JobState::default())
            .unwrap();
        fleet
            .start("healthy", config("fine"), JobState::default())
            .unwrap();
        connected_within(&fleet, "stuck").await;
        connected_within(&fleet, "fine").await;

        let stuck = {
            let fleet = fleet.clone();
            tokio::spawn(async move { fleet.command("stuck", Command::Pause).await })
        };
        // The healthy Printer answers at once while the other hangs.
        let started = std::time::Instant::now();
        fleet.command("fine", Command::Pause).await.unwrap();
        assert!(started.elapsed() < Duration::from_millis(200));
        // And its status keeps flowing.
        let before = fleet.view("fine").unwrap();
        let mut updates = fleet.subscribe();
        let next = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if let Ok(FleetUpdate::Printer(v)) = updates.recv().await
                    && v.id == "fine"
                {
                    return v;
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(next.id, before.id);
        // The hung command gives up rather than waiting forever.
        assert_eq!(
            stuck.await.unwrap(),
            Err(FleetError::Command(CommandError::TimedOut))
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_flooding_printer_does_not_starve_another() {
        let fleet = fleet();
        fleet
            .start("floods", config("noisy"), JobState::default())
            .unwrap();
        fleet
            .start("healthy", config("quiet"), JobState::default())
            .unwrap();
        // The quiet Printer still connects and answers commands promptly.
        connected_within(&fleet, "quiet").await;
        let started = std::time::Instant::now();
        fleet.command("quiet", Command::Pause).await.unwrap();
        assert!(started.elapsed() < Duration::from_millis(200));
        fleet.stop("noisy").await.unwrap();
    }

    #[tokio::test]
    async fn a_panicking_printer_does_not_take_down_another() {
        let fleet = fleet();
        fleet
            .start("panics", config("broken"), JobState::default())
            .unwrap();
        fleet
            .start("healthy", config("fine"), JobState::default())
            .unwrap();
        connected_within(&fleet, "fine").await;
        fleet.command("fine", Command::Pause).await.unwrap();
        // The broken one simply never connects.
        assert_eq!(
            fleet.view("broken").unwrap().status.connection,
            Connection::Connecting
        );
    }

    #[tokio::test]
    async fn commands_to_a_disconnected_printer_are_refused_at_once() {
        let mut status = connected();
        status.connection = Connection::disconnected(DisconnectReason::Unreachable);
        assert_eq!(
            check_command(&status, Capability::Pause),
            Err(CommandError::Disconnected {
                reason: DisconnectReason::Unreachable
            })
        );
        let mut status = connected();
        status.capabilities.clear();
        assert_eq!(
            check_command(&status, Capability::Pause),
            Err(CommandError::Unsupported)
        );
    }

    #[test]
    fn commands_read_and_write_as_flat_json() {
        use crate::driver::Heater;
        let pause: Command =
            serde_json::from_value(serde_json::json!({ "command": "pause" })).unwrap();
        assert_eq!(pause, Command::Pause);
        let heat: Command = serde_json::from_value(
            serde_json::json!({ "command": "set_temperature", "heater": "bed", "target": 60 }),
        )
        .unwrap();
        assert_eq!(
            heat,
            Command::Fine(FineControl::SetTemperature {
                heater: Heater::Bed,
                target: 60.0
            })
        );
        assert_eq!(heat.capability(), Capability::BedTemperature);
        assert_eq!(
            serde_json::to_value(&heat).unwrap(),
            serde_json::json!({ "command": "set_temperature", "heater": "bed", "target": 60.0 })
        );
        assert_eq!(
            serde_json::to_value(Command::Fine(FineControl::Home)).unwrap(),
            serde_json::json!({ "command": "home" })
        );
        assert!(
            serde_json::from_value::<Command>(serde_json::json!({ "command": "explode" })).is_err()
        );
    }

    #[test]
    fn fine_controls_are_checked_before_they_reach_a_driver() {
        use crate::driver::{Axis, Heater};
        assert!(
            FineControl::SetTemperature {
                heater: Heater::Nozzle,
                target: 900.0
            }
            .validate()
            .is_err()
        );
        assert!(
            FineControl::Jog {
                axis: Axis::X,
                distance_mm: 0.0
            }
            .validate()
            .is_err()
        );
        assert!(
            FineControl::SetFan {
                fan: "part".into(),
                percent: 101
            }
            .validate()
            .is_err()
        );
        assert!(FineControl::Home.validate().is_ok());
    }

    #[tokio::test]
    async fn stopping_a_printer_ends_its_tasks() {
        let fleet = fleet();
        fleet
            .start("healthy", config("p"), JobState::default())
            .unwrap();
        connected_within(&fleet, "p").await;
        let mut updates = fleet.subscribe();
        fleet.stop("p").await.unwrap();
        assert!(!fleet.contains("p"));
        assert!(matches!(
            fleet.command("p", Command::Pause).await,
            Err(FleetError::UnknownPrinter(_))
        ));
        let removed = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if let Ok(FleetUpdate::Removed(id)) = updates.recv().await {
                    return id;
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(removed, "p");
    }
}
