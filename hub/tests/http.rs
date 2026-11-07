use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use nixyon_agent::protocol::{
    Artifact, DatasetSummary, ErrorCode, JobKind, JobState, JobStatus, ServerFrame, encode,
};
use nixyon_hub::config::HubConfig;
use nixyon_hub::state::{AppState, MODEL_DOWNLOADS_PER_IP};
use nixyon_hub::{db, routes, users};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tower::ServiceExt;

const ORIGIN: &str = "https://nixyon.test";
const EMAIL: &str = "andrey@example.org";
const PASSWORD: &str = "очень-длинный-пароль";
const TOKEN: &str = "0123456789abcdef0123456789abcdef";

type Reply = Arc<Mutex<Option<ServerFrame>>>;
/// Строки запросов, которые получил агент.
type Seen = Arc<Mutex<Vec<String>>>;

async fn fake_agent(reply: Reply, seen: Seen) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let reply = reply.lock().unwrap().clone();
            let seen = Arc::clone(&seen);
            tokio::spawn(async move {
                let (read, mut write) = stream.into_split();
                let mut line = String::new();
                let _ = BufReader::new(read).read_line(&mut line).await;
                seen.lock().unwrap().push(line);
                if let Some(frame) = reply {
                    let _ = write.write_all(&encode(&frame).unwrap()).await;
                }
            });
        }
    });
    addr.to_string()
}

struct Hub {
    app: Router,
    artifacts: PathBuf,
    reply: Reply,
    seen: Seen,
}

async fn hub(name: &str) -> Hub {
    let artifacts = std::env::temp_dir().join(format!("nixyon-http-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&artifacts);
    std::fs::create_dir_all(&artifacts).unwrap();
    let reply: Reply = Arc::new(Mutex::new(None));
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let agent_addr = fake_agent(Arc::clone(&reply), Arc::clone(&seen)).await;
    let conn = db::open_in_memory().unwrap();
    users::add(&conn, EMAIL, PASSWORD).unwrap();
    let config = HubConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        db_path: PathBuf::from(":memory:"),
        public_origin: ORIGIN.into(),
        agent_addr,
        agent_token: TOKEN.into(),
        artifacts_dir: artifacts.clone(),
        trusted_proxies: vec![[10, 0, 0, 1].into()],
        ci_reports_dir: Some(artifacts.join("ci")),
        models_dir: Some(artifacts.join("models")),
    };
    Hub {
        app: routes::router(AppState::new(conn, config)),
        artifacts,
        reply,
        seen,
    }
}

fn request(method: &str, uri: &str) -> axum::http::request::Builder {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("x-forwarded-for", "203.0.113.7")
        .extension(axum::extract::ConnectInfo(SocketAddr::from((
            [10, 0, 0, 1],
            5555,
        ))))
}

fn login_request(email: &str, password: &str, origin: Option<&str>) -> Request<Body> {
    let mut builder =
        request("POST", "/api/login").header(header::CONTENT_TYPE, "application/json");
    if let Some(origin) = origin {
        builder = builder.header(header::ORIGIN, origin);
    }
    builder
        .body(Body::from(
            serde_json::json!({ "email": email, "password": password }).to_string(),
        ))
        .unwrap()
}

async fn send(app: &Router, req: Request<Body>) -> axum::response::Response {
    app.clone().oneshot(req).await.unwrap()
}

async fn json(response: axum::response::Response) -> serde_json::Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 1 << 20).await.unwrap()).unwrap()
}

async fn sign_in(app: &Router) -> String {
    let response = send(app, login_request(EMAIL, PASSWORD, Some(ORIGIN))).await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let cookie = response
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    cookie.split(';').next().unwrap().to_string()
}

#[tokio::test]
async fn every_response_carries_security_headers() {
    let h = hub("headers").await;
    let response = send(
        &h.app,
        request("GET", "/health").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let headers = response.headers();
    let csp: Vec<&str> = headers[header::CONTENT_SECURITY_POLICY]
        .to_str()
        .unwrap()
        .split(';')
        .map(str::trim)
        .collect();
    assert!(csp.contains(&"frame-ancestors 'none'"));
    assert!(csp.contains(&"script-src 'self'"));
    // xterm задаёт цвета и размеры ячеек через вставленные <style>.
    assert!(csp.contains(&"style-src 'self' 'unsafe-inline'"));
    assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    assert_eq!(headers[header::X_FRAME_OPTIONS], "DENY");
    assert_eq!(headers[header::REFERRER_POLICY], "no-referrer");
}

#[tokio::test]
async fn pages_redirect_to_login_until_signed_in() {
    let h = hub("pages").await;
    let response = send(&h.app, request("GET", "/").body(Body::empty()).unwrap()).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/login");
    let response = send(
        &h.app,
        request("GET", "/login").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let cookie = sign_in(&h.app).await;
    let response = send(
        &h.app,
        request("GET", "/")
            .header(header::COOKIE, &cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let response = send(
        &h.app,
        request("GET", "/assets/../Cargo.toml")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn login_requires_our_origin() {
    let h = hub("origin").await;
    for origin in [None, Some("https://evil.example")] {
        let response = send(&h.app, login_request(EMAIL, PASSWORD, origin)).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{origin:?}");
    }
}

#[tokio::test]
async fn wrong_password_and_unknown_email_look_the_same() {
    let h = hub("wrong").await;
    let a = send(
        &h.app,
        login_request(EMAIL, "неверный-пароль-длинный", Some(ORIGIN)),
    )
    .await;
    let b = send(
        &h.app,
        login_request("nobody@example.org", PASSWORD, Some(ORIGIN)),
    )
    .await;
    assert_eq!(a.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(b.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(json(a).await, json(b).await);
}

#[tokio::test]
async fn session_cookie_is_strict_and_unlocks_api_until_logout() {
    let h = hub("session").await;
    let response = send(&h.app, login_request(EMAIL, PASSWORD, Some(ORIGIN))).await;
    let set_cookie = response.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .to_string();
    for flag in [
        "__Host-nixyon_session=",
        "HttpOnly",
        "Secure",
        "SameSite=Strict",
        "Path=/",
    ] {
        assert!(set_cookie.contains(flag), "{flag}");
    }
    let cookie = set_cookie.split(';').next().unwrap().to_string();

    let me = send(
        &h.app,
        request("GET", "/api/me")
            .header(header::COOKIE, &cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(json(me).await["email"], EMAIL);

    let logout = request("POST", "/api/logout")
        .header(header::COOKIE, &cookie)
        .header(header::ORIGIN, ORIGIN);
    assert_eq!(
        send(&h.app, logout.body(Body::empty()).unwrap())
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    let me = send(
        &h.app,
        request("GET", "/api/me")
            .header(header::COOKIE, &cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(me.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn locked_account_refuses_right_password_and_looks_like_wrong_one() {
    let h = hub("lock").await;
    for _ in 0..5 {
        send(
            &h.app,
            login_request(EMAIL, "неверный-пароль-длинный", Some(ORIGIN)),
        )
        .await;
    }
    let locked = send(&h.app, login_request(EMAIL, PASSWORD, Some(ORIGIN))).await;
    assert_eq!(locked.status(), StatusCode::UNAUTHORIZED);
    let unknown = send(
        &h.app,
        login_request("nobody@example.org", PASSWORD, Some(ORIGIN)),
    )
    .await;
    assert_eq!(
        json(locked).await,
        json(unknown).await,
        "блокировка не должна выдавать, что почта есть"
    );
}

#[tokio::test]
async fn login_attempts_are_rate_limited_per_ip() {
    let h = hub("rate").await;
    for _ in 0..10 {
        send(
            &h.app,
            login_request("nobody@example.org", PASSWORD, Some(ORIGIN)),
        )
        .await;
    }
    let response = send(&h.app, login_request(EMAIL, PASSWORD, Some(ORIGIN))).await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(response.headers().contains_key(header::RETRY_AFTER));
}

#[tokio::test]
async fn oversized_body_is_refused() {
    let h = hub("body").await;
    let big = "ж".repeat(routes::MAX_BODY_BYTES);
    let response = send(&h.app, login_request(EMAIL, &big, Some(ORIGIN))).await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn builds_need_sign_in_and_download_only_listed_files() {
    let h = hub("builds").await;
    let unauth = send(
        &h.app,
        request("GET", "/api/builds").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(unauth.status(), StatusCode::UNAUTHORIZED);

    let artifact = Artifact {
        id: "20261028-120000-abc1234".into(),
        file: "nixyon-20261028-120000-abc1234.apk".into(),
        commit: "abc1234".into(),
        subject: "Проверка".into(),
        created_at: 1,
        size: 3,
        built_by: EMAIL.into(),
    };
    std::fs::write(
        h.artifacts.join(format!("{}.json", artifact.id)),
        serde_json::to_vec(&artifact).unwrap(),
    )
    .unwrap();
    std::fs::write(h.artifacts.join(&artifact.file), b"apk").unwrap();
    std::fs::write(h.artifacts.join("secret.txt"), b"secret").unwrap();

    let cookie = sign_in(&h.app).await;
    let list = send(
        &h.app,
        request("GET", "/api/builds")
            .header(header::COOKIE, &cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(json(list).await[0]["id"], artifact.id);

    let uri = format!("/api/builds/{}/apk", artifact.id);
    let file = send(
        &h.app,
        request("GET", &uri)
            .header(header::COOKIE, &cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(file.status(), StatusCode::OK);
    assert!(
        file.headers()[header::CONTENT_DISPOSITION]
            .to_str()
            .unwrap()
            .contains(&artifact.file)
    );
    assert_eq!(&to_bytes(file.into_body(), 1024).await.unwrap()[..], b"apk");

    let linked = Artifact {
        id: "20261028-120001-def5678".into(),
        file: "nixyon-20261028-120001-def5678.apk".into(),
        ..artifact.clone()
    };
    std::fs::write(
        h.artifacts.join(format!("{}.json", linked.id)),
        serde_json::to_vec(&linked).unwrap(),
    )
    .unwrap();
    std::os::unix::fs::symlink(
        h.artifacts.join("secret.txt"),
        h.artifacts.join(&linked.file),
    )
    .unwrap();
    let symlinked = format!("/api/builds/{}/apk", linked.id);
    for bad in [
        "/api/builds/secret.txt/apk",
        "/api/builds/..%2Fsecret.txt/apk",
        "/api/builds/nope/apk",
        symlinked.as_str(),
    ] {
        let response = send(
            &h.app,
            request("GET", bad)
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{bad}");
    }

    let qr = send(
        &h.app,
        request("GET", &format!("/api/builds/{}/qr.svg", artifact.id))
            .header(header::COOKIE, &cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(qr.headers()[header::CONTENT_TYPE], "image/svg+xml");
}

#[tokio::test]
async fn job_start_maps_agent_answers() {
    let h = hub("jobs").await;
    let cookie = sign_in(&h.app).await;
    let post = || {
        request("POST", "/api/jobs/build")
            .header(header::COOKIE, &cookie)
            .header(header::ORIGIN, ORIGIN)
            .body(Body::empty())
            .unwrap()
    };

    *h.reply.lock().unwrap() = Some(ServerFrame::Started {
        job: JobStatus {
            id: 7,
            kind: JobKind::Build,
            state: JobState::Running,
            started_by: EMAIL.into(),
            started_at: 1,
            finished_at: None,
            artifact: None,
        },
    });
    let ok = send(&h.app, post()).await;
    assert_eq!(ok.status(), StatusCode::OK);
    assert_eq!(json(ok).await["id"], 7);

    *h.reply.lock().unwrap() = Some(ServerFrame::Error {
        code: ErrorCode::Busy,
        message: "занято".into(),
    });
    assert_eq!(send(&h.app, post()).await.status(), StatusCode::CONFLICT);

    *h.reply.lock().unwrap() = None;
    assert_eq!(
        send(&h.app, post()).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );

    let no_origin = request("POST", "/api/jobs/build")
        .header(header::COOKIE, &cookie)
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        send(&h.app, no_origin).await.status(),
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn quality_check_starts_from_our_origin_and_reports_only_known_files() {
    let h = hub("quality").await;
    let cookie = sign_in(&h.app).await;
    *h.reply.lock().unwrap() = Some(ServerFrame::Started {
        job: JobStatus {
            id: 9,
            kind: JobKind::Quality,
            state: JobState::Queued,
            started_by: EMAIL.into(),
            started_at: 1,
            finished_at: None,
            artifact: None,
        },
    });
    let plan =
        r#"{"plan":[{"dataset":"cv21_test","minutes":30},{"dataset":"podlodka","minutes":null}]}"#;
    let post = |body: &'static str| {
        request("POST", "/api/jobs/quality")
            .header(header::COOKIE, &cookie)
            .header(header::ORIGIN, ORIGIN)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .unwrap()
    };
    let started = send(&h.app, post(plan)).await;
    assert_eq!(started.status(), StatusCode::OK);
    assert_eq!(json(started).await["kind"], "quality");
    assert_eq!(
        send(&h.app, post(r#"{"plan":"cv"}"#)).await.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    *h.reply.lock().unwrap() = Some(ServerFrame::Error {
        code: ErrorCode::BadRequest,
        message: "план проверки не подходит".into(),
    });
    assert_eq!(
        send(&h.app, post(plan)).await.status(),
        StatusCode::BAD_REQUEST
    );
    let no_origin = request("POST", "/api/jobs/quality")
        .header(header::COOKIE, &cookie)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(plan))
        .unwrap();
    assert_eq!(
        send(&h.app, no_origin).await.status(),
        StatusCode::FORBIDDEN
    );

    let get = |uri: &str, cookie: Option<&str>| {
        let mut b = request("GET", uri);
        if let Some(c) = cookie {
            b = b.header(header::COOKIE, c);
        }
        b.body(Body::empty()).unwrap()
    };
    assert_eq!(
        send(&h.app, get("/api/quality", None)).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        json(send(&h.app, get("/api/quality", Some(&cookie))).await).await,
        serde_json::json!([])
    );

    let report = h.artifacts.join("quality");
    std::fs::create_dir_all(&report).unwrap();
    std::fs::write(report.join("report.md"), "# Качество").unwrap();
    std::fs::write(report.join("notes.txt"), "x").unwrap();
    let listed = json(send(&h.app, get("/api/quality", Some(&cookie))).await).await;
    assert_eq!(listed[0]["name"], "report.md");
    assert_eq!(listed.as_array().unwrap().len(), 1);

    let file = send(&h.app, get("/api/quality/report.md", Some(&cookie))).await;
    assert_eq!(file.status(), StatusCode::OK);
    assert_eq!(
        file.headers()[header::CONTENT_DISPOSITION],
        "attachment; filename=\"nixyon-quality-report.md\""
    );
    assert_eq!(
        file.headers()[header::CONTENT_TYPE],
        "text/markdown; charset=utf-8"
    );
    assert_eq!(
        to_bytes(file.into_body(), 1 << 20).await.unwrap(),
        "# Качество".as_bytes()
    );
    for uri in [
        "/api/quality/notes.txt",
        "/api/quality/..%2Freport.md",
        "/api/quality/clips.json",
    ] {
        assert_eq!(
            send(&h.app, get(uri, Some(&cookie))).await.status(),
            StatusCode::NOT_FOUND,
            "{uri}"
        );
    }
}

#[tokio::test]
async fn clips_of_the_last_check_are_filtered_downloaded_and_played() {
    let h = hub("clips").await;
    let get = |uri: &str, cookie: Option<&str>| {
        let mut b = request("GET", uri);
        if let Some(c) = cookie {
            b = b.header(header::COOKIE, c);
        }
        b.body(Body::empty()).unwrap()
    };
    for uri in [
        "/api/quality/clip-sets",
        "/api/quality/clips/cv",
        "/api/quality/clips/cv/file",
        "/api/quality/audio/cv/0",
    ] {
        assert_eq!(
            send(&h.app, get(uri, None)).await.status(),
            StatusCode::UNAUTHORIZED,
            "{uri}"
        );
    }
    let cookie = sign_in(&h.app).await;
    let report = h.artifacts.join("quality");
    std::fs::create_dir_all(report.join("clips")).unwrap();
    std::fs::create_dir_all(report.join("audio").join("cv")).unwrap();
    std::fs::write(
        report.join("summary.json"),
        r#"{"datasets":[{"info":{"id":"cv","title":"Common Voice","has_text":true}}]}"#,
    )
    .unwrap();
    let clip = |id: &str, tags: &str| {
        format!(
            r#"{{"id":"{id}","duration_sec":2.0,"reference":"раз два","hypothesis":"раз","speakers":1,"word_errors":1,"words":2,"tags":{tags},"edits":[["=","раз"],["-","два"]],"audio":0}}"#
        )
    };
    std::fs::write(
        report.join("clips").join("cv.jsonl"),
        format!(
            "{}\n{}\n",
            clip("a", r#"["errors","cut_end"]"#),
            clip("b", "[]")
        ),
    )
    .unwrap();
    std::fs::write(report.join("audio").join("cv").join("0.wav"), b"RIFF").unwrap();

    let sets = json(send(&h.app, get("/api/quality/clip-sets", Some(&cookie))).await).await;
    assert_eq!(sets[0]["dataset"], "cv");
    let page = json(
        send(
            &h.app,
            get("/api/quality/clips/cv?tag=cut_end", Some(&cookie)),
        )
        .await,
    )
    .await;
    assert_eq!(page["total"], 1);
    assert_eq!(page["clips"][0]["id"], "a");
    assert_eq!(
        page["clips"][0]["edits"][1],
        serde_json::json!(["-", "два"])
    );
    for (uri, status) in [
        ("/api/quality/clips/cv?tag=nope", StatusCode::BAD_REQUEST),
        ("/api/quality/clips/nope", StatusCode::NOT_FOUND),
        ("/api/quality/clips/..%2Fcv", StatusCode::NOT_FOUND),
        ("/api/quality/audio/cv/1", StatusCode::NOT_FOUND),
    ] {
        assert_eq!(
            send(&h.app, get(uri, Some(&cookie))).await.status(),
            status,
            "{uri}"
        );
    }
    let long = format!("/api/quality/clips/cv?q={}", "a".repeat(201));
    assert_eq!(
        send(&h.app, get(&long, Some(&cookie))).await.status(),
        StatusCode::BAD_REQUEST
    );

    let file = send(&h.app, get("/api/quality/clips/cv/file", Some(&cookie))).await;
    assert_eq!(file.status(), StatusCode::OK);
    assert_eq!(
        file.headers()[header::CONTENT_DISPOSITION],
        "attachment; filename=\"nixyon-quality-clips-cv.jsonl\""
    );
    let sound = send(&h.app, get("/api/quality/audio/cv/0", Some(&cookie))).await;
    assert_eq!(sound.status(), StatusCode::OK);
    assert_eq!(sound.headers()[header::CONTENT_TYPE], "audio/wav");
    let csp = sound.headers()["content-security-policy"]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(csp.contains("media-src 'self' blob:"), "{csp}");
    assert_eq!(
        to_bytes(sound.into_body(), 1 << 20).await.unwrap(),
        "RIFF".as_bytes()
    );
}

#[tokio::test]
async fn past_quality_runs_are_listed_newest_first() {
    let h = hub("runs").await;
    let get = |cookie: Option<&str>| {
        let mut b = request("GET", "/api/quality/runs");
        if let Some(c) = cookie {
            b = b.header(header::COOKIE, c);
        }
        b.body(Body::empty()).unwrap()
    };
    assert_eq!(
        send(&h.app, get(None)).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let cookie = sign_in(&h.app).await;
    assert_eq!(
        json(send(&h.app, get(Some(&cookie))).await).await,
        serde_json::json!([])
    );
    let group = r#"{"label":"g","clips":1,"audio_sec":1.0,"wer":{"errors":1,"reference":10,"rate":0.1},"cer":{"errors":0,"reference":0,"rate":null},"empty":0}"#;
    for (id, at) in [
        ("1761900000-91ecf11", 1_761_900_000),
        ("1761990000", 1_761_990_000),
    ] {
        let dir = h.artifacts.join("quality-history").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("summary.json"),
            format!(
                r#"{{"models_version":"m","finished_at":{at},"audio_sec":1.0,"wall_sec":1.0,"totals":{{"human":null,"all":null}},
                "datasets":[{{"info":{{"id":"cv","title":"CV","has_text":true}},"summary":{{"overall":{group},"by_duration":[],"with_digits":{group},"without_digits":{group}}}}}]}}"#
            ),
        )
        .unwrap();
    }
    let runs = json(send(&h.app, get(Some(&cookie))).await).await;
    assert_eq!(runs[0]["id"], "1761990000");
    assert_eq!(runs[1]["id"], "1761900000-91ecf11");
    assert_eq!(
        runs[1]["commit"],
        serde_json::Value::Null,
        "коммит берётся из итогов"
    );
    assert_eq!(runs[1]["datasets"][0]["reference"], "unknown");
}

#[tokio::test]
async fn clips_with_a_bad_reference_are_marked_and_left_out_of_the_next_check() {
    let h = hub("marks").await;
    let call = |method: &str, uri: &str, cookie: Option<&str>, origin: bool| {
        let mut b = request(method, uri);
        if let Some(c) = cookie {
            b = b.header(header::COOKIE, c);
        }
        if origin {
            b = b.header(header::ORIGIN, ORIGIN);
        }
        b.body(Body::empty()).unwrap()
    };
    assert_eq!(
        send(&h.app, call("GET", "/api/quality/marks/cv", None, false))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let cookie = sign_in(&h.app).await;
    let mark = |clip: &str| {
        call(
            "POST",
            &format!("/api/quality/marks/cv/{clip}"),
            Some(&cookie),
            true,
        )
    };
    assert_eq!(
        send(&h.app, mark("a")).await.status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        send(&h.app, mark("b")).await.status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        send(&h.app, mark("..%2Fx")).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(
            &h.app,
            call("POST", "/api/quality/marks/cv/c", Some(&cookie), false)
        )
        .await
        .status(),
        StatusCode::FORBIDDEN,
        "без своего Origin"
    );
    assert_eq!(
        send(
            &h.app,
            call("DELETE", "/api/quality/marks/cv/b", Some(&cookie), true)
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    let marks = json(
        send(
            &h.app,
            call("GET", "/api/quality/marks/cv", Some(&cookie), false),
        )
        .await,
    )
    .await;
    assert_eq!(marks.as_array().unwrap().len(), 1);
    assert_eq!(marks[0]["clip"], "a");
    assert_eq!(marks[0]["marked_by"], EMAIL);

    *h.reply.lock().unwrap() = Some(ServerFrame::Started {
        job: JobStatus {
            id: 3,
            kind: JobKind::Quality,
            state: JobState::Queued,
            started_by: EMAIL.into(),
            started_at: 1,
            finished_at: None,
            artifact: None,
        },
    });
    let started = request("POST", "/api/jobs/quality")
        .header(header::COOKIE, &cookie)
        .header(header::ORIGIN, ORIGIN)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            r#"{"plan":[{"dataset":"cv","minutes":null,"exclude":["z"]},{"dataset":"talk","minutes":5}]}"#,
        ))
        .unwrap();
    assert_eq!(send(&h.app, started).await.status(), StatusCode::OK);
    let sent = h.seen.lock().unwrap().last().cloned().unwrap();
    let sent: serde_json::Value = serde_json::from_str(sent.trim()).unwrap();
    assert_eq!(
        sent["plan"][0]["exclude"],
        serde_json::json!(["a"]),
        "список из пометок хаба, а не со страницы: {sent}"
    );
    assert!(sent["plan"][1].get("exclude").is_none(), "{sent}");
}

#[tokio::test]
async fn datasets_come_from_the_agent_with_speeds_of_the_last_check() {
    let h = hub("datasets").await;
    let get = |cookie: Option<&str>| {
        let mut b = request("GET", "/api/quality/datasets");
        if let Some(c) = cookie {
            b = b.header(header::COOKIE, c);
        }
        b.body(Body::empty()).unwrap()
    };
    assert_eq!(
        send(&h.app, get(None)).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let cookie = sign_in(&h.app).await;
    *h.reply.lock().unwrap() = Some(ServerFrame::Datasets {
        datasets: vec![DatasetSummary {
            id: "cv21_test".into(),
            title: "Common Voice".into(),
            kind: "single".into(),
            has_text: true,
            reference: "human".into(),
            license: "CC0".into(),
            clips: 23_219,
            audio_sec: 118_746,
            default_selected: true,
        }],
    });
    let report = h.artifacts.join("quality");
    std::fs::create_dir_all(&report).unwrap();
    std::fs::write(
        report.join("summary.json"),
        r#"{"audio_sec":5000.0,"wall_sec":100.0,"datasets":[{"info":{"id":"cv21_test"},"wall_sec":50.0,"summary":{"overall":{"audio_sec":2600.0}}}]}"#,
    )
    .unwrap();
    let view = json(send(&h.app, get(Some(&cookie))).await).await;
    assert_eq!(view["datasets"][0]["id"], "cv21_test");
    assert_eq!(view["datasets"][0]["audio_sec"], 118_746);
    assert_eq!(view["speeds"]["overall"], 50.0);
    assert_eq!(view["speeds"]["datasets"]["cv21_test"], 52.0);

    *h.reply.lock().unwrap() = None;
    assert_eq!(
        send(&h.app, get(Some(&cookie))).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[tokio::test]
async fn websockets_need_sign_in_and_our_origin() {
    let h = hub("ws").await;
    let cookie = sign_in(&h.app).await;
    let upgrade = |origin: &str, cookie: Option<&str>| {
        let mut b = request("GET", "/api/terminal")
            .header(header::CONNECTION, "upgrade")
            .header(header::UPGRADE, "websocket")
            .header(header::SEC_WEBSOCKET_VERSION, "13")
            .header(header::SEC_WEBSOCKET_KEY, "dGhlIHNhbXBsZSBub25jZQ==")
            .header(header::ORIGIN, origin);
        if let Some(c) = cookie {
            b = b.header(header::COOKIE, c);
        }
        b.body(Body::empty()).unwrap()
    };
    assert_eq!(
        send(&h.app, upgrade(ORIGIN, None)).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(&h.app, upgrade("https://evil.example", Some(&cookie)))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn long_lived_connections_see_logout_and_removed_user() {
    let conn = db::open_in_memory().unwrap();
    let user = users::add(&conn, EMAIL, PASSWORD).unwrap();
    let origin = nixyon_hub::sessions::Origin {
        ip: "1.2.3.4",
        user_agent: "test",
    };
    let token =
        nixyon_hub::sessions::create(&conn, user.id, &origin, nixyon_hub::clock::now()).unwrap();
    let config = HubConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        db_path: PathBuf::from(":memory:"),
        public_origin: ORIGIN.into(),
        agent_addr: "127.0.0.1:1".into(),
        agent_token: TOKEN.into(),
        artifacts_dir: std::env::temp_dir(),
        trusted_proxies: Vec::new(),
        ci_reports_dir: None,
        models_dir: None,
    };
    let state = AppState::new(conn, config);
    assert!(nixyon_hub::auth::still_signed_in(&state, &token).await);
    let removed = state
        .with_db(|conn| conn.execute("DELETE FROM users", []))
        .await
        .unwrap();
    assert_eq!(removed, 1);
    assert!(!nixyon_hub::auth::still_signed_in(&state, &token).await);
}

#[tokio::test]
async fn one_user_may_download_in_parallel_up_to_the_limit_until_files_are_sent() {
    let h = hub("slots").await;
    let artifact = Artifact {
        id: "20261028-130000-aaa1111".into(),
        file: "nixyon-20261028-130000-aaa1111.apk".into(),
        commit: "aaa1111".into(),
        subject: "s".into(),
        created_at: 1,
        size: 3,
        built_by: EMAIL.into(),
    };
    std::fs::write(
        h.artifacts.join(format!("{}.json", artifact.id)),
        serde_json::to_vec(&artifact).unwrap(),
    )
    .unwrap();
    std::fs::write(h.artifacts.join(&artifact.file), b"apk").unwrap();
    let cookie = sign_in(&h.app).await;
    let uri = format!("/api/builds/{}/apk", artifact.id);
    let get = || {
        request("GET", &uri)
            .header(header::COOKIE, &cookie)
            .body(Body::empty())
            .unwrap()
    };

    let mut open = Vec::new();
    for _ in 0..32 {
        let response = send(&h.app, get()).await;
        assert_eq!(response.status(), StatusCode::OK);
        open.push(response);
    }
    let refused = send(&h.app, get()).await;
    assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(json(refused).await["error"]["code"], "download_in_progress");

    let first = open.remove(0);
    assert_eq!(
        &to_bytes(first.into_body(), 1024).await.unwrap()[..],
        b"apk"
    );
    assert_eq!(send(&h.app, get()).await.status(), StatusCode::OK);
}

#[tokio::test]
async fn apk_is_sent_in_parts_on_range_requests() {
    let h = hub("range").await;
    let artifact = Artifact {
        id: "20261028-150000-ccc3333".into(),
        file: "nixyon-20261028-150000-ccc3333.apk".into(),
        commit: "ccc3333".into(),
        subject: "s".into(),
        created_at: 1,
        size: 10,
        built_by: EMAIL.into(),
    };
    std::fs::write(
        h.artifacts.join(format!("{}.json", artifact.id)),
        serde_json::to_vec(&artifact).unwrap(),
    )
    .unwrap();
    std::fs::write(h.artifacts.join(&artifact.file), b"0123456789").unwrap();
    std::fs::write(h.artifacts.join(format!("{}.gz", artifact.file)), b"packed").unwrap();
    let cookie = sign_in(&h.app).await;
    let get = |extra: &[(header::HeaderName, &str)]| {
        let mut b = request("GET", &format!("/api/builds/{}/apk", artifact.id))
            .header(header::COOKIE, &cookie);
        for (name, value) in extra {
            b = b.header(name, *value);
        }
        b.body(Body::empty()).unwrap()
    };

    for (range, body, content_range) in [
        ("bytes=2-5", &b"2345"[..], "bytes 2-5/10"),
        ("bytes=7-", &b"789"[..], "bytes 7-9/10"),
        ("bytes=-3", &b"789"[..], "bytes 7-9/10"),
        ("bytes=0-0", &b"0"[..], "bytes 0-0/10"),
    ] {
        // Клиент принимает gzip, но с Range отдаётся несжатый файл.
        let part = send(
            &h.app,
            get(&[(header::RANGE, range), (header::ACCEPT_ENCODING, "gzip")]),
        )
        .await;
        assert_eq!(part.status(), StatusCode::PARTIAL_CONTENT, "{range}");
        assert_eq!(part.headers()[header::CONTENT_RANGE], content_range);
        assert_eq!(part.headers()[header::ACCEPT_RANGES], "bytes");
        assert!(part.headers().get(header::CONTENT_ENCODING).is_none());
        assert_eq!(
            part.headers()[header::CONTENT_LENGTH],
            body.len().to_string().as_str()
        );
        assert_eq!(&to_bytes(part.into_body(), 1024).await.unwrap()[..], body);
    }

    let past_end = send(&h.app, get(&[(header::RANGE, "bytes=10-")])).await;
    assert_eq!(past_end.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(past_end.headers()[header::CONTENT_RANGE], "bytes */10");

    // Непонятый Range и If-Range дают файл целиком.
    for extra in [
        vec![(header::RANGE, "bytes=0-1,4-5")],
        vec![(header::RANGE, "bytes=2-5"), (header::IF_RANGE, "\"x\"")],
    ] {
        let whole = send(&h.app, get(&extra)).await;
        assert_eq!(whole.status(), StatusCode::OK);
        assert_eq!(
            &to_bytes(whole.into_body(), 1024).await.unwrap()[..],
            b"0123456789"
        );
    }

    let plain = send(&h.app, get(&[])).await;
    assert_eq!(plain.headers()[header::ACCEPT_RANGES], "bytes");
    let packed = send(&h.app, get(&[(header::ACCEPT_ENCODING, "gzip")])).await;
    assert_eq!(packed.headers()[header::CONTENT_ENCODING], "gzip");
    assert_eq!(packed.headers()[header::ACCEPT_RANGES], "none");
}

#[tokio::test]
async fn status_shows_current_job_and_queue() {
    let h = hub("queue").await;
    let cookie = sign_in(&h.app).await;
    let job = |id, state| JobStatus {
        id,
        kind: JobKind::Build,
        state,
        started_by: EMAIL.into(),
        started_at: 1,
        finished_at: None,
        artifact: None,
    };
    *h.reply.lock().unwrap() = Some(ServerFrame::Status {
        job: Some(job(4, JobState::Running)),
        queue: vec![job(5, JobState::Queued)],
    });
    let body = json(
        send(
            &h.app,
            request("GET", "/api/jobs/status")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(body["job"]["id"], 4);
    assert_eq!(body["queue"][0]["state"], "queued");
}

#[tokio::test]
async fn history_ci_and_stats_need_sign_in_and_read_saved_files() {
    let h = hub("history").await;
    for uri in [
        "/api/jobs/history",
        "/api/jobs/1/log",
        "/api/ci/runs",
        "/api/ci/runs/9-1",
        "/api/ci/runs/9-1/steps/1/log",
        "/api/stats",
    ] {
        let r = send(&h.app, request("GET", uri).body(Body::empty()).unwrap()).await;
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED, "{uri}");
    }

    let jobs = h.artifacts.join("jobs");
    std::fs::create_dir_all(&jobs).unwrap();
    let saved = JobStatus {
        id: 1,
        kind: JobKind::Build,
        state: JobState::Succeeded,
        started_by: EMAIL.into(),
        started_at: 100,
        finished_at: Some(160),
        artifact: None,
    };
    std::fs::write(jobs.join("1.json"), serde_json::to_vec(&saved).unwrap()).unwrap();
    std::fs::write(jobs.join("1.log"), "$ cargo\nготово\n").unwrap();
    let run = h.artifacts.join("ci/9-1");
    std::fs::create_dir_all(run.join("steps")).unwrap();
    std::fs::write(
        run.join("run.json"),
        r#"{"run_id":"9","attempt":1,"sha":"abc","branch":"main","actor":"andre","subject":"s","url":"https://github.com/a/b/actions/runs/9","started_at":100}"#,
    )
    .unwrap();
    std::fs::write(
        run.join("steps/1.json"),
        r#"{"n":1,"key":"test","title":"Тесты","started_at":100,"finished_at":150,"exit_code":101}"#,
    )
    .unwrap();
    std::fs::write(run.join("steps/1.log"), "упал тест\n").unwrap();
    std::fs::write(
        run.join("finish.json"),
        r#"{"conclusion":"failure","finished_at":155}"#,
    )
    .unwrap();

    let cookie = sign_in(&h.app).await;
    let get = |uri: &str| {
        request("GET", uri)
            .header(header::COOKIE, &cookie)
            .body(Body::empty())
            .unwrap()
    };
    assert_eq!(
        json(send(&h.app, get("/api/jobs/history")).await).await[0]["id"],
        1
    );
    assert_eq!(
        json(send(&h.app, get("/api/jobs/1/log")).await).await["text"],
        "$ cargo\nготово\n"
    );
    assert_eq!(
        send(&h.app, get("/api/jobs/2/log")).await.status(),
        StatusCode::NOT_FOUND
    );
    let runs = json(send(&h.app, get("/api/ci/runs")).await).await;
    assert_eq!(runs[0]["status"], "failure");
    assert_eq!(runs[0]["failed_step"], "Тесты");
    assert_eq!(
        json(send(&h.app, get("/api/ci/runs/9-1/steps/1/log")).await).await["text"],
        "упал тест\n"
    );
    assert_eq!(
        send(&h.app, get("/api/ci/runs/..%2F9-1")).await.status(),
        StatusCode::NOT_FOUND
    );
    let stats = json(send(&h.app, get("/api/stats")).await).await;
    assert_eq!(stats["builds"][0]["seconds"], 60);
    assert_eq!(stats["test_runs"][0]["ok"], false);
    assert_eq!(stats["slowest_steps"][0]["key"], "test");
}

#[tokio::test]
async fn compressed_apk_is_sent_to_clients_that_accept_gzip() {
    let h = hub("gzip").await;
    let artifact = Artifact {
        id: "20261028-140000-bbb2222".into(),
        file: "nixyon-20261028-140000-bbb2222.apk".into(),
        commit: "bbb2222".into(),
        subject: "s".into(),
        created_at: 1,
        size: 3,
        built_by: EMAIL.into(),
    };
    std::fs::write(
        h.artifacts.join(format!("{}.json", artifact.id)),
        serde_json::to_vec(&artifact).unwrap(),
    )
    .unwrap();
    std::fs::write(h.artifacts.join(&artifact.file), b"apk").unwrap();
    std::fs::write(h.artifacts.join(format!("{}.gz", artifact.file)), b"packed").unwrap();
    let cookie = sign_in(&h.app).await;
    let get = |encoding: Option<&str>| {
        let mut b = request("GET", &format!("/api/builds/{}/apk", artifact.id))
            .header(header::COOKIE, &cookie);
        if let Some(e) = encoding {
            b = b.header(header::ACCEPT_ENCODING, e);
        }
        b.body(Body::empty()).unwrap()
    };
    let packed = send(&h.app, get(Some("br, gzip"))).await;
    assert_eq!(packed.headers()[header::CONTENT_ENCODING], "gzip");
    assert_eq!(packed.headers()[header::VARY], "accept-encoding");
    assert_eq!(
        &to_bytes(packed.into_body(), 1024).await.unwrap()[..],
        b"packed"
    );
    for refused in [None, Some("gzip;q=0"), Some("br")] {
        let plain = send(&h.app, get(refused)).await;
        assert!(plain.headers().get(header::CONTENT_ENCODING).is_none());
        assert_eq!(
            &to_bytes(plain.into_body(), 1024).await.unwrap()[..],
            b"apk"
        );
    }
}

#[tokio::test]
async fn live_eval_starts_from_our_origin_and_sends_records_to_the_agent() {
    let h = hub("live-eval").await;
    let body = r#"{"records":["55342","dialogue"]}"#;
    let post = |cookie: Option<&str>, origin: Option<&str>| {
        let mut b =
            request("POST", "/api/jobs/live-eval").header(header::CONTENT_TYPE, "application/json");
        if let Some(c) = cookie {
            b = b.header(header::COOKIE, c);
        }
        if let Some(o) = origin {
            b = b.header(header::ORIGIN, o);
        }
        b.body(Body::from(body)).unwrap()
    };
    assert_eq!(
        send(&h.app, post(None, Some(ORIGIN))).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let cookie = sign_in(&h.app).await;
    for origin in [None, Some("https://evil.test")] {
        assert_eq!(
            send(&h.app, post(Some(&cookie), origin)).await.status(),
            StatusCode::FORBIDDEN,
            "{origin:?}"
        );
    }
    assert!(
        h.seen.lock().unwrap().is_empty(),
        "без входа и Origin агент не нужен"
    );

    *h.reply.lock().unwrap() = Some(ServerFrame::Started {
        job: JobStatus {
            id: 12,
            kind: JobKind::LiveEval,
            state: JobState::Running,
            started_by: EMAIL.into(),
            started_at: 1,
            finished_at: None,
            artifact: None,
        },
    });
    let started = send(&h.app, post(Some(&cookie), Some(ORIGIN))).await;
    assert_eq!(started.status(), StatusCode::OK);
    assert_eq!(json(started).await["kind"], "live_eval");
    let sent: serde_json::Value =
        serde_json::from_str(h.seen.lock().unwrap().last().unwrap()).unwrap();
    assert_eq!(sent["op"], "live_eval");
    assert_eq!(sent["records"], serde_json::json!(["55342", "dialogue"]));
    assert_eq!(sent["user"], EMAIL);

    *h.reply.lock().unwrap() = Some(ServerFrame::Error {
        code: ErrorCode::BadRequest,
        message: "записи не подходят: записи x нет на сервере".into(),
    });
    let refused = send(&h.app, post(Some(&cookie), Some(ORIGIN))).await;
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json(refused).await["error"]["code"], "bad_records");
}

#[tokio::test]
async fn experiment_files_need_sign_in_and_only_known_files_are_sent() {
    let h = hub("experiments").await;
    let exp = h.artifacts.join("experiments/12");
    std::fs::create_dir_all(exp.join("B")).unwrap();
    std::fs::write(exp.join("runs.jsonl"), "{\"combo\":\"B\"}\n").unwrap();
    std::fs::write(exp.join("B/55342.json"), "{\"turns\":[]}").unwrap();
    std::fs::write(exp.join("B/notes.txt"), "x").unwrap();
    let get = |uri: &str, cookie: Option<&str>| {
        let mut b = request("GET", uri);
        if let Some(c) = cookie {
            b = b.header(header::COOKIE, c);
        }
        b.body(Body::empty()).unwrap()
    };
    for uri in [
        "/api/experiments",
        "/api/experiments/12",
        "/api/experiments/12/runs.jsonl",
        "/api/experiments/12/B/55342.json",
    ] {
        assert_eq!(
            send(&h.app, get(uri, None)).await.status(),
            StatusCode::UNAUTHORIZED,
            "{uri}"
        );
    }
    let cookie = sign_in(&h.app).await;
    assert_eq!(
        json(send(&h.app, get("/api/experiments", Some(&cookie))).await).await,
        serde_json::json!([12])
    );
    let files = json(send(&h.app, get("/api/experiments/12", Some(&cookie))).await).await;
    let paths: Vec<&str> = files
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, ["runs.jsonl", "B/55342.json"]);

    let dump = send(
        &h.app,
        get("/api/experiments/12/B/55342.json", Some(&cookie)),
    )
    .await;
    assert_eq!(dump.status(), StatusCode::OK);
    assert_eq!(
        dump.headers()[header::CONTENT_DISPOSITION],
        "attachment; filename=\"nixyon-experiment-12-B-55342.json\""
    );
    assert_eq!(
        to_bytes(dump.into_body(), 1 << 20).await.unwrap(),
        "{\"turns\":[]}".as_bytes()
    );
    let runs = send(&h.app, get("/api/experiments/12/runs.jsonl", Some(&cookie))).await;
    assert_eq!(runs.status(), StatusCode::OK);
    // Место скачивания освобождается, когда файл дочитан.
    to_bytes(runs.into_body(), 1 << 20).await.unwrap();
    for uri in [
        "/api/experiments/13",
        "/api/experiments/..",
        "/api/experiments/12/notes.txt",
        "/api/experiments/12/B/notes.txt",
        "/api/experiments/12/E/55342.json",
        "/api/experiments/12/B/..%2F..%2Fruns.jsonl",
        "/api/experiments/12/..%2F12/runs.jsonl",
    ] {
        assert_eq!(
            send(&h.app, get(uri, Some(&cookie))).await.status(),
            StatusCode::NOT_FOUND,
            "{uri}"
        );
    }
}

#[tokio::test]
async fn cli_starts_live_eval_for_a_known_user_and_shows_the_agent_reason_on_refusal() {
    let reply: Reply = Arc::new(Mutex::new(None));
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let agent_addr = fake_agent(Arc::clone(&reply), Arc::clone(&seen)).await;
    let db_path = std::env::temp_dir().join(format!("nixyon-cli-live-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&db_path);
    users::add(&db::open(&db_path).unwrap(), EMAIL, PASSWORD).unwrap();
    let config = HubConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        db_path: db_path.clone(),
        public_origin: ORIGIN.into(),
        agent_addr,
        agent_token: TOKEN.into(),
        artifacts_dir: std::env::temp_dir(),
        trusted_proxies: Vec::new(),
        ci_reports_dir: None,
        models_dir: None,
    };
    let records = || vec!["55342".to_string()];
    let unknown = nixyon_hub::cli::run_live_eval(&config, "nobody@example.org", records()).await;
    assert!(unknown.unwrap_err().to_string().contains("нет такого"));
    assert!(seen.lock().unwrap().is_empty());

    *reply.lock().unwrap() = Some(ServerFrame::Error {
        code: ErrorCode::BadRequest,
        message: "записи не подходят: записи 55342 нет на сервере".into(),
    });
    let refused = nixyon_hub::cli::run_live_eval(&config, EMAIL, records()).await;
    assert!(
        refused
            .unwrap_err()
            .to_string()
            .contains("55342 нет на сервере")
    );

    *reply.lock().unwrap() = Some(ServerFrame::Started {
        job: JobStatus {
            id: 3,
            kind: JobKind::LiveEval,
            state: JobState::Queued,
            started_by: EMAIL.into(),
            started_at: 1,
            finished_at: None,
            artifact: None,
        },
    });
    let started = nixyon_hub::cli::run_live_eval(&config, EMAIL, records()).await;
    assert_eq!(started.unwrap(), "задание 3: ждёт в очереди");
    let conn = db::open(&db_path).unwrap();
    let detail: String = conn
        .query_row(
            "SELECT detail FROM events WHERE kind = 'live_eval_started'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(detail, "3");
    std::fs::remove_file(&db_path).unwrap();
}

// Как request, но с заданным адресом клиента за прокси.
fn model_request(uri: &str, ip: &str) -> axum::http::request::Builder {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header("x-forwarded-for", ip)
        .extension(axum::extract::ConnectInfo(SocketAddr::from((
            [10, 0, 0, 1],
            5555,
        ))))
}

async fn body_bytes(response: axum::response::Response) -> Vec<u8> {
    to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap()
        .to_vec()
}

#[tokio::test]
async fn model_mirror_is_open_without_sign_in_and_sends_whole_files_and_ranges() {
    let hub = hub("models").await;
    let mirror = hub.artifacts.join("models");
    std::fs::create_dir_all(&mirror).unwrap();
    let sha = "a".repeat(64);
    std::fs::write(mirror.join(&sha), b"0123456789").unwrap();
    std::fs::write(mirror.join(format!(".{}.tmp", "b".repeat(64))), b"half").unwrap();
    std::os::unix::fs::symlink(hub.artifacts.join("secret"), mirror.join("c".repeat(64))).unwrap();
    std::fs::write(hub.artifacts.join("secret"), b"not a model").unwrap();
    let ip = "198.51.100.1";

    let listed = send(
        &hub.app,
        model_request("/models/", ip).body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(listed.status(), StatusCode::OK);
    assert_eq!(
        json(listed).await,
        serde_json::json!([{ "sha256": sha, "size": 10 }])
    );

    let whole = send(
        &hub.app,
        model_request(&format!("/models/{sha}"), ip)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(whole.status(), StatusCode::OK);
    assert_eq!(whole.headers()[header::CONTENT_LENGTH], "10");
    assert_eq!(whole.headers()[header::ACCEPT_RANGES], "bytes");
    assert!(
        whole.headers()[header::CACHE_CONTROL]
            .to_str()
            .unwrap()
            .contains("immutable")
    );
    assert_eq!(body_bytes(whole).await, b"0123456789");

    // Скрипт загрузки узнаёт длину файла запросом HEAD.
    let head = send(
        &hub.app,
        model_request(&format!("/models/{sha}"), ip)
            .method("HEAD")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(head.status(), StatusCode::OK);
    assert_eq!(head.headers()[header::CONTENT_LENGTH], "10");
    assert!(body_bytes(head).await.is_empty());

    let part = send(
        &hub.app,
        model_request(&format!("/models/{sha}"), ip)
            .header(header::RANGE, "bytes=3-5")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(part.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(part.headers()[header::CONTENT_RANGE], "bytes 3-5/10");
    assert_eq!(part.headers()[header::CONTENT_LENGTH], "3");
    assert_eq!(body_bytes(part).await, b"345");

    let past = send(
        &hub.app,
        model_request(&format!("/models/{sha}"), ip)
            .header(header::RANGE, "bytes=10-")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(past.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(past.headers()[header::CONTENT_RANGE], "bytes */10");

    for name in [
        "b".repeat(64),
        "c".repeat(64),
        "d".repeat(64),
        "A".repeat(64),
        "secret".into(),
        "..%2Fsecret".into(),
    ] {
        let response = send(
            &hub.app,
            model_request(&format!("/models/{name}"), ip)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{name}");
    }
    std::fs::remove_dir_all(&hub.artifacts).unwrap();
}

#[tokio::test]
async fn one_address_gets_up_to_the_per_address_limit_of_model_downloads_at_a_time() {
    let hub = hub("models-slots").await;
    let mirror = hub.artifacts.join("models");
    std::fs::create_dir_all(&mirror).unwrap();
    let sha = "e".repeat(64);
    std::fs::write(mirror.join(&sha), b"model").unwrap();
    let get = |ip: &'static str| {
        model_request(&format!("/models/{sha}"), ip)
            .body(Body::empty())
            .unwrap()
    };

    // Тело не прочитано: место занято, пока ответ жив.
    let mut open = Vec::new();
    for _ in 0..MODEL_DOWNLOADS_PER_IP {
        let response = send(&hub.app, get("198.51.100.2")).await;
        assert_eq!(response.status(), StatusCode::OK);
        open.push(response);
    }
    let refused = send(&hub.app, get("198.51.100.2")).await;
    assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        send(&hub.app, get("198.51.100.3")).await.status(),
        StatusCode::OK
    );
    open.pop();
    assert_eq!(
        send(&hub.app, get("198.51.100.2")).await.status(),
        StatusCode::OK
    );
    std::fs::remove_dir_all(&hub.artifacts).unwrap();
}

#[tokio::test]
async fn training_starts_from_our_origin_and_sends_the_sets_to_the_agent() {
    let h = hub("training-start").await;
    let body = r#"{"sets":["speechocean762","samromur_children"]}"#;
    let post = |cookie: Option<&str>, origin: Option<&str>| {
        let mut b =
            request("POST", "/api/jobs/training").header(header::CONTENT_TYPE, "application/json");
        if let Some(c) = cookie {
            b = b.header(header::COOKIE, c);
        }
        if let Some(o) = origin {
            b = b.header(header::ORIGIN, o);
        }
        b.body(Body::from(body)).unwrap()
    };
    assert_eq!(
        send(&h.app, post(None, Some(ORIGIN))).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let cookie = sign_in(&h.app).await;
    for origin in [None, Some("https://evil.test")] {
        assert_eq!(
            send(&h.app, post(Some(&cookie), origin)).await.status(),
            StatusCode::FORBIDDEN,
            "{origin:?}"
        );
    }
    assert!(
        h.seen.lock().unwrap().is_empty(),
        "без входа и Origin агент не нужен"
    );

    *h.reply.lock().unwrap() = Some(ServerFrame::Started {
        job: JobStatus {
            id: 21,
            kind: JobKind::Training,
            state: JobState::Queued,
            started_by: EMAIL.into(),
            started_at: 1,
            finished_at: None,
            artifact: None,
        },
    });
    let started = send(&h.app, post(Some(&cookie), Some(ORIGIN))).await;
    assert_eq!(started.status(), StatusCode::OK);
    assert_eq!(json(started).await["kind"], "training");
    let sent: serde_json::Value =
        serde_json::from_str(h.seen.lock().unwrap().last().unwrap()).unwrap();
    assert_eq!(sent["op"], "training");
    assert_eq!(
        sent["training"],
        serde_json::json!(["speechocean762", "samromur_children"])
    );
    assert_eq!(sent["user"], EMAIL);

    *h.reply.lock().unwrap() = Some(ServerFrame::Error {
        code: ErrorCode::BadRequest,
        message: "наборы для обучения не подходят: набора sps_ru нет на сервере".into(),
    });
    let refused = send(&h.app, post(Some(&cookie), Some(ORIGIN))).await;
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json(refused).await["error"]["code"], "bad_training");
}

#[tokio::test]
async fn training_files_need_sign_in_and_only_reports_and_weights_are_sent() {
    let h = hub("training-files").await;
    let run = h.artifacts.join("training/21");
    std::fs::create_dir_all(&run).unwrap();
    std::fs::write(run.join("report.md"), "# Ребёнок или взрослый\n").unwrap();
    std::fs::write(run.join("child_adult_redimnet2.json"), "{\"bias\":0}").unwrap();
    std::fs::write(run.join("features.json"), "[]").unwrap();
    let get = |uri: &str, cookie: Option<&str>| {
        let mut b = request("GET", uri);
        if let Some(c) = cookie {
            b = b.header(header::COOKIE, c);
        }
        b.body(Body::empty()).unwrap()
    };
    for uri in [
        "/api/training",
        "/api/training/21",
        "/api/training/21/report.md",
    ] {
        assert_eq!(
            send(&h.app, get(uri, None)).await.status(),
            StatusCode::UNAUTHORIZED,
            "{uri}"
        );
    }
    let cookie = sign_in(&h.app).await;
    assert_eq!(
        json(send(&h.app, get("/api/training", Some(&cookie))).await).await,
        serde_json::json!([21])
    );
    let files = json(send(&h.app, get("/api/training/21", Some(&cookie))).await).await;
    let names: Vec<&str> = files
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["report.md", "child_adult_redimnet2.json"]);

    let weights = send(
        &h.app,
        get("/api/training/21/child_adult_redimnet2.json", Some(&cookie)),
    )
    .await;
    assert_eq!(weights.status(), StatusCode::OK);
    assert_eq!(
        weights.headers()[header::CONTENT_DISPOSITION],
        "attachment; filename=\"nixyon-training-21-child_adult_redimnet2.json\""
    );
    assert_eq!(
        to_bytes(weights.into_body(), 1 << 20).await.unwrap(),
        "{\"bias\":0}".as_bytes()
    );
    for uri in [
        "/api/training/22",
        "/api/training/..",
        "/api/training/21/features.json",
        "/api/training/21/..%2Freport.md",
    ] {
        assert_eq!(
            send(&h.app, get(uri, Some(&cookie))).await.status(),
            StatusCode::NOT_FOUND,
            "{uri}"
        );
    }
}
