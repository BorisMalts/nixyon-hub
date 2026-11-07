use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::{broadcast, mpsc};

use super::compress::gzip_beside;
use super::heavy_lock::HeavyLock;
use super::history::History;
use super::job_plan::JobPlan;
use super::retention::{KEEP_APKS, prune_apks};
use super::runner::{StepOutcome, run_step};
use super::workflow::Workflow;
use crate::protocol::{Artifact, JobKind, JobState, JobStatus};

const MAX_LOG_LINES: usize = 5000;
const FOLLOW_CAPACITY: usize = 2048;
const HEAVY_WAIT: std::time::Duration = std::time::Duration::from_secs(2 * 60 * 60);
pub const MAX_QUEUE: usize = 4;

type Finished = ((JobStatus, Vec<String>), Option<Next>);
// Следующее задание из очереди: номер, вид и план.
type Next = (u64, JobKind, JobPlan);

#[derive(Debug, Clone)]
pub enum JobEvent {
    Line(String),
    Finished(JobStatus),
}

#[derive(Debug, thiserror::Error)]
#[error("такое задание уже ждёт в очереди")]
pub struct Busy;

pub struct Follow {
    pub status: Option<JobStatus>,
    pub snapshot: Vec<String>,
    pub events: Option<broadcast::Receiver<JobEvent>>,
}

struct Running {
    status: JobStatus,
    log: Vec<String>,
    events: broadcast::Sender<JobEvent>,
}

impl Running {
    fn new(mut status: JobStatus) -> Self {
        status.state = JobState::Running;
        status.started_at = now();
        let (events, _) = broadcast::channel(FOLLOW_CAPACITY);
        Self {
            status,
            log: Vec::new(),
            events,
        }
    }
}

#[derive(Default)]
struct Inner {
    next_id: u64,
    current: Option<Running>,
    queue: VecDeque<(JobStatus, JobPlan)>,
    last: Option<(JobStatus, Vec<String>)>,
}

pub struct Jobs {
    inner: Mutex<Inner>,
    workflow: Arc<dyn Workflow>,
    artifacts_dir: PathBuf,
    history: History,
    heavy_lock: Option<HeavyLock>,
}

impl Jobs {
    pub fn new(workflow: Arc<dyn Workflow>, artifacts_dir: PathBuf) -> Arc<Self> {
        Self::with_lock(workflow, artifacts_dir, None)
    }

    pub fn with_lock(
        workflow: Arc<dyn Workflow>,
        artifacts_dir: PathBuf,
        heavy_lock: Option<HeavyLock>,
    ) -> Arc<Self> {
        let history = History::new(artifacts_dir.join("jobs"));
        let inner = Inner {
            next_id: history.last_id(),
            ..Inner::default()
        };
        Arc::new(Self {
            inner: Mutex::new(inner),
            workflow,
            artifacts_dir,
            history,
            heavy_lock,
        })
    }

    pub fn start(
        self: &Arc<Self>,
        kind: JobKind,
        user: &str,
        plan: JobPlan,
    ) -> Result<JobStatus, Busy> {
        let mut inner = self.inner.lock().map_err(|_| Busy)?;
        let idle = inner.current.is_none();
        if !idle
            && (inner.queue.len() >= MAX_QUEUE || inner.queue.iter().any(|(q, _)| q.kind == kind))
        {
            return Err(Busy);
        }
        inner.next_id += 1;
        let status = JobStatus {
            id: inner.next_id,
            kind,
            state: JobState::Queued,
            started_by: user.to_string(),
            started_at: now(),
            finished_at: None,
            artifact: None,
        };
        if !idle {
            inner.queue.push_back((status.clone(), plan));
            return Ok(status);
        }
        let running = Running::new(status);
        let started = running.status.clone();
        inner.current = Some(running);
        drop(inner);
        let jobs = Arc::clone(self);
        let id = started.id;
        tokio::spawn(async move { jobs.drive((id, kind, plan)).await });
        Ok(started)
    }

    pub fn status(&self) -> Option<JobStatus> {
        let inner = self.inner.lock().ok()?;
        inner
            .current
            .as_ref()
            .map(|r| r.status.clone())
            .or_else(|| inner.last.as_ref().map(|(s, _)| s.clone()))
    }

    pub fn queue(&self) -> Vec<JobStatus> {
        self.inner
            .lock()
            .map(|inner| inner.queue.iter().map(|(q, _)| q.clone()).collect())
            .unwrap_or_default()
    }

    pub fn follow(&self) -> Follow {
        let Ok(inner) = self.inner.lock() else {
            return Follow {
                status: None,
                snapshot: Vec::new(),
                events: None,
            };
        };
        match (&inner.current, &inner.last) {
            (Some(running), _) => Follow {
                status: Some(running.status.clone()),
                snapshot: running.log.clone(),
                events: Some(running.events.subscribe()),
            },
            (None, Some((status, log))) => Follow {
                status: Some(status.clone()),
                snapshot: log.clone(),
                events: None,
            },
            (None, None) => Follow {
                status: None,
                snapshot: Vec::new(),
                events: None,
            },
        }
    }

    async fn drive(self: Arc<Self>, first: Next) {
        let mut job = first;
        while let Some(next) = Arc::clone(&self).run(job).await {
            job = next;
        }
    }

    async fn run(self: Arc<Self>, (id, kind, plan): Next) -> Option<Next> {
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let collector = {
            let jobs = Arc::clone(&self);
            tokio::spawn(async move {
                while let Some(line) = rx.recv().await {
                    jobs.push_line(line);
                }
            })
        };
        let mut ok = true;
        if let Err(e) = self.workflow.prepare(kind, &plan, id) {
            let _ = tx.send(format!("задание не начато: файлы задания не записаны: {e}"));
            ok = false;
        }
        let steps = if ok {
            self.workflow.steps(kind, &plan, id)
        } else {
            Vec::new()
        };
        for step in steps {
            let _held = match (&self.heavy_lock, step.heavy) {
                (Some(lock), true) => {
                    match lock.acquire(heavy_label(kind), HEAVY_WAIT, &tx).await {
                        Ok(held) => Some(held),
                        Err(e) => {
                            let _ = tx.send(format!("задание не начато: {e}"));
                            ok = false;
                            break;
                        }
                    }
                }
                _ => None,
            };
            let _ = tx.send(format!("$ {} {}", step.program, step.args.join(" ")));
            match run_step(&step, &tx).await {
                StepOutcome::Succeeded => {}
                StepOutcome::Failed(code) => {
                    let _ = tx.send(format!("команда завершилась с кодом {code:?}"));
                    ok = false;
                }
                StepOutcome::TimedOut => {
                    let _ = tx.send("команда прервана по времени".into());
                    ok = false;
                }
                StepOutcome::CouldNotStart(e) => {
                    let _ = tx.send(format!("команда не запустилась: {e}"));
                    ok = false;
                }
            }
            if !ok {
                break;
            }
        }
        let artifact = if ok && kind == JobKind::Build {
            match self.publish_artifact().await {
                Ok(artifact) => Some(artifact),
                Err(e) => {
                    let _ = tx.send(format!("не удалось сохранить APK: {e}"));
                    ok = false;
                    None
                }
            }
        } else {
            None
        };
        drop(tx);
        let _ = collector.await;
        let (done, next) = self.finish(ok, artifact)?;
        {
            let (status, log) = done;
            let jobs = Arc::clone(&self);
            let saved = tokio::task::spawn_blocking(move || {
                jobs.history.save(&status, &log)?;
                jobs.history.prune(now())
            })
            .await;
            if !matches!(saved, Ok(Ok(()))) {
                tracing::warn!("журнал задания не сохранён");
            }
        }
        next
    }

    fn push_line(&self, line: String) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        if let Some(running) = inner.current.as_mut() {
            if running.log.len() >= MAX_LOG_LINES {
                running.log.remove(0);
            }
            running.log.push(line.clone());
            let _ = running.events.send(JobEvent::Line(line));
        }
    }

    // Следующее задание из очереди становится текущим под той же блокировкой, что и
    // завершение: иначе новый запуск успел бы увидеть агент свободным и обойти очередь.
    fn finish(&self, ok: bool, artifact: Option<Artifact>) -> Option<Finished> {
        let mut inner = self.inner.lock().ok()?;
        let mut running = inner.current.take()?;
        running.status.state = if ok {
            JobState::Succeeded
        } else {
            JobState::Failed
        };
        running.status.finished_at = Some(now());
        running.status.artifact = artifact;
        let _ = running
            .events
            .send(JobEvent::Finished(running.status.clone()));
        let done = (running.status, running.log);
        inner.last = Some(done.clone());
        let next = inner.queue.pop_front().map(|(queued, plan)| {
            let (id, kind) = (queued.id, queued.kind);
            inner.current = Some(Running::new(queued));
            (id, kind, plan)
        });
        Some((done, next))
    }

    async fn publish_artifact(&self) -> Result<Artifact, Box<dyn std::error::Error + Send + Sync>> {
        let repo = self.workflow.repo_dir().to_path_buf();
        let out = tokio::process::Command::new("git")
            .args(["log", "-1", "--format=%h%x1f%s"])
            .current_dir(&repo)
            .output()
            .await?;
        let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let (commit, subject) = text.split_once('\u{1f}').unwrap_or(("unknown", ""));
        let commit: String = commit
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .take(12)
            .collect::<String>()
            .to_lowercase();
        let commit = if commit.is_empty() {
            "unknown".to_owned()
        } else {
            commit
        };
        let created_at = now();
        let id = format!("{}-{commit}", stamp(created_at));
        let subject: String = subject.chars().take(200).collect();
        // Лёгкая версия без моделей собирается первой и в списке стоит под полной.
        if let Some(lite) = self.workflow.built_lite_apk().filter(|p| p.is_file()) {
            let built = Built {
                id: format!("{id}-lite"),
                commit: commit.clone(),
                subject: format!("{LITE_PREFIX}{subject}"),
                created_at: created_at - 1,
            };
            self.publish_apk(&lite, built).await?;
        }
        let built = Built {
            id,
            commit,
            subject,
            created_at,
        };
        let artifact = self.publish_apk(&self.workflow.built_apk(), built).await?;
        let dir = self.artifacts_dir.clone();
        if !matches!(
            tokio::task::spawn_blocking(move || prune_apks(&dir, KEEP_APKS)).await,
            Ok(Ok(_))
        ) {
            tracing::warn!("старые APK не удалены");
        }
        Ok(artifact)
    }

    async fn publish_apk(
        &self,
        apk: &Path,
        built: Built,
    ) -> Result<Artifact, Box<dyn std::error::Error + Send + Sync>> {
        let file = format!("nixyon-{}.apk", built.id);
        tokio::fs::create_dir_all(&self.artifacts_dir).await?;
        let target = self.artifacts_dir.join(&file);
        let size = tokio::fs::copy(apk, &target).await?;
        tokio::spawn(async move {
            if let Err(e) = gzip_beside(&target).await {
                tracing::warn!(error = %e, "сжатая копия APK не сделана");
            }
        });
        let artifact = Artifact {
            id: built.id,
            file,
            commit: built.commit,
            subject: built.subject,
            created_at: built.created_at,
            size,
            built_by: self.status().map(|s| s.started_by).unwrap_or_default(),
        };
        let meta = self.artifacts_dir.join(format!("{}.json", artifact.id));
        tokio::fs::write(&meta, serde_json::to_vec_pretty(&artifact)?).await?;
        Ok(artifact)
    }
}

/// Сборка, которую надо сохранить в артефакты: всё, кроме файла и размера.
struct Built {
    id: String,
    commit: String,
    subject: String,
    created_at: i64,
}

/// Подпись лёгкой версии в списке сборок.
const LITE_PREFIX: &str = "Без моделей: ";

fn heavy_label(kind: JobKind) -> &'static str {
    match kind {
        JobKind::Quality => "проверка качества в хабе",
        JobKind::LiveEval => "прогон живых записей в хабе",
        JobKind::Training => "обучение в хабе",
        JobKind::Build | JobKind::Pull => "сборка APK в хабе",
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

fn stamp(unix: i64) -> String {
    let t =
        time::OffsetDateTime::from_unix_timestamp(unix).unwrap_or(time::OffsetDateTime::UNIX_EPOCH);
    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        t.year(),
        u8::from(t.month()),
        t.day(),
        t.hour(),
        t.minute(),
        t.second()
    )
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::Duration;

    use super::*;
    use crate::server::workflow::Step;

    struct Fake {
        dir: PathBuf,
        script: &'static str,
    }

    impl Workflow for Fake {
        fn steps(&self, _kind: JobKind, _plan: &JobPlan, _job: u64) -> Vec<Step> {
            vec![Step {
                program: "sh".into(),
                args: vec!["-c".into(), self.script.into()],
                dir: self.dir.clone(),
                timeout: Duration::from_secs(10),
                heavy: true,
                ..Step::default()
            }]
        }

        fn built_apk(&self) -> PathBuf {
            self.dir.join("app.apk")
        }

        fn built_lite_apk(&self) -> Option<PathBuf> {
            Some(self.dir.join("lite.apk"))
        }

        fn repo_dir(&self) -> &Path {
            &self.dir
        }
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nixyon-jobs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    async fn wait_finished(jobs: &Jobs) -> JobStatus {
        for _ in 0..200 {
            if let Some(s) = jobs.status().filter(|s| s.state != JobState::Running) {
                return s;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("задание не закончилось");
    }

    #[tokio::test]
    async fn jobs_wait_in_queue_run_in_order_and_keep_their_logs() {
        let dir = temp("queue");
        let jobs = Jobs::new(
            Arc::new(Fake {
                dir: dir.clone(),
                script: "sleep 0.3; echo шаг",
            }),
            dir.join("out"),
        );
        let first = jobs
            .start(JobKind::Pull, "a@x.org", JobPlan::default())
            .unwrap();
        assert_eq!(first.state, JobState::Running);
        let second = jobs
            .start(JobKind::Pull, "b@x.org", JobPlan::default())
            .unwrap();
        assert_eq!(second.state, JobState::Queued);
        assert!(
            jobs.start(JobKind::Pull, "c@x.org", JobPlan::default())
                .is_err()
        );
        assert_eq!(jobs.queue(), std::slice::from_ref(&second));
        for _ in 0..200 {
            if jobs
                .status()
                .is_some_and(|s| s.id == second.id && s.state == JobState::Succeeded)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(jobs.queue().is_empty());
        let saved = |id: u64| -> JobStatus {
            serde_json::from_slice(&std::fs::read(dir.join(format!("out/jobs/{id}.json"))).unwrap())
                .unwrap()
        };
        for _ in 0..100 {
            if dir.join(format!("out/jobs/{}.json", second.id)).exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(saved(first.id).started_by, "a@x.org");
        assert_eq!(saved(second.id).started_by, "b@x.org");
        assert!(saved(second.id).started_at >= saved(first.id).finished_at.unwrap());
        assert!(
            std::fs::read_to_string(dir.join(format!("out/jobs/{}.log", second.id)))
                .unwrap()
                .contains("шаг")
        );
        let restarted = Jobs::new(
            Arc::new(Fake {
                dir: dir.clone(),
                script: "true",
            }),
            dir.join("out"),
        );
        assert_eq!(
            restarted
                .start(JobKind::Pull, "a@x.org", JobPlan::default())
                .unwrap()
                .id,
            second.id + 1
        );
    }

    struct Unprepared(PathBuf);

    impl Workflow for Unprepared {
        fn steps(&self, _kind: JobKind, _plan: &JobPlan, _job: u64) -> Vec<Step> {
            vec![Step {
                program: "sh".into(),
                args: vec!["-c".into(), "echo шаг пошёл".into()],
                dir: self.0.clone(),
                timeout: Duration::from_secs(10),
                ..Step::default()
            }]
        }

        fn prepare(&self, _kind: JobKind, _plan: &JobPlan, _job: u64) -> std::io::Result<()> {
            Err(std::io::Error::other("нет места"))
        }

        fn built_apk(&self) -> PathBuf {
            self.0.join("app.apk")
        }

        fn repo_dir(&self) -> &Path {
            &self.0
        }
    }

    #[tokio::test]
    async fn a_job_whose_files_cannot_be_written_fails_without_running_steps() {
        let dir = temp("unprepared");
        let jobs = Jobs::new(Arc::new(Unprepared(dir.clone())), dir.join("out"));
        jobs.start(JobKind::Quality, "a@x.org", JobPlan::default())
            .unwrap();
        assert_eq!(wait_finished(&jobs).await.state, JobState::Failed);
        let log = jobs.follow().snapshot;
        assert!(log.iter().any(|l| l.contains("нет места")), "{log:?}");
        assert!(!log.iter().any(|l| l.contains("шаг пошёл")), "{log:?}");
    }

    #[tokio::test]
    async fn heavy_step_waits_for_the_shared_lock() {
        let dir = temp("heavy");
        let lock_dir = dir.join("heavy.d");
        std::fs::create_dir(&lock_dir).unwrap();
        std::fs::write(lock_dir.join("owner"), "тесты GitHub").unwrap();
        let jobs = Jobs::with_lock(
            Arc::new(Fake {
                dir: dir.clone(),
                script: "echo собрано",
            }),
            dir.join("out"),
            Some(HeavyLock::new(lock_dir.clone())),
        );
        jobs.start(JobKind::Pull, "a@x.org", JobPlan::default())
            .unwrap();
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert_eq!(jobs.status().unwrap().state, JobState::Running);
        assert!(
            jobs.follow()
                .snapshot
                .iter()
                .any(|l| l == "жду свободные ядра: тесты GitHub")
        );
        std::fs::remove_file(lock_dir.join("owner")).unwrap();
        std::fs::remove_dir(&lock_dir).unwrap();
        assert_eq!(wait_finished(&jobs).await.state, JobState::Succeeded);
        assert!(!lock_dir.exists());
    }

    #[tokio::test]
    async fn follower_gets_log_and_finish() {
        let dir = temp("follow");
        let jobs = Jobs::new(
            Arc::new(Fake {
                dir: dir.clone(),
                script: "sleep 0.2; echo готово; exit 2",
            }),
            dir.join("out"),
        );
        jobs.start(JobKind::Pull, "a@x.org", JobPlan::default())
            .unwrap();
        let follow = jobs.follow();
        let mut events = follow.events.unwrap();
        let mut lines = follow.snapshot;
        let finished = loop {
            match events.recv().await.unwrap() {
                JobEvent::Line(l) => lines.push(l),
                JobEvent::Finished(s) => break s,
            }
        };
        assert_eq!(finished.state, JobState::Failed);
        assert!(lines.iter().any(|l| l == "готово"));
        assert!(lines.iter().any(|l| l.contains("кодом Some(2)")));
        let after = jobs.follow();
        assert!(after.events.is_none());
        assert_eq!(after.status.unwrap().state, JobState::Failed);
    }

    #[tokio::test]
    async fn successful_build_publishes_apk_with_description() {
        let dir = temp("publish");
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(&dir)
            .status()
            .unwrap();
        std::process::Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@x",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "Проверка",
            ])
            .current_dir(&dir)
            .status()
            .unwrap();
        let jobs = Jobs::new(
            Arc::new(Fake {
                dir: dir.clone(),
                script: "printf apk > app.apk",
            }),
            dir.join("out"),
        );
        jobs.start(JobKind::Build, "a@x.org", JobPlan::default())
            .unwrap();
        let done = wait_finished(&jobs).await;
        assert_eq!(done.state, JobState::Succeeded);
        let artifact = done.artifact.unwrap();
        assert!(Artifact::is_valid_id(&artifact.id));
        assert_eq!(artifact.subject, "Проверка");
        assert_eq!(artifact.built_by, "a@x.org");
        assert_eq!(
            std::fs::read(dir.join("out").join(&artifact.file)).unwrap(),
            b"apk"
        );
        let meta: Artifact = serde_json::from_slice(
            &std::fs::read(dir.join("out").join(format!("{}.json", artifact.id))).unwrap(),
        )
        .unwrap();
        assert_eq!(meta, artifact);
    }

    #[tokio::test]
    async fn a_build_with_a_light_apk_saves_both_and_the_full_one_is_the_result() {
        let dir = temp("lite");
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(&dir)
            .status()
            .unwrap();
        std::process::Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@x",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "Две версии",
            ])
            .current_dir(&dir)
            .status()
            .unwrap();
        let jobs = Jobs::new(
            Arc::new(Fake {
                dir: dir.clone(),
                script: "printf full > app.apk && printf lite > lite.apk",
            }),
            dir.join("out"),
        );
        jobs.start(JobKind::Build, "a@x.org", JobPlan::default())
            .unwrap();
        let done = wait_finished(&jobs).await;
        assert_eq!(done.state, JobState::Succeeded);
        let full = done.artifact.unwrap();
        assert_eq!(full.subject, "Две версии");
        let lite: Artifact = serde_json::from_slice(
            &std::fs::read(dir.join("out").join(format!("{}-lite.json", full.id))).unwrap(),
        )
        .unwrap();
        assert!(Artifact::is_valid_id(&lite.id));
        assert_eq!(lite.subject, format!("{LITE_PREFIX}Две версии"));
        assert!(lite.created_at < full.created_at);
        assert_eq!(
            std::fs::read(dir.join("out").join(&full.file)).unwrap(),
            b"full"
        );
        assert_eq!(
            std::fs::read(dir.join("out").join(&lite.file)).unwrap(),
            b"lite"
        );
    }

    #[tokio::test]
    async fn build_without_apk_fails() {
        let dir = temp("noapk");
        let jobs = Jobs::new(
            Arc::new(Fake {
                dir: dir.clone(),
                script: "true",
            }),
            dir.join("out"),
        );
        jobs.start(JobKind::Build, "a@x.org", JobPlan::default())
            .unwrap();
        assert_eq!(wait_finished(&jobs).await.state, JobState::Failed);
    }

    #[test]
    fn stamp_is_utc_and_sortable() {
        assert_eq!(stamp(0), "19700101-000000");
        assert_eq!(stamp(1_793_189_045), "20261028-120405");
    }
}
