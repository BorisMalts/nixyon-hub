use rusqlite::{Connection, OptionalExtension, params};

use crate::token;

pub const IDLE_SECONDS: i64 = 12 * 3600;
pub const MAX_AGE_SECONDS: i64 = 7 * 24 * 3600;
const TOUCH_EVERY_SECONDS: i64 = 60;
const MAX_USER_AGENT: usize = 300;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionUser {
    pub user_id: i64,
    pub email: String,
}

pub struct Origin<'a> {
    pub ip: &'a str,
    pub user_agent: &'a str,
}

pub fn create(
    conn: &Connection,
    user_id: i64,
    origin: &Origin<'_>,
    now: i64,
) -> rusqlite::Result<String> {
    let new = token::generate();
    let user_agent: String = origin.user_agent.chars().take(MAX_USER_AGENT).collect();
    conn.execute(
        "INSERT INTO sessions (token_hash, user_id, created_at, last_seen_at, expires_at, ip, user_agent)
         VALUES (?1, ?2, ?3, ?3, ?4, ?5, ?6)",
        params![new.hash, user_id, now, now + MAX_AGE_SECONDS, origin.ip, user_agent],
    )?;
    Ok(new.raw)
}

pub fn lookup(conn: &Connection, raw: &str, now: i64) -> rusqlite::Result<Option<SessionUser>> {
    let hash = token::digest(raw);
    let found = conn
        .query_row(
            "SELECT s.user_id, u.email, s.last_seen_at, s.expires_at
             FROM sessions s JOIN users u ON u.id = s.user_id WHERE s.token_hash = ?1",
            params![hash],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            },
        )
        .optional()?;
    let Some((user_id, email, last_seen, expires)) = found else {
        return Ok(None);
    };
    if now >= expires || now - last_seen >= IDLE_SECONDS {
        conn.execute("DELETE FROM sessions WHERE token_hash = ?1", params![hash])?;
        return Ok(None);
    }
    if now - last_seen >= TOUCH_EVERY_SECONDS {
        conn.execute(
            "UPDATE sessions SET last_seen_at = ?1 WHERE token_hash = ?2",
            params![now, hash],
        )?;
    }
    Ok(Some(SessionUser { user_id, email }))
}

pub fn delete(conn: &Connection, raw: &str) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM sessions WHERE token_hash = ?1",
        params![token::digest(raw)],
    )?;
    Ok(())
}

pub fn delete_expired(conn: &Connection, now: i64) -> rusqlite::Result<usize> {
    conn.execute(
        "DELETE FROM sessions WHERE expires_at <= ?1 OR last_seen_at <= ?2",
        params![now, now - IDLE_SECONDS],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db, users};

    fn setup() -> (Connection, i64) {
        let conn = db::open_in_memory().unwrap();
        let user = users::add(&conn, "a@x.org", "достаточно-длинный").unwrap();
        (conn, user.id)
    }

    const ORIGIN: Origin<'static> = Origin {
        ip: "1.2.3.4",
        user_agent: "test",
    };

    #[test]
    fn created_session_finds_its_user() {
        let (conn, id) = setup();
        let raw = create(&conn, id, &ORIGIN, 1000).unwrap();
        assert_eq!(
            lookup(&conn, &raw, 1001).unwrap(),
            Some(SessionUser {
                user_id: id,
                email: "a@x.org".into()
            })
        );
        assert_eq!(lookup(&conn, "чужой-токен", 1001).unwrap(), None);
    }

    #[test]
    fn idle_session_expires_and_activity_extends_it() {
        let (conn, id) = setup();
        let raw = create(&conn, id, &ORIGIN, 0).unwrap();
        assert!(lookup(&conn, &raw, IDLE_SECONDS - 1).unwrap().is_some());
        assert!(lookup(&conn, &raw, 2 * IDLE_SECONDS - 2).unwrap().is_some());
        assert!(lookup(&conn, &raw, 3 * IDLE_SECONDS).unwrap().is_none());
        assert!(lookup(&conn, &raw, 3 * IDLE_SECONDS - 1).unwrap().is_none());
    }

    #[test]
    fn session_ends_at_max_age_even_when_active() {
        let (conn, id) = setup();
        let raw = create(&conn, id, &ORIGIN, 0).unwrap();
        let mut t = 0;
        while t < MAX_AGE_SECONDS - IDLE_SECONDS {
            t += IDLE_SECONDS - 10;
            assert!(lookup(&conn, &raw, t).unwrap().is_some(), "t = {t}");
        }
        assert!(lookup(&conn, &raw, MAX_AGE_SECONDS).unwrap().is_none());
    }

    #[test]
    fn deleted_session_and_removed_user_end_access() {
        let (conn, id) = setup();
        let raw = create(&conn, id, &ORIGIN, 0).unwrap();
        delete(&conn, &raw).unwrap();
        assert!(lookup(&conn, &raw, 1).unwrap().is_none());

        let raw = create(&conn, id, &ORIGIN, 0).unwrap();
        users::remove(&conn, "a@x.org").unwrap();
        assert!(lookup(&conn, &raw, 1).unwrap().is_none());
    }

    #[test]
    fn expired_sessions_are_cleaned() {
        let (conn, id) = setup();
        create(&conn, id, &ORIGIN, 0).unwrap();
        create(&conn, id, &ORIGIN, IDLE_SECONDS).unwrap();
        assert_eq!(delete_expired(&conn, IDLE_SECONDS + 1).unwrap(), 1);
    }
}
