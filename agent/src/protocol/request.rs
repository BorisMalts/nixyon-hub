use serde::{Deserialize, Serialize};

use super::QualityTake;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    Pull,
    Build,
    Quality,
    LiveEval,
    Training,
    Datasets,
    Status,
    Follow,
    Terminal,
    /// Отменить прогон CI в GitHub Actions.
    CiCancel,
    /// Запустить прогон CI в GitHub Actions заново.
    CiRerun,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub token: String,
    pub op: Op,
    pub user: String,
    #[serde(default)]
    pub cols: u16,
    #[serde(default)]
    pub rows: u16,
    /// Что проверять в задании `Quality`.
    #[serde(default)]
    pub plan: Vec<QualityTake>,
    /// Имена записей для задания `LiveEval`, без расширения.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub records: Vec<String>,
    /// Наборы для задания `Training`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub training: Vec<String>,
    /// Номер прогона GitHub Actions для `CiCancel` и `CiRerun`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub ci_run: String,
}
