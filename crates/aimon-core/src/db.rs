//! Database schema and access layer. Schema is byte-for-byte the same as the
//! Python collector's so old and new rows coexist in the same `aimon.db`.
//! Read/write query functions land in later phases (collector port, dashboard
//! backend port) — this module currently just establishes the schema and the
//! open/connect helpers so downstream crates have something to build against.

pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS process_events(ts TEXT, event TEXT, pid INT, name TEXT, exe TEXT, cmdline TEXT);
CREATE TABLE IF NOT EXISTS child_events(ts TEXT, parent_pid INT, parent_name TEXT, pid INT, name TEXT, cmdline TEXT, flagged INT);
CREATE TABLE IF NOT EXISTS net_events(ts TEXT, pid INT, name TEXT, raddr TEXT, rport INT, rhost TEXT, status TEXT);
CREATE TABLE IF NOT EXISTS device_events(ts TEXT, device TEXT, app TEXT, started TEXT, stopped TEXT, is_ai INT);
";

use rusqlite::{Connection, OpenFlags, Result};
use std::path::Path;

/// Open (creating if needed) the DB for writing, and ensure the schema exists.
/// Used only by the collector.
pub fn open_rw(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

/// Open the DB read-only. Used by the dashboard server and the report generator.
pub fn open_ro(path: &Path) -> Result<Connection> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
}

/// The exact timestamp format used throughout — `YYYY-MM-DDTHH:MM:SS`, local
/// time, second precision. All range queries are lexicographic string
/// comparisons on this format, so it must match the Python collector's
/// `datetime.now().isoformat(timespec="seconds")` byte-for-byte.
pub fn now_str() -> String {
    chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_str_matches_python_isoformat_shape() {
        let s = now_str();
        // e.g. "2026-09-19T15:49:07" — exactly 19 chars, 'T' separator, no offset.
        assert_eq!(s.len(), 19);
        assert_eq!(s.as_bytes()[10], b'T');
        assert!(!s.contains('+') && !s.contains('Z'));
    }

    #[test]
    fn schema_creates_all_four_tables() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        for table in ["process_events", "child_events", "net_events", "device_events"] {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "missing table {table}");
        }
    }
}
