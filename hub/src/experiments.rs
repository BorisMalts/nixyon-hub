use std::os::fd::OwnedFd;
use std::path::Path;

use serde::Serialize;

use crate::nofollow::{open_dir, open_dir_at, open_file_at};
use crate::quality_report::OpenedReport;

/// Папка опытов в папке артефактов, по папке на задание; её пишет агент.
pub const EXPERIMENTS_DIR: &str = "experiments";
/// Сочетания моделей, по папке дампов на каждое.
pub const COMBOS: [&str; 4] = ["A", "B", "C", "D"];
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_EXPERIMENTS: usize = 100;
const MAX_DUMPS: usize = 64;

// Файлы опыта пишет код из репозитория Nixyon, поэтому отдаются только эти имена и
// дампы с именами записей, только на скачивание.
const FILES: [(&str, &str); 2] = [
    ("runs.jsonl", "application/x-ndjson"),
    ("combos.json", "application/json"),
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExperimentFile {
    pub path: String,
    pub size: u64,
}

/// Номер задания как имя папки: только цифры.
fn is_valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 20 && id.bytes().all(|b| b.is_ascii_digit())
}

/// Дамп записи: `<запись>.json`, имя записи как у агента.
fn is_valid_dump(name: &str) -> bool {
    name.strip_suffix(".json").is_some_and(|record| {
        record.len() <= 128
            && record
                .bytes()
                .next()
                .is_some_and(|b| b.is_ascii_alphanumeric())
            && record
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
    })
}

fn root(artifacts_dir: &Path) -> Option<OwnedFd> {
    open_dir(&artifacts_dir.join(EXPERIMENTS_DIR)).ok()
}

fn experiment(artifacts_dir: &Path, id: &str) -> Option<OwnedFd> {
    if !is_valid_id(id) {
        return None;
    }
    open_dir_at(&root(artifacts_dir)?, id).ok()
}

fn names(dir: &OwnedFd) -> Vec<String> {
    let Ok(entries) = rustix::fs::Dir::read_from(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().to_str().ok().map(str::to_owned))
        .collect()
}

/// Номера опытов, новые первыми.
pub fn list_experiments(artifacts_dir: &Path) -> Vec<u64> {
    let Some(dir) = root(artifacts_dir) else {
        return Vec::new();
    };
    let mut ids: Vec<u64> = names(&dir)
        .iter()
        .filter(|n| is_valid_id(n))
        .filter(|n| open_dir_at(&dir, n).is_ok())
        .filter_map(|n| n.parse().ok())
        .collect();
    ids.sort_unstable_by(|a, b| b.cmp(a));
    ids.truncate(MAX_EXPERIMENTS);
    ids
}

/// Файлы опыта: итоги прогонов, флаги сочетаний и дампы по сочетаниям. None: такого
/// опыта нет.
pub fn list_files(artifacts_dir: &Path, id: &str) -> Option<Vec<ExperimentFile>> {
    let dir = experiment(artifacts_dir, id)?;
    let mut files: Vec<ExperimentFile> = FILES
        .iter()
        .filter_map(|&(name, _)| {
            let (_, size) = open_file_at(&dir, name, MAX_FILE_BYTES).ok()?;
            Some(ExperimentFile {
                path: name.to_owned(),
                size,
            })
        })
        .collect();
    for combo in COMBOS {
        let Ok(combo_dir) = open_dir_at(&dir, combo) else {
            continue;
        };
        let mut dumps: Vec<String> = names(&combo_dir)
            .into_iter()
            .filter(|n| is_valid_dump(n))
            .collect();
        dumps.sort();
        dumps.truncate(MAX_DUMPS);
        files.extend(dumps.into_iter().filter_map(|name| {
            let (_, size) = open_file_at(&combo_dir, &name, MAX_FILE_BYTES).ok()?;
            Some(ExperimentFile {
                path: format!("{combo}/{name}"),
                size,
            })
        }));
    }
    Some(files)
}

/// Итоги или флаги сочетаний опыта.
pub fn open_file(artifacts_dir: &Path, id: &str, name: &str) -> Option<OpenedReport> {
    let &(name, content_type) = FILES.iter().find(|(known, _)| *known == name)?;
    let dir = experiment(artifacts_dir, id)?;
    let (file, len) = open_file_at(&dir, name, MAX_FILE_BYTES).ok()?;
    Some(OpenedReport {
        file,
        len,
        content_type,
        download_name: format!("nixyon-experiment-{id}-{name}"),
    })
}

/// Дамп расшифровки записи в одном сочетании.
pub fn open_dump(artifacts_dir: &Path, id: &str, combo: &str, name: &str) -> Option<OpenedReport> {
    if !COMBOS.contains(&combo) || !is_valid_dump(name) {
        return None;
    }
    let dir = open_dir_at(&experiment(artifacts_dir, id)?, combo).ok()?;
    let (file, len) = open_file_at(&dir, name, MAX_FILE_BYTES).ok()?;
    Some(OpenedReport {
        file,
        len,
        content_type: "application/json",
        download_name: format!("nixyon-experiment-{id}-{combo}-{name}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Root(std::path::PathBuf);

    impl Root {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("nixyon-hub-exp-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(path.join(EXPERIMENTS_DIR)).unwrap();
            Self(path)
        }

        fn exp(&self) -> std::path::PathBuf {
            self.0.join(EXPERIMENTS_DIR)
        }
    }

    impl Drop for Root {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn ids_and_dump_names_cannot_escape() {
        assert!(is_valid_id("42"));
        for bad in ["", "..", "4a", "-1", &"9".repeat(21)] {
            assert!(!is_valid_id(bad), "{bad:?}");
        }
        assert!(is_valid_dump("55342.json"));
        assert!(is_valid_dump("priruchenie-ognja_-x.json"));
        for bad in [
            ".json", "..json", "a.txt", "a/b.json", ".x.json", "a b.json",
        ] {
            assert!(!is_valid_dump(bad), "{bad:?}");
        }
    }

    #[test]
    fn listed_and_opened_files_are_only_known_names_without_links() {
        let root = Root::new("list");
        let exp = root.exp().join("7");
        std::fs::create_dir_all(exp.join("A")).unwrap();
        std::fs::create_dir_all(exp.join("E")).unwrap();
        std::fs::write(exp.join("runs.jsonl"), "{}\n").unwrap();
        std::fs::write(exp.join("notes.txt"), "x").unwrap();
        std::fs::write(exp.join("A/talk.json"), "{}").unwrap();
        std::fs::write(exp.join("A/.hidden.json"), "{}").unwrap();
        std::fs::write(exp.join("E/talk.json"), "{}").unwrap();
        std::fs::write(root.0.join("secret.json"), "секрет").unwrap();
        std::os::unix::fs::symlink(root.0.join("secret.json"), exp.join("A/link.json")).unwrap();
        std::os::unix::fs::symlink(&exp, root.exp().join("8")).unwrap();
        std::fs::create_dir_all(root.exp().join("old")).unwrap();

        assert_eq!(list_experiments(&root.0), [7]);
        let files = list_files(&root.0, "7").unwrap();
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["runs.jsonl", "A/talk.json"]);
        assert!(list_files(&root.0, "8").is_none(), "ссылка на папку опыта");
        assert!(list_files(&root.0, "..").is_none());

        assert!(open_file(&root.0, "7", "runs.jsonl").is_some());
        assert!(open_file(&root.0, "7", "notes.txt").is_none());
        let dump = open_dump(&root.0, "7", "A", "talk.json").unwrap();
        assert_eq!(dump.download_name, "nixyon-experiment-7-A-talk.json");
        assert!(open_dump(&root.0, "7", "A", "link.json").is_none());
        assert!(open_dump(&root.0, "7", "E", "talk.json").is_none());
        assert!(open_dump(&root.0, "7", "A", "..json").is_none());
        assert!(open_dump(&root.0, "8", "A", "talk.json").is_none());
    }
}
