//! Timelapses (PRD §7.5): each Printer's setting, and each Job's frames.
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;

use super::ApiError;
use crate::AppState;
use crate::audit::{self, EntryExt};
use crate::store::audit::Action;
use crate::store::timelapses::{Mode, Timelapse, TimelapseSetting};
use crate::store::users::User;

/// The shortest and longest timer.
pub const INTERVAL_SECS: std::ops::RangeInclusive<u32> = 5..=3600;

/// `GET /api/printers/{id}/timelapse`.
pub async fn setting(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<TimelapseSetting>, ApiError> {
    state
        .store()
        .timelapse_setting(&id)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound("no such Printer".into()))
}

#[derive(Debug, Deserialize)]
pub struct SettingForm {
    mode: Mode,
    #[serde(default)]
    interval_secs: Option<u32>,
}

/// `PUT /api/printers/{id}/timelapse`: an Admin turns capture off, or on
/// per layer or on a timer. It applies from the next Job.
pub async fn set(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
    Json(form): Json<SettingForm>,
) -> Result<Json<TimelapseSetting>, ApiError> {
    let result = async {
        let mut setting = state
            .store()
            .timelapse_setting(&id)
            .await?
            .ok_or_else(|| ApiError::NotFound("no such Printer".into()))?;
        setting.mode = form.mode;
        if let Some(secs) = form.interval_secs {
            if !INTERVAL_SECS.contains(&secs) {
                return Err(ApiError::field(
                    "interval_secs",
                    "Must be 5 to 3600 seconds",
                ));
            }
            setting.interval_secs = secs;
        }
        state.store().set_timelapse_setting(&id, setting).await?;
        Ok(setting)
    }
    .await;
    audit::record(
        &state,
        audit::by(&user, Action::PrinterTimelapse)
            .printer(&id)
            .detail(serde_json::json!({ "mode": form.mode, "interval_secs": form.interval_secs }))
            .result(&result),
    )
    .await;
    result.map(Json)
}

async fn find(state: &AppState, job: i64) -> Result<Timelapse, ApiError> {
    state
        .store()
        .timelapse(job)
        .await?
        .ok_or_else(|| ApiError::NotFound("this Job has no timelapse".into()))
}

/// `GET /api/jobs/{id}/timelapse`.
pub async fn get(
    State(state): State<AppState>,
    Path(job): Path<i64>,
) -> Result<Json<Timelapse>, ApiError> {
    Ok(Json(find(&state, job).await?))
}

/// `GET /api/jobs/{id}/timelapse/frames/{frame}`: one JPEG, from 1.
pub async fn frame(
    State(state): State<AppState>,
    Path((job, frame)): Path<(i64, u32)>,
) -> Result<Response, ApiError> {
    let timelapse = find(&state, job).await?;
    if frame == 0 || frame > timelapse.frames {
        return Err(ApiError::NotFound("no such frame".into()));
    }
    let bytes = tokio::fs::read(state.timelapses().frame(job, frame))
        .await
        .map_err(|_| ApiError::NotFound("no such frame".into()))?;
    let mut response = Body::from(bytes).into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("image/jpeg"));
    // A frame never changes once it is written.
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=31536000, immutable"),
    );
    Ok(response)
}

/// `DELETE /api/jobs/{id}/timelapse`: removes the frames. The Job stays in
/// History.
pub async fn remove(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(job): Path<i64>,
) -> Result<StatusCode, ApiError> {
    let result = async {
        let timelapse = find(&state, job).await?;
        if !timelapse.finished {
            return Err(ApiError::Conflict(
                "this Job is still running; remove its timelapse once it ends".into(),
            ));
        }
        state
            .timelapses()
            .remove(job)
            .await
            .map_err(ApiError::internal)?;
        state.store().delete_timelapse(job).await?;
        Ok(StatusCode::NO_CONTENT)
    }
    .await;
    audit::record(
        &state,
        audit::by(&user, Action::TimelapseRemoved)
            .target(format!("Job {job}"))
            .result(&result),
    )
    .await;
    result
}
