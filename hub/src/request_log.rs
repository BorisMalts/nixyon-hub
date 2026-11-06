use std::time::Instant;

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;

use crate::client::client_ip;
use crate::state::AppState;

pub async fn log(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let started = Instant::now();
    let method = request.method().clone();
    let path = request.uri().path().chars().take(200).collect::<String>();
    let (parts, body) = request.into_parts();
    let ip = client_ip(&parts, &state.config.trusted_proxies);
    let response = next.run(Request::from_parts(parts, body)).await;
    tracing::info!(
        %method,
        path,
        status = response.status().as_u16(),
        %ip,
        duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        "запрос"
    );
    response
}
