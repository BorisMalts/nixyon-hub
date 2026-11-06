use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

/// Сколько клипов одного набора можно пометить: столько же принимает агент.
pub const MAX_MARKS: usize = 20_000;

/// Клип с плохим эталоном: кто и когда пометил.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Mark {
    pub clip: String,
    pub marked_by: String,
    pub marked_at: i64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum MarkOutcome {
    Marked,
    AlreadyMarked,
    TooMany,
}

/// Id клипа из индекса набора: буквы, цифры, точка, дефис и подчёркивание.
pub fn is_valid_clip(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

pub fn mark(
    conn: &Connection,
    dataset: &str,
    clip: &str,
    by: &str,
    now: i64,
) -> rusqlite::Result<MarkOutcome> {
    let exists: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM quality_marks WHERE dataset = ?1 AND clip = ?2",
            params![dataset, clip],
            |row| row.get(0),
        )
        .optional()?;
    if exists.is_some() {
        return Ok(MarkOutcome::AlreadyMarked);
    }
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM quality_marks WHERE dataset = ?1",
        params![dataset],
        |row| row.get(0),
    )?;
    if usize::try_from(count).unwrap_or(usize::MAX) >= MAX_MARKS {
        return Ok(MarkOutcome::TooMany);
    }
    conn.execute(
        "INSERT INTO quality_marks (dataset, clip, marked_by, marked_at) VALUES (?1, ?2, ?3, ?4)",
        params![dataset, clip, by, now],
    )?;
    Ok(MarkOutcome::Marked)
}

/// Снимает пометку; false, если её не было.
pub fn unmark(conn: &Connection, dataset: &str, clip: &str) -> rusqlite::Result<bool> {
    let removed = conn.execute(
        "DELETE FROM quality_marks WHERE dataset = ?1 AND clip = ?2",
        params![dataset, clip],
    )?;
    Ok(removed > 0)
}

/// Пометки набора, свежие раньше.
pub fn list(conn: &Connection, dataset: &str) -> rusqlite::Result<Vec<Mark>> {
    let mut stmt = conn.prepare(
        "SELECT clip, marked_by, marked_at FROM quality_marks WHERE dataset = ?1 \
         ORDER BY marked_at DESC, clip LIMIT ?2",
    )?;
    let limit = i64::try_from(MAX_MARKS).unwrap_or(i64::MAX);
    stmt.query_map(params![dataset, limit], |row| {
        Ok(Mark {
            clip: row.get(0)?,
            marked_by: row.get(1)?,
            marked_at: row.get(2)?,
        })
    })?
    .collect()
}

/// Id помеченных клипов набора: их проверка не берёт.
pub fn clips(conn: &Connection, dataset: &str) -> rusqlite::Result<Vec<String>> {
    Ok(list(conn, dataset)?.into_iter().map(|m| m.clip).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    #[test]
    fn a_clip_is_marked_once_listed_and_unmarked() {
        let conn = db::open_in_memory().unwrap();
        assert_eq!(
            mark(&conn, "cv", "a", "o@x.org", 1).unwrap(),
            MarkOutcome::Marked
        );
        assert_eq!(
            mark(&conn, "cv", "a", "p@x.org", 2).unwrap(),
            MarkOutcome::AlreadyMarked
        );
        mark(&conn, "cv", "b", "o@x.org", 3).unwrap();
        mark(&conn, "radio", "a", "o@x.org", 4).unwrap();
        let marks = list(&conn, "cv").unwrap();
        assert_eq!(marks[0].clip, "b", "свежие раньше");
        assert_eq!(marks[1].marked_by, "o@x.org");
        assert_eq!(clips(&conn, "radio").unwrap(), ["a"]);
        assert!(unmark(&conn, "cv", "a").unwrap());
        assert!(!unmark(&conn, "cv", "a").unwrap());
        assert_eq!(clips(&conn, "cv").unwrap(), ["b"]);
    }

    #[test]
    fn clip_ids_are_plain() {
        assert!(is_valid_clip("common_voice_ru_18875903"));
        assert!(is_valid_clip("1756465406.16342"));
        assert!(!is_valid_clip("../x"));
        assert!(!is_valid_clip(""));
        assert!(!is_valid_clip(&"a".repeat(129)));
    }
}
