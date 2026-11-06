use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use rustix::fs::{AtFlags, Mode, OFlags, fstat, mkdirat, open, openat, renameat, statat, unlinkat};
use rustix::io::Errno;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

// Общая блокировка ядер между сборкой APK и тестами GitHub Actions. Они живут в разных
// песочницах gVisor, а flock действует только внутри своей песочницы. Создание папки
// идёт через файловую систему хоста и атомарно для обеих сторон. Держатель обновляет
// время файла owner; если оно не менялось дольше STALE_AFTER, держатель считается
// упавшим и блокировка снимается. Тот же порядок повторяет скрипт nixyon-ci.
//
// Папку с блокировкой может менять и раннер, а он исполняет непроверенный код. Поэтому
// агент работает только через открытые дескрипторы: родительская папка (точка
// монтирования тома) открыта один раз, внутрь блокировки агент заходит без перехода по
// ссылкам, а свою папку удаляет, лишь убедившись, что под этим именем всё ещё она.

const BEAT_EVERY: Duration = Duration::from_secs(5);
const STALE_AFTER: Duration = Duration::from_secs(60);
const POLL_EVERY: Duration = Duration::from_secs(1);
const OWNER_MAX_CHARS: usize = 120;
const OWNER_MAX_BYTES: u64 = 512;
const DIR: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);
const READ: OFlags = OFlags::RDONLY
    .union(OFlags::NOFOLLOW)
    .union(OFlags::NONBLOCK)
    .union(OFlags::CLOEXEC);

pub struct HeavyLock {
    dir: PathBuf,
}

pub struct Held {
    parent: OwnedFd,
    name: OsString,
    own: OwnedFd,
    beat: JoinHandle<()>,
}

#[derive(Debug, thiserror::Error)]
pub enum LockError {
    #[error("не дождались свободных ядер")]
    TimedOut,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl From<Errno> for LockError {
    fn from(e: Errno) -> Self {
        Self::Io(e.into())
    }
}

impl HeavyLock {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub async fn acquire(
        &self,
        who: &str,
        wait_at_most: Duration,
        log: &mpsc::UnboundedSender<String>,
    ) -> Result<Held, LockError> {
        let (Some(parent_path), Some(name)) = (self.dir.parent(), self.dir.file_name()) else {
            return Err(std::io::Error::other("у блокировки нет родительской папки").into());
        };
        let parent = open(parent_path, DIR, Mode::empty())?;
        let started = tokio::time::Instant::now();
        let mut told = false;
        loop {
            match mkdirat(&parent, name, Mode::from_raw_mode(0o755)) {
                Ok(()) => {
                    let own = openat(&parent, name, DIR, Mode::empty())?;
                    let fd = openat(
                        &own,
                        "owner",
                        OFlags::WRONLY
                            | OFlags::CREATE
                            | OFlags::EXCL
                            | OFlags::NOFOLLOW
                            | OFlags::CLOEXEC,
                        Mode::from_raw_mode(0o644),
                    )?;
                    let mut owner = File::from(fd);
                    owner.write_all(who.as_bytes())?;
                    let beat = tokio::spawn(async move {
                        let mut tick = tokio::time::interval(BEAT_EVERY);
                        loop {
                            tick.tick().await;
                            let _ = owner.set_modified(SystemTime::now());
                        }
                    });
                    return Ok(Held {
                        parent,
                        name: name.to_owned(),
                        own,
                        beat,
                    });
                }
                Err(Errno::EXIST) => {}
                Err(e) => return Err(e.into()),
            }
            if is_stale(&parent, name) {
                let _ = log.send("предыдущая блокировка ядер брошена, снимаю её".into());
                break_stale(&parent, parent_path, name);
                continue;
            }
            if !told {
                let _ = log.send(format!("жду свободные ядра: {}", owner(&parent, name)));
                told = true;
            }
            if started.elapsed() >= wait_at_most {
                return Err(LockError::TimedOut);
            }
            tokio::time::sleep(POLL_EVERY).await;
        }
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        self.beat.abort();
        let _ = unlinkat(&self.own, "owner", AtFlags::empty());
        let (Ok(mine), Ok(now)) = (
            fstat(&self.own),
            statat(&self.parent, &self.name, AtFlags::SYMLINK_NOFOLLOW),
        ) else {
            return;
        };
        if mine.st_dev == now.st_dev && mine.st_ino == now.st_ino {
            let _ = unlinkat(&self.parent, &self.name, AtFlags::REMOVEDIR);
        }
    }
}

fn age(file: File) -> Option<Duration> {
    let modified = file.metadata().ok()?.modified().ok()?;
    SystemTime::now().duration_since(modified).ok()
}

// Не папка под этим именем (например, подложенная ссылка) ничего не держит и снимается
// сразу. У папки без owner (короткий миг между созданием и записью) считаем по времени
// самой папки.
fn is_stale(parent: &OwnedFd, name: &OsStr) -> bool {
    let Ok(own) = openat(parent, name, DIR, Mode::empty()) else {
        return statat(parent, name, AtFlags::SYMLINK_NOFOLLOW).is_ok();
    };
    let owner_age = openat(&own, "owner", READ, Mode::empty())
        .ok()
        .map(File::from)
        .and_then(age);
    owner_age
        .or_else(|| age(File::from(own)))
        .is_some_and(|a| a > STALE_AFTER)
}

// Брошенную блокировку сначала переименовывает один из ждущих: переименование атомарно,
// и чужую свежую блокировку второй ждущий уже не тронет, проверив её заново.
// remove_dir_all из std не переходит по ссылкам внутри удаляемой папки.
fn break_stale(parent: &OwnedFd, parent_path: &Path, name: &OsStr) {
    if !is_stale(parent, name) {
        return;
    }
    let mut moved = name.to_owned();
    moved.push(format!(".stale-{}", std::process::id()));
    if renameat(parent, name, parent, &moved).is_err() {
        return;
    }
    if openat(parent, &moved, DIR, Mode::empty()).is_ok() {
        let _ = std::fs::remove_dir_all(parent_path.join(&moved));
    } else {
        let _ = unlinkat(parent, &moved, AtFlags::empty());
    }
}

fn owner(parent: &OwnedFd, name: &OsStr) -> String {
    let mut text = String::new();
    let file = openat(parent, name, DIR, Mode::empty())
        .and_then(|own| openat(&own, "owner", READ, Mode::empty()))
        .map(File::from);
    if let Ok(file) = file
        && file.metadata().is_ok_and(|m| m.is_file())
    {
        let _ = file.take(OWNER_MAX_BYTES).read_to_string(&mut text);
    }
    let clean: String = text
        .chars()
        .filter(|c| !c.is_control())
        .take(OWNER_MAX_CHARS)
        .collect();
    if clean.is_empty() {
        "другое задание".into()
    } else {
        clean
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nixyon-lock-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("heavy.d")
    }

    #[tokio::test]
    async fn second_holder_waits_until_the_first_lets_go() {
        let lock = HeavyLock::new(temp("wait"));
        let (tx, mut rx) = mpsc::unbounded_channel();
        let first = lock
            .acquire("сборка APK", Duration::from_secs(5), &tx)
            .await
            .unwrap();
        let waiting = tokio::spawn({
            let dir = lock.dir.clone();
            let tx = tx.clone();
            async move {
                HeavyLock::new(dir)
                    .acquire("тесты", Duration::from_secs(5), &tx)
                    .await
                    .map(|_| ())
            }
        });
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(!waiting.is_finished());
        drop(first);
        waiting.await.unwrap().unwrap();
        assert_eq!(rx.recv().await.unwrap(), "жду свободные ядра: сборка APK");
        assert!(!lock.dir.exists());
    }

    #[tokio::test]
    async fn busy_lock_gives_up_after_the_wait_limit() {
        let lock = HeavyLock::new(temp("limit"));
        let (tx, _rx) = mpsc::unbounded_channel();
        let _held = lock
            .acquire("a", Duration::from_secs(1), &tx)
            .await
            .unwrap();
        assert!(matches!(
            lock.acquire("b", Duration::ZERO, &tx).await,
            Err(LockError::TimedOut)
        ));
    }

    #[tokio::test]
    async fn planted_links_do_not_redirect_heartbeat_or_cleanup() {
        let dir = temp("links");
        let victim_dir = dir.with_file_name("victim");
        std::fs::create_dir_all(&victim_dir).unwrap();
        let victim = victim_dir.join("owner");
        std::fs::write(&victim, "код сборки").unwrap();
        let old = SystemTime::now() - Duration::from_secs(3600);
        std::fs::File::options()
            .append(true)
            .open(&victim)
            .unwrap()
            .set_modified(old)
            .unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let held = HeavyLock::new(dir.clone())
            .acquire("сборка APK", Duration::from_secs(5), &tx)
            .await
            .unwrap();
        std::fs::remove_file(dir.join("owner")).unwrap();
        std::os::unix::fs::symlink(&victim, dir.join("owner")).unwrap();
        tokio::time::sleep(BEAT_EVERY + Duration::from_millis(500)).await;
        assert_eq!(std::fs::metadata(&victim).unwrap().modified().unwrap(), old);
        std::fs::remove_file(dir.join("owner")).unwrap();
        std::fs::remove_dir(&dir).unwrap();
        std::os::unix::fs::symlink(&victim_dir, &dir).unwrap();
        drop(held);
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "код сборки");
        let parent = open(dir.parent().unwrap(), DIR, Mode::empty()).unwrap();
        assert!(owner(&parent, dir.file_name().unwrap()).contains("другое задание"));
    }

    #[tokio::test]
    async fn link_planted_instead_of_the_lock_is_removed_without_touching_its_target() {
        let dir = temp("planted");
        let victim = dir.with_file_name("victim");
        std::fs::create_dir_all(&victim).unwrap();
        std::fs::write(victim.join("owner"), "код сборки").unwrap();
        std::os::unix::fs::symlink(&victim, &dir).unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let held = HeavyLock::new(dir.clone())
            .acquire("сборка APK", Duration::from_secs(5), &tx)
            .await
            .unwrap();
        assert!(rx.recv().await.unwrap().contains("брошена"));
        assert!(std::fs::symlink_metadata(&dir).unwrap().is_dir());
        assert_eq!(
            std::fs::read_to_string(victim.join("owner")).unwrap(),
            "код сборки"
        );
        drop(held);
        assert!(!dir.exists());
    }

    #[tokio::test]
    async fn abandoned_lock_is_taken_over() {
        let dir = temp("stale");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("owner"), "упавшая сборка").unwrap();
        let old = SystemTime::now() - STALE_AFTER - Duration::from_secs(5);
        std::fs::File::options()
            .append(true)
            .open(dir.join("owner"))
            .unwrap()
            .set_modified(old)
            .unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let _held = HeavyLock::new(dir.clone())
            .acquire("новая", Duration::from_secs(5), &tx)
            .await
            .unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("owner")).unwrap(), "новая");
        assert!(rx.recv().await.unwrap().contains("брошена"));
    }
}
