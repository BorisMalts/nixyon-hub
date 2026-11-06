use std::path::{Path, PathBuf};
use std::time::Duration;

use super::experiment_dirs::{KEEP_EXPERIMENTS, fresh_experiment_dir};
use super::job_plan::JobPlan;
use super::live_records::LIVE_DIR;
use super::training_sets::{TrainingSource, training_set};
use crate::protocol::{JobKind, QualityTake};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Step {
    pub program: String,
    pub args: Vec<String>,
    pub dir: PathBuf,
    pub timeout: Duration,
    pub heavy: bool,
    /// Переменные окружения шага.
    pub env: Vec<(String, String)>,
}

pub trait Workflow: Send + Sync + 'static {
    /// Шаги задания номер `job`; `plan` уже сверен агентом.
    fn steps(&self, kind: JobKind, plan: &JobPlan, job: u64) -> Vec<Step>;
    /// Файлы и папки задания, которые нужны его шагам, готовятся до первого шага.
    fn prepare(&self, _kind: JobKind, _plan: &JobPlan, _job: u64) -> std::io::Result<()> {
        Ok(())
    }
    fn built_apk(&self) -> PathBuf;
    fn repo_dir(&self) -> &Path;
}

pub struct AndroidWorkflow {
    pub repo_dir: PathBuf,
    pub repo_url: String,
    pub datasets: PathBuf,
    pub quality_dir: PathBuf,
    /// Папка опытов, по папке на задание прогона живых записей.
    pub experiments_dir: PathBuf,
    /// Папка сборки cargo (CARGO_TARGET_DIR), в ней лежит собранный eval.
    pub target_dir: PathBuf,
    /// Папка зеркала моделей, которую раздаёт хаб. None: модели не выкладываются.
    pub models_mirror: Option<PathBuf>,
    /// Папка свободных наборов для обучения, её пишет скрипт загрузки.
    pub training_data: PathBuf,
    /// Папка итогов обучения, по папке на задание.
    pub training_dir: PathBuf,
}

const PULL_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const BUILD_TIMEOUT: Duration = Duration::from_secs(60 * 60);
// Полный прогон по всем наборам, включая части без текста, идёт сутками.
const QUALITY_TIMEOUT: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const LIVE_EVAL_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);
// Свободные наборы весят около 15 ГБ.
const DATASETS_TIMEOUT: Duration = Duration::from_secs(12 * 60 * 60);
// Признаки обеих моделей по десяткам тысяч клипов считаются часами.
const TRAINING_TIMEOUT: Duration = Duration::from_secs(3 * 24 * 60 * 60);
/// Папка итогов обучения в папке артефактов; её читает хаб.
pub const TRAINING_DIR: &str = "training";
const TRAINING_FETCH_SCRIPT: &str = "fetch-child-datasets.py";
const TRAINING_BIN: &str = "child_adult_eval";
/// Адрес зеркала для скриптов загрузки; пустой выключает зеркало.
const MIRROR_ENV: &str = "NIXYON_MODELS_MIRROR";
const PUBLISH_SCRIPT: &str = "publish-models.sh";
/// Программа, которая гоняет eval по записям и сочетаниям моделей и меряет память;
/// лежит в образе рабочего контейнера.
pub const LIVE_EVAL_DRIVER: &str = "nixyon-live-eval";

impl AndroidWorkflow {
    /// Список клипов с плохим эталоном для quality.sh: рядом с папкой отчёта, её читает
    /// рабочий контейнер. Задания идут по одному, поэтому файл один.
    pub fn exclude_file(&self) -> PathBuf {
        self.quality_dir.with_file_name("quality-exclude.json")
    }

    /// Папка с файлами опыта задания номер `job`.
    pub fn experiment_dir(&self, job: u64) -> PathBuf {
        self.experiments_dir.join(job.to_string())
    }

    /// Папка с итогами обучения задания номер `job`.
    pub fn training_run_dir(&self, job: u64) -> PathBuf {
        self.training_dir.join(job.to_string())
    }

    fn cargo_build(&self, bin: &str) -> Step {
        Step {
            heavy: true,
            ..self.step(
                "cargo",
                &[
                    "build",
                    "--release",
                    "--locked",
                    "-p",
                    "nixyon-eval",
                    "--bin",
                    bin,
                ],
                BUILD_TIMEOUT,
            )
        }
    }

    fn training_steps(&self, sets: &[String], job: u64) -> Vec<Step> {
        let chosen: Vec<_> = sets.iter().filter_map(|s| training_set(s)).collect();
        let mut fetch_args = vec![
            "-I".to_owned(),
            self.script(TRAINING_FETCH_SCRIPT),
            self.training_data.to_string_lossy().into_owned(),
        ];
        fetch_args.extend(
            chosen
                .iter()
                .filter(|s| s.source == TrainingSource::Download)
                .map(|s| s.name.to_owned()),
        );
        let mut run_args = vec![
            "--out".to_owned(),
            self.training_run_dir(job).to_string_lossy().into_owned(),
            "--ort-models".to_owned(),
            self.repo_dir
                .join("models/ort")
                .to_string_lossy()
                .into_owned(),
        ];
        for set in &chosen {
            let root = match set.source {
                TrainingSource::Download => &self.training_data,
                TrainingSource::Manual => &self.datasets,
            };
            run_args.push(set.flag.to_owned());
            run_args.push(root.join(set.dir).to_string_lossy().into_owned());
        }
        let mut steps = self.fetch_steps(&[]);
        steps.push(Step {
            args: fetch_args,
            ..self.step("python3", &[], DATASETS_TIMEOUT)
        });
        steps.push(self.cargo_build(TRAINING_BIN));
        steps.push(Step {
            args: run_args,
            heavy: true,
            ..self.step(
                &self
                    .target_dir
                    .join("release")
                    .join(TRAINING_BIN)
                    .to_string_lossy(),
                &[],
                TRAINING_TIMEOUT,
            )
        });
        steps
    }

    fn script(&self, name: &str) -> String {
        self.repo_dir
            .join("scripts")
            .join(name)
            .to_string_lossy()
            .into_owned()
    }

    fn step(&self, program: &str, args: &[&str], timeout: Duration) -> Step {
        Step {
            program: program.into(),
            args: args.iter().map(|a| a.to_string()).collect(),
            dir: self.repo_dir.clone(),
            timeout,
            ..Step::default()
        }
    }

    // Библиотека sherpa-onnx и модели не хранятся в git. Скрипты сверяют SHA-256 и
    // повторно не качают; в первый раз это сотни мегабайт. Скачанные модели затем
    // выкладываются в зеркало хаба.
    fn fetch_steps(&self, env: &[(&str, &str)]) -> Vec<Step> {
        // Сервер сам источник зеркала: качать с него же незачем.
        let env = std::iter::once((MIRROR_ENV, ""))
            .chain(env.iter().copied())
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect();
        let models = Step {
            env,
            ..self.step(&self.script("fetch-models.sh"), &[], BUILD_TIMEOUT)
        };
        let mut steps = vec![
            self.step(&self.script("fetch-sherpa.sh"), &[], BUILD_TIMEOUT),
            models,
        ];
        steps.extend(self.publish_step());
        steps
    }

    // Скрипта нет в рабочих копиях до его появления в Nixyon: тогда шаг пропускается,
    // а не роняет сборку.
    fn publish_step(&self) -> Option<Step> {
        let mirror = self.models_mirror.as_ref()?;
        let script = self.repo_dir.join("scripts").join(PUBLISH_SCRIPT);
        script.is_file().then(|| {
            self.step(
                &script.to_string_lossy(),
                &[&mirror.to_string_lossy()],
                BUILD_TIMEOUT,
            )
        })
    }
}

fn has_excluded(plan: &[QualityTake]) -> bool {
    plan.iter().any(|t| !t.exclude.is_empty())
}

impl Workflow for AndroidWorkflow {
    fn prepare(&self, kind: JobKind, plan: &JobPlan, job: u64) -> std::io::Result<()> {
        match kind {
            JobKind::Quality => {}
            JobKind::LiveEval => {
                return fresh_experiment_dir(&self.experiments_dir, job, KEEP_EXPERIMENTS)
                    .map(|_| ());
            }
            JobKind::Training => {
                return fresh_experiment_dir(&self.training_dir, job, KEEP_EXPERIMENTS).map(|_| ());
            }
            JobKind::Pull | JobKind::Build => return Ok(()),
        }
        let plan = &plan.takes;
        let path = self.exclude_file();
        if !has_excluded(plan) {
            return match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            };
        }
        let sets: std::collections::BTreeMap<&str, &[String]> = plan
            .iter()
            .filter(|t| !t.exclude.is_empty())
            .map(|t| (t.dataset.as_str(), t.exclude.as_slice()))
            .collect();
        let part = path.with_extension("json.part");
        std::fs::write(&part, serde_json::to_vec(&sets)?)?;
        std::fs::rename(&part, &path)
    }

    fn steps(&self, kind: JobKind, plan: &JobPlan, job: u64) -> Vec<Step> {
        let parent = self
            .repo_dir
            .parent()
            .map_or_else(|| PathBuf::from("/"), Path::to_path_buf);
        let repo = self.repo_dir.to_string_lossy().into_owned();
        match kind {
            JobKind::Pull if !self.repo_dir.join(".git").exists() => vec![Step {
                dir: parent,
                ..self.step("git", &["clone", &self.repo_url, &repo], PULL_TIMEOUT)
            }],
            JobKind::Pull => vec![self.step("git", &["pull", "--ff-only"], PULL_TIMEOUT)],
            JobKind::Build => {
                let apk = self.built_apk().to_string_lossy().into_owned();
                let mut steps = vec![
                    // Gradle дописывает APK на месте и оставляет в файле старые куски:
                    // без удаления APK раздувался втрое.
                    self.step("rm", &["-f", &apk], PULL_TIMEOUT),
                ];
                steps.extend(self.fetch_steps(&[]));
                // Release с моделями внутри APK, подпись отладочным ключом.
                steps.push(Step {
                    heavy: true,
                    ..self.step(&self.script("build-demo-apk.sh"), &[], BUILD_TIMEOUT)
                });
                steps
            }
            JobKind::Quality => {
                let mut args = Vec::new();
                if has_excluded(&plan.takes) {
                    args.push("--exclude".to_owned());
                    args.push(self.exclude_file().to_string_lossy().into_owned());
                }
                args.push(self.datasets.to_string_lossy().into_owned());
                args.push(self.quality_dir.to_string_lossy().into_owned());
                args.extend(plan.takes.iter().map(QualityTake::arg));
                let mut steps = self.fetch_steps(&[]);
                // Отчёт пишет скрипт из репозитория Nixyon: таблица в журнал, файлы
                // отчёта в папку, которую хаб отдаёт на скачивание.
                steps.push(Step {
                    args,
                    heavy: true,
                    ..self.step(&self.script("quality.sh"), &[], QUALITY_TIMEOUT)
                });
                steps
            }
            JobKind::LiveEval => {
                let eval = self.target_dir.join("release/eval");
                let models = self.repo_dir.join("models");
                let records = self.datasets.join(LIVE_DIR);
                let out = self.experiment_dir(job);
                let mut driver_args: Vec<String> = [
                    ("--eval", &eval),
                    ("--models", &models),
                    ("--records", &records),
                    ("--out", &out),
                ]
                .iter()
                .flat_map(|(flag, path)| [flag.to_string(), path.to_string_lossy().into_owned()])
                .collect();
                // Имена после «--»: ни одно не прочитается как флаг.
                driver_args.push("--".into());
                driver_args.extend(plan.records.iter().cloned());
                // Сочетание с Sortformer fp32 берёт модель, которую скрипт качает только
                // по этому флагу.
                let mut steps = self.fetch_steps(&[("NIXYON_SORTFORMER_FP32", "1")]);
                steps.push(self.cargo_build("eval"));
                steps.push(Step {
                    args: driver_args,
                    heavy: true,
                    ..self.step(LIVE_EVAL_DRIVER, &[], LIVE_EVAL_TIMEOUT)
                });
                steps
            }
            JobKind::Training => self.training_steps(&plan.training, job),
        }
    }

    fn built_apk(&self) -> PathBuf {
        self.repo_dir.join("nixyon-app/src-tauri/gen/android/app/build/outputs/apk/universal/release/app-universal-release.apk")
    }

    fn repo_dir(&self) -> &Path {
        &self.repo_dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pull_clones_when_repo_is_missing_and_pulls_otherwise() {
        let dir = std::env::temp_dir().join(format!("nixyon-wf-{}", std::process::id()));
        let flow = AndroidWorkflow {
            repo_dir: dir.join("repo"),
            repo_url: "git@example:x.git".into(),
            datasets: "/datasets".into(),
            quality_dir: "/artifacts/quality".into(),
            experiments_dir: "/artifacts/experiments".into(),
            target_dir: "/cache/target".into(),
            models_mirror: None,
            training_data: "/training-data".into(),
            training_dir: "/artifacts/training".into(),
        };
        assert_eq!(
            flow.steps(JobKind::Pull, &JobPlan::default(), 1)[0].args[0],
            "clone"
        );
        std::fs::create_dir_all(dir.join("repo/.git")).unwrap();
        assert_eq!(
            flow.steps(JobKind::Pull, &JobPlan::default(), 1)[0].args,
            ["pull", "--ff-only"]
        );
        let build = flow.steps(JobKind::Build, &JobPlan::default(), 1);
        assert_eq!(build[0].program, "rm");
        assert_eq!(build[0].args[1], flow.built_apk().to_string_lossy());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn quality_fetches_libraries_and_models_then_scores_the_clips_under_the_heavy_lock() {
        let repo = PathBuf::from("/work/repo");
        let flow = AndroidWorkflow {
            repo_dir: repo.clone(),
            repo_url: "git@example:x.git".into(),
            datasets: "/datasets".into(),
            quality_dir: "/artifacts/quality".into(),
            experiments_dir: "/artifacts/experiments".into(),
            target_dir: "/cache/target".into(),
            models_mirror: None,
            training_data: "/training-data".into(),
            training_dir: "/artifacts/training".into(),
        };
        let plan = [
            QualityTake {
                dataset: "cv21_test".into(),
                minutes: Some(90),
                exclude: Vec::new(),
            },
            QualityTake {
                dataset: "podlodka".into(),
                minutes: None,
                exclude: Vec::new(),
            },
        ];
        let steps = flow.steps(JobKind::Quality, &JobPlan::quality(plan.to_vec()), 1);
        let programs: Vec<&str> = steps.iter().map(|s| s.program.as_str()).collect();
        assert_eq!(
            programs,
            [
                "/work/repo/scripts/fetch-sherpa.sh",
                "/work/repo/scripts/fetch-models.sh",
                "/work/repo/scripts/quality.sh"
            ]
        );
        assert_eq!(
            steps[2].args,
            [
                "/datasets",
                "/artifacts/quality",
                "cv21_test=90",
                "podlodka=all"
            ]
        );
        let heavy: Vec<bool> = steps.iter().map(|s| s.heavy).collect();
        assert_eq!(heavy, [false, false, true]);
        assert_eq!(steps[2].timeout, QUALITY_TIMEOUT);
        assert!(steps.iter().all(|s| s.dir == repo));
    }

    #[test]
    fn clips_with_a_bad_reference_go_to_a_file_passed_to_the_quality_script() {
        let dir = std::env::temp_dir().join(format!("nixyon-wf-exclude-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let flow = AndroidWorkflow {
            repo_dir: "/work/repo".into(),
            repo_url: "git@example:x.git".into(),
            datasets: "/datasets".into(),
            quality_dir: dir.join("quality"),
            experiments_dir: dir.join("experiments"),
            target_dir: "/cache/target".into(),
            models_mirror: None,
            training_data: "/training-data".into(),
            training_dir: "/artifacts/training".into(),
        };
        let plan = [
            QualityTake {
                dataset: "cv".into(),
                minutes: None,
                exclude: vec!["a".into(), "b".into()],
            },
            QualityTake {
                dataset: "talk".into(),
                minutes: None,
                exclude: Vec::new(),
            },
        ];
        flow.prepare(JobKind::Quality, &JobPlan::quality(plan.to_vec()), 1)
            .unwrap();
        let written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(flow.exclude_file()).unwrap()).unwrap();
        assert_eq!(written, serde_json::json!({"cv": ["a", "b"]}));
        let steps = flow.steps(JobKind::Quality, &JobPlan::quality(plan.to_vec()), 1);
        assert_eq!(steps[2].args[0], "--exclude");
        assert_eq!(steps[2].args[1], flow.exclude_file().to_string_lossy());

        let clean = [QualityTake {
            dataset: "cv".into(),
            minutes: None,
            exclude: Vec::new(),
        }];
        flow.prepare(JobKind::Quality, &JobPlan::quality(clean.to_vec()), 1)
            .unwrap();
        assert!(
            !flow.exclude_file().exists(),
            "старый список не должен остаться"
        );
        assert_ne!(
            flow.steps(JobKind::Quality, &JobPlan::quality(clean.to_vec()), 1)[2].args[0],
            "--exclude"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn build_fetches_sherpa_and_models_then_builds_the_apk_with_models() {
        let repo = PathBuf::from("/work/repo");
        let flow = AndroidWorkflow {
            repo_dir: repo.clone(),
            repo_url: "git@example:x.git".into(),
            datasets: "/datasets".into(),
            quality_dir: "/artifacts/quality".into(),
            experiments_dir: "/artifacts/experiments".into(),
            target_dir: "/cache/target".into(),
            models_mirror: None,
            training_data: "/training-data".into(),
            training_dir: "/artifacts/training".into(),
        };
        let build = flow.steps(JobKind::Build, &JobPlan::default(), 1);
        let programs: Vec<&str> = build.iter().map(|s| s.program.as_str()).collect();
        assert_eq!(
            programs,
            [
                "rm",
                "/work/repo/scripts/fetch-sherpa.sh",
                "/work/repo/scripts/fetch-models.sh",
                "/work/repo/scripts/build-demo-apk.sh",
            ]
        );
        let heavy: Vec<bool> = build.iter().map(|s| s.heavy).collect();
        assert_eq!(heavy, [false, false, false, true]);
        assert!(build.iter().all(|s| s.dir == repo));
        assert!(build[1..].iter().all(|s| s.timeout == BUILD_TIMEOUT));
        assert!(
            flow.built_apk()
                .ends_with("apk/universal/release/app-universal-release.apk")
        );
    }

    #[test]
    fn live_eval_fetches_fp32_models_builds_eval_and_runs_the_driver_under_the_heavy_lock() {
        let flow = AndroidWorkflow {
            repo_dir: "/work/repo".into(),
            repo_url: "git@example:x.git".into(),
            datasets: "/datasets".into(),
            quality_dir: "/artifacts/quality".into(),
            experiments_dir: "/artifacts/experiments".into(),
            target_dir: "/cache/target".into(),
            models_mirror: None,
            training_data: "/training-data".into(),
            training_dir: "/artifacts/training".into(),
        };
        let plan = JobPlan::records(vec!["55342".into(), "dialogue".into()]);
        let steps = flow.steps(JobKind::LiveEval, &plan, 42);
        let programs: Vec<&str> = steps.iter().map(|s| s.program.as_str()).collect();
        assert_eq!(
            programs,
            [
                "/work/repo/scripts/fetch-sherpa.sh",
                "/work/repo/scripts/fetch-models.sh",
                "cargo",
                LIVE_EVAL_DRIVER
            ]
        );
        assert_eq!(
            steps[1].env,
            [
                (MIRROR_ENV.to_string(), String::new()),
                ("NIXYON_SORTFORMER_FP32".to_string(), "1".to_string())
            ]
        );
        assert!(steps[0].env.is_empty());
        assert_eq!(
            steps[2].args,
            [
                "build",
                "--release",
                "--locked",
                "-p",
                "nixyon-eval",
                "--bin",
                "eval"
            ]
        );
        assert_eq!(
            steps[3].args,
            [
                "--eval",
                "/cache/target/release/eval",
                "--models",
                "/work/repo/models",
                "--records",
                "/datasets/live",
                "--out",
                "/artifacts/experiments/42",
                "--",
                "55342",
                "dialogue"
            ]
        );
        let heavy: Vec<bool> = steps.iter().map(|s| s.heavy).collect();
        assert_eq!(heavy, [false, false, true, true]);
        assert_eq!(steps[3].timeout, LIVE_EVAL_TIMEOUT);
        assert!(steps.iter().all(|s| s.dir == flow.repo_dir));
        let quality = flow.steps(
            JobKind::Quality,
            &JobPlan::quality(vec![QualityTake {
                dataset: "cv".into(),
                minutes: None,
                exclude: Vec::new(),
            }]),
            1,
        );
        assert!(
            quality
                .iter()
                .flat_map(|s| &s.env)
                .all(|(k, _)| k != "NIXYON_SORTFORMER_FP32"),
            "fp32 только для прогона живых записей"
        );
    }

    #[test]
    fn live_eval_prepares_an_empty_folder_for_its_job() {
        let dir = std::env::temp_dir().join(format!("nixyon-wf-live-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let flow = AndroidWorkflow {
            repo_dir: "/work/repo".into(),
            repo_url: "git@example:x.git".into(),
            datasets: "/datasets".into(),
            quality_dir: dir.join("quality"),
            experiments_dir: dir.join("experiments"),
            target_dir: "/cache/target".into(),
            models_mirror: None,
            training_data: "/training-data".into(),
            training_dir: "/artifacts/training".into(),
        };
        let plan = JobPlan::records(vec!["talk".into()]);
        flow.prepare(JobKind::LiveEval, &plan, 3).unwrap();
        assert!(flow.experiment_dir(3).is_dir());
        assert!(!flow.exclude_file().exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn models_go_to_the_hub_mirror_after_fetching_once_the_script_is_in_the_repo() {
        let dir = std::env::temp_dir().join(format!("nixyon-wf-mirror-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("repo/scripts")).unwrap();
        let flow = AndroidWorkflow {
            repo_dir: dir.join("repo"),
            repo_url: "git@example:x.git".into(),
            datasets: "/datasets".into(),
            quality_dir: "/artifacts/quality".into(),
            experiments_dir: "/artifacts/experiments".into(),
            target_dir: "/cache/target".into(),
            models_mirror: Some("/models-mirror".into()),
            training_data: "/training-data".into(),
            training_dir: "/artifacts/training".into(),
        };
        let programs = |kind| -> Vec<String> {
            flow.steps(kind, &JobPlan::default(), 1)
                .into_iter()
                .map(|s| s.program)
                .collect()
        };
        let publish = dir.join("repo/scripts/publish-models.sh");
        assert!(
            !programs(JobKind::Build).contains(&publish.to_string_lossy().into_owned()),
            "без скрипта шага нет"
        );
        std::fs::write(&publish, "").unwrap();
        for kind in [
            JobKind::Build,
            JobKind::Quality,
            JobKind::LiveEval,
            JobKind::Training,
        ] {
            let steps = flow.steps(kind, &JobPlan::default(), 1);
            let at = steps
                .iter()
                .position(|s| s.program == publish.to_string_lossy())
                .expect("шаг публикации");
            assert!(steps[at - 1].program.ends_with("fetch-models.sh"));
            assert_eq!(steps[at - 1].env[0], (MIRROR_ENV.into(), String::new()));
            assert_eq!(steps[at].args, ["/models-mirror"]);
            assert!(!steps[at].heavy);
        }
        let without = AndroidWorkflow {
            models_mirror: None,
            ..flow
        };
        assert!(
            !without
                .steps(JobKind::Build, &JobPlan::default(), 1)
                .iter()
                .any(|s| s.program == publish.to_string_lossy())
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn training_downloads_free_sets_builds_the_trainer_and_runs_it_under_the_heavy_lock() {
        let flow = AndroidWorkflow {
            repo_dir: "/work/repo".into(),
            repo_url: "git@example:x.git".into(),
            datasets: "/datasets".into(),
            quality_dir: "/artifacts/quality".into(),
            experiments_dir: "/artifacts/experiments".into(),
            target_dir: "/cache/target".into(),
            models_mirror: None,
            training_data: "/training-data".into(),
            training_dir: "/artifacts/training".into(),
        };
        let plan = JobPlan::training(vec![
            "speechocean762".into(),
            "childmandarin".into(),
            "samromur_children".into(),
        ]);
        let steps = flow.steps(JobKind::Training, &plan, 7);
        let programs: Vec<&str> = steps.iter().map(|s| s.program.as_str()).collect();
        assert_eq!(
            programs,
            [
                "/work/repo/scripts/fetch-sherpa.sh",
                "/work/repo/scripts/fetch-models.sh",
                "python3",
                "cargo",
                "/cache/target/release/child_adult_eval"
            ]
        );
        assert_eq!(
            steps[2].args,
            [
                "-I",
                "/work/repo/scripts/fetch-child-datasets.py",
                "/training-data",
                "speechocean762",
                "samromur_children"
            ]
        );
        assert_eq!(steps[3].args.last().unwrap(), "child_adult_eval");
        assert_eq!(
            steps[4].args,
            [
                "--out",
                "/artifacts/training/7",
                "--ort-models",
                "/work/repo/models/ort",
                "--speechocean",
                "/training-data/speechocean762/speechocean762",
                "--childmandarin",
                "/datasets/childmandarin",
                "--samromur",
                "/training-data/samromur_children"
            ]
        );
        let heavy: Vec<bool> = steps.iter().map(|s| s.heavy).collect();
        assert_eq!(heavy, [false, false, false, true, true]);
        assert_eq!(steps[2].timeout, DATASETS_TIMEOUT);
        assert_eq!(steps[4].timeout, TRAINING_TIMEOUT);
        assert!(steps.iter().all(|s| s.dir == flow.repo_dir));
    }

    #[test]
    fn training_prepares_an_empty_folder_for_its_job() {
        let dir = std::env::temp_dir().join(format!("nixyon-wf-training-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let flow = AndroidWorkflow {
            repo_dir: "/work/repo".into(),
            repo_url: "git@example:x.git".into(),
            datasets: "/datasets".into(),
            quality_dir: dir.join("quality"),
            experiments_dir: dir.join("experiments"),
            target_dir: "/cache/target".into(),
            models_mirror: None,
            training_data: dir.join("training-data"),
            training_dir: dir.join("training"),
        };
        let plan = JobPlan::training(vec!["speechocean762".into()]);
        flow.prepare(JobKind::Training, &plan, 5).unwrap();
        assert!(flow.training_run_dir(5).is_dir());
        assert!(!flow.experiment_dir(5).exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
