use std::net::SocketAddr;
use std::process::ExitCode;
use std::time::Duration;

use nixyon_hub::cli::{self, Command};
use nixyon_hub::config::HubConfig;
use nixyon_hub::state::AppState;
use nixyon_hub::{clock, db, routes, sessions};

const CLEANUP_EVERY: Duration = Duration::from_secs(3600);

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = match cli::parse(&args) {
        Ok(c) => c,
        Err(usage) => {
            eprintln!("{usage}");
            return ExitCode::from(2);
        }
    };
    let result = match command {
        Command::Serve => serve(),
        Command::LiveEval { email, records } => live_eval(&email, records),
        Command::User(user) => {
            let db_path = std::env::var("NIXYON_HUB_DB").unwrap_or_else(|_| "nixyon-hub.db".into());
            cli::run_user(db_path.as_ref(), user, &mut std::io::stdin().lock())
                .map(|out| println!("{out}"))
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ошибка: {e}");
            ExitCode::FAILURE
        }
    }
}

fn live_eval(email: &str, records: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    let config = HubConfig::from_env()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let out = runtime.block_on(cli::run_live_eval(&config, email, records))?;
    println!("{out}");
    Ok(())
}

fn serve() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().json().with_target(false).init();
    let config = HubConfig::from_env()?;
    let conn = db::open(&config.db_path)?;
    let listen = config.listen;
    let state = AppState::new(conn, config);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let cleanup_state = state.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(CLEANUP_EVERY).await;
                let removed = cleanup_state
                    .with_db(|conn| sessions::delete_expired(conn, clock::now()))
                    .await;
                if let Ok(n) = removed {
                    tracing::info!(removed = n, "истёкшие сессии удалены");
                }
            }
        });
        let app = routes::router(state).into_make_service_with_connect_info::<SocketAddr>();
        let listener = tokio::net::TcpListener::bind(listen).await?;
        tracing::info!(%listen, "хаб слушает");
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = tokio::signal::ctrl_c().await;
            })
            .await?;
        Ok(())
    })
}
