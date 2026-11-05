//! Timelapse capture (PRD §7.5). Off by default; an Admin turns it on per
//! Printer, for one frame per layer or one on a timer. While a Job runs on
//! such a Printer, a frame is taken from its camera through the camera
//! relay, so the camera is pulled only for the moment a frame is needed
//! (unless someone is watching it anyway). Frames are JPEG files in
//! `<data dir>/timelapses/<job id>/`, and the timelapse belongs to its Job
//! in History.
//!
//! Timelapses have size-based cleanup ([`crate::retention`]): past the
//! Admin's limit the oldest finished timelapses are removed, files and all.
//! The Jobs they belonged to stay in History untouched.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gantry_core::{Capability, FleetUpdate, PrinterView};
use tokio::sync::broadcast::error::RecvError;

use crate::AppState;
use crate::retention::{self, Stored};
use crate::store::audit::{Action, Entry};
use crate::store::timelapses::{Mode, TimelapseSetting};

/// How long to wait for a camera's first frame.
const FRAME_WAIT: Duration = Duration::from_secs(10);

/// Where timelapses live.
#[derive(Debug, Clone)]
pub struct Timelapses {
    dir: PathBuf,
}

impl Timelapses {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            dir: data_dir.join("timelapses"),
        }
    }

    pub fn folder(&self, job_id: i64) -> PathBuf {
        self.dir.join(job_id.to_string())
    }

    pub fn frame(&self, job_id: i64, frame: u32) -> PathBuf {
        self.folder(job_id).join(format!("{frame:05}.jpg"))
    }

    /// Removes a timelapse's files. Missing files are no error.
    pub async fn remove(&self, job_id: i64) -> std::io::Result<()> {
        match tokio::fs::remove_dir_all(self.folder(job_id)).await {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

/// What is being captured on one Printer.
struct Capture {
    job_id: i64,
    setting: TimelapseSetting,
    last_layer: Option<u32>,
    last_frame: Option<Instant>,
    /// A frame is being taken now, so a second isn't started over it.
    busy: Arc<AtomicBool>,
}

/// Starts following the Fleet and capturing. Runs until the process ends.
pub fn spawn(state: &AppState) {
    let state = state.clone();
    let mut updates = state.fleet().subscribe();
    tokio::spawn(async move {
        let mut captures: HashMap<String, Capture> = HashMap::new();
        for view in state.fleet().snapshot() {
            follow(&state, &mut captures, &view).await;
        }
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                update = updates.recv() => match update {
                    Ok(FleetUpdate::Printer(view)) => follow(&state, &mut captures, &view).await,
                    Ok(FleetUpdate::Removed(id)) => {
                        if let Some(capture) = captures.remove(&id) {
                            finish(&state, capture.job_id).await;
                        }
                    }
                    Err(RecvError::Lagged(_)) => {
                        for view in state.fleet().snapshot() {
                            follow(&state, &mut captures, &view).await;
                        }
                    }
                    Err(RecvError::Closed) => return,
                },
                _ = tick.tick() => {
                    for (printer, capture) in &mut captures {
                        if capture.setting.mode == Mode::Interval
                            && capture.last_frame.is_none_or(|at| {
                                at.elapsed() >= Duration::from_secs(u64::from(capture.setting.interval_secs))
                            })
                        {
                            capture.last_frame = Some(Instant::now());
                            take(&state, printer, capture);
                        }
                    }
                }
                () = state.stopping() => return,
            }
        }
    });
}

async fn follow(state: &AppState, captures: &mut HashMap<String, Capture>, view: &PrinterView) {
    let running = view.job.as_ref().map(|j| j.id);
    if let Some(capture) = captures.get(&view.id)
        && Some(capture.job_id) != running
    {
        let job_id = capture.job_id;
        captures.remove(&view.id);
        finish(state, job_id).await;
    }
    let Some(job_id) = running else {
        return;
    };
    if !captures.contains_key(&view.id) {
        let setting = match state.store().timelapse_setting(&view.id).await {
            Ok(Some(setting)) => setting,
            Ok(None) => return,
            Err(e) => {
                tracing::warn!(printer = %view.id, error = %e, "could not read the timelapse setting");
                return;
            }
        };
        if setting.mode == Mode::Off {
            return;
        }
        captures.insert(
            view.id.clone(),
            Capture {
                job_id,
                setting,
                last_layer: None,
                last_frame: None,
                busy: Arc::default(),
            },
        );
    }
    let capture = captures.get_mut(&view.id).expect("just inserted");
    if capture.setting.mode == Mode::Layer
        && view.status.connection.is_connected()
        && let Some(layer) = view.status.job.as_ref().and_then(|j| j.layer)
        && capture.last_layer != Some(layer)
    {
        capture.last_layer = Some(layer);
        capture.last_frame = Some(Instant::now());
        take(state, &view.id, capture);
    }
}

/// Takes one frame in the background, unless one is being taken already.
fn take(state: &AppState, printer: &str, capture: &Capture) {
    if capture.busy.swap(true, Ordering::SeqCst) {
        return;
    }
    let state = state.clone();
    let printer = printer.to_owned();
    let job_id = capture.job_id;
    let busy = capture.busy.clone();
    tokio::spawn(async move {
        if let Err(e) = frame(&state, &printer, job_id).await {
            tracing::debug!(printer = %printer, job_id, error = %e, "no timelapse frame");
        }
        busy.store(false, Ordering::SeqCst);
    });
}

async fn frame(state: &AppState, printer: &str, job_id: i64) -> anyhow::Result<()> {
    let view = state.fleet().view(printer)?;
    if !view.status.has(Capability::Camera) {
        anyhow::bail!("the Printer has no camera");
    }
    let source = state
        .fleet()
        .handle(printer)?
        .camera()
        .ok_or_else(|| anyhow::anyhow!("the Printer has no camera"))?;
    // Through the relay, so a camera someone is watching isn't opened twice.
    let mut viewer = state.cameras().watch(printer, source);
    let jpeg = match viewer.first.take() {
        Some(jpeg) => jpeg,
        None => tokio::time::timeout(FRAME_WAIT, viewer.frames.recv())
            .await
            .map_err(|_| anyhow::anyhow!("the camera sent nothing"))??,
    };
    drop(viewer);
    let timelapses = state.timelapses();
    tokio::fs::create_dir_all(timelapses.folder(job_id)).await?;
    let temporary = timelapses.folder(job_id).join(".frame");
    tokio::fs::write(&temporary, &jpeg).await?;
    let number = state
        .store()
        .add_timelapse_frame(job_id, printer, jpeg.len() as u64)
        .await?;
    tokio::fs::rename(&temporary, timelapses.frame(job_id, number)).await?;
    Ok(())
}

async fn finish(state: &AppState, job_id: i64) {
    if let Err(e) = state.store().finish_timelapse(job_id).await {
        tracing::warn!(job_id, error = %e, "could not finish a timelapse");
    }
    clean_logged(state).await;
}

/// Removes the oldest finished timelapses until they fit the Admin's limit.
/// Each removal is audited as done by Gantry. Returns the Jobs whose
/// timelapses went; the Jobs themselves are untouched.
pub async fn clean(state: &AppState) -> Result<Vec<i64>, sqlx::Error> {
    let limit = crate::api::settings::current(state)
        .await?
        .timelapse_limit_mb;
    let stored: Vec<Stored> = state
        .store()
        .timelapses()
        .await?
        .iter()
        .map(|t| Stored {
            id: t.job_id,
            bytes: t.bytes,
            added_at: t.started(),
            protected: !t.finished,
        })
        .collect();
    let remove = retention::to_remove(&stored, retention::library_limit_bytes(limit));
    for &job_id in &remove {
        if let Err(e) = state.timelapses().remove(job_id).await {
            tracing::warn!(job_id, error = %e, "could not remove a timelapse's files");
            continue;
        }
        state.store().delete_timelapse(job_id).await?;
        tracing::info!(job_id, "removed a timelapse to stay under the size limit");
        let mut entry = Entry::new(Action::TimelapseRemoved);
        entry.target = Some(format!("Job {job_id}"));
        entry.detail = Some(serde_json::json!({ "reason": "size limit", "limit_mb": limit }));
        crate::audit::record(state, entry).await;
    }
    Ok(remove)
}

pub async fn clean_logged(state: &AppState) {
    if let Err(e) = clean(state).await {
        tracing::warn!(error = %e, "could not clean up timelapses");
    }
}
