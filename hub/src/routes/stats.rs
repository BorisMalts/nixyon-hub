use axum::Json;
use axum::extract::State;

use crate::api_error::ApiError;
use crate::auth::CurrentUser;
use crate::state::AppState;
use crate::stats::{self, Stats};
use crate::{artifacts, clock, job_history};

pub async fn stats(
    State(state): State<AppState>,
    _user: CurrentUser,
) -> Result<Json<Stats>, ApiError> {
    let config = state.config.clone();
    let cache = state.ci_runs.clone();
    let stats = tokio::task::spawn_blocking(move || {
        let jobs = job_history::list(&job_history::dir(&config.artifacts_dir));
        let runs = config
            .ci_reports_dir
            .as_deref()
            .map(|dir| cache.get(dir, clock::now()))
            .unwrap_or_default();
        let builds = artifacts::list(&config.artifacts_dir);
        stats::compute(&jobs, &runs, &builds)
    })
    .await
    .map_err(|_| ApiError::internal())?;
    Ok(Json(stats))
}
