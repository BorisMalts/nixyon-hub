use axum::Json;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use nixyon_agent::protocol::Artifact;
use qrcode::QrCode;
use qrcode::render::svg;
use std::io::{Seek, SeekFrom};

use crate::api_error::ApiError;
use crate::artifacts;
use crate::auth::CurrentUser;
use crate::byte_range::{RangeRequest, parse_range};
use crate::file_body::FileBody;
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

pub fn accepts_gzip(headers: &HeaderMap) -> bool {
    headers
        .get_all(header::ACCEPT_ENCODING)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .any(|item| {
            let mut parts = item.split(';').map(str::trim);
            let name = parts.next().unwrap_or_default();
            let refused = parts.any(|p| {
                p.strip_prefix("q=")
                    .and_then(|q| q.parse::<f32>().ok())
                    .is_some_and(|q| q <= 0.0)
            });
            name.eq_ignore_ascii_case("gzip") && !refused
        })
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
    // Куски берутся только из несжатого файла: смещения в gzip-потоке клиенту ни к чему.
    let wants_range = request_headers.contains_key(header::RANGE);
    let gzip_ok = accepts_gzip(&request_headers) && !wants_range;
    let found = tokio::task::spawn_blocking(move || artifacts::open_apk(&dir, &id, gzip_ok))
        .await
        .map_err(|_| ApiError::internal())?;
    let mut apk = found.ok_or_else(ApiError::not_found)?;
    let disposition = format!("attachment; filename=\"{}\"", apk.artifact.file);
    // С If-Range проверить нечем: у файла нет ни ETag, ни даты, поэтому отдаётся целиком.
    let range_header = if request_headers.contains_key(header::IF_RANGE) {
        None
    } else {
        request_headers
            .get(header::RANGE)
            .and_then(|v| v.to_str().ok())
    };
    let (status, start, count, content_range) = match parse_range(range_header, apk.len) {
        RangeRequest::Whole => (StatusCode::OK, 0, apk.len, None),
        RangeRequest::Part(part) => (
            StatusCode::PARTIAL_CONTENT,
            part.start,
            part.byte_count(),
            Some(format!("bytes {}-{}/{}", part.start, part.end, apk.len)),
        ),
        RangeRequest::Unsatisfiable => {
            return Ok((
                StatusCode::RANGE_NOT_SATISFIABLE,
                [(header::CONTENT_RANGE, format!("bytes */{}", apk.len))],
            )
                .into_response());
        }
    };
    if start > 0 {
        apk.file
            .seek(SeekFrom::Start(start))
            .map_err(|_| ApiError::internal())?;
    }
    let mut response = Body::new(FileBody::new(apk.file, count, [user_slot, slot])).into_response();
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(header::VARY, HeaderValue::from_static("accept-encoding"));
    // Для сжатого ответа куски не обещаем: длина и смещения там другие, чем у APK.
    headers.insert(
        header::ACCEPT_RANGES,
        HeaderValue::from_static(if apk.gzipped { "none" } else { "bytes" }),
    );
    if let Some(content_range) = content_range {
        headers.insert(
            header::CONTENT_RANGE,
            HeaderValue::from_str(&content_range).map_err(|_| ApiError::internal())?,
        );
    }
    if apk.gzipped {
        headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
    }
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/vnd.android.package-archive"),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&disposition).map_err(|_| ApiError::internal())?,
    );
    Ok(response)
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
