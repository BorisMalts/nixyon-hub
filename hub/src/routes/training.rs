use axum::Json;
use axum::extract::{Path, State};
use axum::response::Response;

use super::quality::{download_slots, file_response};
use crate::api_error::ApiError;
use crate::auth::CurrentUser;
use crate::state::AppState;
use crate::training_runs::{self, TrainingFile};

/// Номера обучений на сервере, новые первыми.
pub async fn list(
    State(state): State<AppState>,
    _user: CurrentUser,
) -> Result<Json<Vec<u64>>, ApiError> {
    let dir = state.config.artifacts_dir.clone();
    tokio::task::spawn_blocking(move || training_runs::list_runs(&dir))
        .await
        .map(Json)
        .map_err(|_| ApiError::internal())
}

/// Готовые файлы одного обучения.
pub async fn files(
    State(state): State<AppState>,
    _user: CurrentUser,
    Path(id): Path<String>,
) -> Result<Json<Vec<TrainingFile>>, ApiError> {
    let dir = state.config.artifacts_dir.clone();
    tokio::task::spawn_blocking(move || training_runs::list_files(&dir, &id))
        .await
        .map_err(|_| ApiError::internal())?
        .map(Json)
        .ok_or_else(ApiError::not_found)
}

/// Отчёт, итоги или веса обучения, только на скачивание.
pub async fn download(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((id, name)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let slots = download_slots(&state, &user)?;
    let dir = state.config.artifacts_dir.clone();
    let found = tokio::task::spawn_blocking(move || training_runs::open_file(&dir, &id, &name))
        .await
        .map_err(|_| ApiError::internal())?;
    let file = found.ok_or_else(ApiError::not_found)?;
    let disposition = format!("attachment; filename=\"{}\"", file.download_name);
    file_response(file, &disposition, slots)
}
