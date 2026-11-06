use std::io::{Seek, SeekFrom};

use axum::Json;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use tokio::sync::OwnedSemaphorePermit;

use crate::api_error::ApiError;
use crate::byte_range::{RangeRequest, parse_range};
use crate::client::Client;
use crate::file_body::FileBody;
use crate::model_mirror::{self, MirrorFile};
use crate::state::AppState;

// Имя файла и есть сумма содержимого: по одному адресу всегда одно и то же.
const IMMUTABLE: &str = "public, max-age=31536000, immutable";

/// Список файлов зеркала моделей. Открыт без входа: модели не секрет, а скрипты
/// загрузки сверяют сумму каждого файла.
pub async fn list(State(state): State<AppState>) -> Result<Json<Vec<MirrorFile>>, ApiError> {
    let Some(dir) = state.config.models_dir.clone() else {
        return Err(ApiError::not_found());
    };
    let files = tokio::task::spawn_blocking(move || model_mirror::list(&dir))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|e| {
            tracing::error!(error = %e, "список зеркала моделей не прочитан");
            ApiError::internal()
        })?;
    Ok(Json(files))
}

/// Файл зеркала по его SHA-256, целиком или одним куском по Range.
pub async fn file(
    State(state): State<AppState>,
    client: Client,
    Path(sha256): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let Some(dir) = state.config.models_dir.clone() else {
        return Err(ApiError::not_found());
    };
    if !model_mirror::is_sha256(&sha256) {
        return Err(ApiError::not_found());
    }
    let slots = model_slots(&state, &client)?;
    let opened = tokio::task::spawn_blocking(move || model_mirror::open(&dir, &sha256))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|e| {
            tracing::error!(error = %e, "файл зеркала моделей не открылся");
            ApiError::internal()
        })?;
    let (mut file, len) = opened.ok_or_else(ApiError::not_found)?;
    let range = headers.get(header::RANGE).and_then(|v| v.to_str().ok());
    let (status, start, count) = match parse_range(range, len) {
        RangeRequest::Whole => (StatusCode::OK, 0, len),
        RangeRequest::Part(part) => (StatusCode::PARTIAL_CONTENT, part.start, part.byte_count()),
        RangeRequest::Unsatisfiable => {
            let mut response = StatusCode::RANGE_NOT_SATISFIABLE.into_response();
            response.headers_mut().insert(
                header::CONTENT_RANGE,
                header_value(&format!("bytes */{len}"))?,
            );
            return Ok(response);
        }
    };
    if start > 0 {
        file.seek(SeekFrom::Start(start))
            .map_err(|_| ApiError::internal())?;
    }
    let mut response = Body::new(FileBody::new(file, count, slots)).into_response();
    *response.status_mut() = status;
    let out = response.headers_mut();
    out.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    out.insert(header::CONTENT_LENGTH, header_value(&count.to_string())?);
    out.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    out.insert(header::CACHE_CONTROL, HeaderValue::from_static(IMMUTABLE));
    if status == StatusCode::PARTIAL_CONTENT {
        let last = start + count - 1;
        out.insert(
            header::CONTENT_RANGE,
            header_value(&format!("bytes {start}-{last}/{len}"))?,
        );
    }
    Ok(response)
}

// Место адреса и место во всём хабе: один адрес не займёт всю отдачу.
fn model_slots(state: &AppState, client: &Client) -> Result<[OwnedSemaphorePermit; 2], ApiError> {
    let ip_slot = state
        .model_ip_slots
        .try_acquire(client.ip)
        .ok_or_else(|| ApiError::too_many(5))?;
    let Ok(slot) = state.model_downloads.clone().try_acquire_owned() else {
        return Err(ApiError::too_many(5));
    };
    Ok([ip_slot, slot])
}

fn header_value(text: &str) -> Result<HeaderValue, ApiError> {
    HeaderValue::from_str(text).map_err(|_| ApiError::internal())
}
