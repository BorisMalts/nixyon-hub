use serde::{Deserialize, Serialize};

use super::Artifact;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    Pull,
    Build,
    Quality,
    /// Прогон записей без разметки через eval с несколькими сочетаниями моделей.
    LiveEval,
    /// Обучение и замер классификатора «ребёнок или взрослый».
    Training,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Running,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobStatus {
    pub id: u64,
    pub kind: JobKind,
    pub state: JobState,
    pub started_by: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub artifact: Option<Artifact>,
}
