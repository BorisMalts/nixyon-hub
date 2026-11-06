use std::path::Path;

use nixyon_agent::protocol::JobStatus;

use crate::artifacts::read_regular;

const MAX_LISTED: usize = 500;
const MAX_STATUS_BYTES: u64 = 64 * 1024;

pub fn dir(artifacts_dir: &Path) -> std::path::PathBuf {
    artifacts_dir.join("jobs")
}

pub fn list(dir: &Path) -> Vec<JobStatus> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut jobs: Vec<JobStatus> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let path = e.path();
            if path.extension()? != "json" {
                return None;
            }
            let id: u64 = path.file_stem()?.to_str()?.parse().ok()?;
            let bytes = read_regular(&path, MAX_STATUS_BYTES).ok()?;
            let job: JobStatus = serde_json::from_slice(&bytes).ok()?;
            (job.id == id).then_some(job)
        })
        .collect();
    jobs.sort_unstable_by_key(|j| std::cmp::Reverse(j.id));
    jobs.truncate(MAX_LISTED);
    jobs
}

pub fn log_path(dir: &Path, id: u64) -> std::path::PathBuf {
    dir.join(format!("{id}.log"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nixyon_agent::protocol::{JobKind, JobState};

    fn job(id: u64) -> JobStatus {
        JobStatus {
            id,
            kind: JobKind::Build,
            state: JobState::Succeeded,
            started_by: "a@x.org".into(),
            started_at: 10,
            finished_at: Some(70),
            artifact: None,
        }
    }

    #[test]
    fn newest_first_and_files_that_lie_about_their_id_are_skipped() {
        let dir = std::env::temp_dir().join(format!("nixyon-jobhist-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for id in [3, 12, 7] {
            std::fs::write(
                dir.join(format!("{id}.json")),
                serde_json::to_vec(&job(id)).unwrap(),
            )
            .unwrap();
        }
        std::fs::write(dir.join("99.json"), serde_json::to_vec(&job(1)).unwrap()).unwrap();
        std::fs::write(dir.join("x.json"), b"{").unwrap();
        let ids: Vec<u64> = list(&dir).into_iter().map(|j| j.id).collect();
        assert_eq!(ids, [12, 7, 3]);
    }
}
