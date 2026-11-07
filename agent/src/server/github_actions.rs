//! Отмена и перезапуск прогонов CI в GitHub Actions. Хаб в интернет не ходит, поэтому
//! команды выполняет агент. Запрос делает `curl`: он уже есть в контейнере сборки, свой
//! HTTP-клиент с TLS агенту не нужен. Токен уходит в `curl` через стандартный ввод, поэтому
//! в списке процессов его не видно.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::protocol::ErrorCode;

const API: &str = "https://api.github.com";
const TIMEOUT: Duration = Duration::from_secs(30);
const MAX_TOKEN_BYTES: u64 = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CiAction {
    Cancel,
    Rerun,
}

impl CiAction {
    fn path(self) -> &'static str {
        match self {
            CiAction::Cancel => "cancel",
            CiAction::Rerun => "rerun",
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CiError {
    #[error("токен GitHub на сервере не настроен")]
    NotConfigured,
    #[error("номер прогона не число")]
    BadRun,
    #[error("GitHub не нашёл прогон")]
    NoRun,
    #[error("GitHub не дал доступа, код {0}")]
    Denied(u16),
    #[error("прогон в таком состоянии, что команда к нему не подходит, код {0}")]
    Conflict(u16),
    #[error("GitHub не ответил: {0}")]
    Unreachable(String),
}

impl CiError {
    pub fn code(&self) -> ErrorCode {
        match self {
            CiError::NotConfigured => ErrorCode::NotConfigured,
            CiError::BadRun | CiError::NoRun => ErrorCode::BadRequest,
            CiError::Denied(_) => ErrorCode::Denied,
            CiError::Conflict(_) => ErrorCode::Conflict,
            CiError::Unreachable(_) => ErrorCode::Failed,
        }
    }
}

pub struct GithubActions {
    api: String,
    repo: String,
    token_file: Option<PathBuf>,
}

impl GithubActions {
    /// Репозиторий берётся из адреса, с которого агент клонирует Nixyon. Без токена агент
    /// работает как раньше, команды CI отвечают `NotConfigured`.
    pub fn new(repo_url: &str, token_file: Option<PathBuf>) -> Option<Self> {
        Some(Self {
            api: API.into(),
            repo: repo_slug(repo_url)?,
            token_file,
        })
    }

    #[cfg(test)]
    fn with_api(api: String, repo: &str, token_file: Option<PathBuf>) -> Self {
        Self {
            api,
            repo: repo.into(),
            token_file,
        }
    }

    pub async fn run(&self, action: CiAction, run_id: &str) -> Result<(), CiError> {
        if !is_run_id(run_id) {
            return Err(CiError::BadRun);
        }
        let token = self.token().await?;
        let url = format!(
            "{}/repos/{}/actions/runs/{run_id}/{}",
            self.api,
            self.repo,
            action.path()
        );
        let status = post(&url, &token).await?;
        outcome(status)
    }

    async fn token(&self) -> Result<String, CiError> {
        let Some(path) = &self.token_file else {
            return Err(CiError::NotConfigured);
        };
        let bytes = tokio::fs::read(path)
            .await
            .map_err(|_| CiError::NotConfigured)?;
        if bytes.len() as u64 > MAX_TOKEN_BYTES {
            return Err(CiError::NotConfigured);
        }
        let token = String::from_utf8(bytes).map_err(|_| CiError::NotConfigured)?;
        let token = token.trim();
        let printable = token.bytes().all(|b| b.is_ascii_graphic());
        if token.is_empty() || !printable {
            return Err(CiError::NotConfigured);
        }
        Ok(token.to_owned())
    }
}

async fn post(url: &str, token: &str) -> Result<u16, CiError> {
    let mut child = Command::new("curl")
        .args([
            "-sS",
            "-o",
            "/dev/null",
            "-w",
            "%{http_code}",
            "--max-time",
            "20",
            "-X",
            "POST",
            "-H",
            "Accept: application/vnd.github+json",
            "-H",
            "X-GitHub-Api-Version: 2022-11-28",
            "-H",
            "@-",
            url,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| CiError::Unreachable(format!("curl не запустился: {e}")))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(format!("Authorization: Bearer {token}\n").as_bytes())
            .await
            .map_err(|e| CiError::Unreachable(e.to_string()))?;
    }
    let out = tokio::time::timeout(TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| CiError::Unreachable("время ожидания вышло".into()))?
        .map_err(|e| CiError::Unreachable(e.to_string()))?;
    let code = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    match code.parse::<u16>() {
        Ok(status) if status != 0 => Ok(status),
        _ => Err(CiError::Unreachable(
            String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        )),
    }
}

fn outcome(status: u16) -> Result<(), CiError> {
    match status {
        200..=299 => Ok(()),
        404 => Err(CiError::NoRun),
        401 | 403 => Err(CiError::Denied(status)),
        409 | 422 => Err(CiError::Conflict(status)),
        other => Err(CiError::Unreachable(format!("ответ {other}"))),
    }
}

/// Номер прогона GitHub: только цифры, не больше 20.
pub fn is_run_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 20 && id.bytes().all(|b| b.is_ascii_digit())
}

/// «владелец/репозиторий» из адреса GitHub по HTTPS или SSH.
fn repo_slug(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("git@github.com:"))
        .or_else(|| url.strip_prefix("ssh://git@github.com/"))?;
    let rest = rest.trim_end_matches('/');
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    let (owner, name) = rest.split_once('/')?;
    let allowed = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    };
    (allowed(owner) && allowed(name) && !name.contains('/')).then(|| format!("{owner}/{name}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, BufReader};
    use tokio::net::TcpListener;

    #[test]
    fn repo_is_taken_from_https_and_ssh_addresses() {
        let slug = |u| repo_slug(u);
        assert_eq!(
            slug("https://github.com/Andre-wb/Nixyon.git").as_deref(),
            Some("Andre-wb/Nixyon")
        );
        assert_eq!(
            slug("git@github.com:Andre-wb/Nixyon.git").as_deref(),
            Some("Andre-wb/Nixyon")
        );
        assert_eq!(
            slug("https://github.com/Andre-wb/Nixyon").as_deref(),
            Some("Andre-wb/Nixyon")
        );
        assert_eq!(slug("https://example.org/a/b"), None);
        assert_eq!(slug("https://github.com/a/b/c"), None);
        assert_eq!(slug("https://github.com/a/b?x=1"), None);
    }

    #[test]
    fn run_id_must_be_digits() {
        assert!(is_run_id("42372406574"));
        assert!(!is_run_id(""));
        assert!(!is_run_id("1/../2"));
        assert!(!is_run_id("123456789012345678901"));
    }

    #[test]
    fn github_status_codes_become_errors_the_hub_can_explain() {
        assert_eq!(outcome(202), Ok(()));
        assert_eq!(outcome(201), Ok(()));
        assert_eq!(outcome(404), Err(CiError::NoRun));
        assert_eq!(outcome(403), Err(CiError::Denied(403)));
        assert_eq!(outcome(409), Err(CiError::Conflict(409)));
        assert!(matches!(outcome(500), Err(CiError::Unreachable(_))));
    }

    #[tokio::test]
    async fn without_token_nothing_is_sent() {
        let api = GithubActions::with_api("http://127.0.0.1:9".into(), "a/b", None);
        assert_eq!(
            api.run(CiAction::Cancel, "1").await,
            Err(CiError::NotConfigured)
        );
        let missing = std::env::temp_dir().join("nixyon-no-such-github-token");
        let api = GithubActions::with_api("http://127.0.0.1:9".into(), "a/b", Some(missing));
        assert_eq!(
            api.run(CiAction::Cancel, "1").await,
            Err(CiError::NotConfigured)
        );
    }

    #[tokio::test]
    async fn bad_run_id_is_refused_before_any_request() {
        let api = GithubActions::with_api("http://127.0.0.1:9".into(), "a/b", None);
        assert_eq!(api.run(CiAction::Rerun, "../x").await, Err(CiError::BadRun));
    }

    // Подставной GitHub: принимает один запрос, отвечает 202 и отдаёт строку запроса и
    // заголовок авторизации.
    async fn fake_github() -> (String, tokio::task::JoinHandle<(String, String)>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read, mut write) = stream.into_split();
            let mut reader = BufReader::new(read);
            let mut first = String::new();
            reader.read_line(&mut first).await.unwrap();
            let mut auth = String::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).await.unwrap();
                if line.trim().is_empty() {
                    break;
                }
                if line.to_ascii_lowercase().starts_with("authorization:") {
                    auth = line.trim().to_owned();
                }
            }
            write
                .write_all(
                    b"HTTP/1.1 202 Accepted\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                )
                .await
                .unwrap();
            (first.trim().to_owned(), auth)
        });
        (format!("http://{addr}"), handle)
    }

    #[tokio::test]
    async fn cancel_posts_to_the_run_with_the_token_from_the_file() {
        let token = std::env::temp_dir().join(format!("nixyon-gh-token-{}", std::process::id()));
        std::fs::write(&token, "github_pat_test\n").unwrap();
        let (api, server) = fake_github().await;
        let actions = GithubActions::with_api(api, "Andre-wb/Nixyon", Some(token.clone()));
        assert_eq!(actions.run(CiAction::Cancel, "123").await, Ok(()));
        let (request, auth) = server.await.unwrap();
        assert_eq!(
            request,
            "POST /repos/Andre-wb/Nixyon/actions/runs/123/cancel HTTP/1.1"
        );
        assert_eq!(auth, "Authorization: Bearer github_pat_test");
        let _ = std::fs::remove_file(token);
    }
}
