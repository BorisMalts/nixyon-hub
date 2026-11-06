use std::collections::BTreeMap;
use std::io::Read;
use std::os::fd::OwnedFd;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::nofollow::{open_dir, open_dir_at, open_file_at};

pub const QUALITY_DIR: &str = "quality";
const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_SUMMARY_BYTES: u64 = 8 * 1024 * 1024;

// Отчёт пишет код из репозитория Nixyon, поэтому отдаются только эти имена и только
// на скачивание.
const FILES: [(&str, &str); 3] = [
    ("report.md", "text/markdown; charset=utf-8"),
    ("summary.json", "application/json"),
    ("clips.json", "application/json"),
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QualityFile {
    pub name: &'static str,
    pub size: u64,
    pub modified_at: i64,
}

pub struct OpenedReport {
    pub file: std::fs::File,
    pub len: u64,
    pub content_type: &'static str,
    pub download_name: String,
}

pub fn list(artifacts_dir: &Path) -> Vec<QualityFile> {
    let Some(dir) = report_dir(artifacts_dir) else {
        return Vec::new();
    };
    FILES
        .iter()
        .filter_map(|&(name, _)| {
            let (file, size) = open_file_at(&dir, name, MAX_FILE_BYTES).ok()?;
            let modified_at = file.metadata().ok()?.mtime();
            Some(QualityFile {
                name,
                size,
                modified_at,
            })
        })
        .collect()
}

pub fn open(artifacts_dir: &Path, name: &str) -> Option<OpenedReport> {
    let &(name, content_type) = FILES.iter().find(|(known, _)| *known == name)?;
    let dir = report_dir(artifacts_dir)?;
    let (file, len) = open_file_at(&dir, name, MAX_FILE_BYTES).ok()?;
    Some(OpenedReport {
        file,
        len,
        content_type,
        download_name: format!("nixyon-quality-{name}"),
    })
}

/// Скорость прошлой проверки, во сколько раз быстрее реального времени: по всему прогону
/// и по каждому набору. По ней страница считает, сколько займёт новая проверка.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Speeds {
    pub overall: Option<f64>,
    pub datasets: BTreeMap<String, f64>,
}

// Поля summary.json, которые пишет quality_eval из репозитория Nixyon.
#[derive(Deserialize)]
struct SummaryFile {
    audio_sec: f64,
    wall_sec: f64,
    #[serde(default)]
    datasets: Vec<SummaryDataset>,
}

#[derive(Deserialize)]
struct SummaryDataset {
    info: SummaryInfo,
    wall_sec: f64,
    summary: SummaryTotals,
}

#[derive(Deserialize)]
struct SummaryInfo {
    id: String,
}

#[derive(Deserialize)]
struct SummaryTotals {
    overall: SummaryOverall,
}

#[derive(Deserialize)]
struct SummaryOverall {
    audio_sec: f64,
}

/// Скорости из summary.json прошлой проверки. Отчёта нет, он старого вида или битый:
/// скоростей нет, страница возьмёт свою оценку.
pub fn speeds(artifacts_dir: &Path) -> Speeds {
    let read = || -> Option<SummaryFile> {
        let dir = report_dir(artifacts_dir)?;
        let (file, _) = open_file_at(&dir, "summary.json", MAX_SUMMARY_BYTES).ok()?;
        let mut text = String::new();
        file.take(MAX_SUMMARY_BYTES)
            .read_to_string(&mut text)
            .ok()?;
        serde_json::from_str(&text).ok()
    };
    let Some(summary) = read() else {
        return Speeds::default();
    };
    let ratio = |audio: f64, wall: f64| {
        (audio.is_finite() && wall.is_finite() && audio > 0.0 && wall > 0.0).then(|| audio / wall)
    };
    Speeds {
        overall: ratio(summary.audio_sec, summary.wall_sec),
        datasets: summary
            .datasets
            .into_iter()
            .filter_map(|d| Some((d.info.id, ratio(d.summary.overall.audio_sec, d.wall_sec)?)))
            .collect(),
    }
}

// Папку отчёта пишет рабочий контейнер: открытие по шагам без перехода по ссылкам,
// чтобы её нельзя было подменить между проверкой и чтением.
fn report_dir(artifacts_dir: &Path) -> Option<OwnedFd> {
    let root = open_dir(artifacts_dir).ok()?;
    open_dir_at(&root, QUALITY_DIR).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(std::path::PathBuf);

    impl Dir {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("nixyon-quality-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn speeds_come_from_the_last_summary_and_an_old_summary_gives_none() {
        let dir = Dir::new("speeds");
        assert_eq!(speeds(&dir.0), Speeds::default());
        let report = dir.0.join(QUALITY_DIR);
        std::fs::create_dir_all(&report).unwrap();
        std::fs::write(report.join("summary.json"), r#"{"overall":{"clips":1}}"#).unwrap();
        assert_eq!(speeds(&dir.0), Speeds::default());
        std::fs::write(
            report.join("summary.json"),
            r#"{"audio_sec":1000.0,"wall_sec":20.0,"datasets":[
                {"info":{"id":"cv"},"wall_sec":10.0,"summary":{"overall":{"audio_sec":600.0}}},
                {"info":{"id":"zero"},"wall_sec":0.0,"summary":{"overall":{"audio_sec":5.0}}}
            ]}"#,
        )
        .unwrap();
        let found = speeds(&dir.0);
        assert_eq!(found.overall, Some(50.0));
        assert_eq!(found.datasets.get("cv"), Some(&60.0));
        assert!(!found.datasets.contains_key("zero"));
    }

    #[test]
    fn without_a_report_nothing_is_listed() {
        let dir = Dir::new("none");
        assert!(list(&dir.0).is_empty());
        assert!(open(&dir.0, "report.md").is_none());
    }

    #[test]
    fn known_report_files_are_listed_in_fixed_order() {
        let dir = Dir::new("listed");
        let report = dir.0.join(QUALITY_DIR);
        std::fs::create_dir_all(&report).unwrap();
        std::fs::write(report.join("clips.json"), b"[]").unwrap();
        std::fs::write(report.join("report.md"), b"# x").unwrap();
        std::fs::write(report.join("other.txt"), b"x").unwrap();
        let names: Vec<&str> = list(&dir.0).iter().map(|f| f.name).collect();
        assert_eq!(names, ["report.md", "clips.json"]);
        let opened = open(&dir.0, "report.md").unwrap();
        assert_eq!(opened.len, 3);
        assert_eq!(opened.content_type, "text/markdown; charset=utf-8");
        assert_eq!(opened.download_name, "nixyon-quality-report.md");
    }

    #[test]
    fn only_known_names_can_be_opened() {
        let dir = Dir::new("names");
        let report = dir.0.join(QUALITY_DIR);
        std::fs::create_dir_all(&report).unwrap();
        std::fs::write(report.join("other.txt"), b"x").unwrap();
        assert!(open(&dir.0, "other.txt").is_none());
        assert!(open(&dir.0, "../quality/report.md").is_none());
    }

    #[test]
    fn a_symlinked_report_folder_or_file_is_not_served() {
        let dir = Dir::new("links");
        let elsewhere = dir.0.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("report.md"), b"secret").unwrap();
        std::os::unix::fs::symlink(&elsewhere, dir.0.join(QUALITY_DIR)).unwrap();
        assert!(open(&dir.0, "report.md").is_none());
        std::fs::remove_file(dir.0.join(QUALITY_DIR)).unwrap();
        std::fs::create_dir_all(dir.0.join(QUALITY_DIR)).unwrap();
        std::os::unix::fs::symlink(
            elsewhere.join("report.md"),
            dir.0.join(QUALITY_DIR).join("report.md"),
        )
        .unwrap();
        assert!(open(&dir.0, "report.md").is_none());
        assert!(list(&dir.0).is_empty());
    }
}
