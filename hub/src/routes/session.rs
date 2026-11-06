use std::time::Instant;

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;

use crate::api_error::ApiError;
use crate::auth::{CurrentUser, SameOrigin};
use crate::client::Client;
use crate::events::{self, Event, EventKind};
use crate::rate_limit::Decision;
use crate::sessions::{self, MAX_AGE_SECONDS};
use crate::state::AppState;
use crate::{clock, password, session_cookie, users};

#[derive(Deserialize)]
pub struct LoginForm {
    email: String,
    password: String,
}

enum Outcome {
    Signed(String),
    Refused,
}

const WRONG: ApiError = ApiError::new(
    StatusCode::UNAUTHORIZED,
    "wrong_credentials",
    "Неверная почта или пароль. После нескольких ошибок вход закрывается на 15 минут.",
);

pub async fn login(
    State(state): State<AppState>,
    _origin: SameOrigin,
    client: Client,
    Json(form): Json<LoginForm>,
) -> Result<Response, ApiError> {
    if let Decision::Limited { retry_after } = state.login_limiter.check(client.ip, Instant::now())
    {
        return Err(ApiError::too_many(retry_after.as_secs()));
    }
    let ip = client.ip_text();
    let ua = client.user_agent.clone();
    let outcome = state
        .with_db(move |conn| {
            let now = clock::now();
            let user = users::find(conn, &form.email).map_err(|e| match e {
                users::UserError::Db(e) => e,
                other => rusqlite::Error::InvalidParameterName(other.to_string()),
            })?;
            let Some(user) = user else {
                password::verify(&form.password, password::dummy_hash());
                events::record(
                    conn,
                    &event_of(&ip, &ua, EventKind::LoginFailed, None, None),
                    now,
                )?;
                return Ok(Outcome::Refused);
            };
            if user.locked_until.is_some_and(|until| until > now) {
                password::verify(&form.password, password::dummy_hash());
                events::record(
                    conn,
                    &event_of(
                        &ip,
                        &ua,
                        EventKind::LoginLocked,
                        Some(user.id),
                        Some(&user.email),
                    ),
                    now,
                )?;
                return Ok(Outcome::Refused);
            }
            if !password::verify(&form.password, &user.password_hash) {
                let locked = users::record_failure(conn, user.id, now)?;
                let kind = if locked {
                    EventKind::LoginLocked
                } else {
                    EventKind::LoginFailed
                };
                events::record(
                    conn,
                    &event_of(&ip, &ua, kind, Some(user.id), Some(&user.email)),
                    now,
                )?;
                return Ok(Outcome::Refused);
            }
            users::record_success(conn, user.id)?;
            let token = sessions::create(
                conn,
                user.id,
                &sessions::Origin {
                    ip: &ip,
                    user_agent: &ua,
                },
                now,
            )?;
            events::record(
                conn,
                &event_of(&ip, &ua, EventKind::Login, Some(user.id), Some(&user.email)),
                now,
            )?;
            Ok(Outcome::Signed(token))
        })
        .await?;
    match outcome {
        Outcome::Signed(token) => {
            let cookie =
                session_cookie::set(&token, state.config.secure_cookies(), MAX_AGE_SECONDS);
            let mut headers = HeaderMap::new();
            headers.insert(
                header::SET_COOKIE,
                HeaderValue::from_str(&cookie).map_err(|_| ApiError::internal())?,
            );
            Ok((StatusCode::NO_CONTENT, headers).into_response())
        }
        Outcome::Refused => Err(WRONG),
    }
}

fn event_of<'a>(
    ip: &'a str,
    user_agent: &'a str,
    kind: EventKind,
    user_id: Option<i64>,
    email: Option<&'a str>,
) -> Event<'a> {
    Event {
        kind,
        user_id,
        email,
        ip,
        user_agent,
        detail: "",
    }
}

pub async fn logout(
    State(state): State<AppState>,
    _origin: SameOrigin,
    client: Client,
    current: CurrentUser,
) -> Result<Response, ApiError> {
    let ip = client.ip_text();
    let ua = client.user_agent;
    let token = current.token;
    let user = current.user;
    state
        .with_db(move |conn| {
            sessions::delete(conn, &token)?;
            events::record(
                conn,
                &Event {
                    kind: EventKind::Logout,
                    user_id: Some(user.user_id),
                    email: Some(&user.email),
                    ip: &ip,
                    user_agent: &ua,
                    detail: "",
                },
                clock::now(),
            )
        })
        .await?;
    let mut headers = HeaderMap::new();
    let cookie = session_cookie::clear(state.config.secure_cookies());
    headers.insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie).map_err(|_| ApiError::internal())?,
    );
    Ok((StatusCode::NO_CONTENT, headers).into_response())
}

pub async fn me(current: CurrentUser) -> Json<serde_json::Value> {
    Json(json!({ "email": current.user.email }))
}
