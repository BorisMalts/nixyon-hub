use std::fs::File;
use std::path::Path;

use serde::Serialize;

use crate::nofollow::{OpenError, open_dir, open_file_at};

/// Самый большой файл зеркала: языковая модель около 2,7 ГБ, запас на следующие.
pub const MAX_MODEL_BYTES: u64 = 16 * 1024 * 1024 * 1024;
/// Сколько файлов зеркала показывать в списке.
const MAX_LISTED: usize = 1000;

/// Файл зеркала: имя файла и есть его SHA-256.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct MirrorFile {
    pub sha256: String,
    pub size: u64,
}

/// Имя файла зеркала: ровно 64 строчных шестнадцатеричных знака.
pub fn is_sha256(name: &str) -> bool {
    name.len() == 64 && name.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Открывает файл зеркала, не переходя по ссылкам. None: нет такого файла или это не
/// обычный файл.
pub fn open(dir: &Path, sha256: &str) -> std::io::Result<Option<(File, u64)>> {
    if !is_sha256(sha256) {
        return Ok(None);
    }
    let dir = match open_dir(dir) {
        Ok(dir) => dir,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    match open_file_at(&dir, sha256, MAX_MODEL_BYTES) {
        Ok(opened) => Ok(Some(opened)),
        Err(OpenError::NotRegular | OpenError::TooLarge) => Ok(None),
        Err(OpenError::Io(e))
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::InvalidInput
            ) || e.raw_os_error() == Some(rustix::io::Errno::LOOP.raw_os_error()) =>
        {
            Ok(None)
        }
        Err(OpenError::Io(e)) => Err(e),
    }
}

/// Обычные файлы зеркала с именем-суммой, по имени. Временные файлы и ссылки не входят.
pub fn list(dir: &Path) -> std::io::Result<Vec<MirrorFile>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        // DirEntry::metadata не переходит по ссылкам.
        let meta = entry.metadata()?;
        if is_sha256(&name) && meta.file_type().is_file() {
            files.push(MirrorFile {
                sha256: name,
                size: meta.len(),
            });
        }
    }
    files.sort_by(|a, b| a.sha256.cmp(&b.sha256));
    files.truncate(MAX_LISTED);
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn sum(c: char) -> String {
        c.to_string().repeat(64)
    }

    fn mirror(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("nixyon-mirror-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn only_lowercase_hex_of_sha256_length_is_a_name() {
        assert!(is_sha256(&sum('a')));
        assert!(!is_sha256(&sum('A')));
        assert!(!is_sha256(&sum('g')));
        assert!(!is_sha256(&"a".repeat(63)));
        assert!(!is_sha256(&format!("../{}", "a".repeat(61))));
    }

    #[test]
    fn files_open_by_sum_and_links_temporaries_and_folders_do_not() {
        let dir = mirror("open");
        std::fs::write(dir.join(sum('a')), b"model").unwrap();
        std::fs::write(dir.join(format!(".{}.tmp", sum('b'))), b"half").unwrap();
        std::os::unix::fs::symlink(dir.join(sum('a')), dir.join(sum('c'))).unwrap();
        std::fs::create_dir(dir.join(sum('d'))).unwrap();

        let (mut file, len) = open(&dir, &sum('a')).unwrap().unwrap();
        let mut text = String::new();
        file.read_to_string(&mut text).unwrap();
        assert_eq!((text.as_str(), len), ("model", 5));
        for name in [
            sum('b'),
            sum('c'),
            sum('d'),
            sum('e'),
            "../etc/passwd".into(),
        ] {
            assert!(open(&dir, &name).unwrap().is_none(), "{name}");
        }
        assert_eq!(
            list(&dir).unwrap(),
            [MirrorFile {
                sha256: sum('a'),
                size: 5
            }]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_mirror_is_empty() {
        let dir = std::env::temp_dir().join("nixyon-mirror-missing-folder");
        assert!(list(&dir).unwrap().is_empty());
        assert!(open(&dir, &sum('a')).unwrap().is_none());
    }
}
