use crate::protocol::QualityTake;

/// Что делать в задании, уже сверенное агентом: наборы для проверки качества, имена
/// записей для прогона без разметки или наборы для обучения. У других заданий план пустой.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JobPlan {
    pub takes: Vec<QualityTake>,
    pub records: Vec<String>,
    pub training: Vec<String>,
}

impl JobPlan {
    pub fn quality(takes: Vec<QualityTake>) -> Self {
        Self {
            takes,
            ..Self::default()
        }
    }

    pub fn records(records: Vec<String>) -> Self {
        Self {
            records,
            ..Self::default()
        }
    }

    pub fn training(training: Vec<String>) -> Self {
        Self {
            training,
            ..Self::default()
        }
    }
}
