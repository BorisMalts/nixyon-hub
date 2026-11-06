use std::path::{Path, PathBuf};

/// Папка опытов внутри папки артефактов, по папке на задание с его номером.
pub const EXPERIMENTS_DIR: &str = "experiments";
/// Сколько последних опытов хранится.
pub const KEEP_EXPERIMENTS: usize = 30;

/// Готовит пустую папку опыта `<корень>/<номер>`: старые опыты сверх предела удаляются
/// так, чтобы вместе с новым осталось `keep`. Номера заданий растут, поэтому старые
/// опыты те, у кого номер меньше.
pub fn fresh_experiment_dir(root: &Path, job: u64, keep: usize) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(root)?;
    prune_experiments(root, keep.saturating_sub(1))?;
    let dir = root.join(job.to_string());
    // Папка с тем же номером могла остаться от прежней истории заданий.
    match std::fs::remove_dir_all(&dir) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    std::fs::create_dir(&dir)?;
    Ok(dir)
}

/// Удаляет папки опытов, кроме `keep` с наибольшими номерами. Чужие имена и ссылки не
/// трогает: удаляются только настоящие папки с номером вместо имени.
pub fn prune_experiments(root: &Path, keep: usize) -> std::io::Result<usize> {
    let mut found: Vec<(u64, PathBuf)> = std::fs::read_dir(root)?
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let is_number = !name.is_empty() && name.bytes().all(|b| b.is_ascii_digit());
            let id = name.parse::<u64>().ok().filter(|_| is_number)?;
            Some((id, e.path()))
        })
        .collect();
    found.sort_by_key(|f| std::cmp::Reverse(f.0));
    let mut removed = 0;
    for (_, old) in found.iter().skip(keep) {
        std::fs::remove_dir_all(old)?;
        removed += 1;
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Root(PathBuf);

    impl Root {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("nixyon-experiments-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Root {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn only_the_newest_numbered_folders_stay() {
        let root = Root::new("prune");
        for id in [1, 2, 10, 3] {
            std::fs::create_dir_all(root.0.join(id.to_string()).join("A")).unwrap();
        }
        std::fs::create_dir_all(root.0.join("notes")).unwrap();
        std::fs::write(root.0.join("4"), b"file").unwrap();
        let outside = root.0.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, root.0.join("5")).unwrap();
        assert_eq!(prune_experiments(&root.0, 2).unwrap(), 2);
        let mut left: Vec<String> = std::fs::read_dir(&root.0)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(left, ["10", "3", "4", "5", "notes", "outside"]);
    }

    #[test]
    fn fresh_folder_is_empty_and_keeps_the_limit_together_with_the_new_one() {
        let root = Root::new("fresh");
        let base = root.0.join("experiments");
        for id in 1..=3 {
            std::fs::create_dir_all(base.join(id.to_string())).unwrap();
        }
        std::fs::create_dir_all(base.join("7")).unwrap();
        std::fs::write(base.join("7/old.json"), b"{}").unwrap();
        let dir = fresh_experiment_dir(&base, 7, 3).unwrap();
        assert_eq!(dir, base.join("7"));
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        let mut left: Vec<String> = std::fs::read_dir(&base)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(left, ["3", "7"]);
    }
}
