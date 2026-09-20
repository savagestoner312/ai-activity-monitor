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
CREATE TABLE IF NOT EXISTS perf_samples(ts TEXT, tool TEXT, proc_count INT, cpu_pct REAL, mem_bytes INT, gpu_pct REAL);
CREATE INDEX IF NOT EXISTS idx_perf_samples_ts ON perf_samples(ts);
";

use rusqlite::{params, Connection, OpenFlags, Result};
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

pub fn insert_process_event(
    conn: &Connection,
    ts: &str,
    event: &str,
    pid: u32,
    name: &str,
    exe: &str,
    cmdline: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO process_events VALUES(?1,?2,?3,?4,?5,?6)",
        params![ts, event, pid, name, exe, cmdline],
    )?;
    Ok(())
}

pub fn insert_child_event(
    conn: &Connection,
    ts: &str,
    parent_pid: u32,
    parent_name: &str,
    pid: u32,
    name: &str,
    cmdline: &str,
    flagged: bool,
) -> Result<()> {
    conn.execute(
        "INSERT INTO child_events VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![ts, parent_pid, parent_name, pid, name, cmdline, flagged as i64],
    )?;
    Ok(())
}

pub fn insert_net_event(
    conn: &Connection,
    ts: &str,
    pid: u32,
    name: &str,
    raddr: &str,
    rport: u16,
    rhost: &str,
    status: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO net_events VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![ts, pid, name, raddr, rport, rhost, status],
    )?;
    Ok(())
}

pub fn insert_device_event(
    conn: &Connection,
    ts: &str,
    device: &str,
    app: &str,
    started: &str,
    stopped: &str,
    is_ai: bool,
) -> Result<()> {
    conn.execute(
        "INSERT INTO device_events VALUES(?1,?2,?3,?4,?5,?6)",
        params![ts, device, app, started, stopped, is_ai as i64],
    )?;
    Ok(())
}

pub fn insert_perf_sample(
    conn: &Connection,
    ts: &str,
    tool: &str,
    proc_count: u32,
    cpu_pct: f32,
    mem_bytes: u64,
    gpu_pct: Option<f32>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO perf_samples VALUES(?1,?2,?3,?4,?5,?6)",
        params![ts, tool, proc_count, cpu_pct, mem_bytes, gpu_pct],
    )?;
    Ok(())
}

/// Deletes perf_samples rows older than `cutoff`, returning the count
/// deleted. Unlike every other table here, perf_samples inserts every cycle
/// for whatever's currently running (not just on state changes), so unlike
/// process_events et al. it needs active pruning to avoid unbounded growth.
pub fn prune_perf_samples(conn: &Connection, cutoff: &str) -> Result<usize> {
    conn.execute("DELETE FROM perf_samples WHERE ts < ?1", params![cutoff])
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
    fn schema_creates_all_five_tables() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        for table in ["process_events", "child_events", "net_events", "device_events", "perf_samples"] {
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

    #[test]
    fn perf_samples_insert_and_prune() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        insert_perf_sample(&conn, "2026-09-19T10:00:00", "claude.exe", 2, 12.5, 800_000_000, Some(3.1)).unwrap();
        insert_perf_sample(&conn, "2026-09-19T12:00:00", "claude.exe", 1, 5.0, 400_000_000, None).unwrap();

        let count: i64 = conn.query_row("SELECT COUNT(*) FROM perf_samples", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 2);

        let deleted = prune_perf_samples(&conn, "2026-09-19T11:00:00").unwrap();
        assert_eq!(deleted, 1);
        let remaining: i64 = conn.query_row("SELECT COUNT(*) FROM perf_samples", [], |r| r.get(0)).unwrap();
        assert_eq!(remaining, 1);
    }
}
