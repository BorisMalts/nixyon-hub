use std::path::Path;

use rusqlite::Connection;

const SCHEMA: &str = "
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;
CREATE TABLE IF NOT EXISTS users (
    id INTEGER PRIMARY KEY,
    email TEXT NOT NULL UNIQUE COLLATE NOCASE,
    password_hash TEXT NOT NULL,
    failed_logins INTEGER NOT NULL DEFAULT 0,
    locked_until INTEGER,
    created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS sessions (
    token_hash BLOB PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    last_seen_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    ip TEXT NOT NULL,
    user_agent TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS sessions_user ON sessions(user_id);
CREATE TABLE IF NOT EXISTS events (
    id INTEGER PRIMARY KEY,
    at INTEGER NOT NULL,
    kind TEXT NOT NULL,
    user_id INTEGER,
    email TEXT,
    ip TEXT NOT NULL,
    user_agent TEXT NOT NULL,
    detail TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS quality_marks (
    dataset TEXT NOT NULL,
    clip TEXT NOT NULL,
    marked_by TEXT NOT NULL,
    marked_at INTEGER NOT NULL,
    PRIMARY KEY (dataset, clip)
);
CREATE TRIGGER IF NOT EXISTS events_no_update BEFORE UPDATE ON events
BEGIN SELECT RAISE(ABORT, 'events are append-only'); END;
CREATE TRIGGER IF NOT EXISTS events_no_delete BEFORE DELETE ON events
BEGIN SELECT RAISE(ABORT, 'events are append-only'); END;
";

pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

pub fn open_in_memory() -> rusqlite::Result<Connection> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_cannot_be_changed_or_deleted() {
        let conn = open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO events (at, kind, ip, user_agent, detail) VALUES (1, 'login', '1.1.1.1', 'ua', '')",
            [],
        )
        .unwrap();
        assert!(conn.execute("UPDATE events SET kind = 'x'", []).is_err());
        assert!(conn.execute("DELETE FROM events", []).is_err());
    }

    #[test]
    fn schema_is_idempotent() {
        let conn = open_in_memory().unwrap();
        conn.execute_batch(SCHEMA).unwrap();
    }
}
