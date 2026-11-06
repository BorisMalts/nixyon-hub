use std::io::Read;
use std::path::Path;

use serde::Deserialize;

use crate::protocol::{DatasetSummary, QualityTake};

const INFO_FILE: &str = "dataset.json";
const MAX_INFO_BYTES: u64 = 64 * 1024;
const MAX_DATASETS: usize = 200;
/// Сколько наборов можно проверить за одно задание.
pub const MAX_PLAN: usize = 32;
/// Сколько клипов одного набора можно исключить как клипы с плохим эталоном.
pub const MAX_EXCLUDED: usize = 20_000;

// Описание набора пишет scripts/build-dataset-index.py из репозитория Nixyon.
#[derive(Deserialize)]
struct Info {
    id: String,
    title: String,
    kind: String,
    has_text: bool,
    #[serde(default)]
    reference: Option<String>,
    license: String,
    clips: u64,
    hours: f64,
    #[serde(default = "selected")]
    default_selected: bool,
}

fn selected() -> bool {
    true
}

/// Наборы в корне по имени. Папка без описания, с битым описанием или ссылка вместо
/// папки пропускается: в корне лежат и сами записи.
pub fn list_datasets(root: &Path) -> Vec<DatasetSummary> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut found: Vec<DatasetSummary> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| is_valid_id(name))
        .filter_map(|name| read_info(root, &name))
        .collect();
    found.sort_by(|a, b| a.id.cmp(&b.id));
    found.truncate(MAX_DATASETS);
    found
}

/// План проверки годится: не пустой, наборы есть и не повторяются, минут от 1 и не
/// больше, чем звука в наборе, исключённых клипов не больше предела и их id обычные.
pub fn check_plan(plan: &[QualityTake], available: &[DatasetSummary]) -> Result<(), String> {
    if plan.is_empty() {
        return Err("не выбран ни один набор".into());
    }
    if plan.len() > MAX_PLAN {
        return Err(format!("наборов больше {MAX_PLAN}"));
    }
    for (i, take) in plan.iter().enumerate() {
        let Some(set) = available.iter().find(|d| d.id == take.dataset) else {
            return Err(format!("набора {} нет на сервере", take.dataset));
        };
        if plan[..i].iter().any(|t| t.dataset == take.dataset) {
            return Err(format!("набор {} выбран дважды", take.dataset));
        }
        let most = set.audio_sec.div_ceil(60).max(1);
        if take.minutes.is_some_and(|m| m == 0 || u64::from(m) > most) {
            return Err(format!("{}: минут от 1 до {most}", take.dataset));
        }
        if take.exclude.len() > MAX_EXCLUDED {
            return Err(format!(
                "{}: исключённых клипов больше {MAX_EXCLUDED}",
                take.dataset
            ));
        }
        if !take.exclude.iter().all(|id| is_valid_clip_id(id)) {
            return Err(format!("{}: недопустимый id клипа", take.dataset));
        }
    }
    Ok(())
}

// Описание пишет скрипт из репозитория Nixyon; незнакомое значение страница показала бы
// как есть, поэтому оно сводится к «неизвестно».
fn reference_of(value: Option<&str>) -> String {
    match value {
        Some(known @ ("human" | "auto")) => known.into(),
        _ => "unknown".into(),
    }
}

/// Id клипа из индекса набора: буквы, цифры, точка, дефис и подчёркивание.
pub fn is_valid_clip_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn read_info(root: &Path, id: &str) -> Option<DatasetSummary> {
    let path = root.join(id).join(INFO_FILE);
    let meta = std::fs::symlink_metadata(&path).ok()?;
    if !meta.is_file() || meta.len() > MAX_INFO_BYTES {
        return None;
    }
    let mut text = String::new();
    std::fs::File::open(&path)
        .ok()?
        .take(MAX_INFO_BYTES)
        .read_to_string(&mut text)
        .ok()?;
    let info: Info = serde_json::from_str(&text).ok()?;
    if info.id != id || !info.hours.is_finite() || info.hours < 0.0 {
        return None;
    }
    Some(DatasetSummary {
        id: info.id,
        title: info.title.chars().take(200).collect(),
        kind: info.kind.chars().take(32).collect(),
        has_text: info.has_text,
        reference: reference_of(info.reference.as_deref()),
        license: info.license.chars().take(100).collect(),
        clips: info.clips,
        // Часы набора конечны и неотрицательны, секунды помещаются в u64.
        audio_sec: (info.hours * 3600.0).round() as u64,
        default_selected: info.default_selected,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Root(std::path::PathBuf);

    impl Root {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("nixyon-agent-sets-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn set(&self, dir: &str, id: &str) {
            std::fs::create_dir_all(self.0.join(dir)).unwrap();
            std::fs::write(
                self.0.join(dir).join(INFO_FILE),
                format!(
                    r#"{{"id":"{id}","title":"Набор","kind":"single","has_text":true,"license":"CC0","clips":10,"hours":1.5}}"#
                ),
            )
            .unwrap();
        }
    }

    impl Drop for Root {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn take(dataset: &str, minutes: Option<u32>) -> QualityTake {
        QualityTake {
            dataset: dataset.into(),
            minutes,
            exclude: Vec::new(),
        }
    }

    #[test]
    fn described_sets_are_listed_and_the_rest_is_skipped() {
        let root = Root::new("list");
        root.set("cv", "cv");
        root.set("alias", "cv");
        std::fs::create_dir_all(root.0.join("sources")).unwrap();
        std::os::unix::fs::symlink(root.0.join("cv"), root.0.join("linked")).unwrap();
        let sets = list_datasets(&root.0);
        assert_eq!(sets.len(), 1);
        assert_eq!(sets[0].id, "cv");
        assert_eq!(sets[0].audio_sec, 5_400);
        assert!(sets[0].default_selected, "без поля набор выбран");
        assert_eq!(sets[0].reference, "unknown", "без поля неизвестно");
    }

    #[test]
    fn a_set_can_be_left_out_unless_chosen_by_hand() {
        let root = Root::new("off");
        std::fs::create_dir_all(root.0.join("radio_full")).unwrap();
        std::fs::write(
            root.0.join("radio_full").join(INFO_FILE),
            r#"{"id":"radio_full","title":"Радио","kind":"single","has_text":false,"reference":"auto","license":"CC BY-NC 4.0","clips":10,"hours":2600.0,"default_selected":false}"#,
        )
        .unwrap();
        let sets = list_datasets(&root.0);
        assert!(!sets[0].default_selected);
        assert_eq!(sets[0].reference, "auto");
        assert!(check_plan(&[take("radio_full", Some(10))], &sets).is_ok());
    }

    #[test]
    fn an_unknown_reference_value_reads_as_unknown() {
        let root = Root::new("reference");
        std::fs::create_dir_all(root.0.join("odd")).unwrap();
        std::fs::write(
            root.0.join("odd").join(INFO_FILE),
            r#"{"id":"odd","title":"Набор","kind":"single","has_text":true,"reference":"robot","license":"CC0","clips":1,"hours":0.1}"#,
        )
        .unwrap();
        assert_eq!(list_datasets(&root.0)[0].reference, "unknown");
    }

    #[test]
    fn a_plan_must_name_existing_sets_once_with_sane_minutes() {
        let root = Root::new("plan");
        root.set("cv", "cv");
        root.set("talk", "talk");
        let sets = list_datasets(&root.0);
        assert!(check_plan(&[take("cv", Some(30)), take("talk", None)], &sets).is_ok());
        assert!(check_plan(&[], &sets).is_err());
        assert!(check_plan(&[take("nope", None)], &sets).is_err());
        assert!(check_plan(&[take("cv", None), take("cv", Some(5))], &sets).is_err());
        assert!(check_plan(&[take("cv", Some(0))], &sets).is_err());
        assert!(
            check_plan(&[take("cv", Some(90))], &sets).is_ok(),
            "весь набор в минутах"
        );
        assert!(
            check_plan(&[take("cv", Some(91))], &sets).is_err(),
            "больше, чем есть"
        );
        assert!(check_plan(&[take("../cv", None)], &sets).is_err());
        let mut marked = take("cv", None);
        marked.exclude = vec!["common_voice_ru_1".into(), "1756465406.16342".into()];
        assert!(check_plan(std::slice::from_ref(&marked), &sets).is_ok());
        marked.exclude = vec!["../x".into()];
        assert!(check_plan(std::slice::from_ref(&marked), &sets).is_err());
        marked.exclude = vec!["a".into(); MAX_EXCLUDED + 1];
        assert!(check_plan(std::slice::from_ref(&marked), &sets).is_err());
    }
}
