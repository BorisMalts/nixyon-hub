use axum::Router;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;

use crate::auth::CurrentUser;
use crate::state::AppState;

const INDEX: &str = include_str!("../../ui/index.html");
const LOGIN: &str = include_str!("../../ui/login.html");

const ASSETS: &[(&str, &str, &str)] = &[
    (
        "styles.css",
        "text/css; charset=utf-8",
        include_str!("../../ui/styles.css"),
    ),
    (
        "app.js",
        "text/javascript; charset=utf-8",
        include_str!("../../ui/app.js"),
    ),
    (
        "dom.js",
        "text/javascript; charset=utf-8",
        include_str!("../../ui/dom.js"),
    ),
    (
        "format.js",
        "text/javascript; charset=utf-8",
        include_str!("../../ui/format.js"),
    ),
    (
        "tabs.js",
        "text/javascript; charset=utf-8",
        include_str!("../../ui/tabs.js"),
    ),
    (
        "builds.js",
        "text/javascript; charset=utf-8",
        include_str!("../../ui/builds.js"),
    ),
    (
        "tests.js",
        "text/javascript; charset=utf-8",
        include_str!("../../ui/tests.js"),
    ),
    (
        "quality.js",
        "text/javascript; charset=utf-8",
        include_str!("../../ui/quality.js"),
    ),
    (
        "quality_clips.js",
        "text/javascript; charset=utf-8",
        include_str!("../../ui/quality_clips.js"),
    ),
    (
        "quality_history.js",
        "text/javascript; charset=utf-8",
        include_str!("../../ui/quality_history.js"),
    ),
    (
        "quality_log.js",
        "text/javascript; charset=utf-8",
        include_str!("../../ui/quality_log.js"),
    ),
    (
        "quality_plan.js",
        "text/javascript; charset=utf-8",
        include_str!("../../ui/quality_plan.js"),
    ),
    (
        "quality_presets.js",
        "text/javascript; charset=utf-8",
        include_str!("../../ui/quality_presets.js"),
    ),
    (
        "quality_progress.js",
        "text/javascript; charset=utf-8",
        include_str!("../../ui/quality_progress.js"),
    ),
    (
        "quality_time.js",
        "text/javascript; charset=utf-8",
        include_str!("../../ui/quality_time.js"),
    ),
    (
        "report_view.js",
        "text/javascript; charset=utf-8",
        include_str!("../../ui/report_view.js"),
    ),
    (
        "training.js",
        "text/javascript; charset=utf-8",
        include_str!("../../ui/training.js"),
    ),
    (
        "terminal.js",
        "text/javascript; charset=utf-8",
        include_str!("../../ui/terminal.js"),
    ),
    (
        "stats.js",
        "text/javascript; charset=utf-8",
        include_str!("../../ui/stats.js"),
    ),
    (
        "login.js",
        "text/javascript; charset=utf-8",
        include_str!("../../ui/login.js"),
    ),
    (
        "vendor/xterm.js",
        "text/javascript; charset=utf-8",
        include_str!("../../ui/vendor/xterm.js"),
    ),
    (
        "vendor/xterm.css",
        "text/css; charset=utf-8",
        include_str!("../../ui/vendor/xterm.css"),
    ),
    (
        "vendor/addon-fit.js",
        "text/javascript; charset=utf-8",
        include_str!("../../ui/vendor/addon-fit.js"),
    ),
];

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(index))
        .route("/login", get(login))
        .route("/assets/{*file}", get(asset))
}

fn html(body: &'static str) -> Response {
    (
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/html; charset=utf-8"),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
        body,
    )
        .into_response()
}

async fn index(user: Result<CurrentUser, crate::api_error::ApiError>) -> Response {
    match user {
        Ok(_) => html(INDEX),
        Err(_) => Redirect::to("/login").into_response(),
    }
}

async fn login(
    State(_state): State<AppState>,
    user: Result<CurrentUser, crate::api_error::ApiError>,
) -> Response {
    match user {
        Ok(_) => Redirect::to("/").into_response(),
        Err(_) => html(LOGIN),
    }
}

async fn asset(Path(file): Path<String>) -> Response {
    match ASSETS.iter().find(|(name, _, _)| *name == file) {
        Some((_, mime, body)) => (
            [
                (header::CONTENT_TYPE, HeaderValue::from_static(mime)),
                (header::CACHE_CONTROL, HeaderValue::from_static("no-cache")),
            ],
            *body,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_module_imported_by_the_page_is_served() {
        let served: Vec<&str> = ASSETS.iter().map(|(name, _, _)| *name).collect();
        let sources = ASSETS
            .iter()
            .filter(|(name, _, _)| name.ends_with(".js"))
            .map(|(_, _, body)| *body)
            .chain([INDEX]);
        for source in sources {
            for part in source.split("from \"./").skip(1) {
                let module = part.split('"').next().unwrap_or_default();
                assert!(served.contains(&module), "не отдаётся {module}");
            }
            for part in source.split("src=\"/assets/").skip(1) {
                let file = part.split('"').next().unwrap_or_default();
                assert!(served.contains(&file), "не отдаётся {file}");
            }
        }
        assert!(served.contains(&"quality.js"));
        assert!(served.contains(&"training.js"));
        assert!(served.contains(&"report_view.js"));
    }
}
