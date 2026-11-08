use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Response};

use super::apk_response::{apk_response, gzip_allowed};
use crate::api_error::ApiError;
use crate::apk_page;
use crate::artifacts;
use crate::client::Client;
use crate::latest_apk::{ApkKind, latest};
use crate::state::AppState;

// Последняя сборка меняется, поэтому ни страница, ни файл не кэшируются.
const NO_STORE: HeaderValue = HeaderValue::from_static("no-store");

/// Страница скачивания последних APK без входа: название вида, вес и «Скачать».
pub async fn page(State(state): State<AppState>) -> Result<Response, ApiError> {
    let dir = state.config.artifacts_dir.clone();
    let variants = tokio::task::spawn_blocking(move || {
        let items = artifacts::list(&dir);
        [ApkKind::Full, ApkKind::Lite]
            .into_iter()
            .filter_map(|kind| latest(&items, kind).map(|a| (kind, a.size)))
            .collect::<Vec<_>>()
    })
    .await
    .map_err(|_| ApiError::internal())?;
    Ok((
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/html; charset=utf-8"),
            ),
            (header::CACHE_CONTROL, NO_STORE),
        ],
        apk_page::render(&variants),
    )
        .into_response())
}

pub async fn full(
    State(state): State<AppState>,
    client: Client,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    download(state, client, headers, ApkKind::Full).await
}

pub async fn lite(
    State(state): State<AppState>,
    client: Client,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    download(state, client, headers, ApkKind::Lite).await
}

// Место адреса и место в общей отдаче APK: без входа один адрес не займёт всю отдачу.
async fn download(
    state: AppState,
    client: Client,
    headers: HeaderMap,
    kind: ApkKind,
) -> Result<Response, ApiError> {
    let ip_slot = state
        .public_apk_ip_slots
        .try_acquire(client.ip)
        .ok_or_else(|| ApiError::too_many(5))?;
    let Ok(slot) = state.downloads.clone().try_acquire_owned() else {
        return Err(ApiError::too_many(5));
    };
    let dir = state.config.artifacts_dir.clone();
    let gzip_ok = gzip_allowed(&headers);
    let found = tokio::task::spawn_blocking(move || {
        let items = artifacts::list(&dir);
        let id = latest(&items, kind)?.id.clone();
        artifacts::open_apk(&dir, &id, gzip_ok)
    })
    .await
    .map_err(|_| ApiError::internal())?;
    let apk = found.ok_or_else(ApiError::not_found)?;
    let mut response = apk_response(apk, &headers, kind.file_name(), [ip_slot, slot])?;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, NO_STORE);
    Ok(response)
}
