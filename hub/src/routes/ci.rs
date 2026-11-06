use axum::Json;
use axum::extract::{Path, State};

use crate::api_error::ApiError;
use crate::auth::CurrentUser;
use crate::ci_reports::{self, CiRun};
use crate::clock;
use crate::routes::history::{LogText, MAX_LOG_BYTES};
use crate::state::AppState;
use crate::text_file::tail_of;

pub async fn list(
    State(state): State<AppState>,
    _user: CurrentUser,
) -> Result<Json<Vec<CiRun>>, ApiError> {
    let Some(dir) = state.config.ci_reports_dir.clone() else {
        return Ok(Json(Vec::new()));
    };
    let cache = state.ci_runs.clone();
    let runs = tokio::task::spawn_blocking(move || cache.get(&dir, clock::now()))
        .await
        .map_err(|_| ApiError::internal())?;
    Ok(Json(runs.as_ref().clone()))
}

pub async fn run(
    State(state): State<AppState>,
    _user: CurrentUser,
    Path(id): Path<String>,
) -> Result<Json<CiRun>, ApiError> {
    let dir = state
        .config
        .ci_reports_dir
        .clone()
        .ok_or_else(ApiError::not_found)?;
    tokio::task::spawn_blocking(move || ci_reports::read_run(&dir, &id, clock::now()))
        .await
        .map_err(|_| ApiError::internal())?
        .map(Json)
        .ok_or_else(ApiError::not_found)
}

pub async fn step_log(
    State(state): State<AppState>,
    _user: CurrentUser,
    Path((id, n)): Path<(String, u32)>,
) -> Result<Json<LogText>, ApiError> {
    let dir = state
        .config
        .ci_reports_dir
        .clone()
        .ok_or_else(ApiError::not_found)?;
    let tail = tokio::task::spawn_blocking(move || {
        let (file, len) = ci_reports::open_step_log(&dir, &id, n)?;
        tail_of(file, len, MAX_LOG_BYTES).ok()
    })
    .await
    .map_err(|_| ApiError::internal())?
    .ok_or_else(ApiError::not_found)?;
    Ok(Json(LogText {
        text: tail.text,
        cut: tail.cut,
    }))
}
