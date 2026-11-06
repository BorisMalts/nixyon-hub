use std::collections::BTreeMap;

use nixyon_agent::protocol::{Artifact, JobKind, JobState, JobStatus};
use serde::Serialize;

use crate::ci_reports::{CiRun, RunStatus, StepStatus};

// Дни и медианы по дням считает браузер: так сутки режутся в поясе устройства.

#[derive(Debug, Serialize, PartialEq)]
pub struct Timing {
    pub started_at: i64,
    pub seconds: i64,
    pub ok: bool,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct StepTiming {
    pub key: String,
    pub title: String,
    pub runs: usize,
    pub median_seconds: i64,
    pub max_seconds: i64,
}

#[derive(Debug, Default, Serialize, PartialEq)]
pub struct Person {
    pub who: String,
    pub builds: usize,
    pub pulls: usize,
    pub test_runs: usize,
    pub quality_runs: usize,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct ApkSize {
    pub created_at: i64,
    pub size: u64,
}

#[derive(Debug, Serialize)]
pub struct Stats {
    pub builds: Vec<Timing>,
    pub test_runs: Vec<Timing>,
    pub slowest_steps: Vec<StepTiming>,
    pub people: Vec<Person>,
    pub apk_sizes: Vec<ApkSize>,
}

const SLOWEST_STEPS: usize = 8;

pub fn compute(jobs: &[JobStatus], runs: &[CiRun], artifacts: &[Artifact]) -> Stats {
    let builds = jobs
        .iter()
        .filter(|j| j.kind == JobKind::Build)
        .filter_map(|j| {
            Some(Timing {
                started_at: j.started_at,
                seconds: j.finished_at? - j.started_at,
                ok: j.state == JobState::Succeeded,
            })
        })
        .collect();
    let test_runs = runs
        .iter()
        .filter(|r| matches!(r.status, RunStatus::Success | RunStatus::Failure))
        .filter_map(|r| {
            Some(Timing {
                started_at: r.started_at,
                seconds: r.finished_at? - r.started_at,
                ok: r.status == RunStatus::Success,
            })
        })
        .collect();
    let mut people: BTreeMap<String, Person> = BTreeMap::new();
    for job in jobs {
        // Прогоны живых записей и обучение запускают опыты, а не сборки и проверки: в
        // счёт людей они не идут.
        if matches!(job.kind, JobKind::LiveEval | JobKind::Training) {
            continue;
        }
        let person = people.entry(job.started_by.clone()).or_default();
        match job.kind {
            JobKind::Build => person.builds += 1,
            JobKind::Pull => person.pulls += 1,
            JobKind::Quality => person.quality_runs += 1,
            JobKind::LiveEval | JobKind::Training => {}
        }
    }
    for run in runs {
        people.entry(run.actor.clone()).or_default().test_runs += 1;
    }
    let mut people: Vec<Person> = people
        .into_iter()
        .map(|(who, p)| Person { who, ..p })
        .collect();
    people.sort_by(|a, b| {
        (b.builds + b.pulls + b.test_runs + b.quality_runs)
            .cmp(&(a.builds + a.pulls + a.test_runs + a.quality_runs))
            .then_with(|| a.who.cmp(&b.who))
    });
    let mut apk_sizes: Vec<ApkSize> = artifacts
        .iter()
        .map(|a| ApkSize {
            created_at: a.created_at,
            size: a.size,
        })
        .collect();
    apk_sizes.sort_by_key(|a| a.created_at);
    Stats {
        builds,
        test_runs,
        slowest_steps: slowest_steps(runs),
        people,
        apk_sizes,
    }
}

fn slowest_steps(runs: &[CiRun]) -> Vec<StepTiming> {
    let mut by_key: BTreeMap<String, (String, Vec<i64>)> = BTreeMap::new();
    for step in runs.iter().flat_map(|r| &r.steps) {
        let (StepStatus::Success | StepStatus::Failure, Some(end)) =
            (step.status, step.finished_at)
        else {
            continue;
        };
        let entry = by_key
            .entry(step.key.clone())
            .or_insert_with(|| (step.title.clone(), Vec::new()));
        entry.1.push(end - step.started_at);
    }
    let mut steps: Vec<StepTiming> = by_key
        .into_iter()
        .map(|(key, (title, mut seconds))| {
            seconds.sort_unstable();
            StepTiming {
                key,
                title,
                runs: seconds.len(),
                median_seconds: seconds[seconds.len() / 2],
                max_seconds: seconds[seconds.len() - 1],
            }
        })
        .collect();
    steps.sort_by(|a, b| {
        b.median_seconds
            .cmp(&a.median_seconds)
            .then_with(|| a.key.cmp(&b.key))
    });
    steps.truncate(SLOWEST_STEPS);
    steps
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ci_reports::CiStep;

    fn job(
        kind: JobKind,
        who: &str,
        started_at: i64,
        finished: Option<i64>,
        ok: bool,
    ) -> JobStatus {
        JobStatus {
            id: 1,
            kind,
            state: if ok {
                JobState::Succeeded
            } else {
                JobState::Failed
            },
            started_by: who.into(),
            started_at,
            finished_at: finished,
            artifact: None,
        }
    }

    fn ci_step(key: &str, seconds: i64) -> CiStep {
        CiStep {
            n: 1,
            key: key.into(),
            title: key.to_uppercase(),
            started_at: 0,
            finished_at: Some(seconds),
            exit_code: Some(0),
            status: StepStatus::Success,
        }
    }

    fn ci_run(actor: &str, status: RunStatus, steps: Vec<CiStep>) -> CiRun {
        CiRun {
            id: "1-1".into(),
            run_id: "1".into(),
            attempt: 1,
            sha: "abc".into(),
            branch: "main".into(),
            actor: actor.into(),
            subject: String::new(),
            url: None,
            started_at: 100,
            finished_at: (status != RunStatus::Running).then_some(400),
            status,
            failed_step: None,
            steps,
        }
    }

    #[test]
    fn durations_people_and_slowest_steps_are_counted() {
        let jobs = [
            job(JobKind::Build, "b@x", 10, Some(70), true),
            job(JobKind::Build, "b@x", 100, None, true),
            job(JobKind::Pull, "a@x", 0, Some(2), true),
        ];
        let runs = [
            ci_run(
                "andre",
                RunStatus::Success,
                vec![ci_step("test", 30), ci_step("fmt", 2)],
            ),
            ci_run("andre", RunStatus::Failure, vec![ci_step("test", 50)]),
            ci_run("andre", RunStatus::Running, vec![ci_step("test", 90)]),
            ci_run("andre", RunStatus::Cancelled, vec![]),
        ];
        let stats = compute(&jobs, &runs, &[]);
        assert_eq!(
            stats.builds,
            [Timing {
                started_at: 10,
                seconds: 60,
                ok: true
            }]
        );
        assert_eq!(stats.test_runs.len(), 2);
        assert!(!stats.test_runs[1].ok);
        assert_eq!(stats.slowest_steps[0].key, "test");
        assert_eq!(stats.slowest_steps[0].runs, 3);
        assert_eq!(stats.slowest_steps[0].median_seconds, 50);
        assert_eq!(stats.slowest_steps[0].max_seconds, 90);
        assert_eq!(stats.people[0].who, "andre");
        assert_eq!(stats.people[0].test_runs, 4);
        assert_eq!(stats.people[1].who, "b@x");
        assert_eq!(stats.people[1].builds, 2);
    }
}
