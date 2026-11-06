use serde::{Deserialize, Serialize};

/// Набор записей для проверки качества, как его описывает `dataset.json` на сервере.
/// Длительность в целых секундах.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatasetSummary {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub has_text: bool,
    /// Кто писал тексты: human, auto или unknown. Старый агент поле не присылает.
    #[serde(default = "unknown")]
    pub reference: String,
    pub license: String,
    pub clips: u64,
    pub audio_sec: u64,
    /// Выбран ли набор на странице проверки сразу. Нет: включается только вручную
    /// (например, полные части без текста, прогон по ним идёт сутками).
    #[serde(default = "selected")]
    pub default_selected: bool,
}

fn unknown() -> String {
    "unknown".into()
}

fn selected() -> bool {
    true
}
