use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub listen: SocketAddr,
    pub token_file: PathBuf,
    pub repo_dir: PathBuf,
    pub repo_url: String,
    pub artifacts_dir: PathBuf,
    pub datasets: PathBuf,
    pub allowed_peers: Option<Vec<IpAddr>>,
    pub heavy_lock: Option<PathBuf>,
    /// Папка сборки cargo; без CARGO_TARGET_DIR это target в рабочей копии.
    pub target_dir: PathBuf,
    /// Папка зеркала моделей, которую раздаёт хаб.
    pub models_mirror: Option<PathBuf>,
    /// Папка свободных наборов для обучения, которые агент качает сам.
    pub training_data: PathBuf,
    /// Токен GitHub для отмены и перезапуска прогонов CI; без него эти команды отказывают.
    pub github_token_file: Option<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
#[error("не задана переменная окружения {0}")]
pub struct MissingVar(pub &'static str);

impl AgentConfig {
    pub fn from_env() -> Result<Self, Box<dyn std::error::Error>> {
        let var = |name: &'static str| std::env::var(name).map_err(|_| MissingVar(name));
        let repo_dir: PathBuf = var("NIXYON_REPO_DIR")?.into();
        Ok(Self {
            target_dir: std::env::var_os("CARGO_TARGET_DIR")
                .map_or_else(|| repo_dir.join("target"), PathBuf::from),
            listen: var("NIXYON_AGENT_LISTEN")?.parse()?,
            token_file: var("NIXYON_AGENT_TOKEN_FILE")?.into(),
            repo_dir,
            repo_url: var("NIXYON_REPO_URL")?,
            artifacts_dir: var("NIXYON_ARTIFACTS_DIR")?.into(),
            datasets: var("NIXYON_DATASETS")?.into(),
            heavy_lock: std::env::var_os("NIXYON_HEAVY_LOCK").map(PathBuf::from),
            models_mirror: std::env::var_os("NIXYON_MODELS_MIRROR_DIR").map(PathBuf::from),
            training_data: var("NIXYON_TRAINING_DATA")?.into(),
            github_token_file: std::env::var_os("NIXYON_GITHUB_TOKEN_FILE").map(PathBuf::from),
            allowed_peers: match std::env::var("NIXYON_AGENT_ALLOWED_PEERS") {
                Ok(list) => Some(
                    list.split(',')
                        .map(|ip| ip.trim().parse())
                        .collect::<Result<Vec<IpAddr>, _>>()?,
                ),
                Err(_) => None,
            },
        })
    }
}
