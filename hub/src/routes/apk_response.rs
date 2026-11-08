use std::io::{Seek, SeekFrom};

use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use tokio::sync::OwnedSemaphorePermit;

use crate::api_error::ApiError;
use crate::artifacts::OpenedApk;
use crate::byte_range::{RangeRequest, parse_range};
use crate::file_body::FileBody;

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

/// Можно ли отдать сжатую копию. Куски берутся только из несжатого файла: смещения в
/// gzip-потоке клиенту ни к чему.
pub fn gzip_allowed(request_headers: &HeaderMap) -> bool {
    accepts_gzip(request_headers) && !request_headers.contains_key(header::RANGE)
}

/// Ответ с APK целиком или одним куском по Range. Места `slots` держатся, пока тело не
/// отдано. `file_name` идёт в `Content-Disposition`.
pub fn apk_response(
    mut apk: OpenedApk,
    request_headers: &HeaderMap,
    file_name: &str,
    slots: impl IntoIterator<Item = OwnedSemaphorePermit>,
) -> Result<Response, ApiError> {
    let disposition = format!("attachment; filename=\"{file_name}\"");
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
    let mut response = Body::new(FileBody::new(apk.file, count, slots)).into_response();
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
