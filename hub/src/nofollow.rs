use std::fs::File;
use std::os::fd::OwnedFd;
use std::path::Path;

use rustix::fs::{FileType, Mode, OFlags, fstat, open, openat};

// Открытие по шагам от уже открытой папки, ни на одном шаге не переходя по ссылке.
// Нужно для томов, куда пишет непроверенный код: проверка «это папка», а затем
// открытие по пути оставляли бы миг, когда папку можно подменить ссылкой.

const DIR: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);
const FILE: OFlags = OFlags::RDONLY
    .union(OFlags::NOFOLLOW)
    .union(OFlags::NONBLOCK)
    .union(OFlags::CLOEXEC);

pub fn open_dir(path: &Path) -> std::io::Result<OwnedFd> {
    Ok(open(path, DIR, Mode::empty())?)
}

pub fn open_dir_at(dir: &OwnedFd, name: &str) -> std::io::Result<OwnedFd> {
    Ok(openat(dir, name, DIR, Mode::empty())?)
}

pub enum OpenError {
    NotRegular,
    TooLarge,
    Io(std::io::Error),
}

impl From<rustix::io::Errno> for OpenError {
    fn from(e: rustix::io::Errno) -> Self {
        Self::Io(e.into())
    }
}

pub fn open_file_at(dir: &OwnedFd, name: &str, max: u64) -> Result<(File, u64), OpenError> {
    let fd = openat(dir, name, FILE, Mode::empty())?;
    let stat = fstat(&fd)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
        return Err(OpenError::NotRegular);
    }
    let len = u64::try_from(stat.st_size).unwrap_or(0);
    if len > max {
        return Err(OpenError::TooLarge);
    }
    Ok((File::from(fd), len))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_are_refused_at_every_step() {
        let root = std::env::temp_dir().join(format!("nixyon-nofollow-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("real/inner")).unwrap();
        std::fs::write(root.join("real/inner/a.json"), "{}").unwrap();
        std::fs::write(root.join("secret"), "секрет").unwrap();
        std::os::unix::fs::symlink(root.join("real"), root.join("link")).unwrap();
        std::os::unix::fs::symlink(root.join("real/inner"), root.join("real/inner-link")).unwrap();
        std::os::unix::fs::symlink(root.join("secret"), root.join("real/inner/b.json")).unwrap();

        assert!(open_dir(&root.join("link")).is_err());
        let real = open_dir(&root.join("real")).unwrap();
        assert!(open_dir_at(&real, "inner-link").is_err());
        let inner = open_dir_at(&real, "inner").unwrap();
        assert!(open_file_at(&inner, "a.json", 10).is_ok());
        assert!(open_file_at(&inner, "b.json", 10).is_err());
        assert!(matches!(
            open_file_at(&inner, "a.json", 1),
            Err(OpenError::TooLarge)
        ));
        assert!(matches!(
            open_file_at(&real, "inner", 10),
            Err(OpenError::NotRegular)
        ));
    }
}
