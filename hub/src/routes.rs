mod builds;
mod ci;
mod experiments;
mod health;
mod history;
mod jobs;
mod marks;
mod models;
mod pages;
mod quality;
mod session;
mod stats;
mod terminal;
mod training;

use axum::Router;
use axum::http::{HeaderName, HeaderValue, header};
use axum::middleware;
use axum::routing::{get, post};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::set_header::SetResponseHeaderLayer;

use crate::request_log;
use crate::state::AppState;

pub const MAX_BODY_BYTES: usize = 16 * 1024;

// Встроенные стили разрешены ради xterm: цвета, шрифт и ширину ячеек он задаёт через
// вставленные элементы <style>, без nonce. Скрипты остаются только свои. Звук клипов
// страница скачивает целиком и играет из blob:, поэтому media-src разрешает blob:.
const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; \
media-src 'self' blob:; connect-src 'self'; font-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'; object-src 'none'";

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/login", post(session::login))
        .route("/logout", post(session::logout))
        .route("/me", get(session::me))
        .route("/builds", get(builds::list))
        .route("/builds/{id}/apk", get(builds::download))
        .route("/builds/{id}/qr.svg", get(builds::qr))
        .route("/jobs/pull", post(jobs::start_pull))
        .route("/jobs/build", post(jobs::start_build))
        .route("/jobs/quality", post(jobs::start_quality))
        .route("/jobs/live-eval", post(jobs::start_live_eval))
        .route("/jobs/training", post(jobs::start_training))
        .route("/jobs/status", get(jobs::status))
        .route("/jobs/follow", get(jobs::follow))
        .route("/jobs/history", get(history::list))
        .route("/jobs/{id}/log", get(history::log))
        .route("/ci/runs", get(ci::list))
        .route("/ci/runs/{id}", get(ci::run))
        .route("/ci/runs/{id}/steps/{n}/log", get(ci::step_log))
        .route("/quality", get(quality::summary))
        .route("/quality/datasets", get(quality::datasets))
        .route("/quality/clip-sets", get(quality::clip_sets))
        .route("/quality/runs", get(quality::runs))
        .route("/quality/marks/{dataset}", get(marks::list))
        .route(
            "/quality/marks/{dataset}/{clip}",
            post(marks::mark).delete(marks::unmark),
        )
        .route("/quality/clips/{dataset}", get(quality::clips))
        .route(
            "/quality/clips/{dataset}/file",
            get(quality::clips_download),
        )
        .route("/quality/audio/{dataset}/{n}", get(quality::audio))
        .route("/quality/{file}", get(quality::download))
        .route("/experiments", get(experiments::list))
        .route("/experiments/{id}", get(experiments::files))
        .route("/experiments/{id}/{file}", get(experiments::download))
        .route("/experiments/{id}/{combo}/{file}", get(experiments::dump))
        .route("/training", get(training::list))
        .route("/training/{id}", get(training::files))
        .route("/training/{id}/{file}", get(training::download))
        .route("/stats", get(stats::stats))
        .route("/terminal", get(terminal::open))
        .layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ));

    Router::new()
        .nest("/api", api)
        .route("/health", get(health::health))
        .route("/models/", get(models::list))
        .route("/models/{sha256}", get(models::file))
        .merge(pages::routes())
        .layer(RequestBodyLimitLayer::new(MAX_BODY_BYTES))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            request_log::log,
        ))
        .layer(header_layer(header::CONTENT_SECURITY_POLICY, CSP))
        .layer(header_layer(header::X_CONTENT_TYPE_OPTIONS, "nosniff"))
        .layer(header_layer(header::X_FRAME_OPTIONS, "DENY"))
        .layer(header_layer(header::REFERRER_POLICY, "no-referrer"))
        .layer(header_layer(
            HeaderName::from_static("permissions-policy"),
            "camera=(), microphone=(), geolocation=()",
        ))
        .layer(header_layer(
            header::STRICT_TRANSPORT_SECURITY,
            "max-age=31536000",
        ))
        .with_state(state)
}

fn header_layer(name: HeaderName, value: &'static str) -> SetResponseHeaderLayer<HeaderValue> {
    SetResponseHeaderLayer::overriding(name, HeaderValue::from_static(value))
}
