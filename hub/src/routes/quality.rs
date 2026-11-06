use axum::Json;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use nixyon_agent::protocol::{DatasetSummary, Op, ServerFrame};
use serde::{Deserialize, Serialize};

use crate::agent_link::LinkError;
use crate::api_error::ApiError;
use crate::auth::CurrentUser;
use crate::file_body::FileBody;
use crate::quality_clips::{self, ClipPage, ClipQuery, ClipSet, MAX_QUERY_CHARS, TAGS};
use crate::quality_history::{self, QualityRun};
use crate::quality_report::{self, OpenedReport, QualityFile, Speeds};
use crate::state::AppState;

#[derive(Serialize)]
pub struct DatasetsView {
    pub datasets: Vec<DatasetSummary>,
    pub speeds: Speeds,
}

const AGENT_DOWN: ApiError = ApiError::new(
    StatusCode::SERVICE_UNAVAILABLE,
    "agent_unavailable",
    "Рабочая машина сейчас недоступна",
);

/// Наборы записей на сервере и скорость прошлой проверки: из них страница собирает план.
pub async fn datasets(
    State(state): State<AppState>,
    user: CurrentUser,
) -> Result<Json<DatasetsView>, ApiError> {
    let frame = state
        .agent
        .single(Op::Datasets, &user.user.email)
        .await
        .map_err(|e: LinkError| {
            tracing::error!(error = %e, "агент не ответил");
            AGENT_DOWN
        })?;
    let ServerFrame::Datasets { datasets } = frame else {
        return Err(AGENT_DOWN);
    };
    let dir = state.config.artifacts_dir.clone();
    let speeds = tokio::task::spawn_blocking(move || quality_report::speeds(&dir))
        .await
        .map_err(|_| ApiError::internal())?;
    Ok(Json(DatasetsView { datasets, speeds }))
}

pub async fn summary(
    State(state): State<AppState>,
    _user: CurrentUser,
) -> Result<Json<Vec<QualityFile>>, ApiError> {
    let dir = state.config.artifacts_dir.clone();
    let files = tokio::task::spawn_blocking(move || quality_report::list(&dir))
        .await
        .map_err(|_| ApiError::internal())?;
    Ok(Json(files))
}

const BAD_CLIP_QUERY: ApiError = ApiError::new(
    StatusCode::BAD_REQUEST,
    "bad_clip_query",
    "Такой отбор клипов не поддерживается",
);
// Дальше в файле клипов страница не листает: столько клипов вручную не смотрят.
const MAX_OFFSET: usize = 10_000_000;

#[derive(Deserialize)]
pub struct ClipParams {
    tag: Option<String>,
    q: Option<String>,
    offset: Option<usize>,
}

/// Прошлые прогоны проверки, новые раньше: для истории, сравнения и графика.
pub async fn runs(
    State(state): State<AppState>,
    _user: CurrentUser,
) -> Result<Json<Vec<QualityRun>>, ApiError> {
    let dir = state.config.artifacts_dir.clone();
    let runs = tokio::task::spawn_blocking(move || quality_history::list(&dir))
        .await
        .map_err(|_| ApiError::internal())?;
    Ok(Json(runs))
}

/// Наборы последнего отчёта, у которых есть файл клипов.
pub async fn clip_sets(
    State(state): State<AppState>,
    _user: CurrentUser,
) -> Result<Json<Vec<ClipSet>>, ApiError> {
    let dir = state.config.artifacts_dir.clone();
    let sets = tokio::task::spawn_blocking(move || quality_clips::clip_sets(&dir))
        .await
        .map_err(|_| ApiError::internal())?;
    Ok(Json(sets))
}

/// Клипы набора по метке и тексту, по 50 за раз. Файл читается целиком, поэтому
/// одновременных отборов не больше, чем скачиваний.
pub async fn clips(
    State(state): State<AppState>,
    _user: CurrentUser,
    Path(dataset): Path<String>,
    Query(params): Query<ClipParams>,
) -> Result<Json<ClipPage>, ApiError> {
    let query = clip_query(params)?;
    let Ok(_slot) = state.downloads.clone().try_acquire_owned() else {
        return Err(ApiError::too_many(5));
    };
    let dir = state.config.artifacts_dir.clone();
    let page = tokio::task::spawn_blocking(move || quality_clips::query(&dir, &dataset, &query))
        .await
        .map_err(|_| ApiError::internal())?;
    page.map(Json).ok_or_else(ApiError::not_found)
}

fn clip_query(params: ClipParams) -> Result<ClipQuery, ApiError> {
    let tag = params.tag.filter(|t| !t.is_empty());
    if tag.as_deref().is_some_and(|t| !TAGS.contains(&t)) {
        return Err(BAD_CLIP_QUERY);
    }
    let text = params
        .q
        .map(|q| q.trim().to_owned())
        .filter(|q| !q.is_empty());
    if text
        .as_deref()
        .is_some_and(|q| q.chars().count() > MAX_QUERY_CHARS)
    {
        return Err(BAD_CLIP_QUERY);
    }
    let offset = params.offset.unwrap_or(0);
    if offset > MAX_OFFSET {
        return Err(BAD_CLIP_QUERY);
    }
    Ok(ClipQuery { tag, text, offset })
}

/// Файл клипов набора на скачивание.
pub async fn clips_download(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(dataset): Path<String>,
) -> Result<Response, ApiError> {
    let slots = download_slots(&state, &user)?;
    let dir = state.config.artifacts_dir.clone();
    let found = tokio::task::spawn_blocking(move || quality_clips::open_clips(&dir, &dataset))
        .await
        .map_err(|_| ApiError::internal())?;
    let file = found.ok_or_else(ApiError::not_found)?;
    let disposition = format!("attachment; filename=\"{}\"", file.download_name);
    file_response(file, &disposition, slots)
}

/// Звук худшего клипа. Страница скачивает его целиком и играет у себя, поэтому
/// диапазоны не нужны. Берёт только общее место под отдачу: личное может быть занято
/// скачиванием большого файла клипов, а прослушать клип в это время всё равно можно.
pub async fn audio(
    State(state): State<AppState>,
    _user: CurrentUser,
    Path((dataset, n)): Path<(String, u32)>,
) -> Result<Response, ApiError> {
    let Ok(slot) = state.downloads.clone().try_acquire_owned() else {
        return Err(ApiError::too_many(5));
    };
    let dir = state.config.artifacts_dir.clone();
    let found = tokio::task::spawn_blocking(move || quality_clips::open_audio(&dir, &dataset, n))
        .await
        .map_err(|_| ApiError::internal())?;
    let file = found.ok_or_else(ApiError::not_found)?;
    let body = Body::new(FileBody::new(file.file, file.len, [slot]));
    let mut response = body.into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(file.content_type),
    );
    Ok(response)
}

pub(super) type Slots = [tokio::sync::OwnedSemaphorePermit; 2];

pub(super) fn download_slots(state: &AppState, user: &CurrentUser) -> Result<Slots, ApiError> {
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
    Ok([user_slot, slot])
}

pub async fn download(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(name): Path<String>,
) -> Result<Response, ApiError> {
    let slots = download_slots(&state, &user)?;
    let dir = state.config.artifacts_dir.clone();
    let found = tokio::task::spawn_blocking(move || quality_report::open(&dir, &name))
        .await
        .map_err(|_| ApiError::internal())?;
    let report = found.ok_or_else(ApiError::not_found)?;
    let disposition = format!("attachment; filename=\"{}\"", report.download_name);
    file_response(report, &disposition, slots)
}

pub(super) fn file_response(
    report: OpenedReport,
    disposition: &str,
    slots: Slots,
) -> Result<Response, ApiError> {
    let mut response = Body::new(FileBody::new(report.file, report.len, slots)).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(report.content_type),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(disposition).map_err(|_| ApiError::internal())?,
    );
    Ok(response)
}
