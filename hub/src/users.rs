use rusqlite::{Connection, OptionalExtension, params};

use crate::clock;
use crate::password::{self, PasswordError};

pub const MAX_FAILED_LOGINS: i64 = 5;
pub const LOCK_SECONDS: i64 = 15 * 60;
const MAX_EMAIL_LENGTH: usize = 254;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub id: i64,
    pub email: String,
    pub password_hash: String,
    pub locked_until: Option<i64>,
}

#[derive(Debug, thiserror::Error)]
pub enum UserError {
    #[error("почта выглядит неправильно")]
    BadEmail,
    #[error("такой пользователь уже есть")]
    Exists,
    #[error("нет такого пользователя")]
    NotFound,
    #[error(transparent)]
    Password(#[from] PasswordError),
    #[error(transparent)]
    Db(#[from] rusqlite::Error),
}

pub fn normalize_email(email: &str) -> Result<String, UserError> {
    let email = email.trim().to_lowercase();
    let valid = email.len() <= MAX_EMAIL_LENGTH
        && !email.chars().any(|c| c.is_whitespace() || c.is_control())
        && email.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty() && domain.contains('.') && !domain.contains('@')
        });
    if valid {
        Ok(email)
    } else {
        Err(UserError::BadEmail)
    }
}

pub fn add(conn: &Connection, email: &str, plain_password: &str) -> Result<User, UserError> {
    let email = normalize_email(email)?;
    let hash = password::hash(plain_password)?;
    let inserted = conn.execute(
        "INSERT INTO users (email, password_hash, created_at) VALUES (?1, ?2, ?3)",
        params![email, hash, clock::now()],
    );
    match inserted {
        Ok(_) => find(conn, &email)?.ok_or(UserError::NotFound),
        Err(rusqlite::Error::SqliteFailure(e, _))
            if e.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            Err(UserError::Exists)
        }
        Err(e) => Err(e.into()),
    }
}

pub fn set_password(conn: &Connection, email: &str, plain_password: &str) -> Result<(), UserError> {
    let email = normalize_email(email)?;
    let hash = password::hash(plain_password)?;
    let changed = conn.execute(
        "UPDATE users SET password_hash = ?1, failed_logins = 0, locked_until = NULL WHERE email = ?2",
        params![hash, email],
    )?;
    if changed == 0 {
        return Err(UserError::NotFound);
    }
    conn.execute(
        "DELETE FROM sessions WHERE user_id = (SELECT id FROM users WHERE email = ?1)",
        params![email],
    )?;
    Ok(())
}

pub fn remove(conn: &Connection, email: &str) -> Result<(), UserError> {
    let email = normalize_email(email)?;
    let removed = conn.execute("DELETE FROM users WHERE email = ?1", params![email])?;
    if removed == 0 {
        Err(UserError::NotFound)
    } else {
        Ok(())
    }
}

pub fn list(conn: &Connection) -> Result<Vec<String>, UserError> {
    let mut stmt = conn.prepare("SELECT email FROM users ORDER BY email, id")?;
    let rows = stmt.query_map([], |r| r.get(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn find(conn: &Connection, email: &str) -> Result<Option<User>, UserError> {
    let Ok(email) = normalize_email(email) else {
        return Ok(None);
    };
    Ok(conn
        .query_row(
            "SELECT id, email, password_hash, locked_until FROM users WHERE email = ?1",
            params![email],
            |r| {
                Ok(User {
                    id: r.get(0)?,
                    email: r.get(1)?,
                    password_hash: r.get(2)?,
                    locked_until: r.get(3)?,
                })
            },
        )
        .optional()?)
}

pub fn record_failure(conn: &Connection, user_id: i64, now: i64) -> rusqlite::Result<bool> {
    conn.execute(
        "UPDATE users SET failed_logins = failed_logins + 1 WHERE id = ?1",
        params![user_id],
    )?;
    let failed: i64 = conn.query_row(
        "SELECT failed_logins FROM users WHERE id = ?1",
        params![user_id],
        |r| r.get(0),
    )?;
    if failed >= MAX_FAILED_LOGINS {
        conn.execute(
            "UPDATE users SET failed_logins = 0, locked_until = ?1 WHERE id = ?2",
            params![now + LOCK_SECONDS, user_id],
        )?;
        return Ok(true);
    }
    Ok(false)
}

pub fn record_success(conn: &Connection, user_id: i64) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE users SET failed_logins = 0, locked_until = NULL WHERE id = ?1",
        params![user_id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    const PASSWORD: &str = "достаточно-длинный";

    #[test]
    fn email_is_normalized_and_validated() {
        assert_eq!(
            normalize_email("  Andrey@Gmail.COM ").unwrap(),
            "andrey@gmail.com"
        );
        for bad in [
            "",
            "без-собаки",
            "@gmail.com",
            "a@b",
            "a b@c.d",
            "a@b@c.d",
            "a@b\u{7}.c",
        ] {
            assert!(normalize_email(bad).is_err(), "{bad:?}");
        }
        assert!(normalize_email(&format!("{}@b.c", "a".repeat(260))).is_err());
        assert_eq!(normalize_email("a@b.c\n").unwrap(), "a@b.c");
    }

    #[test]
    fn add_find_list_and_remove() {
        let conn = db::open_in_memory().unwrap();
        add(&conn, "B@x.org", PASSWORD).unwrap();
        add(&conn, "a@x.org", PASSWORD).unwrap();
        assert_eq!(list(&conn).unwrap(), ["a@x.org", "b@x.org"]);
        assert!(find(&conn, "b@X.org").unwrap().is_some());
        assert!(matches!(
            add(&conn, "b@x.org", PASSWORD),
            Err(UserError::Exists)
        ));
        remove(&conn, "b@x.org").unwrap();
        assert!(find(&conn, "b@x.org").unwrap().is_none());
        assert!(matches!(remove(&conn, "b@x.org"), Err(UserError::NotFound)));
    }

    #[test]
    fn short_password_is_refused() {
        let conn = db::open_in_memory().unwrap();
        assert!(matches!(
            add(&conn, "a@x.org", "коротко"),
            Err(UserError::Password(_))
        ));
    }

    #[test]
    fn fifth_failure_locks_and_resets_counter() {
        let conn = db::open_in_memory().unwrap();
        let user = add(&conn, "a@x.org", PASSWORD).unwrap();
        for _ in 0..MAX_FAILED_LOGINS - 1 {
            assert!(!record_failure(&conn, user.id, 100).unwrap());
        }
        assert!(record_failure(&conn, user.id, 100).unwrap());
        assert_eq!(
            find(&conn, "a@x.org").unwrap().unwrap().locked_until,
            Some(100 + LOCK_SECONDS)
        );
        record_success(&conn, user.id).unwrap();
        assert_eq!(find(&conn, "a@x.org").unwrap().unwrap().locked_until, None);
    }

    #[test]
    fn new_password_ends_all_sessions() {
        let conn = db::open_in_memory().unwrap();
        let user = add(&conn, "a@x.org", PASSWORD).unwrap();
        conn.execute(
            "INSERT INTO sessions VALUES (x'01', ?1, 0, 0, 9999999999, 'ip', 'ua')",
            params![user.id],
        )
        .unwrap();
        set_password(&conn, "a@x.org", "новый-длинный-пароль").unwrap();
        let left: i64 = conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 0);
    }
}
