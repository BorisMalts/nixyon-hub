use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::response::Response;
use nixyon_agent::protocol::{ClientFrame, Op, ServerFrame, encode};
use serde::Deserialize;
use tokio::io::AsyncWriteExt;

use crate::auth::{CurrentUser, RECHECK_EVERY, SameOrigin, still_signed_in};
use crate::client::Client;
use crate::clock;
use crate::events::{self, Event, EventKind};
use crate::state::AppState;

const MAX_MESSAGE: usize = 64 * 1024;

#[derive(Deserialize)]
pub struct Size {
    #[serde(default = "default_cols")]
    cols: u16,
    #[serde(default = "default_rows")]
    rows: u16,
}

fn default_cols() -> u16 {
    100
}

fn default_rows() -> u16 {
    30
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Control {
    Resize { cols: u16, rows: u16 },
}

pub async fn open(
    State(state): State<AppState>,
    _origin: SameOrigin,
    client: Client,
    current: CurrentUser,
    Query(size): Query<Size>,
    ws: WebSocketUpgrade,
) -> Response {
    ws.max_message_size(MAX_MESSAGE)
        .on_upgrade(move |socket| run(state, client, current, size, socket))
}

async fn record(state: &AppState, kind: EventKind, client: &Client, user_id: i64, email: &str) {
    let (ip, ua, email) = (
        client.ip_text(),
        client.user_agent.clone(),
        email.to_string(),
    );
    let result = state
        .with_db(move |conn| {
            events::record(
                conn,
                &Event {
                    kind,
                    user_id: Some(user_id),
                    email: Some(&email),
                    ip: &ip,
                    user_agent: &ua,
                    detail: "",
                },
                clock::now(),
            )
        })
        .await;
    if result.is_err() {
        tracing::error!("не удалось записать событие терминала");
    }
}

async fn run(
    state: AppState,
    client: Client,
    current: CurrentUser,
    size: Size,
    mut socket: WebSocket,
) {
    let user = current.user;
    let token = current.token;
    let mut recheck = tokio::time::interval(RECHECK_EVERY);
    recheck.tick().await;
    let Ok(mut session) = state
        .agent
        .open(Op::Terminal, &user.email, size.cols, size.rows)
        .await
    else {
        let _ = socket.send(Message::Close(None)).await;
        return;
    };
    record(
        &state,
        EventKind::TerminalOpened,
        &client,
        user.user_id,
        &user.email,
    )
    .await;
    loop {
        tokio::select! {
            _ = recheck.tick() => {
                if !still_signed_in(&state, &token).await {
                    break;
                }
            }
            frame = session.reader.next() => match frame {
                Ok(Some(ServerFrame::Output { data })) => {
                    if socket.send(Message::Binary(data.into())).await.is_err() {
                        break;
                    }
                }
                Ok(Some(_)) | Ok(None) | Err(_) => break,
            },
            incoming = socket.recv() => {
                let frame = match incoming {
                    Some(Ok(Message::Binary(bytes))) => ClientFrame::Input { data: bytes.to_vec() },
                    Some(Ok(Message::Text(text))) => match serde_json::from_str::<Control>(&text) {
                        Ok(Control::Resize { cols, rows }) => ClientFrame::Resize { cols, rows },
                        Err(_) => continue,
                    },
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    Some(Ok(_)) => continue,
                };
                let Ok(line) = encode(&frame) else { continue };
                if session.writer.write_all(&line).await.is_err() {
                    break;
                }
            },
        }
    }
    let _ = socket.send(Message::Close(None)).await;
    record(
        &state,
        EventKind::TerminalClosed,
        &client,
        user.user_id,
        &user.email,
    )
    .await;
}
