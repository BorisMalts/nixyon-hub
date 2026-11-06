use std::fs::File;
use std::io::Read;
use std::os::fd::OwnedFd;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::nofollow::{open_dir, open_dir_at, open_file_at};

// Отчёты пишет код из репозитория во время теста, то есть непроверенный код: каждое
// поле здесь ограничено по длине и набору символов, а выводит их интерфейс только как текст.

const MAX_RUNS: usize = 200;
// Папок может оказаться сколько угодно: просмотр и чтение ограничены до разбора, а не
// после него.
const MAX_SCANNED: usize = 5000;
pub const CACHE_FOR: Duration = Duration::from_secs(5);
const MAX_STEPS: u32 = 100;
const MAX_JSON_BYTES: u64 = 16 * 1024;
const LOST_AFTER_SECONDS: i64 = 3 * 60 * 60;

#[derive(Deserialize)]
struct RunFile {
    run_id: String,
    attempt: u32,
    sha: String,
    branch: String,
    actor: String,
    subject: String,
    url: String,
    started_at: i64,
}

#[derive(Deserialize)]
struct StepFile {
    n: u32,
    key: String,
    title: String,
    started_at: i64,
    finished_at: Option<i64>,
    exit_code: Option<i32>,
}

#[derive(Deserialize)]
struct FinishFile {
    conclusion: String,
    finished_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    Success,
    Failure,
    Cancelled,
    Lost,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Running,
    Success,
    Failure,
    Lost,
}

#[derive(Debug, Clone, Serialize)]
pub struct CiStep {
    pub n: u32,
    pub key: String,
    pub title: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub exit_code: Option<i32>,
    pub status: StepStatus,
}

#[derive(Debug, Clone, Serialize)]
pub struct CiRun {
    pub id: String,
    pub run_id: String,
    pub attempt: u32,
    pub sha: String,
    pub branch: String,
    pub actor: String,
    pub subject: String,
    pub url: Option<String>,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub status: RunStatus,
    pub failed_step: Option<String>,
    pub steps: Vec<CiStep>,
}

pub fn is_valid_run_id(id: &str) -> bool {
    let Some((run, attempt)) = id.split_once('-') else {
        return false;
    };
    let digits = |s: &str, max: usize| {
        !s.is_empty() && s.len() <= max && s.bytes().all(|b| b.is_ascii_digit())
    };
    digits(run, 20) && digits(attempt, 4)
}

fn run_order(id: &str) -> Option<(u64, u32)> {
    let (run, attempt) = id.split_once('-')?;
    Some((run.parse().ok()?, attempt.parse().ok()?))
}

pub fn list(dir: &Path, now: i64) -> Vec<CiRun> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<((u64, u32), String)> = entries
        .take(MAX_SCANNED)
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| is_valid_run_id(name))
        .filter_map(|name| Some((run_order(&name)?, name)))
        .collect();
    names.sort_unstable_by_key(|n| std::cmp::Reverse(n.0));
    names.truncate(MAX_RUNS);
    let mut runs: Vec<CiRun> = names
        .into_iter()
        .filter_map(|(_, name)| read_run(dir, &name, now))
        .collect();
    runs.sort_unstable_by(|a, b| {
        b.started_at
            .cmp(&a.started_at)
            .then_with(|| b.id.cmp(&a.id))
    });
    runs.truncate(MAX_RUNS);
    runs
}

pub fn read_run(dir: &Path, id: &str, now: i64) -> Option<CiRun> {
    if !is_valid_run_id(id) {
        return None;
    }
    let root = open_dir(&dir.join(id)).ok()?;
    let run: RunFile = read_json(&root, "run.json")?;
    let finish: Option<FinishFile> = read_json(&root, "finish.json");
    let lost = finish.is_none() && now - run.started_at > LOST_AFTER_SECONDS;
    let steps: Vec<CiStep> = match open_dir_at(&root, "steps") {
        Ok(steps) => (1..=MAX_STEPS)
            .map_while(|n| read_json::<StepFile>(&steps, &format!("{n}.json")))
            .filter_map(|s| step(s, lost))
            .collect(),
        Err(_) => Vec::new(),
    };
    let status = match &finish {
        Some(f) => match f.conclusion.as_str() {
            "success" => RunStatus::Success,
            "cancelled" => RunStatus::Cancelled,
            _ => RunStatus::Failure,
        },
        None if lost => RunStatus::Lost,
        None => RunStatus::Running,
    };
    let failed_step = steps
        .iter()
        .find(|s| s.status == StepStatus::Failure)
        .map(|s| s.title.clone());
    Some(CiRun {
        id: id.to_string(),
        run_id: digits_only(&run.run_id, 20),
        attempt: run.attempt,
        sha: hex_only(&run.sha, 40),
        branch: text(&run.branch, 200),
        actor: text(&run.actor, 100),
        subject: text(&run.subject, 300),
        url: github_url(&run.url),
        started_at: run.started_at,
        finished_at: finish.map(|f| f.finished_at),
        status,
        failed_step,
        steps,
    })
}

pub fn open_step_log(dir: &Path, id: &str, n: u32) -> Option<(File, u64)> {
    if !is_valid_run_id(id) || !(1..=MAX_STEPS).contains(&n) {
        return None;
    }
    let root = open_dir(&dir.join(id)).ok()?;
    let steps = open_dir_at(&root, "steps").ok()?;
    open_file_at(&steps, &format!("{n}.log"), u64::MAX).ok()
}

// Лента и статистика читают одни и те же отчёты: короткий общий кэш не даёт частым
// запросам каждый раз перечитывать сотни файлов.
#[derive(Default)]
pub struct RunsCache {
    inner: Mutex<Option<(Instant, Arc<Vec<CiRun>>)>>,
}

impl RunsCache {
    pub fn get(&self, dir: &Path, now: i64) -> Arc<Vec<CiRun>> {
        if let Ok(guard) = self.inner.lock()
            && let Some((at, runs)) = guard.as_ref()
            && at.elapsed() < CACHE_FOR
        {
            return Arc::clone(runs);
        }
        let runs = Arc::new(list(dir, now));
        if let Ok(mut guard) = self.inner.lock() {
            *guard = Some((Instant::now(), Arc::clone(&runs)));
        }
        runs
    }
}

fn step(file: StepFile, lost: bool) -> Option<CiStep> {
    let status = match (file.finished_at, file.exit_code) {
        (Some(_), Some(0)) => StepStatus::Success,
        (Some(_), _) => StepStatus::Failure,
        (None, _) if lost => StepStatus::Lost,
        (None, _) => StepStatus::Running,
    };
    Some(CiStep {
        n: file.n,
        key: file
            .key
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
            .take(40)
            .collect(),
        title: text(&file.title, 120),
        started_at: file.started_at,
        finished_at: file.finished_at,
        exit_code: file.exit_code,
        status,
    })
}

fn read_json<T: for<'de> Deserialize<'de>>(dir: &OwnedFd, name: &str) -> Option<T> {
    let (file, _) = open_file_at(dir, name, MAX_JSON_BYTES).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_JSON_BYTES).read_to_end(&mut bytes).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn text(s: &str, max: usize) -> String {
    s.chars().filter(|c| !c.is_control()).take(max).collect()
}

fn digits_only(s: &str, max: usize) -> String {
    s.chars().filter(char::is_ascii_digit).take(max).collect()
}

fn hex_only(s: &str, max: usize) -> String {
    s.chars()
        .filter(char::is_ascii_hexdigit)
        .take(max)
        .collect::<String>()
        .to_lowercase()
}

fn github_url(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://github.com/")?;
    let safe = rest.len() <= 300
        && rest
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/-_.".contains(&b));
    safe.then(|| url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nixyon-ci-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, value: serde_json::Value) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
    }

    fn run(dir: &Path, id: &str, started_at: i64, url: &str) {
        write(
            &dir.join(id).join("run.json"),
            json!({
                "run_id": id.split('-').next().unwrap(), "attempt": 1,
                "sha": "ABCdef0123<script>", "branch": "main", "actor": "andre",
                "subject": "строка\u{1b}[31m", "url": url, "started_at": started_at
            }),
        );
    }

    fn step_file(dir: &Path, id: &str, n: u32, finished_at: Option<i64>, exit_code: Option<i32>) {
        write(
            &dir.join(id).join(format!("steps/{n}.json")),
            json!({
                "n": n, "key": "clippy core!", "title": format!("Шаг {n}"),
                "started_at": 100, "finished_at": finished_at, "exit_code": exit_code
            }),
        );
    }

    #[test]
    fn run_status_comes_from_finish_and_first_failed_step_is_named() {
        let dir = temp("status");
        run(
            &dir,
            "11-1",
            100,
            "https://github.com/Andre-wb/Nixyon/actions/runs/11",
        );
        step_file(&dir, "11-1", 1, Some(110), Some(0));
        step_file(&dir, "11-1", 2, Some(120), Some(101));
        write(
            &dir.join("11-1/finish.json"),
            json!({"conclusion": "failure", "finished_at": 130}),
        );
        let r = read_run(&dir, "11-1", 200).unwrap();
        assert_eq!(r.status, RunStatus::Failure);
        assert_eq!(r.failed_step.as_deref(), Some("Шаг 2"));
        assert_eq!(r.steps.len(), 2);
        assert_eq!(r.steps[0].key, "clippycore");
        assert_eq!(r.sha, "abcdef0123c");
        assert_eq!(r.subject, "строка[31m");
        assert_eq!(r.finished_at, Some(130));
    }

    #[test]
    fn unfinished_run_is_running_and_after_three_hours_lost() {
        let dir = temp("lost");
        run(&dir, "12-1", 100, "https://github.com/x/y");
        step_file(&dir, "12-1", 1, None, None);
        assert_eq!(
            read_run(&dir, "12-1", 200).unwrap().status,
            RunStatus::Running
        );
        let lost = read_run(&dir, "12-1", 100 + LOST_AFTER_SECONDS + 1).unwrap();
        assert_eq!(lost.status, RunStatus::Lost);
        assert_eq!(lost.steps[0].status, StepStatus::Lost);
    }

    #[test]
    fn only_the_newest_runs_are_read_and_linked_folders_are_skipped() {
        let dir = temp("bounded");
        for n in 1..=(MAX_RUNS as u64 + 50) {
            run(&dir, &format!("{n}-1"), 100, "https://github.com/x/y");
        }
        let outside = dir.with_file_name("ci-outside");
        let _ = std::fs::remove_dir_all(&outside);
        run(&outside, "999999-1", 100, "https://github.com/x/y");
        std::os::unix::fs::symlink(outside.join("999999-1"), dir.join("999999-1")).unwrap();
        let runs = list(&dir, 200);
        assert_eq!(runs.len(), MAX_RUNS);
        assert!(runs.iter().all(|r| r.id != "999999-1"));
        assert!(runs.iter().any(|r| r.id == format!("{}-1", MAX_RUNS + 50)));
        assert!(runs.iter().all(|r| r.id != "50-1"));
        assert!(read_run(&dir, "999999-1", 200).is_none());
        assert!(open_step_log(&dir, "999999-1", 1).is_none());
    }

    #[test]
    fn foreign_links_and_bad_ids_are_not_trusted() {
        let dir = temp("trust");
        run(&dir, "13-1", 100, "javascript:alert(1)");
        run(&dir, "14-1", 200, "https://github.com/a/b?x=<y>");
        std::fs::create_dir_all(dir.join("../escape")).unwrap();
        assert!(read_run(&dir, "13-1", 300).unwrap().url.is_none());
        assert!(read_run(&dir, "14-1", 300).unwrap().url.is_none());
        assert!(read_run(&dir, "../escape", 300).is_none());
        assert!(open_step_log(&dir, "13-1", 0).is_none());
        assert!(open_step_log(&dir, "13-1/..", 1).is_none());
        let ids: Vec<String> = list(&dir, 300).into_iter().map(|r| r.id).collect();
        assert_eq!(ids, ["14-1", "13-1"]);
    }
}
