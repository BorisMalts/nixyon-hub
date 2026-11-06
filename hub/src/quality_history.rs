use std::io::Read;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::nofollow::{open_dir, open_file_at};

/// Папку истории пишет quality_eval из репозитория Nixyon: `quality-history/<имя>/summary.json`.
pub const HISTORY_DIR: &str = "quality-history";
const SUMMARY: &str = "summary.json";
const MAX_SUMMARY_BYTES: u64 = 8 * 1024 * 1024;
pub const MAX_RUNS: usize = 100;
const MAX_SCANNED: usize = 1000;
const MAX_DATASETS: usize = 64;
const MAX_GROUPS: usize = 16;

/// Доля ошибок, как её пишет quality_eval.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rate {
    pub errors: u64,
    pub reference: u64,
    pub rate: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SetsTotal {
    pub sets: u64,
    pub wer: Rate,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Totals {
    pub human: Option<SetsTotal>,
    pub all: Option<SetsTotal>,
}

/// Группа клипов набора (по длине или по цифрам в ответе).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RunGroup {
    pub label: String,
    pub clips: u64,
    pub audio_sec: f64,
    pub wer: Rate,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RunDataset {
    pub id: String,
    pub title: String,
    pub has_text: bool,
    pub reference: String,
    pub clips: u64,
    pub audio_sec: f64,
    pub wer: Rate,
    pub cer: Rate,
    pub empty: u64,
    pub groups: Vec<RunGroup>,
}

/// Прошлый прогон проверки: когда, на каком коммите и моделях, с какими итогами.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QualityRun {
    pub id: String,
    pub finished_at: i64,
    pub commit: Option<String>,
    pub models_version: String,
    pub audio_sec: f64,
    pub wall_sec: f64,
    pub totals: Totals,
    pub datasets: Vec<RunDataset>,
}

// Поля summary.json, которые нужны истории.
#[derive(Deserialize)]
struct SummaryFile {
    finished_at: i64,
    #[serde(default)]
    commit: Option<String>,
    models_version: String,
    audio_sec: f64,
    wall_sec: f64,
    totals: Totals,
    datasets: Vec<SummaryDataset>,
}

#[derive(Deserialize)]
struct SummaryDataset {
    info: SummaryInfo,
    summary: SummaryTotals,
}

#[derive(Deserialize)]
struct SummaryInfo {
    id: String,
    title: String,
    has_text: bool,
    #[serde(default)]
    reference: Option<String>,
}

#[derive(Deserialize)]
struct SummaryTotals {
    overall: SummaryGroup,
    by_duration: Vec<SummaryGroup>,
    with_digits: SummaryGroup,
    without_digits: SummaryGroup,
}

#[derive(Deserialize)]
struct SummaryGroup {
    label: String,
    clips: u64,
    audio_sec: f64,
    wer: Rate,
    cer: Rate,
    empty: u64,
}

pub fn is_run_id(id: &str) -> bool {
    let (time, commit) = match id.split_once('-') {
        Some((time, commit)) => (time, Some(commit)),
        None => (id, None),
    };
    time.len() == 10
        && time.bytes().all(|b| b.is_ascii_digit())
        && commit.is_none_or(|c| {
            (7..=40).contains(&c.len())
                && c.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
}

/// Прошлые прогоны, новые раньше. Прогоны с битыми или старыми итогами пропускаются.
pub fn list(artifacts_dir: &Path) -> Vec<QualityRun> {
    let dir = artifacts_dir.join(HISTORY_DIR);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .take(MAX_SCANNED)
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| is_run_id(name))
        .collect();
    // Время в имени из 10 цифр: по имени и по времени порядок один.
    names.sort_unstable_by(|a, b| b.cmp(a));
    names
        .into_iter()
        .filter_map(|name| read_run(&dir, name))
        .take(MAX_RUNS)
        .collect()
}

fn read_run(dir: &Path, id: String) -> Option<QualityRun> {
    // Открытие без перехода по ссылке: папку пишет рабочий контейнер.
    let root = open_dir(&dir.join(&id)).ok()?;
    let (file, _) = open_file_at(&root, SUMMARY, MAX_SUMMARY_BYTES).ok()?;
    let mut text = String::new();
    file.take(MAX_SUMMARY_BYTES)
        .read_to_string(&mut text)
        .ok()?;
    let summary: SummaryFile = serde_json::from_str(&text).ok()?;
    Some(QualityRun {
        id,
        finished_at: summary.finished_at,
        commit: summary.commit.filter(|c| c.len() <= 40),
        models_version: short(&summary.models_version, 300),
        audio_sec: summary.audio_sec,
        wall_sec: summary.wall_sec,
        totals: summary.totals,
        datasets: summary
            .datasets
            .into_iter()
            .take(MAX_DATASETS)
            .map(dataset)
            .collect(),
    })
}

fn dataset(d: SummaryDataset) -> RunDataset {
    let s = d.summary;
    let groups = s
        .by_duration
        .into_iter()
        .chain([s.with_digits, s.without_digits])
        .take(MAX_GROUPS)
        .map(|g| RunGroup {
            label: short(&g.label, 64),
            clips: g.clips,
            audio_sec: g.audio_sec,
            wer: g.wer,
        })
        .collect();
    RunDataset {
        id: short(&d.info.id, 64),
        title: short(&d.info.title, 200),
        has_text: d.info.has_text,
        reference: match d.info.reference.as_deref() {
            Some(known @ ("human" | "auto")) => known.into(),
            _ => "unknown".into(),
        },
        clips: s.overall.clips,
        audio_sec: s.overall.audio_sec,
        wer: s.overall.wer,
        cer: s.overall.cer,
        empty: s.overall.empty,
        groups,
    }
}

fn short(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(std::path::PathBuf);

    impl Dir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "nixyon-quality-history-{name}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(path.join(HISTORY_DIR)).unwrap();
            Self(path)
        }

        fn run(&self, id: &str, summary: &str) {
            let dir = self.0.join(HISTORY_DIR).join(id);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(SUMMARY), summary).unwrap();
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn group(label: &str, errors: u64) -> String {
        format!(
            r#"{{"label":"{label}","clips":2,"audio_sec":4.0,"wer":{{"errors":{errors},"reference":10,"rate":{}}},"cer":{{"errors":0,"reference":0,"rate":null}},"empty":0}}"#,
            errors as f64 / 10.0
        )
    }

    fn summary(finished_at: i64, errors: u64) -> String {
        format!(
            r#"{{"models_version":"m1","commit":"91ecf11","finished_at":{finished_at},"audio_sec":8.0,"wall_sec":1.0,
            "totals":{{"human":{{"sets":1,"wer":{{"errors":{errors},"reference":10,"rate":0.1}}}},"all":null}},
            "datasets":[{{"info":{{"id":"cv","title":"Common Voice","kind":"single","has_text":true,"reference":"human"}},
            "summary":{{"overall":{},"by_duration":[{}],"with_digits":{},"without_digits":{},"worst":[]}}}}]}}"#,
            group("все клипы", errors),
            group("до 3 с", errors),
            group("в ответе есть цифры", 0),
            group("в ответе нет цифр", errors)
        )
    }

    #[test]
    fn runs_are_read_newest_first_and_broken_ones_skipped() {
        let dir = Dir::new("list");
        assert!(list(&dir.0).is_empty());
        dir.run("1761900000-91ecf11", &summary(1_761_900_000, 1));
        dir.run("1761990000-91ecf11", &summary(1_761_990_000, 3));
        dir.run("1761950000", r#"{"overall":{}}"#);
        dir.run("notes", &summary(1, 1));
        let runs = list(&dir.0);
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].id, "1761990000-91ecf11");
        assert_eq!(runs[0].commit.as_deref(), Some("91ecf11"));
        assert_eq!(runs[0].totals.human.unwrap().wer.errors, 3);
        let cv = &runs[0].datasets[0];
        assert_eq!((cv.id.as_str(), cv.reference.as_str()), ("cv", "human"));
        let labels: Vec<&str> = cv.groups.iter().map(|g| g.label.as_str()).collect();
        assert_eq!(
            labels,
            ["до 3 с", "в ответе есть цифры", "в ответе нет цифр"]
        );
    }

    #[test]
    fn a_linked_run_folder_is_not_read() {
        let dir = Dir::new("links");
        let elsewhere = dir.0.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(elsewhere.join(SUMMARY), summary(1_761_900_000, 1)).unwrap();
        std::os::unix::fs::symlink(&elsewhere, dir.0.join(HISTORY_DIR).join("1761900000")).unwrap();
        assert!(list(&dir.0).is_empty());
    }

    #[test]
    fn run_ids_are_time_and_an_optional_commit() {
        assert!(is_run_id("1761900000"));
        assert!(is_run_id(
            "1761900000-91ecf11b7ea0a40380d75b6e1fb2fae0f3f422a0"
        ));
        assert!(!is_run_id("1761900000-XYZ1234"));
        assert!(!is_run_id("../1761900000"));
    }
}
