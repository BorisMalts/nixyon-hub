use rusqlite::{Connection, params};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    Login,
    LoginFailed,
    LoginLocked,
    Logout,
    BuildStarted,
    PullStarted,
    QualityStarted,
    LiveEvalStarted,
    TrainingStarted,
    QualityMarked,
    QualityUnmarked,
    TerminalOpened,
    TerminalClosed,
    CiCancelled,
    CiRerun,
}

impl EventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EventKind::Login => "login",
            EventKind::LoginFailed => "login_failed",
            EventKind::LoginLocked => "login_locked",
            EventKind::Logout => "logout",
            EventKind::BuildStarted => "build_started",
            EventKind::PullStarted => "pull_started",
            EventKind::QualityStarted => "quality_started",
            EventKind::LiveEvalStarted => "live_eval_started",
            EventKind::TrainingStarted => "training_started",
            EventKind::QualityMarked => "quality_marked",
            EventKind::QualityUnmarked => "quality_unmarked",
            EventKind::TerminalOpened => "terminal_opened",
            EventKind::TerminalClosed => "terminal_closed",
            EventKind::CiCancelled => "ci_cancelled",
            EventKind::CiRerun => "ci_rerun",
        }
    }
}

pub struct Event<'a> {
    pub kind: EventKind,
    pub user_id: Option<i64>,
    pub email: Option<&'a str>,
    pub ip: &'a str,
    pub user_agent: &'a str,
    pub detail: &'a str,
}

const MAX_FIELD: usize = 300;

pub fn record(conn: &Connection, event: &Event<'_>, now: i64) -> rusqlite::Result<()> {
    let clip = |s: &str| s.chars().take(MAX_FIELD).collect::<String>();
    conn.execute(
        "INSERT INTO events (at, kind, user_id, email, ip, user_agent, detail) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            now,
            event.kind.as_str(),
            event.user_id,
            event.email.map(clip),
            clip(event.ip),
            clip(event.user_agent),
            clip(event.detail)
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    #[test]
    fn event_is_stored_with_clipped_fields() {
        let conn = db::open_in_memory().unwrap();
        let long = "ж".repeat(1000);
        record(
            &conn,
            &Event {
                kind: EventKind::LoginFailed,
                user_id: None,
                email: Some("a@x.org"),
                ip: "1.2.3.4",
                user_agent: &long,
                detail: "",
            },
            5,
        )
        .unwrap();
        let (kind, ua): (String, String) = conn
            .query_row("SELECT kind, user_agent FROM events", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(kind, "login_failed");
        assert_eq!(ua.chars().count(), MAX_FIELD);
    }
}
