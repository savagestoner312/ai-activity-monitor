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

/// Columns added after the original (Python-compatible) schema. Nullable, so
/// rows written before they existed stay valid; `migrate` backfills them.
const ADDED_COLUMNS: &[(&str, &str)] = &[
    ("process_events", "app"),
    ("process_events", "started"),
    ("child_events", "parent_app"),
    ("net_events", "app"),
];

const INDEXES: &str = "
CREATE INDEX IF NOT EXISTS idx_process_events_ts ON process_events(ts);
CREATE INDEX IF NOT EXISTS idx_process_events_pid ON process_events(pid);
CREATE INDEX IF NOT EXISTS idx_child_events_ts ON child_events(ts);
CREATE INDEX IF NOT EXISTS idx_net_events_ts ON net_events(ts);
CREATE INDEX IF NOT EXISTS idx_device_events_ts ON device_events(ts);
";

/// Open (creating if needed) the DB for writing, and ensure the schema exists.
/// Used only by the collector.
pub fn open_rw(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    migrate(&conn)?;
    Ok(conn)
}

/// Brings any `aimon.db` up to the current schema: creates the tables, adds
/// the newer columns, and fills in `app` for rows written before it existed.
/// Idempotent, and safe to race with another process doing the same (the
/// collector and dashboard both call it at startup).
pub fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(SCHEMA)?;
    for (table, column) in ADDED_COLUMNS {
        if !has_column(conn, table, column)? {
            match conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} TEXT")) {
                Ok(()) => {}
                // Lost a race with the other process adding it.
                Err(e) if e.to_string().contains("duplicate column") => {}
                Err(e) => return Err(e),
            }
        }
    }
    conn.execute_batch(INDEXES)?;
    backfill_apps(conn)
}

fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let names = stmt.query_map([], |r| r.get::<_, String>(1))?.collect::<Result<Vec<_>>>()?;
    Ok(names.iter().any(|n| n == column))
}

fn backfill_apps(conn: &Connection) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    {
        let mut select = tx.prepare("SELECT rowid, name, exe, cmdline FROM process_events WHERE app IS NULL")?;
        let rows = select
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                ))
            })?
            .collect::<Result<Vec<_>>>()?;
        let mut update = tx.prepare("UPDATE process_events SET app=?1 WHERE rowid=?2")?;
        for (rowid, name, exe, cmdline) in rows {
            update.execute(params![crate::apps::app_for(&name, &exe, &cmdline), rowid])?;
        }
    }
    // Children and connections take the app of the process they belong to,
    // as recorded by its most recent start at or before the event.
    tx.execute_batch(
        "UPDATE child_events SET parent_app=(SELECT p.app FROM process_events p
           WHERE p.pid=child_events.parent_pid AND p.ts<=child_events.ts AND p.app IS NOT NULL
           ORDER BY p.ts DESC LIMIT 1) WHERE parent_app IS NULL;
         UPDATE net_events SET app=(SELECT p.app FROM process_events p
           WHERE p.pid=net_events.pid AND p.ts<=net_events.ts AND p.app IS NOT NULL
           ORDER BY p.ts DESC LIMIT 1) WHERE app IS NULL;",
    )?;
    // Whatever's left had no matching process row: classify by name alone.
    for (table, name_col, app_col) in [("child_events", "parent_name", "parent_app"), ("net_events", "name", "app")] {
        let names = tx
            .prepare(&format!("SELECT DISTINCT {name_col} FROM {table} WHERE {app_col} IS NULL"))?
            .query_map([], |r| Ok(r.get::<_, Option<String>>(0)?.unwrap_or_default()))?
            .collect::<Result<Vec<_>>>()?;
        for name in names {
            tx.execute(
                &format!("UPDATE {table} SET {app_col}=?1 WHERE {app_col} IS NULL AND {name_col}=?2"),
                params![crate::apps::app_for(&name, "", ""), name],
            )?;
        }
    }
    tx.commit()
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

/// `event` is `start` (launched while the collector watched), `baseline`
/// (already running when the collector started), or `stop`. `started` is the
/// process's own start time, when the OS reports one.
#[allow(clippy::too_many_arguments)]
pub fn insert_process_event(
    conn: &Connection,
    ts: &str,
    event: &str,
    pid: u32,
    name: &str,
    exe: &str,
    cmdline: &str,
    app: &str,
    started: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO process_events(ts,event,pid,name,exe,cmdline,app,started) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
        params![ts, event, pid, name, exe, cmdline, app, started],
    )?;
    Ok(())
}

/// Marks a collector start. Anything recorded as running before the latest
/// marker is re-established by that session's `baseline` rows, so a process
/// that exited while the collector was down doesn't read as running forever.
pub fn insert_session_marker(conn: &Connection, ts: &str, collector_pid: u32) -> Result<()> {
    conn.execute(
        "INSERT INTO process_events(ts,event,pid,name,exe,cmdline,app) VALUES(?1,'session',?2,'aimon-collector','','','')",
        params![ts, collector_pid],
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn insert_child_event(
    conn: &Connection,
    ts: &str,
    parent_pid: u32,
    parent_name: &str,
    parent_app: &str,
    pid: u32,
    name: &str,
    cmdline: &str,
    flagged: bool,
) -> Result<()> {
    conn.execute(
        "INSERT INTO child_events(ts,parent_pid,parent_name,pid,name,cmdline,flagged,parent_app) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
        params![ts, parent_pid, parent_name, pid, name, cmdline, flagged as i64, parent_app],
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn insert_net_event(
    conn: &Connection,
    ts: &str,
    pid: u32,
    name: &str,
    app: &str,
    raddr: &str,
    rport: u16,
    rhost: &str,
    status: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO net_events(ts,pid,name,raddr,rport,rhost,status,app) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
        params![ts, pid, name, raddr, rport, rhost, status, app],
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
        params![ts, tool, proc_count, cpu_pct, i64::try_from(mem_bytes).unwrap_or(i64::MAX), gpu_pct],
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
    fn migrate_upgrades_an_old_database_and_backfills_apps() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        // Rows as the pre-migration collector wrote them.
        conn.execute_batch(
            "INSERT INTO process_events VALUES('2026-09-19T10:00:00','start',7,'Claude Helper',
               '/Applications/Claude.app/Contents/Frameworks/Claude Helper.app/Contents/MacOS/Claude Helper','');
             INSERT INTO child_events VALUES('2026-09-19T10:01:00',7,'Claude Helper',8,'zsh','zsh -l',1);
             INSERT INTO child_events VALUES('2026-09-19T10:01:00',99,'ollama.exe',9,'curl.exe','curl x',1);
             INSERT INTO net_events VALUES('2026-09-19T10:02:00',7,'Claude Helper','1.2.3.4',443,'','Established');",
        )
        .unwrap();

        migrate(&conn).unwrap();
        migrate(&conn).unwrap(); // idempotent

        let app: String = conn.query_row("SELECT app FROM process_events", [], |r| r.get(0)).unwrap();
        assert_eq!(app, "Claude");
        let parents: Vec<String> = conn
            .prepare("SELECT parent_app FROM child_events ORDER BY pid")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_>>()
            .unwrap();
        assert_eq!(parents, ["Claude", "ollama"]);
        let net_app: String = conn.query_row("SELECT app FROM net_events", [], |r| r.get(0)).unwrap();
        assert_eq!(net_app, "Claude");

        insert_process_event(&conn, "2026-09-19T10:03:00", "start", 10, "ollama", "/usr/bin/ollama", "ollama serve", "ollama", Some("2026-09-19T10:02:59")).unwrap();
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
