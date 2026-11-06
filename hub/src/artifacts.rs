use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use nixyon_agent::protocol::Artifact;

const MAX_LISTED: usize = 200;
const MAX_DESCRIPTION_BYTES: u64 = 64 * 1024;
// Сборка с вшитыми моделями весит несколько гигабайт.
pub const MAX_APK_BYTES: u64 = 8 * 1024 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error("не обычный файл")]
    NotRegular,
    #[error("файл больше предела")]
    TooLarge,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub fn open_regular(path: &Path, max: u64) -> Result<(std::fs::File, u64), ReadError> {
    let link = std::fs::symlink_metadata(path)?;
    if !link.file_type().is_file() {
        return Err(ReadError::NotRegular);
    }
    let file = std::fs::File::open(path)?;
    let opened = file.metadata()?;
    if !opened.is_file() || opened.ino() != link.ino() || opened.dev() != link.dev() {
        return Err(ReadError::NotRegular);
    }
    if opened.len() > max {
        return Err(ReadError::TooLarge);
    }
    Ok((file, opened.len()))
}

pub fn read_regular(path: &Path, max: u64) -> Result<Vec<u8>, ReadError> {
    let (file, len) = open_regular(path, max)?;
    let mut bytes = Vec::with_capacity(usize::try_from(len).unwrap_or(0));
    file.take(max + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        return Err(ReadError::TooLarge);
    }
    Ok(bytes)
}

pub fn list(dir: &Path) -> Vec<Artifact> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut items: Vec<Artifact> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter_map(|p| read_regular(&p, MAX_DESCRIPTION_BYTES).ok())
        .filter_map(|bytes| serde_json::from_slice::<Artifact>(&bytes).ok())
        .filter(|a| Artifact::is_valid_id(&a.id) && a.file == format!("nixyon-{}.apk", a.id))
        .collect();
    items.sort_by(|a, b| {
        b.created_at
            .cmp(&a.created_at)
            .then_with(|| b.id.cmp(&a.id))
    });
    items.truncate(MAX_LISTED);
    items
}

pub struct OpenedApk {
    pub artifact: Artifact,
    pub file: std::fs::File,
    pub len: u64,
    pub gzipped: bool,
}

// Сжатую копию агент кладёт рядом после сборки. Пока её нет, отдаётся сам APK.
pub fn open_apk(dir: &Path, id: &str, gzip_ok: bool) -> Option<OpenedApk> {
    if !Artifact::is_valid_id(id) {
        return None;
    }
    let artifact = list(dir).into_iter().find(|a| a.id == id)?;
    let gz = dir.join(format!("{}.gz", artifact.file));
    if gzip_ok && let Ok((file, len)) = open_regular(&gz, MAX_APK_BYTES) {
        return Some(OpenedApk {
            artifact,
            file,
            len,
            gzipped: true,
        });
    }
    let (file, len) = open_regular(&dir.join(&artifact.file), MAX_APK_BYTES).ok()?;
    Some(OpenedApk {
        artifact,
        file,
        len,
        gzipped: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn write(dir: &Path, id: &str, created_at: i64, file: Option<&str>) {
        let artifact = Artifact {
            id: id.into(),
            file: file.map_or_else(|| format!("nixyon-{id}.apk"), str::to_string),
            commit: "abc".into(),
            subject: "s".into(),
            created_at,
            size: 3,
            built_by: "a@x.org".into(),
        };
        std::fs::write(
            dir.join(format!("{id}.json")),
            serde_json::to_vec(&artifact).unwrap(),
        )
        .unwrap();
        if !artifact.file.contains("..") {
            std::fs::write(dir.join(&artifact.file), b"apk").unwrap();
        }
    }

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("nixyon-artifacts-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn newest_first_and_broken_descriptions_skipped() {
        let dir = temp("list");
        write(&dir, "b-1", 10, None);
        write(&dir, "a-2", 20, None);
        std::fs::write(dir.join("broken.json"), b"{").unwrap();
        let ids: Vec<String> = list(&dir).into_iter().map(|a| a.id).collect();
        assert_eq!(ids, ["a-2", "b-1"]);
    }

    #[test]
    fn description_pointing_outside_is_ignored() {
        let dir = temp("escape");
        write(&dir, "x-1", 10, Some("../../etc/passwd"));
        assert!(list(&dir).is_empty());
        assert!(open_apk(&dir, "x-1", true).is_none());
    }

    #[test]
    fn apk_is_found_only_by_valid_listed_id() {
        let dir = temp("find");
        write(&dir, "c-3", 10, None);
        assert_eq!(open_apk(&dir, "c-3", true).unwrap().len, 3);
        assert!(open_apk(&dir, "../c-3", true).is_none());
        assert!(open_apk(&dir, "d-4", true).is_none());
    }

    #[test]
    fn symlinked_apk_or_description_is_never_read() {
        let dir = temp("symlink");
        let secret = dir.join("hub.db");
        std::fs::write(&secret, b"secret").unwrap();
        write(&dir, "e-5", 10, None);
        std::fs::remove_file(dir.join("nixyon-e-5.apk")).unwrap();
        std::os::unix::fs::symlink(&secret, dir.join("nixyon-e-5.apk")).unwrap();
        assert!(open_apk(&dir, "e-5", true).is_none());

        let real = temp("symlink-outside").join("real.json");
        std::fs::rename(dir.join("e-5.json"), &real).unwrap();
        std::os::unix::fs::symlink(&real, dir.join("e-5.json")).unwrap();
        assert!(list(&dir).iter().all(|a| a.id != "e-5"));
    }

    #[test]
    fn compressed_copy_is_used_only_when_the_client_accepts_it() {
        let dir = temp("gzip");
        write(&dir, "g-6", 10, None);
        std::fs::write(dir.join("nixyon-g-6.apk.gz"), b"gz-bytes").unwrap();
        let packed = open_apk(&dir, "g-6", true).unwrap();
        assert!(packed.gzipped);
        assert_eq!(packed.len, 8);
        let plain = open_apk(&dir, "g-6", false).unwrap();
        assert!(!plain.gzipped);
        assert_eq!(plain.len, 3);
        std::fs::remove_file(dir.join("nixyon-g-6.apk.gz")).unwrap();
        std::os::unix::fs::symlink(dir.join("g-6.json"), dir.join("nixyon-g-6.apk.gz")).unwrap();
        assert!(!open_apk(&dir, "g-6", true).unwrap().gzipped);
    }

    #[test]
    fn multi_gigabyte_apk_is_served_but_above_the_limit_is_refused() {
        let dir = temp("huge");
        write(&dir, "h-7", 10, None);
        // Разреженные файлы: на диске места не занимают.
        let apk = dir.join("nixyon-h-7.apk");
        std::fs::File::create(&apk)
            .unwrap()
            .set_len(4 * 1024 * 1024 * 1024)
            .unwrap();
        assert_eq!(
            open_apk(&dir, "h-7", false).unwrap().len,
            4 * 1024 * 1024 * 1024
        );
        std::fs::File::create(&apk)
            .unwrap()
            .set_len(MAX_APK_BYTES + 1)
            .unwrap();
        assert!(open_apk(&dir, "h-7", false).is_none());
    }

    #[test]
    fn oversized_file_is_refused() {
        let dir = temp("size");
        std::fs::write(dir.join("big"), vec![0u8; 100]).unwrap();
        assert!(matches!(
            read_regular(&dir.join("big"), 10),
            Err(ReadError::TooLarge)
        ));
        assert_eq!(read_regular(&dir.join("big"), 100).unwrap().len(), 100);
    }
}
