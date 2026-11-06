use std::io::Write;
use std::path::{Path, PathBuf};

use crate::protocol::JobStatus;

pub const KEEP_JOBS: usize = 500;
pub const KEEP_SECONDS: i64 = 30 * 24 * 60 * 60;

pub struct History {
    dir: PathBuf,
}

impl History {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn last_id(&self) -> u64 {
        self.ids().into_iter().max().unwrap_or(0)
    }

    // Журнал пишется раньше описания: хаб показывает задание по описанию и сразу
    // находит рядом готовый журнал.
    pub fn save(&self, status: &JobStatus, log: &[String]) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let mut text = log.join("\n");
        text.push('\n');
        write_atomic(
            &self.dir.join(format!("{}.log", status.id)),
            text.as_bytes(),
        )?;
        let json = serde_json::to_vec(status).map_err(std::io::Error::other)?;
        write_atomic(&self.dir.join(format!("{}.json", status.id)), &json)
    }

    pub fn prune(&self, now: i64) -> std::io::Result<()> {
        let mut jobs: Vec<(u64, i64)> = self
            .ids()
            .into_iter()
            .map(|id| (id, self.started_at(id).unwrap_or(0)))
            .collect();
        jobs.sort_unstable_by_key(|job| std::cmp::Reverse(job.0));
        for (index, (id, started_at)) in jobs.into_iter().enumerate() {
            if index >= KEEP_JOBS || started_at < now - KEEP_SECONDS {
                for ext in ["json", "log"] {
                    match std::fs::remove_file(self.dir.join(format!("{id}.{ext}"))) {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => return Err(e),
                    }
                }
            }
        }
        Ok(())
    }

    fn ids(&self) -> Vec<u64> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        entries
            .filter_map(Result::ok)
            .filter_map(|e| id_of(&e.path()))
            .collect()
    }

    fn started_at(&self, id: u64) -> Option<i64> {
        let bytes = std::fs::read(self.dir.join(format!("{id}.json"))).ok()?;
        serde_json::from_slice::<JobStatus>(&bytes)
            .ok()
            .map(|s| s.started_at)
    }
}

fn id_of(path: &Path) -> Option<u64> {
    if path.extension()? != "json" {
        return None;
    }
    path.file_stem()?.to_str()?.parse().ok()
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let part = path.with_extension("part");
    let mut file = std::fs::File::create(&part)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    std::fs::rename(&part, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{JobKind, JobState};

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("nixyon-history-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn job(id: u64, started_at: i64) -> JobStatus {
        JobStatus {
            id,
            kind: JobKind::Build,
            state: JobState::Succeeded,
            started_by: "a@x.org".into(),
            started_at,
            finished_at: Some(started_at + 5),
            artifact: None,
        }
    }

    #[test]
    fn saved_job_keeps_its_log_and_raises_last_id() {
        let history = History::new(temp("save"));
        assert_eq!(history.last_id(), 0);
        history
            .save(&job(7, 100), &["первая".into(), "вторая".into()])
            .unwrap();
        assert_eq!(history.last_id(), 7);
        assert_eq!(
            std::fs::read_to_string(history.dir.join("7.log")).unwrap(),
            "первая\nвторая\n"
        );
    }

    #[test]
    fn prune_drops_old_jobs_and_everything_beyond_the_limit() {
        let history = History::new(temp("prune"));
        let now = 10 * KEEP_SECONDS;
        history.save(&job(1, now - KEEP_SECONDS - 1), &[]).unwrap();
        for id in 2..=(KEEP_JOBS as u64 + 2) {
            history.save(&job(id, now - 10), &[]).unwrap();
        }
        history.prune(now).unwrap();
        let mut ids = history.ids();
        ids.sort_unstable();
        assert_eq!(ids.len(), KEEP_JOBS);
        assert_eq!(ids[0], 3);
        assert!(!history.dir.join("1.log").exists());
        assert!(!history.dir.join("2.log").exists());
    }
}
