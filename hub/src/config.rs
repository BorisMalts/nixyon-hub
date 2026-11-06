use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct HubConfig {
    pub listen: SocketAddr,
    pub db_path: PathBuf,
    pub public_origin: String,
    pub agent_addr: String,
    pub agent_token: String,
    pub artifacts_dir: PathBuf,
    pub trusted_proxies: Vec<IpAddr>,
    pub ci_reports_dir: Option<PathBuf>,
    /// Папка зеркала моделей, файлы под их SHA-256. None: зеркала нет.
    pub models_dir: Option<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("не задана переменная окружения {0}")]
    Missing(&'static str),
    #[error("переменная {0} задана неверно")]
    Invalid(&'static str),
    #[error("не удалось прочитать токен агента: {0}")]
    Token(std::io::Error),
}

impl HubConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        let var = |name: &'static str| std::env::var(name).map_err(|_| ConfigError::Missing(name));
        let public_origin = var("NIXYON_PUBLIC_ORIGIN")?;
        if !public_origin.starts_with("https://") && !public_origin.starts_with("http://localhost")
        {
            return Err(ConfigError::Invalid("NIXYON_PUBLIC_ORIGIN"));
        }
        let agent_token = std::fs::read_to_string(var("NIXYON_AGENT_TOKEN_FILE")?)
            .map_err(ConfigError::Token)?
            .trim()
            .to_string();
        if agent_token.len() < 32 {
            return Err(ConfigError::Invalid("NIXYON_AGENT_TOKEN_FILE"));
        }
        Ok(Self {
            listen: var("NIXYON_HUB_LISTEN")?
                .parse()
                .map_err(|_| ConfigError::Invalid("NIXYON_HUB_LISTEN"))?,
            db_path: var("NIXYON_HUB_DB")?.into(),
            public_origin: public_origin.trim_end_matches('/').to_string(),
            agent_addr: var("NIXYON_AGENT_ADDR")?,
            agent_token,
            artifacts_dir: var("NIXYON_ARTIFACTS_DIR")?.into(),
            ci_reports_dir: std::env::var_os("NIXYON_CI_REPORTS_DIR").map(PathBuf::from),
            models_dir: std::env::var_os("NIXYON_MODELS_DIR").map(PathBuf::from),
            trusted_proxies: match std::env::var("NIXYON_TRUSTED_PROXIES") {
                Ok(list) if !list.trim().is_empty() => list
                    .split(',')
                    .map(|ip| ip.trim().parse())
                    .collect::<Result<Vec<IpAddr>, _>>()
                    .map_err(|_| ConfigError::Invalid("NIXYON_TRUSTED_PROXIES"))?,
                _ => Vec::new(),
            },
        })
    }

    pub fn secure_cookies(&self) -> bool {
        self.public_origin.starts_with("https://")
    }
}
