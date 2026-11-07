use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use nixyon_agent::server::{
    AgentConfig, AndroidWorkflow, Context, EXPERIMENTS_DIR, GithubActions, HeavyLock, Jobs,
    REQUEST_TIMEOUT, TRAINING_DIR, TokenCheck, serve,
};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().json().with_target(false).init();
    let config = AgentConfig::from_env()?;
    let token = std::fs::read_to_string(&config.token_file)?;
    let token = TokenCheck::new(&token).ok_or("токен агента короче 32 символов")?;
    let workflow = Arc::new(AndroidWorkflow {
        repo_dir: config.repo_dir.clone(),
        repo_url: config.repo_url.clone(),
        datasets: config.datasets.clone(),
        quality_dir: config.artifacts_dir.join("quality"),
        experiments_dir: config.artifacts_dir.join(EXPERIMENTS_DIR),
        target_dir: config.target_dir.clone(),
        models_mirror: config.models_mirror.clone(),
        training_data: config.training_data.clone(),
        training_dir: config.artifacts_dir.join(TRAINING_DIR),
    });
    let context = Arc::new(Context {
        jobs: Jobs::with_lock(
            workflow,
            config.artifacts_dir.clone(),
            config.heavy_lock.clone().map(HeavyLock::new),
        ),
        token,
        repo_dir: config.repo_dir.clone(),
        datasets: config.datasets.clone(),
        shell: "bash".into(),
        terminals: AtomicUsize::new(0),
        request_timeout: REQUEST_TIMEOUT,
        allowed_peers: config.allowed_peers.clone(),
        github: GithubActions::new(&config.repo_url, config.github_token_file.clone()),
    });
    let listener = TcpListener::bind(config.listen).await?;
    tracing::info!(listen = %config.listen, "агент слушает");
    serve(listener, context).await?;
    Ok(())
}
