use axum::Json;
use axum::extract::{Path, State};
use nixyon_agent::protocol::JobStatus;
use serde::Serialize;

use crate::api_error::ApiError;
use crate::auth::CurrentUser;
use crate::job_history;
use crate::state::AppState;
use crate::text_file::read_tail;

pub const MAX_LOG_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Serialize)]
pub struct LogText {
    pub text: String,
    pub cut: bool,
}

pub async fn list(
    State(state): State<AppState>,
    _user: CurrentUser,
) -> Result<Json<Vec<JobStatus>>, ApiError> {
    let dir = job_history::dir(&state.config.artifacts_dir);
    let jobs = tokio::task::spawn_blocking(move || job_history::list(&dir))
        .await
        .map_err(|_| ApiError::internal())?;
    Ok(Json(jobs))
}

pub async fn log(
    State(state): State<AppState>,
    _user: CurrentUser,
    Path(id): Path<u64>,
) -> Result<Json<LogText>, ApiError> {
    let path = job_history::log_path(&job_history::dir(&state.config.artifacts_dir), id);
    let tail = tokio::task::spawn_blocking(move || read_tail(&path, MAX_LOG_BYTES))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::not_found())?;
    Ok(Json(LogText {
        text: tail.text,
        cut: tail.cut,
    }))
}
