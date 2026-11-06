use std::os::fd::OwnedFd;
use std::path::Path;

use serde::Serialize;

use crate::nofollow::{open_dir, open_dir_at, open_file_at};
use crate::quality_report::OpenedReport;

/// Папка итогов обучения в папке артефактов, по папке на задание; её пишет агент.
pub const TRAINING_DIR: &str = "training";
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_RUNS: usize = 100;

// Итоги пишет программа из репозитория Nixyon, поэтому отдаются только эти имена.
// Признаки клипов (features.json) не отдаются: это образцы голоса.
const FILES: [(&str, &str); 4] = [
    ("report.md", "text/markdown; charset=utf-8"),
    ("summary.json", "application/json"),
    ("child_adult_redimnet2.json", "application/json"),
    ("child_adult_age_gender_hidden.json", "application/json"),
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TrainingFile {
    pub name: String,
    pub size: u64,
}

/// Номер задания как имя папки: только цифры.
fn is_valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 20 && id.bytes().all(|b| b.is_ascii_digit())
}

fn root(artifacts_dir: &Path) -> Option<OwnedFd> {
    open_dir(&artifacts_dir.join(TRAINING_DIR)).ok()
}

fn run(artifacts_dir: &Path, id: &str) -> Option<OwnedFd> {
    if !is_valid_id(id) {
        return None;
    }
    open_dir_at(&root(artifacts_dir)?, id).ok()
}

/// Номера обучений, новые первыми.
pub fn list_runs(artifacts_dir: &Path) -> Vec<u64> {
    let Some(dir) = root(artifacts_dir) else {
        return Vec::new();
    };
    let Ok(entries) = rustix::fs::Dir::read_from(&dir) else {
        return Vec::new();
    };
    let mut ids: Vec<u64> = entries
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().to_str().ok().map(str::to_owned))
        .filter(|n| is_valid_id(n) && open_dir_at(&dir, n).is_ok())
        .filter_map(|n| n.parse().ok())
        .collect();
    ids.sort_unstable_by(|a, b| b.cmp(a));
    ids.truncate(MAX_RUNS);
    ids
}

/// Готовые файлы обучения. None: такого обучения нет.
pub fn list_files(artifacts_dir: &Path, id: &str) -> Option<Vec<TrainingFile>> {
    let dir = run(artifacts_dir, id)?;
    Some(
        FILES
            .iter()
            .filter_map(|&(name, _)| {
                let (_, size) = open_file_at(&dir, name, MAX_FILE_BYTES).ok()?;
                Some(TrainingFile {
                    name: name.to_owned(),
                    size,
                })
            })
            .collect(),
    )
}

/// Отчёт, итоги или веса обучения.
pub fn open_file(artifacts_dir: &Path, id: &str, name: &str) -> Option<OpenedReport> {
    let &(name, content_type) = FILES.iter().find(|(known, _)| *known == name)?;
    let dir = run(artifacts_dir, id)?;
    let (file, len) = open_file_at(&dir, name, MAX_FILE_BYTES).ok()?;
    Some(OpenedReport {
        file,
        len,
        content_type,
        download_name: format!("nixyon-training-{id}-{name}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Root(std::path::PathBuf);

    impl Root {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("nixyon-hub-training-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(path.join(TRAINING_DIR)).unwrap();
            Self(path)
        }

        fn runs(&self) -> std::path::PathBuf {
            self.0.join(TRAINING_DIR)
        }
    }

    impl Drop for Root {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn only_known_files_of_real_run_folders_are_listed_and_opened() {
        let root = Root::new("list");
        let run = root.runs().join("12");
        std::fs::create_dir_all(&run).unwrap();
        std::fs::create_dir_all(root.runs().join("3")).unwrap();
        std::fs::write(run.join("report.md"), "# Ребёнок или взрослый\n").unwrap();
        std::fs::write(run.join("features.json"), "[]").unwrap();
        std::fs::write(root.0.join("secret.json"), "секрет").unwrap();
        std::os::unix::fs::symlink(
            root.0.join("secret.json"),
            run.join("child_adult_redimnet2.json"),
        )
        .unwrap();
        std::os::unix::fs::symlink(&run, root.runs().join("13")).unwrap();
        std::fs::create_dir_all(root.runs().join("old")).unwrap();

        assert_eq!(list_runs(&root.0), [12, 3]);
        let names: Vec<String> = list_files(&root.0, "12")
            .unwrap()
            .into_iter()
            .map(|f| f.name)
            .collect();
        assert_eq!(names, ["report.md"]);
        assert!(list_files(&root.0, "13").is_none(), "ссылка на папку");
        assert!(list_files(&root.0, "..").is_none());

        let report = open_file(&root.0, "12", "report.md").unwrap();
        assert_eq!(report.download_name, "nixyon-training-12-report.md");
        assert!(open_file(&root.0, "12", "features.json").is_none());
        assert!(open_file(&root.0, "12", "child_adult_redimnet2.json").is_none());
        assert!(open_file(&root.0, "13", "report.md").is_none());
    }

    #[test]
    fn no_training_folder_means_no_runs() {
        let root = Root::new("empty");
        std::fs::remove_dir_all(root.runs()).unwrap();
        assert!(list_runs(&root.0).is_empty());
        assert!(list_files(&root.0, "1").is_none());
    }
}
