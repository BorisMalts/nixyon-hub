use std::path::Path;

use crate::protocol::Artifact;

pub const KEEP_APKS: usize = 20;

// Каждый APK весит 50–150 МБ: храним только последние, вместе с описанием и сжатой
// копией. Скачивание, которое уже идёт, не обрывается: открытый файл остаётся доступен.
pub fn prune_apks(dir: &Path, keep: usize) -> std::io::Result<usize> {
    let mut artifacts: Vec<Artifact> = std::fs::read_dir(dir)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter(|p| std::fs::symlink_metadata(p).is_ok_and(|m| m.is_file()))
        .filter_map(|p| serde_json::from_slice::<Artifact>(&std::fs::read(p).ok()?).ok())
        .filter(|a| Artifact::is_valid_id(&a.id) && a.file == format!("nixyon-{}.apk", a.id))
        .collect();
    artifacts.sort_by(|a, b| {
        b.created_at
            .cmp(&a.created_at)
            .then_with(|| b.id.cmp(&a.id))
    });
    let mut removed = 0;
    for old in artifacts.iter().skip(keep) {
        for name in [
            format!("{}.json", old.id),
            old.file.clone(),
            format!("{}.gz", old.file),
            format!("{}.gz.part", old.file),
        ] {
            match std::fs::remove_file(dir.join(name)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        removed += 1;
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artifact(dir: &Path, n: i64) -> Artifact {
        let id = format!("20261028-{:06}-abc{n}", 100_000 + n);
        let artifact = Artifact {
            id: id.clone(),
            file: format!("nixyon-{id}.apk"),
            commit: "abc".into(),
            subject: "s".into(),
            created_at: n,
            size: 3,
            built_by: "a@x.org".into(),
        };
        std::fs::write(
            dir.join(format!("{id}.json")),
            serde_json::to_vec(&artifact).unwrap(),
        )
        .unwrap();
        std::fs::write(dir.join(&artifact.file), b"apk").unwrap();
        std::fs::write(dir.join(format!("{}.gz", artifact.file)), b"gz").unwrap();
        artifact
    }

    #[test]
    fn only_the_newest_apks_stay_with_their_descriptions_and_copies() {
        let dir = std::env::temp_dir().join(format!("nixyon-retention-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("jobs")).unwrap();
        let all: Vec<Artifact> = (1..=5).map(|n| artifact(&dir, n)).collect();
        std::fs::write(dir.join("jobs/1.json"), b"{}").unwrap();
        assert_eq!(prune_apks(&dir, 3).unwrap(), 2);
        for old in &all[..2] {
            assert!(!dir.join(&old.file).exists());
            assert!(!dir.join(format!("{}.json", old.id)).exists());
            assert!(!dir.join(format!("{}.gz", old.file)).exists());
        }
        for kept in &all[2..] {
            assert!(dir.join(&kept.file).exists());
        }
        assert!(dir.join("jobs/1.json").exists());
        assert_eq!(prune_apks(&dir, 3).unwrap(), 0);
    }
}
