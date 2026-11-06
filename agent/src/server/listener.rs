use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::io::{AsyncWrite, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use super::auth::TokenCheck;
use super::datasets::{check_plan, list_datasets};
use super::frames::{read_line_max, send};
use super::job_plan::JobPlan;
use super::jobs::{JobEvent, Jobs};
use super::live_records::{LIVE_DIR, check_records};
use super::terminal;
use super::training_sets::check_training;
use crate::protocol::{ErrorCode, JobKind, Op, Request, ServerFrame, decode};

pub const MAX_TERMINALS: usize = 4;
pub const MAX_CONNECTIONS: usize = 64;
pub const MAX_UNAUTHENTICATED: usize = 16;
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_REQUEST_BYTES: usize = 16 * 1024;

pub struct Context {
    pub jobs: Arc<Jobs>,
    pub token: TokenCheck,
    pub repo_dir: PathBuf,
    /// Корень наборов записей для проверки качества.
    pub datasets: PathBuf,
    pub shell: String,
    pub terminals: AtomicUsize,
    pub request_timeout: Duration,
    pub allowed_peers: Option<Vec<IpAddr>>,
}

pub async fn serve(listener: TcpListener, context: Arc<Context>) -> std::io::Result<()> {
    let unauthenticated = Arc::new(Semaphore::new(MAX_UNAUTHENTICATED));
    let authenticated = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    loop {
        let (stream, peer) = listener.accept().await?;
        if let Some(allowed) = &context.allowed_peers
            && !allowed.contains(&peer.ip())
        {
            tracing::warn!(%peer, "соединение с агентом не с адреса хаба, сброшено");
            continue;
        }
        let Ok(pre_auth) = Arc::clone(&unauthenticated).try_acquire_owned() else {
            tracing::warn!(%peer, "слишком много соединений до проверки токена, соединение сброшено");
            continue;
        };
        let context = Arc::clone(&context);
        let authenticated = Arc::clone(&authenticated);
        tokio::spawn(async move {
            if let Err(e) = handle(stream, &context, pre_auth, &authenticated).await {
                tracing::warn!(%peer, error = %e, "соединение с агентом закрыто с ошибкой");
            }
        });
    }
}

async fn reply_error<W: AsyncWrite + Unpin>(
    writer: &mut W,
    code: ErrorCode,
    message: &str,
) -> Result<(), super::frames::FrameError> {
    send(
        writer,
        &ServerFrame::Error {
            code,
            message: message.into(),
        },
    )
    .await
}

async fn handle(
    stream: TcpStream,
    context: &Context,
    pre_auth: OwnedSemaphorePermit,
    authenticated: &Arc<Semaphore>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (read, mut writer) = stream.into_split();
    let mut reader = BufReader::new(read);
    let Ok(first) = tokio::time::timeout(
        context.request_timeout,
        read_line_max(&mut reader, MAX_REQUEST_BYTES),
    )
    .await
    else {
        return Ok(());
    };
    let line = match first {
        Ok(Some(line)) => line,
        Ok(None) => return Ok(()),
        Err(_) => {
            return Ok(
                reply_error(&mut writer, ErrorCode::BadRequest, "запрос не читается").await?,
            );
        }
    };
    let request: Request = match decode(&line) {
        Ok(r) => r,
        Err(_) => {
            return Ok(
                reply_error(&mut writer, ErrorCode::BadRequest, "запрос не читается").await?,
            );
        }
    };
    if !context.token.matches(&request.token) {
        return Ok(reply_error(&mut writer, ErrorCode::Unauthorized, "нет доступа").await?);
    }
    drop(pre_auth);
    let Ok(_slot) = Arc::clone(authenticated).try_acquire_owned() else {
        return Ok(reply_error(&mut writer, ErrorCode::Busy, "агент перегружен").await?);
    };
    match request.op {
        Op::Pull | Op::Build | Op::Quality | Op::LiveEval | Op::Training => {
            let kind = match request.op {
                Op::Pull => JobKind::Pull,
                Op::Build => JobKind::Build,
                Op::Quality => JobKind::Quality,
                Op::LiveEval => JobKind::LiveEval,
                _ => JobKind::Training,
            };
            let checked = match kind {
                JobKind::Quality => {
                    let root = context.datasets.clone();
                    let available = tokio::task::spawn_blocking(move || list_datasets(&root))
                        .await
                        .unwrap_or_default();
                    check_plan(&request.plan, &available)
                        .map(|()| JobPlan::quality(request.plan))
                        .map_err(|reason| format!("план проверки не подходит: {reason}"))
                }
                JobKind::LiveEval => {
                    let dir = context.datasets.join(LIVE_DIR);
                    let records = request.records;
                    tokio::task::spawn_blocking(move || {
                        check_records(&records, &dir).map(|()| JobPlan::records(records))
                    })
                    .await
                    .unwrap_or_else(|_| Err("записи не проверены".into()))
                    .map_err(|reason| format!("записи не подходят: {reason}"))
                }
                JobKind::Training => {
                    let root = context.datasets.clone();
                    let sets = request.training;
                    tokio::task::spawn_blocking(move || {
                        check_training(&sets, &root).map(|()| JobPlan::training(sets))
                    })
                    .await
                    .unwrap_or_else(|_| Err("наборы не проверены".into()))
                    .map_err(|reason| format!("наборы для обучения не подходят: {reason}"))
                }
                JobKind::Pull | JobKind::Build => Ok(JobPlan::default()),
            };
            let plan = match checked {
                Ok(plan) => plan,
                Err(message) => {
                    return Ok(reply_error(&mut writer, ErrorCode::BadRequest, &message).await?);
                }
            };
            match context.jobs.start(kind, &request.user, plan) {
                Ok(job) => send(&mut writer, &ServerFrame::Started { job }).await?,
                Err(_) => {
                    reply_error(
                        &mut writer,
                        ErrorCode::Busy,
                        "такое задание уже ждёт в очереди",
                    )
                    .await?
                }
            }
        }
        Op::Status => {
            send(
                &mut writer,
                &ServerFrame::Status {
                    job: context.jobs.status(),
                    queue: context.jobs.queue(),
                },
            )
            .await?
        }
        Op::Datasets => {
            let root = context.datasets.clone();
            let datasets = tokio::task::spawn_blocking(move || list_datasets(&root))
                .await
                .unwrap_or_default();
            send(&mut writer, &ServerFrame::Datasets { datasets }).await?
        }
        Op::Follow => follow(&mut writer, context).await?,
        Op::Terminal => {
            if context.terminals.fetch_add(1, Ordering::SeqCst) >= MAX_TERMINALS {
                context.terminals.fetch_sub(1, Ordering::SeqCst);
                return Ok(reply_error(
                    &mut writer,
                    ErrorCode::Busy,
                    "открыто слишком много терминалов",
                )
                .await?);
            }
            let result = terminal::session(
                &mut reader,
                &mut writer,
                &context.shell,
                &context.repo_dir,
                request.cols,
                request.rows,
            )
            .await;
            context.terminals.fetch_sub(1, Ordering::SeqCst);
            result?;
        }
    }
    Ok(())
}

async fn follow<W: AsyncWrite + Unpin>(
    writer: &mut W,
    context: &Context,
) -> Result<(), super::frames::FrameError> {
    let follow = context.jobs.follow();
    send(
        writer,
        &ServerFrame::Status {
            job: follow.status.clone(),
            queue: context.jobs.queue(),
        },
    )
    .await?;
    for line in follow.snapshot {
        send(writer, &ServerFrame::Log { line }).await?;
    }
    let Some(mut events) = follow.events else {
        if let Some(job) = follow.status {
            send(writer, &ServerFrame::Finished { job }).await?;
        }
        return Ok(());
    };
    loop {
        match events.recv().await {
            Ok(JobEvent::Line(line)) => send(writer, &ServerFrame::Log { line }).await?,
            Ok(JobEvent::Finished(job)) => {
                return send(writer, &ServerFrame::Finished { job }).await;
            }
            Err(RecvError::Lagged(n)) => {
                send(
                    writer,
                    &ServerFrame::Log {
                        line: format!("пропущено строк журнала: {n}"),
                    },
                )
                .await?
            }
            Err(RecvError::Closed) => return Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::Duration;

    use super::*;
    use crate::protocol::QualityTake;
    use crate::protocol::{JobState, encode};
    use crate::server::frames::read_line;
    use crate::server::workflow::{Step, Workflow};
    use tokio::io::AsyncReadExt;
    use tokio::io::AsyncWriteExt;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    struct Echo(PathBuf);

    impl Workflow for Echo {
        fn steps(&self, _kind: JobKind, _plan: &JobPlan, _job: u64) -> Vec<Step> {
            vec![Step {
                program: "sh".into(),
                args: vec!["-c".into(), "sleep 0.2; echo из-задания".into()],
                dir: self.0.clone(),
                timeout: Duration::from_secs(10),
                heavy: true,
                ..Step::default()
            }]
        }
        fn built_apk(&self) -> PathBuf {
            self.0.join("none.apk")
        }
        fn repo_dir(&self) -> &Path {
            &self.0
        }
    }

    // Корень наборов с одним набором cv, свой у каждого процесса тестов.
    fn datasets_root() -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("nixyon-agent-datasets-{}", std::process::id()));
        std::fs::create_dir_all(root.join("cv")).unwrap();
        std::fs::write(
            root.join("cv/dataset.json"),
            r#"{"id":"cv","title":"CV","kind":"single","has_text":true,"license":"CC0","clips":3,"hours":0.5}"#,
        )
        .unwrap();
        root
    }

    async fn start() -> std::net::SocketAddr {
        let dir = std::env::temp_dir();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let context = Arc::new(Context {
            jobs: Jobs::new(
                Arc::new(Echo(dir.clone())),
                dir.join("nixyon-agent-test-out"),
            ),
            token: TokenCheck::new(TOKEN).unwrap(),
            repo_dir: dir.clone(),
            datasets: datasets_root(),
            shell: "sh".into(),
            terminals: AtomicUsize::new(0),
            request_timeout: Duration::from_millis(300),
            allowed_peers: None,
        });
        tokio::spawn(serve(listener, context));
        addr
    }

    async fn call(addr: std::net::SocketAddr, token: &str, op: Op) -> Vec<ServerFrame> {
        call_with(addr, token, op, Vec::new()).await
    }

    async fn call_with(
        addr: std::net::SocketAddr,
        token: &str,
        op: Op,
        plan: Vec<QualityTake>,
    ) -> Vec<ServerFrame> {
        call_full(addr, token, op, plan, Vec::new()).await
    }

    async fn call_full(
        addr: std::net::SocketAddr,
        token: &str,
        op: Op,
        plan: Vec<QualityTake>,
        records: Vec<String>,
    ) -> Vec<ServerFrame> {
        call_training(addr, token, op, plan, records, Vec::new()).await
    }

    async fn call_training(
        addr: std::net::SocketAddr,
        token: &str,
        op: Op,
        plan: Vec<QualityTake>,
        records: Vec<String>,
        training: Vec<String>,
    ) -> Vec<ServerFrame> {
        let stream = TcpStream::connect(addr).await.unwrap();
        let (read, mut write) = stream.into_split();
        let request = Request {
            token: token.into(),
            op,
            user: "a@x.org".into(),
            cols: 80,
            rows: 24,
            plan,
            records,
            training,
        };
        write.write_all(&encode(&request).unwrap()).await.unwrap();
        let mut reader = BufReader::new(read);
        let mut frames = Vec::new();
        while let Ok(Some(line)) =
            tokio::time::timeout(Duration::from_secs(10), read_line(&mut reader))
                .await
                .unwrap()
        {
            frames.push(decode(&line).unwrap());
        }
        frames
    }

    #[tokio::test]
    async fn datasets_are_listed_from_the_datasets_root() {
        let addr = start().await;
        let frames = call(addr, TOKEN, Op::Datasets).await;
        assert!(
            matches!(frames.as_slice(), [ServerFrame::Datasets { datasets }]
                if datasets.len() == 1 && datasets[0].id == "cv" && datasets[0].audio_sec == 1_800),
            "{frames:?}"
        );
    }

    #[tokio::test]
    async fn wrong_token_gets_nothing_but_unauthorized() {
        let addr = start().await;
        let frames = call(addr, "неверный", Op::Status).await;
        assert!(matches!(
            frames.as_slice(),
            [ServerFrame::Error {
                code: ErrorCode::Unauthorized,
                ..
            }]
        ));
    }

    #[tokio::test]
    async fn job_starts_next_waits_in_queue_and_same_kind_is_not_queued_twice() {
        let addr = start().await;
        assert!(matches!(
            call(addr, TOKEN, Op::Pull).await.as_slice(),
            [ServerFrame::Started { job }] if job.state == JobState::Running
        ));
        assert!(matches!(
            call(addr, TOKEN, Op::Build).await.as_slice(),
            [ServerFrame::Started { job }] if job.state == JobState::Queued
        ));
        assert!(matches!(
            call(addr, TOKEN, Op::Build).await.as_slice(),
            [ServerFrame::Error {
                code: ErrorCode::Busy,
                ..
            }]
        ));
        assert!(matches!(
            call(addr, TOKEN, Op::Quality).await.as_slice(),
            [ServerFrame::Error {
                code: ErrorCode::BadRequest,
                ..
            }]
        ));
        let missing = vec![QualityTake {
            dataset: "nope".into(),
            minutes: Some(5),
            exclude: Vec::new(),
        }];
        assert!(matches!(
            call_with(addr, TOKEN, Op::Quality, missing)
                .await
                .as_slice(),
            [ServerFrame::Error {
                code: ErrorCode::BadRequest,
                ..
            }]
        ));
        let plan = vec![QualityTake {
            dataset: "cv".into(),
            minutes: Some(5),
            exclude: Vec::new(),
        }];
        assert!(matches!(
            call_with(addr, TOKEN, Op::Quality, plan).await.as_slice(),
            [ServerFrame::Started { job }]
                if job.state == JobState::Queued && job.kind == JobKind::Quality
        ));
        let frames = call(addr, TOKEN, Op::Follow).await;
        assert!(
            frames
                .iter()
                .any(|f| matches!(f, ServerFrame::Log { line } if line == "из-задания"))
        );
        assert!(
            matches!(frames.last(), Some(ServerFrame::Finished { job }) if job.state == JobState::Succeeded)
        );
    }

    #[tokio::test]
    async fn training_starts_only_with_known_sets_and_manual_sets_present_on_the_server() {
        let addr = start().await;
        let names = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        for bad in [
            names(&[]),
            names(&["samromur"]),
            names(&["speechocean762", "../x"]),
            names(&["speechocean762", "sps_ru"]),
        ] {
            let frames = call_training(
                addr,
                TOKEN,
                Op::Training,
                Vec::new(),
                Vec::new(),
                bad.clone(),
            )
            .await;
            assert!(
                matches!(
                    frames.as_slice(),
                    [ServerFrame::Error { code: ErrorCode::BadRequest, message }]
                        if message.starts_with("наборы для обучения не подходят")
                ),
                "{bad:?}: {frames:?}"
            );
        }
        let frames = call_training(
            addr,
            TOKEN,
            Op::Training,
            Vec::new(),
            Vec::new(),
            names(&["speechocean762", "samromur_children"]),
        )
        .await;
        assert!(
            matches!(
                frames.as_slice(),
                [ServerFrame::Started { job }] if job.kind == JobKind::Training
            ),
            "{frames:?}"
        );
    }

    #[tokio::test]
    async fn live_eval_starts_only_with_known_records_from_the_live_folder() {
        let addr = start().await;
        let live = datasets_root().join("live");
        std::fs::create_dir_all(&live).unwrap();
        std::fs::write(live.join("talk.wav"), b"RIFF").unwrap();
        for bad in [
            vec![],
            vec!["nope".to_string()],
            vec!["../cv/dataset".to_string()],
            vec!["talk".to_string(), "talk".to_string()],
        ] {
            let frames = call_full(addr, TOKEN, Op::LiveEval, Vec::new(), bad.clone()).await;
            assert!(
                matches!(
                    frames.as_slice(),
                    [ServerFrame::Error { code: ErrorCode::BadRequest, message }]
                        if message.starts_with("записи не подходят")
                ),
                "{bad:?}: {frames:?}"
            );
        }
        let frames = call_full(addr, TOKEN, Op::LiveEval, Vec::new(), vec!["talk".into()]).await;
        assert!(
            matches!(
                frames.as_slice(),
                [ServerFrame::Started { job }] if job.kind == JobKind::LiveEval
            ),
            "{frames:?}"
        );
    }

    #[tokio::test]
    async fn garbage_request_is_bad_request() {
        let addr = start().await;
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream.write_all(b"{not json\n").await.unwrap();
        let mut reader = BufReader::new(stream);
        let line = read_line(&mut reader).await.unwrap().unwrap();
        assert!(matches!(
            decode(&line).unwrap(),
            ServerFrame::Error {
                code: ErrorCode::BadRequest,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn silent_client_is_dropped_after_request_timeout() {
        let addr = start().await;
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let mut buf = [0u8; 1];
        let read = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buf))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(read, 0, "агент должен закрыть молчащее соединение");
    }

    #[tokio::test]
    async fn oversized_request_line_is_refused_before_auth() {
        let addr = start().await;
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream
            .write_all(&vec![b'x'; MAX_REQUEST_BYTES + 100])
            .await
            .unwrap();
        let mut reader = BufReader::new(stream);
        let line = read_line(&mut reader).await.unwrap().unwrap();
        assert!(matches!(
            decode(&line).unwrap(),
            ServerFrame::Error {
                code: ErrorCode::BadRequest,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn connections_beyond_the_limit_are_dropped() {
        let addr = start().await;
        let mut held = Vec::new();
        for _ in 0..MAX_UNAUTHENTICATED {
            held.push(TcpStream::connect(addr).await.unwrap());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        let mut extra = TcpStream::connect(addr).await.unwrap();
        let mut buf = [0u8; 1];
        let read = tokio::time::timeout(Duration::from_secs(5), extra.read(&mut buf))
            .await
            .unwrap();
        assert!(
            matches!(read, Ok(0) | Err(_)),
            "лишнее соединение должно быть сброшено"
        );
    }

    #[tokio::test]
    async fn connection_from_address_outside_allowlist_is_dropped() {
        let dir = std::env::temp_dir();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let context = Arc::new(Context {
            jobs: Jobs::new(
                Arc::new(Echo(dir.clone())),
                dir.join("nixyon-agent-test-out"),
            ),
            token: TokenCheck::new(TOKEN).unwrap(),
            repo_dir: dir.clone(),
            datasets: dir.join("datasets"),
            shell: "sh".into(),
            terminals: AtomicUsize::new(0),
            request_timeout: Duration::from_millis(300),
            allowed_peers: Some(vec![IpAddr::from([10, 9, 8, 7])]),
        });
        tokio::spawn(serve(listener, context));
        let stream = TcpStream::connect(addr).await.unwrap();
        let (read, mut write) = stream.into_split();
        let request = Request {
            token: TOKEN.into(),
            op: Op::Status,
            user: "a@x.org".into(),
            cols: 0,
            rows: 0,
            plan: Vec::new(),
            records: Vec::new(),
            training: Vec::new(),
        };
        let _ = write.write_all(&encode(&request).unwrap()).await;
        let mut reader = BufReader::new(read);
        let reply = tokio::time::timeout(Duration::from_secs(5), read_line(&mut reader))
            .await
            .unwrap();
        assert!(
            matches!(reply, Ok(None) | Err(_)),
            "чужой адрес не должен получить ответ"
        );
    }

    #[tokio::test]
    async fn waiting_unauthenticated_sockets_do_not_block_authenticated_work() {
        let addr = start().await;
        let mut held = Vec::new();
        for _ in 0..MAX_UNAUTHENTICATED - 1 {
            held.push(TcpStream::connect(addr).await.unwrap());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(matches!(
            call(addr, TOKEN, Op::Status).await.as_slice(),
            [ServerFrame::Status { .. }]
        ));
    }
}
