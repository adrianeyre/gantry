//! The Print File library (PRD §7.3, §8): upload, list, set a plain
//! `.gcode`'s model, remove, Plate thumbnails, the 3D model, and sending a
//! Plate to a Printer.
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use gantry_core::{Capability, JobRecord, JobStart, PrintUpload};
use serde::{Deserialize, Serialize};
use sha2::Digest;

use super::ApiError;
use crate::AppState;
use crate::audit::{self, EntryExt};
use crate::print_files::parse::{self, Kind, ParseError};
use crate::print_files::{mesh, models, safe_name, toolpath};
use crate::store::audit::Action;
use crate::store::print_files::PrintFile;
use crate::store::users::User;

/// The largest Print File accepted.
pub const MAX_UPLOAD_BYTES: usize = 512 * 1024 * 1024;

/// A Print File as the API sends it.
#[derive(Debug, Serialize)]
pub struct PrintFileJson {
    #[serde(flatten)]
    pub file: PrintFile,
    /// Every filament type its Plates need.
    pub filament_kinds: Vec<String>,
}

impl From<PrintFile> for PrintFileJson {
    fn from(file: PrintFile) -> Self {
        Self {
            filament_kinds: file.filament_kinds(),
            file,
        }
    }
}

async fn find(state: &AppState, id: i64) -> Result<PrintFile, ApiError> {
    state
        .store()
        .print_file(id)
        .await?
        .ok_or_else(|| ApiError::NotFound("no such Print File".into()))
}

/// `GET /api/print-files`: the library, newest first.
pub async fn list(State(state): State<AppState>) -> Result<Json<Vec<PrintFileJson>>, ApiError> {
    Ok(Json(
        state
            .store()
            .print_files()
            .await?
            .into_iter()
            .map(Into::into)
            .collect(),
    ))
}

/// `GET /api/print-files/{id}`.
pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<PrintFileJson>, ApiError> {
    Ok(Json(find(&state, id).await?.into()))
}

#[derive(Debug, Deserialize)]
pub struct UploadQuery {
    name: String,
}

/// `POST /api/print-files?name=<file name>`, the file as the body: adds a
/// Print File to the library. Unsliced models are refused with what to do.
pub async fn upload(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Query(query): Query<UploadQuery>,
    body: Bytes,
) -> Response {
    let name = safe_name(&query.name);
    let result = async {
        let name = name
            .clone()
            .ok_or_else(|| ApiError::field("name", "The file needs a name"))?;
        if body.is_empty() {
            return Err(ApiError::field("file", "The file is empty"));
        }
        let parsed = {
            let (name, body) = (name.clone(), body.clone());
            tokio::task::spawn_blocking(move || parse::parse(&name, &body))
                .await
                .map_err(ApiError::internal)?
        }
        .map_err(|e| match e {
            ParseError::NeedsSlicing(m)
            | ParseError::NotAPrintFile(m)
            | ParseError::Unreadable(m) => ApiError::field("file", &m),
        })?;
        let sha = hex(&sha2::Sha256::digest(&body));
        let id = state
            .store()
            .insert_print_file(&name, &parsed, body.len(), &sha, Some(&user.username))
            .await?;
        let thumbnails: Vec<(u32, &parse::Thumbnail)> = parsed
            .plates
            .iter()
            .filter_map(|p| p.thumbnail.as_ref().map(|t| (p.number, t)))
            .collect();
        if let Err(e) = state.library().save(id, &name, &body, &thumbnails).await {
            let _ = state.store().delete_print_file(id).await;
            let _ = state.library().remove(id).await;
            return Err(ApiError::internal(format!(
                "could not store a Print File: {e}"
            )));
        }
        find(&state, id).await
    }
    .await;
    let mut entry = audit::by(&user, Action::PrintFileUploaded)
        .target(name.unwrap_or_else(|| query.name.clone()))
        .result(&result);
    if let Ok(file) = &result {
        entry = entry.detail(serde_json::json!({
            "print_file_id": file.id,
            "plates": file.plates.len(),
            "printer_model": file.printer_model,
            "bytes": file.size_bytes,
        }));
    }
    audit::record(&state, entry).await;
    if let Ok(file) = &result {
        // The new file stays; older ones make room for it.
        crate::retention::clean_library_logged(&state, Some(file.id)).await;
        // Its toolpaths are worked out now, so the preview opens at once.
        let (state, file) = (state.clone(), file.clone());
        tokio::spawn(async move {
            for plate in file.plates.iter().map(|p| p.number) {
                if let Err(e) = ensure_toolpath(&state, &file, plate).await {
                    tracing::debug!(print_file = file.id, plate, error = %e.message(), "no toolpath");
                }
            }
        });
    }
    match result {
        Ok(file) => (StatusCode::CREATED, Json(PrintFileJson::from(file))).into_response(),
        Err(e) => e.into_response(),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The body of `PUT /api/print-files/{id}`.
#[derive(Debug, Deserialize)]
pub struct UpdateForm {
    /// The model a plain `.gcode` was sliced for; `null` or empty clears it.
    printer_model: Option<String>,
}

/// `PUT /api/print-files/{id}`: sets the model a plain `.gcode` was sliced
/// for, when the file doesn't say (PRD §7.3). A `.gcode.3mf` names its own.
pub async fn update(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<i64>,
    Json(form): Json<UpdateForm>,
) -> Result<Json<PrintFileJson>, ApiError> {
    let model = form
        .printer_model
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(str::to_owned);
    let result = async {
        let file = find(&state, id).await?;
        if file.kind == Kind::Gcode3mf {
            return Err(ApiError::Conflict(
                "a .gcode.3mf names the model it was sliced for, so it can't be changed".into(),
            ));
        }
        if model.as_ref().is_some_and(|m| m.chars().count() > 64) {
            return Err(ApiError::field(
                "printer_model",
                "Use 64 characters or fewer",
            ));
        }
        state
            .store()
            .set_print_file_model(id, model.as_deref())
            .await?;
        find(&state, id).await
    }
    .await;
    audit::record(
        &state,
        audit::by(&user, Action::PrintFileUpdated)
            .target(id.to_string())
            .detail(serde_json::json!({ "printer_model": model }))
            .result(&result),
    )
    .await;
    result.map(|f| Json(f.into()))
}

/// `DELETE /api/print-files/{id}`: removes a Print File from the library.
/// Jobs that printed it keep their History.
pub async fn remove(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    let result = async {
        let file = find(&state, id).await?;
        state.store().delete_print_file(id).await?;
        if let Err(e) = state.library().remove(id).await {
            tracing::warn!(print_file = id, error = %e, "could not delete a Print File's folder");
        }
        Ok(file)
    }
    .await;
    let target = match &result {
        Ok(file) => file.name.clone(),
        Err(_) => id.to_string(),
    };
    audit::record(
        &state,
        audit::by(&user, Action::PrintFileRemoved)
            .target(target)
            .result(&result),
    )
    .await;
    result.map(|_| StatusCode::NO_CONTENT)
}

/// `GET /api/print-files/{id}/plates/{plate}/thumbnail`: the Plate's picture,
/// or 404 when it has none (the UI shows a placeholder).
pub async fn thumbnail(
    State(state): State<AppState>,
    Path((id, plate)): Path<(i64, u32)>,
) -> Result<Response, ApiError> {
    let (bytes, content_type) = state
        .library()
        .thumbnail(id, plate)
        .await
        .ok_or_else(|| ApiError::NotFound("this Plate has no thumbnail".into()))?;
    Ok((
        [
            (header::CONTENT_TYPE, content_type),
            // A Print File never changes once uploaded.
            (header::CACHE_CONTROL, "private, max-age=86400"),
        ],
        bytes,
    )
        .into_response())
}

/// `GET /api/print-files/{id}/model`: the meshes in a `.gcode.3mf`, for the
/// 3D viewer, simplified when big. Worked out once, then cached.
pub async fn model(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Response, ApiError> {
    let file = find(&state, id).await?;
    if file.kind != Kind::Gcode3mf {
        return Err(ApiError::NotFound(
            "only a .gcode.3mf has a 3D model; a plain .gcode doesn't".into(),
        ));
    }
    let cache = state.library().cache(id, "model.json");
    let json = match tokio::fs::read(&cache).await {
        Ok(json) => json,
        Err(_) => {
            let bytes = state
                .library()
                .read(id, &file.name)
                .await
                .map_err(|e| ApiError::internal(format!("could not read a Print File: {e}")))?;
            let json = tokio::task::spawn_blocking(move || {
                mesh::read(&bytes).map(|m| serde_json::to_vec(&mesh::ModelJson::from(&m)))
            })
            .await
            .map_err(ApiError::internal)?
            .map_err(|e| match e {
                mesh::MeshError::NoModel => ApiError::NotFound(e.to_string()),
                other => ApiError::Conflict(other.to_string()),
            })?
            .map_err(ApiError::internal)?;
            if let Err(e) = tokio::fs::write(&cache, &json).await {
                tracing::warn!(print_file = id, error = %e, "could not cache a 3D model");
            }
            json
        }
    };
    Ok((
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "private, max-age=86400"),
        ],
        json,
    )
        .into_response())
}

/// Works out a Plate's toolpath and caches it, unless it already is.
/// Returns the cached file.
pub async fn ensure_toolpath(
    state: &AppState,
    file: &PrintFile,
    plate: u32,
) -> Result<std::path::PathBuf, ApiError> {
    let cache = state
        .library()
        .cache(file.id, &format!("toolpath-{plate}.bin"));
    if tokio::fs::metadata(&cache).await.is_ok() {
        return Ok(cache);
    }
    let bytes = state
        .library()
        .read(file.id, &file.name)
        .await
        .map_err(|e| ApiError::internal(format!("could not read a Print File: {e}")))?;
    let kind = file.kind;
    let encoded = tokio::task::spawn_blocking(move || {
        parse::plate_gcode(kind, &bytes, plate).map(|gcode| {
            toolpath::parse(&gcode)
                .simplified(toolpath::POINT_BUDGET)
                .encode()
        })
    })
    .await
    .map_err(ApiError::internal)?
    .ok_or_else(|| ApiError::NotFound("this Plate has no G-code".into()))?;
    // Written whole under a name of its own, then renamed, so a half-written
    // cache is never read, even when two requests work it out at once.
    let partial = cache.with_extension(format!("{}.partial", uuid::Uuid::new_v4()));
    let written = tokio::fs::write(&partial, &encoded).await;
    let renamed = match written {
        Ok(()) => tokio::fs::rename(&partial, &cache).await,
        Err(e) => Err(e),
    };
    if let Err(e) = renamed {
        let _ = tokio::fs::remove_file(&partial).await;
        // Another request may have won the race; its cache is as good.
        if tokio::fs::metadata(&cache).await.is_err() {
            return Err(ApiError::internal(format!(
                "could not cache a toolpath: {e}"
            )));
        }
    }
    Ok(cache)
}

/// `GET /api/print-files/{id}/plates/{plate}/toolpath`: the Plate's
/// toolpath for the preview (PRD §8.2), simplified, in the binary format of
/// [`toolpath::Toolpath::encode`], streamed so the UI can draw the first
/// layers as they arrive. Worked out once, then cached.
pub async fn toolpath(
    State(state): State<AppState>,
    Path((id, plate)): Path<(i64, u32)>,
) -> Result<Response, ApiError> {
    let file = find(&state, id).await?;
    if file.plate(plate).is_none() {
        return Err(ApiError::NotFound(format!(
            "{} has no Plate {plate}",
            file.name
        )));
    }
    let cache = ensure_toolpath(&state, &file, plate).await?;
    let file = tokio::fs::File::open(&cache)
        .await
        .map_err(|e| ApiError::internal(format!("could not read a toolpath: {e}")))?;
    let length = file.metadata().await.map(|m| m.len()).unwrap_or_default();
    let chunks = futures::stream::unfold(file, |mut file| async move {
        use tokio::io::AsyncReadExt;
        let mut buffer = vec![0u8; 256 * 1024];
        match file.read(&mut buffer).await {
            Ok(0) => None,
            Ok(n) => {
                buffer.truncate(n);
                Some((Ok::<_, std::io::Error>(Bytes::from(buffer)), file))
            }
            Err(e) => Some((Err(e), file)),
        }
    });
    Ok((
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_owned()),
            (header::CONTENT_LENGTH, length.to_string()),
            (header::CACHE_CONTROL, "private, max-age=86400".to_owned()),
        ],
        axum::body::Body::from_stream(chunks),
    )
        .into_response())
}

/// The body of `POST /api/printers/{id}/start`.
#[derive(Debug, Deserialize)]
pub struct StartForm {
    pub print_file_id: i64,
    /// From 1. Optional for a plain `.gcode`, which has one.
    pub plate: Option<u32>,
}

/// Why a Print File can't go to a Printer, or `None` if it can: the Printer
/// must be of the model the file was sliced for (PRD §7.3).
pub fn model_mismatch(
    file: &PrintFile,
    printer_name: &str,
    printer_model: Option<&str>,
) -> Option<String> {
    let Some(sliced_for) = file.printer_model.as_deref() else {
        return Some(format!(
            "{} doesn't say which printer model it was sliced for: set the model in the \
             library before sending it",
            file.name
        ));
    };
    let Some(model) = printer_model else {
        return Some(format!(
            "Gantry doesn't know {printer_name}'s model, so it can't check {} was sliced for it",
            file.name
        ));
    };
    (!models::same_model(sliced_for, model)).then(|| {
        format!(
            "{} was sliced for {sliced_for}, but {printer_name}'s model is {model}: a Print File only goes to the model it was sliced for",
            file.name
        )
    })
}

/// Sends one Plate to a Printer and starts the Job, after the checks every
/// path (the library, the Queue) shares. Returns the Job.
pub async fn start_plate(
    state: &AppState,
    printer: &str,
    file: &PrintFile,
    plate: u32,
) -> Result<JobRecord, ApiError> {
    let view = state.fleet().view(printer)?;
    let plate = file
        .plate(plate)
        .ok_or_else(|| ApiError::field("plate", &format!("{} has no Plate {plate}", file.name)))?;
    if !view.status.has(Capability::UploadAndStart) && view.status.connection.is_connected() {
        return Err(ApiError::Conflict(format!(
            "{} can't be sent Print Files",
            view.name
        )));
    }
    if let Some(why) = model_mismatch(file, &view.name, view.model.as_deref()) {
        return Err(ApiError::Conflict(why));
    }
    let data = state
        .library()
        .read(file.id, &file.name)
        .await
        .map_err(|e| ApiError::internal(format!("could not read a Print File: {e}")))?;
    let job = state
        .fleet()
        .start_job(
            printer,
            JobStart {
                print_file_id: file.id,
                print_file: file.name.clone(),
                plate: plate.number,
            },
            PrintUpload {
                file_name: file.name.clone(),
                plate: (file.kind == Kind::Gcode3mf).then_some(plate.number),
                data: data.into(),
                estimated_secs: plate.estimated_secs,
                layers: plate.layers,
            },
        )
        .await?;
    Ok(job)
}

/// `POST /api/printers/{id}/start`: an Operator sends a Plate to a Printer
/// and starts the Job (PRD §7.3).
pub async fn start(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(printer): Path<String>,
    Json(form): Json<StartForm>,
) -> Result<(StatusCode, Json<JobRecord>), ApiError> {
    let result = async {
        let file = find(&state, form.print_file_id).await?;
        let plate = match (form.plate, file.kind) {
            (Some(plate), _) => plate,
            (None, Kind::Gcode) => 1,
            (None, Kind::Gcode3mf) => {
                return Err(ApiError::field("plate", "Choose which Plate to print"));
            }
        };
        let job = start_plate(&state, &printer, &file, plate).await?;
        Ok((file, job))
    }
    .await;
    let mut entry = audit::by(&user, Action::JobStarted)
        .printer(&printer)
        .detail(serde_json::json!({ "print_file_id": form.print_file_id, "plate": form.plate }))
        .result(&result);
    if let Ok((file, job)) = &result {
        entry = entry.target(file.name.clone()).detail(serde_json::json!({
            "print_file_id": file.id,
            "plate": job.plate,
            "job": job.id,
        }));
    }
    audit::record(&state, entry).await;
    result.map(|(_, job)| (StatusCode::CREATED, Json(job)))
}
