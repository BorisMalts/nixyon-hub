use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

use super::workflow::Step;

#[derive(Debug, PartialEq, Eq)]
pub enum StepOutcome {
    Succeeded,
    Failed(Option<i32>),
    TimedOut,
    CouldNotStart(String),
}

const MAX_LOG_LINE: usize = 4000;

pub async fn run_step(step: &Step, log: &mpsc::UnboundedSender<String>) -> StepOutcome {
    let mut command = Command::new(&step.program);
    command.args(&step.args).envs(step.env.iter().cloned());
    let mut child = match command
        .current_dir(&step.dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(child) => child,
        Err(e) => return StepOutcome::CouldNotStart(e.to_string()),
    };
    let stdout = child.stdout.take().map(forward(log.clone()));
    let stderr = child.stderr.take().map(forward(log.clone()));
    let outcome = match tokio::time::timeout(step.timeout, child.wait()).await {
        Ok(Ok(status)) if status.success() => StepOutcome::Succeeded,
        Ok(Ok(status)) => StepOutcome::Failed(status.code()),
        Ok(Err(e)) => StepOutcome::CouldNotStart(e.to_string()),
        Err(_) => {
            let _ = child.kill().await;
            StepOutcome::TimedOut
        }
    };
    for task in [stdout, stderr].into_iter().flatten() {
        let _ = task.await;
    }
    outcome
}

fn forward<R>(log: mpsc::UnboundedSender<String>) -> impl FnOnce(R) -> tokio::task::JoinHandle<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    move |stream| {
        tokio::spawn(async move {
            let mut lines = BufReader::new(stream).split(b'\n');
            while let Ok(Some(raw)) = lines.next_segment().await {
                let line: String = String::from_utf8_lossy(&raw)
                    .chars()
                    .take(MAX_LOG_LINE)
                    .collect();
                if log.send(line).is_err() {
                    break;
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn step(program: &str, args: &[&str], timeout: Duration) -> Step {
        Step {
            program: program.into(),
            args: args.iter().map(|a| a.to_string()).collect(),
            dir: std::env::temp_dir(),
            timeout,
            ..Step::default()
        }
    }

    #[tokio::test]
    async fn env_of_the_step_reaches_the_program() {
        let check = Step {
            env: vec![("NIXYON_SORTFORMER_FP32".into(), "1".into())],
            ..step(
                "sh",
                &["-c", "echo \"fp32=$NIXYON_SORTFORMER_FP32\""],
                Duration::from_secs(10),
            )
        };
        let (tx, mut rx) = mpsc::unbounded_channel();
        assert_eq!(run_step(&check, &tx).await, StepOutcome::Succeeded);
        drop(tx);
        let mut lines = Vec::new();
        while let Some(l) = rx.recv().await {
            lines.push(l);
        }
        assert_eq!(lines, ["fp32=1"]);
    }

    #[tokio::test]
    async fn output_of_both_streams_reaches_the_log() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let outcome = run_step(
            &step(
                "sh",
                &["-c", "echo out; echo err >&2"],
                Duration::from_secs(10),
            ),
            &tx,
        )
        .await;
        assert_eq!(outcome, StepOutcome::Succeeded);
        drop(tx);
        let mut lines = Vec::new();
        while let Some(l) = rx.recv().await {
            lines.push(l);
        }
        lines.sort();
        assert_eq!(lines, ["err", "out"]);
    }

    #[tokio::test]
    async fn failure_timeout_and_missing_program_are_reported() {
        let (tx, _rx) = mpsc::unbounded_channel();
        assert_eq!(
            run_step(&step("sh", &["-c", "exit 3"], Duration::from_secs(10)), &tx).await,
            StepOutcome::Failed(Some(3))
        );
        assert_eq!(
            run_step(&step("sleep", &["5"], Duration::from_millis(200)), &tx).await,
            StepOutcome::TimedOut
        );
        assert!(matches!(
            run_step(
                &step("нет-такой-программы", &[], Duration::from_secs(1)),
                &tx
            )
            .await,
            StepOutcome::CouldNotStart(_)
        ));
    }
}
