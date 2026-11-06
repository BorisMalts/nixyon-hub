use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;

use crate::api_error::ApiError;
use crate::auth::{CurrentUser, SameOrigin};
use crate::client::Client;
use crate::clock;
use crate::events::{self, Event, EventKind};
use crate::quality_clips::is_valid_dataset;
use crate::quality_marks::{self, Mark, MarkOutcome, is_valid_clip};
use crate::sessions::SessionUser;
use crate::state::AppState;

const TOO_MANY_MARKS: ApiError = ApiError::new(
    StatusCode::CONFLICT,
    "too_many_marks",
    "В наборе слишком много пометок, снимите лишние",
);

fn checked(dataset: &str, clip: Option<&str>) -> Result<(), ApiError> {
    if is_valid_dataset(dataset) && clip.is_none_or(is_valid_clip) {
        Ok(())
    } else {
        Err(ApiError::not_found())
    }
}

/// Клипы набора с пометкой «плохой эталон».
pub async fn list(
    State(state): State<AppState>,
    _user: CurrentUser,
    Path(dataset): Path<String>,
) -> Result<Json<Vec<Mark>>, ApiError> {
    checked(&dataset, None)?;
    let marks = state
        .with_db(move |conn| quality_marks::list(conn, &dataset))
        .await?;
    Ok(Json(marks))
}

/// Помечает клип: следующие проверки его не берут. Пометка и снятие пишутся в журнал
/// событий.
pub async fn mark(
    State(state): State<AppState>,
    _origin: SameOrigin,
    client: Client,
    current: CurrentUser,
    Path((dataset, clip)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    checked(&dataset, Some(&clip))?;
    let who = Who {
        ip: client.ip_text(),
        ua: client.user_agent,
        user: current.user,
    };
    let outcome = state
        .with_db(move |conn| {
            let now = clock::now();
            let outcome = quality_marks::mark(conn, &dataset, &clip, &who.user.email, now)?;
            if outcome == MarkOutcome::Marked {
                let target = format!("{dataset}/{clip}");
                record(conn, EventKind::QualityMarked, &who, &target, now)?;
            }
            Ok(outcome)
        })
        .await?;
    match outcome {
        MarkOutcome::TooMany => Err(TOO_MANY_MARKS),
        MarkOutcome::Marked | MarkOutcome::AlreadyMarked => Ok(StatusCode::NO_CONTENT),
    }
}

pub async fn unmark(
    State(state): State<AppState>,
    _origin: SameOrigin,
    client: Client,
    current: CurrentUser,
    Path((dataset, clip)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    checked(&dataset, Some(&clip))?;
    let who = Who {
        ip: client.ip_text(),
        ua: client.user_agent,
        user: current.user,
    };
    state
        .with_db(move |conn| {
            if quality_marks::unmark(conn, &dataset, &clip)? {
                let target = format!("{dataset}/{clip}");
                record(
                    conn,
                    EventKind::QualityUnmarked,
                    &who,
                    &target,
                    clock::now(),
                )?;
            }
            Ok(())
        })
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Кто действует: для журнала событий.
struct Who {
    user: SessionUser,
    ip: String,
    ua: String,
}

fn record(
    conn: &rusqlite::Connection,
    kind: EventKind,
    who: &Who,
    target: &str,
    now: i64,
) -> rusqlite::Result<()> {
    events::record(
        conn,
        &Event {
            kind,
            user_id: Some(who.user.user_id),
            email: Some(&who.user.email),
            ip: &who.ip,
            user_agent: &who.ua,
            detail: target,
        },
        now,
    )
}
