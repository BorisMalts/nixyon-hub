use axum::Json;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::Response;
use nixyon_agent::protocol::{ErrorCode, JobKind, JobStatus, Op, QualityTake, ServerFrame};
use serde::{Deserialize, Serialize};
use std::time::Instant;

use crate::agent_link::{Job, LinkError};
use crate::api_error::ApiError;
use crate::auth::{CurrentUser, RECHECK_EVERY, SameOrigin, still_signed_in};
use crate::client::Client;
use crate::clock;
use crate::events::{self, Event, EventKind};
use crate::quality_marks;
use crate::rate_limit::Decision;
use crate::state::AppState;

const BUSY: ApiError = ApiError::new(
    StatusCode::CONFLICT,
    "busy",
    "Такое задание уже ждёт в очереди",
);
const BAD_PLAN: ApiError = ApiError::new(
    StatusCode::BAD_REQUEST,
    "bad_plan",
    "Выбранные наборы не подходят, обновите страницу и выберите снова",
);
const BAD_RECORDS: ApiError = ApiError::new(
    StatusCode::BAD_REQUEST,
    "bad_records",
    "Таких записей нет на сервере или имена недопустимы",
);
const BAD_TRAINING: ApiError = ApiError::new(
    StatusCode::BAD_REQUEST,
    "bad_training",
    "Выбранных наборов нет на сервере, обновите страницу и выберите снова",
);
const AGENT_DOWN: ApiError = ApiError::new(
    StatusCode::SERVICE_UNAVAILABLE,
    "agent_unavailable",
    "Рабочая машина сейчас недоступна",
);

fn link_error(e: LinkError) -> ApiError {
    link_error_or(e, BAD_PLAN)
}

// Отказ агента по плану задания показывается своим текстом для каждого вида задания.
fn link_error_or(e: LinkError, bad_request: ApiError) -> ApiError {
    match e {
        LinkError::Agent(ErrorCode::Busy, _) => BUSY,
        LinkError::Agent(ErrorCode::BadRequest, _) => bad_request,
        other => {
            tracing::error!(error = %other, "агент не ответил");
            AGENT_DOWN
        }
    }
}

async fn start(
    state: AppState,
    client: Client,
    current: CurrentUser,
    kind: JobKind,
    job: Job,
) -> Result<Json<JobStatus>, ApiError> {
    if let Decision::Limited { retry_after } = state.action_limiter.check(client.ip, Instant::now())
    {
        return Err(ApiError::too_many(retry_after.as_secs()));
    }
    let (op, event_kind) = match kind {
        JobKind::Pull => (Op::Pull, EventKind::PullStarted),
        JobKind::Build => (Op::Build, EventKind::BuildStarted),
        JobKind::Quality => (Op::Quality, EventKind::QualityStarted),
        JobKind::LiveEval => (Op::LiveEval, EventKind::LiveEvalStarted),
        JobKind::Training => (Op::Training, EventKind::TrainingStarted),
    };
    let bad_request = match kind {
        JobKind::LiveEval => BAD_RECORDS,
        JobKind::Training => BAD_TRAINING,
        _ => BAD_PLAN,
    };
    let frame = state
        .agent
        .single_with(op, &current.user.email, job)
        .await
        .map_err(|e| link_error_or(e, bad_request))?;
    let ServerFrame::Started { job } = frame else {
        return Err(AGENT_DOWN);
    };
    let (ip, ua, user) = (client.ip_text(), client.user_agent, current.user);
    let detail = job.id.to_string();
    state
        .with_db(move |conn| {
            events::record(
                conn,
                &Event {
                    kind: event_kind,
                    user_id: Some(user.user_id),
                    email: Some(&user.email),
                    ip: &ip,
                    user_agent: &ua,
                    detail: &detail,
                },
                clock::now(),
            )
        })
        .await?;
    Ok(Json(job))
}

pub async fn start_pull(
    State(state): State<AppState>,
    _origin: SameOrigin,
    client: Client,
    current: CurrentUser,
) -> Result<Json<JobStatus>, ApiError> {
    start(state, client, current, JobKind::Pull, Job::default()).await
}

pub async fn start_build(
    State(state): State<AppState>,
    _origin: SameOrigin,
    client: Client,
    current: CurrentUser,
) -> Result<Json<JobStatus>, ApiError> {
    start(state, client, current, JobKind::Build, Job::default()).await
}

/// Что проверять: наборы и минуты из каждого. Сверяет план агент, по списку наборов
/// на сервере. Исключённые клипы страница не задаёт: их берёт хаб из своих пометок.
#[derive(Deserialize)]
pub struct QualityRequest {
    pub plan: Vec<QualityTake>,
}

pub async fn start_quality(
    State(state): State<AppState>,
    _origin: SameOrigin,
    client: Client,
    current: CurrentUser,
    Json(request): Json<QualityRequest>,
) -> Result<Json<JobStatus>, ApiError> {
    let plan = request.plan;
    let plan = state
        .with_db(move |conn| {
            plan.into_iter()
                .map(|take| {
                    let exclude = quality_marks::clips(conn, &take.dataset)?;
                    Ok(QualityTake { exclude, ..take })
                })
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .await?;
    let job = Job {
        plan,
        ..Job::default()
    };
    start(state, client, current, JobKind::Quality, job).await
}

/// Какие записи из папки live на сервере прогнать через eval. Сверяет их агент.
#[derive(Deserialize)]
pub struct LiveEvalRequest {
    pub records: Vec<String>,
}

pub async fn start_live_eval(
    State(state): State<AppState>,
    _origin: SameOrigin,
    client: Client,
    current: CurrentUser,
    Json(request): Json<LiveEvalRequest>,
) -> Result<Json<JobStatus>, ApiError> {
    let job = Job {
        records: request.records,
        ..Job::default()
    };
    start(state, client, current, JobKind::LiveEval, job).await
}

/// На каких наборах обучать классификатор «ребёнок или взрослый». Сверяет их агент.
#[derive(Deserialize)]
pub struct TrainingRequest {
    pub sets: Vec<String>,
}

pub async fn start_training(
    State(state): State<AppState>,
    _origin: SameOrigin,
    client: Client,
    current: CurrentUser,
    Json(request): Json<TrainingRequest>,
) -> Result<Json<JobStatus>, ApiError> {
    let job = Job {
        training: request.sets,
        ..Job::default()
    };
    start(state, client, current, JobKind::Training, job).await
}

#[derive(Serialize)]
pub struct StatusView {
    pub job: Option<JobStatus>,
    pub queue: Vec<JobStatus>,
}

pub async fn status(
    State(state): State<AppState>,
    current: CurrentUser,
) -> Result<Json<StatusView>, ApiError> {
    match state
        .agent
        .single(Op::Status, &current.user.email)
        .await
        .map_err(link_error)?
    {
        ServerFrame::Status { job, queue } => Ok(Json(StatusView { job, queue })),
        _ => Err(AGENT_DOWN),
    }
}

pub async fn follow(
    State(state): State<AppState>,
    _origin: SameOrigin,
    current: CurrentUser,
    ws: WebSocketUpgrade,
) -> Response {
    ws.max_message_size(4096)
        .on_upgrade(move |socket| relay(state, current, socket))
}

async fn relay(state: AppState, current: CurrentUser, mut socket: WebSocket) {
    let email = current.user.email;
    let token = current.token;
    let mut recheck = tokio::time::interval(RECHECK_EVERY);
    recheck.tick().await;
    let Ok(mut session) = state.agent.open(Op::Follow, &email, 0, 0).await else {
        let _ = socket.send(Message::Close(None)).await;
        return;
    };
    loop {
        tokio::select! {
            _ = recheck.tick() => {
                if !still_signed_in(&state, &token).await {
                    break;
                }
            }
            frame = session.reader.next() => match frame {
                Ok(Some(frame)) => {
                    let Ok(text) = serde_json::to_string(&frame) else { break };
                    if socket.send(Message::Text(text.into())).await.is_err() {
                        break;
                    }
                }
                _ => break,
            },
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                _ => {}
            },
        }
    }
    let _ = socket.send(Message::Close(None)).await;
}
