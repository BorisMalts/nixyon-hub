use axum::Json;
use axum::extract::{Path, State};
use axum::response::Response;

use super::quality::{download_slots, file_response};
use crate::api_error::ApiError;
use crate::auth::CurrentUser;
use crate::experiments::{self, ExperimentFile};
use crate::quality_report::OpenedReport;
use crate::state::AppState;

/// Номера опытов на сервере, новые первыми.
pub async fn list(
    State(state): State<AppState>,
    _user: CurrentUser,
) -> Result<Json<Vec<u64>>, ApiError> {
    let dir = state.config.artifacts_dir.clone();
    tokio::task::spawn_blocking(move || experiments::list_experiments(&dir))
        .await
        .map(Json)
        .map_err(|_| ApiError::internal())
}

/// Файлы одного опыта.
pub async fn files(
    State(state): State<AppState>,
    _user: CurrentUser,
    Path(id): Path<String>,
) -> Result<Json<Vec<ExperimentFile>>, ApiError> {
    let dir = state.config.artifacts_dir.clone();
    tokio::task::spawn_blocking(move || experiments::list_files(&dir, &id))
        .await
        .map_err(|_| ApiError::internal())?
        .map(Json)
        .ok_or_else(ApiError::not_found)
}

/// Итоги прогонов или флаги сочетаний опыта.
pub async fn download(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((id, name)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let dir = state.config.artifacts_dir.clone();
    send(&state, &user, move || {
        experiments::open_file(&dir, &id, &name)
    })
    .await
}

/// Дамп расшифровки записи в одном сочетании.
pub async fn dump(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((id, combo, name)): Path<(String, String, String)>,
) -> Result<Response, ApiError> {
    let dir = state.config.artifacts_dir.clone();
    send(&state, &user, move || {
        experiments::open_dump(&dir, &id, &combo, &name)
    })
    .await
}

async fn send(
    state: &AppState,
    user: &CurrentUser,
    open: impl FnOnce() -> Option<OpenedReport> + Send + 'static,
) -> Result<Response, ApiError> {
    let slots = download_slots(state, user)?;
    let found = tokio::task::spawn_blocking(open)
        .await
        .map_err(|_| ApiError::internal())?;
    let file = found.ok_or_else(ApiError::not_found)?;
    let disposition = format!("attachment; filename=\"{}\"", file.download_name);
    file_response(file, &disposition, slots)
}
