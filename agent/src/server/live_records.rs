use std::path::Path;

/// Папка записей без разметки внутри корня наборов. Её заполняет владелец на сервере.
pub const LIVE_DIR: &str = "live";
/// Сколько записей можно прогнать за одно задание.
pub const MAX_RECORDS: usize = 16;

/// Имя записи: буквы, цифры, точка, дефис и подчёркивание, первым идёт буква или цифра,
/// поэтому ни «..», ни скрытый файл, ни путь именем не станут.
pub fn is_valid_record_name(name: &str) -> bool {
    name.len() <= 128
        && name
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphanumeric())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

/// Записи годятся: список не пустой и не больше предела, имена допустимы и не
/// повторяются, у каждой есть `<имя>.wav` обычным файлом в обычной папке, без ссылок.
pub fn check_records(records: &[String], dir: &Path) -> Result<(), String> {
    if records.is_empty() {
        return Err("не выбрана ни одна запись".into());
    }
    if records.len() > MAX_RECORDS {
        return Err(format!("записей больше {MAX_RECORDS}"));
    }
    if !std::fs::symlink_metadata(dir).is_ok_and(|m| m.is_dir()) {
        return Err(format!("нет папки записей {}", dir.display()));
    }
    for (i, name) in records.iter().enumerate() {
        if !is_valid_record_name(name) {
            return Err("недопустимое имя записи".into());
        }
        if records[..i].contains(name) {
            return Err(format!("запись {name} выбрана дважды"));
        }
        let wav = dir.join(format!("{name}.wav"));
        if !std::fs::symlink_metadata(&wav).is_ok_and(|m| m.is_file()) {
            return Err(format!("записи {name} нет на сервере"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(std::path::PathBuf);

    impl Dir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("nixyon-agent-live-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(path.join("live")).unwrap();
            Self(path)
        }

        fn live(&self) -> std::path::PathBuf {
            self.0.join("live")
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn names_cannot_become_paths_or_hidden_files() {
        for good in ["55342", "dialogue", "priruchenie-ognja_-stanislav.v2"] {
            assert!(is_valid_record_name(good), "{good}");
        }
        for bad in [
            "",
            ".",
            "..",
            ".hidden",
            "-flag",
            "_x",
            "a/b",
            "../live",
            "a\\b",
            "a b",
            "запись",
            "a\0b",
            &"a".repeat(129),
        ] {
            assert!(!is_valid_record_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn existing_plain_wav_files_pass() {
        let dir = Dir::new("ok");
        std::fs::write(dir.live().join("55342.wav"), b"RIFF").unwrap();
        std::fs::write(dir.live().join("dialogue.wav"), b"RIFF").unwrap();
        assert_eq!(
            check_records(&names(&["55342", "dialogue"]), &dir.live()),
            Ok(())
        );
    }

    #[test]
    fn empty_too_long_repeated_or_missing_records_are_refused() {
        let dir = Dir::new("bad");
        std::fs::write(dir.live().join("a.wav"), b"RIFF").unwrap();
        std::fs::write(dir.live().join("notes.txt"), b"x").unwrap();
        std::fs::create_dir_all(dir.live().join("folder.wav")).unwrap();
        let live = dir.live();
        assert!(check_records(&[], &live).is_err());
        let many: Vec<String> = (0..=MAX_RECORDS).map(|i| format!("r{i}")).collect();
        assert!(check_records(&many, &live).unwrap_err().contains("больше"));
        assert!(
            check_records(&names(&["a", "a"]), &live)
                .unwrap_err()
                .contains("дважды")
        );
        for missing in ["b", "notes", "folder"] {
            assert!(
                check_records(&names(&[missing]), &live)
                    .unwrap_err()
                    .contains("нет на сервере"),
                "{missing}"
            );
        }
        assert!(check_records(&names(&["../live/a"]), &live).is_err());
        assert!(check_records(&names(&["a"]), &dir.0.join("nope")).is_err());
    }

    #[test]
    fn links_to_files_or_to_the_folder_are_refused() {
        let dir = Dir::new("links");
        std::fs::write(dir.0.join("secret.wav"), b"RIFF").unwrap();
        std::os::unix::fs::symlink(dir.0.join("secret.wav"), dir.live().join("x.wav")).unwrap();
        assert!(check_records(&names(&["x"]), &dir.live()).is_err());
        std::fs::write(dir.live().join("a.wav"), b"RIFF").unwrap();
        std::os::unix::fs::symlink(dir.live(), dir.0.join("linked")).unwrap();
        assert!(check_records(&names(&["a"]), &dir.0.join("linked")).is_err());
    }
}
