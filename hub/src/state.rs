use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rusqlite::Connection;
use tokio::sync::Semaphore;

use crate::agent_link::AgentLink;
use crate::ci_reports::RunsCache;
use crate::config::HubConfig;
use crate::ip_slots::IpSlots;
use crate::rate_limit::RateLimiter;

pub const LOGIN_ATTEMPTS_PER_MINUTE: usize = 10;
pub const ACTIONS_PER_MINUTE: usize = 30;
const LIMITER_KEYS: usize = 10_000;
/// Одновременных отдач APK на весь хаб.
pub const PARALLEL_DOWNLOADS: usize = 64;
/// Одновременных отдач APK одному пользователю: менеджер загрузок качает файл кусками.
pub const USER_DOWNLOADS: usize = 32;
/// Одновременных отдач файлов зеркала моделей: на весь хаб и на один адрес. Приложение
/// качает модели 52 соединениями сразу, скрипт загрузки большой файл 16 кусками.
pub const MODEL_DOWNLOADS: usize = 256;
pub const MODEL_DOWNLOADS_PER_IP: usize = 52;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Mutex<Connection>>,
    pub config: Arc<HubConfig>,
    pub agent: AgentLink,
    pub login_limiter: Arc<RateLimiter>,
    pub action_limiter: Arc<RateLimiter>,
    pub downloads: Arc<Semaphore>,
    pub user_downloads: Arc<Mutex<HashMap<i64, Arc<Semaphore>>>>,
    pub ci_runs: Arc<RunsCache>,
    pub model_downloads: Arc<Semaphore>,
    pub model_ip_slots: Arc<IpSlots>,
}

impl AppState {
    pub fn new(db: Connection, config: HubConfig) -> Self {
        let agent = AgentLink::new(config.agent_addr.clone(), config.agent_token.clone());
        Self {
            db: Arc::new(Mutex::new(db)),
            config: Arc::new(config),
            agent,
            login_limiter: Arc::new(RateLimiter::new(
                LOGIN_ATTEMPTS_PER_MINUTE,
                Duration::from_secs(60),
                LIMITER_KEYS,
            )),
            downloads: Arc::new(Semaphore::new(PARALLEL_DOWNLOADS)),
            user_downloads: Arc::new(Mutex::new(HashMap::new())),
            ci_runs: Arc::new(RunsCache::default()),
            model_downloads: Arc::new(Semaphore::new(MODEL_DOWNLOADS)),
            model_ip_slots: Arc::new(IpSlots::new(MODEL_DOWNLOADS_PER_IP, LIMITER_KEYS)),
            action_limiter: Arc::new(RateLimiter::new(
                ACTIONS_PER_MINUTE,
                Duration::from_secs(60),
                LIMITER_KEYS,
            )),
        }
    }

    pub async fn with_db<T, F>(&self, f: F) -> Result<T, crate::api_error::ApiError>
    where
        F: FnOnce(&Connection) -> rusqlite::Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let db = Arc::clone(&self.db);
        tokio::task::spawn_blocking(move || {
            let conn = db
                .lock()
                .map_err(|_| crate::api_error::ApiError::internal())?;
            f(&conn).map_err(|e| {
                tracing::error!(error = %e, "ошибка базы хаба");
                crate::api_error::ApiError::internal()
            })
        })
        .await
        .map_err(|_| crate::api_error::ApiError::internal())?
    }

    pub fn user_download_slot(&self, user_id: i64) -> Option<tokio::sync::OwnedSemaphorePermit> {
        let semaphore = {
            let mut map = self.user_downloads.lock().ok()?;
            Arc::clone(
                map.entry(user_id)
                    .or_insert_with(|| Arc::new(Semaphore::new(USER_DOWNLOADS))),
            )
        };
        semaphore.try_acquire_owned().ok()
    }
}
