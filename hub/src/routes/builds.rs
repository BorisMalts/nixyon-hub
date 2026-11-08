use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use nixyon_agent::protocol::Artifact;
use qrcode::QrCode;
use qrcode::render::svg;

use super::apk_response::{apk_response, gzip_allowed};
use crate::api_error::ApiError;
use crate::artifacts;
use crate::auth::CurrentUser;
use crate::state::AppState;

pub async fn list(
    State(state): State<AppState>,
    _user: CurrentUser,
) -> Result<Json<Vec<Artifact>>, ApiError> {
    let dir = state.config.artifacts_dir.clone();
    let items = tokio::task::spawn_blocking(move || artifacts::list(&dir))
        .await
        .map_err(|_| ApiError::internal())?;
    Ok(Json(items))
}

pub async fn download(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
    request_headers: HeaderMap,
) -> Result<Response, ApiError> {
    let user_slot = state
        .user_download_slot(user.user.user_id)
        .ok_or(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "download_in_progress",
            "Дождитесь, пока закончится предыдущее скачивание",
        ))?;
    let Ok(slot) = state.downloads.clone().try_acquire_owned() else {
        return Err(ApiError::too_many(5));
    };
    let dir = state.config.artifacts_dir.clone();
    let gzip_ok = gzip_allowed(&request_headers);
    let found = tokio::task::spawn_blocking(move || artifacts::open_apk(&dir, &id, gzip_ok))
        .await
        .map_err(|_| ApiError::internal())?;
    let apk = found.ok_or_else(ApiError::not_found)?;
    let file_name = apk.artifact.file.clone();
    apk_response(apk, &request_headers, &file_name, [user_slot, slot])
}

pub async fn qr(
    State(state): State<AppState>,
    _user: CurrentUser,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    if !Artifact::is_valid_id(&id) {
        return Err(ApiError::not_found());
    }
    let url = format!("{}/api/builds/{id}/apk", state.config.public_origin);
    let code = QrCode::new(url.as_bytes()).map_err(|_| ApiError::internal())?;
    let image = code
        .render::<svg::Color<'_>>()
        .min_dimensions(240, 240)
        .quiet_zone(true)
        .build();
    Ok(([(header::CONTENT_TYPE, "image/svg+xml")], image).into_response())
}
