use serde::{Deserialize, Serialize};

/// Сколько брать из одного набора при проверке качества: целые минуты звука или весь
/// набор (`None`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QualityTake {
    pub dataset: String,
    pub minutes: Option<u32>,
    /// Клипы набора с плохим эталоном: проверка их не берёт.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
}

impl QualityTake {
    /// Аргумент для scripts/quality.sh: `<набор>=<минуты>` или `<набор>=all`.
    pub fn arg(&self) -> String {
        match self.minutes {
            Some(m) => format!("{}={m}", self.dataset),
            None => format!("{}=all", self.dataset),
        }
    }
}
