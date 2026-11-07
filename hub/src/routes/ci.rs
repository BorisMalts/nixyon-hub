use std::time::Instant;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use nixyon_agent::protocol::{ErrorCode, Op, ServerFrame};

use crate::agent_link::{Job, LinkError};
use crate::api_error::ApiError;
use crate::auth::{CurrentUser, SameOrigin};
use crate::ci_reports::{self, CiRun};
use crate::client::Client;
use crate::clock;
use crate::events::{self, Event, EventKind};
use crate::rate_limit::Decision;
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

const BAD_RUN: ApiError = ApiError::new(StatusCode::BAD_REQUEST, "bad_run", "Нет такого прогона");
const NOT_CONFIGURED: ApiError = ApiError::new(
    StatusCode::SERVICE_UNAVAILABLE,
    "ci_not_configured",
    "Токен GitHub на сервере не настроен",
);
const DENIED: ApiError = ApiError::new(
    StatusCode::BAD_GATEWAY,
    "ci_denied",
    "GitHub не дал доступа: проверьте токен на сервере",
);
const FINISHED: ApiError =
    ApiError::new(StatusCode::CONFLICT, "ci_finished", "Прогон уже закончился");
const STILL_RUNNING: ApiError = ApiError::new(
    StatusCode::CONFLICT,
    "ci_running",
    "Прогон ещё идёт: остановите его или дождитесь конца",
);
const GITHUB_DOWN: ApiError = ApiError::new(
    StatusCode::BAD_GATEWAY,
    "ci_failed",
    "GitHub не ответил, попробуйте ещё раз",
);
const AGENT_DOWN: ApiError = ApiError::new(
    StatusCode::SERVICE_UNAVAILABLE,
    "agent_unavailable",
    "Рабочая машина сейчас недоступна",
);

/// Номер прогона GitHub: только цифры, как его пишет раннер в отчёт.
fn is_github_run(id: &str) -> bool {
    !id.is_empty() && id.len() <= 20 && id.bytes().all(|b| b.is_ascii_digit())
}

/// Отмена или перезапуск прогона CI. Сам запрос к GitHub делает агент: у хаба нет выхода
/// в интернет.
async fn control(
    state: AppState,
    client: Client,
    current: CurrentUser,
    run_id: String,
    op: Op,
) -> Result<StatusCode, ApiError> {
    if let Decision::Limited { retry_after } = state.action_limiter.check(client.ip, Instant::now())
    {
        return Err(ApiError::too_many(retry_after.as_secs()));
    }
    if !is_github_run(&run_id) {
        return Err(BAD_RUN);
    }
    let conflict = if op == Op::CiCancel {
        FINISHED
    } else {
        STILL_RUNNING
    };
    let job = Job {
        ci_run: run_id.clone(),
        ..Job::default()
    };
    match state.agent.single_with(op, &current.user.email, job).await {
        Ok(ServerFrame::Done) => {}
        Ok(_) => return Err(AGENT_DOWN),
        Err(LinkError::Agent(code, _)) => {
            return Err(match code {
                ErrorCode::BadRequest => BAD_RUN,
                ErrorCode::NotConfigured => NOT_CONFIGURED,
                ErrorCode::Denied => DENIED,
                ErrorCode::Conflict => conflict,
                ErrorCode::Unauthorized | ErrorCode::Busy => AGENT_DOWN,
                ErrorCode::Failed => GITHUB_DOWN,
            });
        }
        Err(other) => {
            tracing::error!(error = %other, "агент не ответил");
            return Err(AGENT_DOWN);
        }
    }
    let kind = if op == Op::CiCancel {
        EventKind::CiCancelled
    } else {
        EventKind::CiRerun
    };
    let (ip, ua, user) = (client.ip_text(), client.user_agent, current.user);
    state
        .with_db(move |conn| {
            events::record(
                conn,
                &Event {
                    kind,
                    user_id: Some(user.user_id),
                    email: Some(&user.email),
                    ip: &ip,
                    user_agent: &ua,
                    detail: &run_id,
                },
                clock::now(),
            )
        })
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn cancel(
    State(state): State<AppState>,
    _origin: SameOrigin,
    client: Client,
    current: CurrentUser,
    Path(run_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    control(state, client, current, run_id, Op::CiCancel).await
}

pub async fn rerun(
    State(state): State<AppState>,
    _origin: SameOrigin,
    client: Client,
    current: CurrentUser,
    Path(run_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    control(state, client, current, run_id, Op::CiRerun).await
}
