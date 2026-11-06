use axum::extract::FromRequestParts;
use axum::http::Method;
use axum::http::header::ORIGIN;
use axum::http::request::Parts;

use crate::api_error::ApiError;
use crate::clock;
use crate::session_cookie;
use crate::sessions::{self, SessionUser};
use crate::state::AppState;

pub struct CurrentUser {
    pub user: SessionUser,
    pub token: String,
}

impl FromRequestParts<AppState> for CurrentUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = session_cookie::read(&parts.headers, state.config.secure_cookies())
            .ok_or_else(ApiError::unauthorized)?;
        let lookup_token = token.clone();
        let user = state
            .with_db(move |conn| sessions::lookup(conn, &lookup_token, clock::now()))
            .await?
            .ok_or_else(ApiError::unauthorized)?;
        Ok(Self { user, token })
    }
}

pub struct SameOrigin;

impl FromRequestParts<AppState> for SameOrigin {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let is_upgrade = parts.headers.contains_key(axum::http::header::UPGRADE);
        if parts.method == Method::GET && !is_upgrade {
            return Ok(Self);
        }
        let origin = parts.headers.get(ORIGIN).and_then(|v| v.to_str().ok());
        if origin == Some(state.config.public_origin.as_str()) {
            Ok(Self)
        } else {
            Err(ApiError::forbidden_origin())
        }
    }
}

pub const RECHECK_EVERY: std::time::Duration = std::time::Duration::from_secs(30);

pub async fn still_signed_in(state: &AppState, token: &str) -> bool {
    let token = token.to_string();
    matches!(
        state
            .with_db(move |conn| sessions::lookup(conn, &token, clock::now()))
            .await,
        Ok(Some(_))
    )
}
