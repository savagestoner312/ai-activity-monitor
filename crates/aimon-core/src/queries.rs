//! Read-side queries backing the dashboard API and the report generator.
//! `events_in_range`/`state_for_range` generalize the Python dashboard's
//! tail-only `events(after)` / hardcoded-to-"today" `state()` into arbitrary
//! `[start, end)` windows, which is what makes history browsing possible.
use aimon_api_types::{
    Counts, DeviceChip, EndpointStat, EventKind, RiverBucket, RiverLane, RunningTool, StateSummary, ToolPerf, UnifiedEvent,
};
use chrono::{Duration, NaiveDateTime};
use rusqlite::{params, Connection, Result};
use std::collections::BTreeMap;

const TS_FMT: &str = "%Y-%m-%dT%H:%M:%S";

/// Every loggable event, normalized to one shape. Mirrors the Python
/// dashboard's `events()` UNION-ALL query, parameterized by an end bound
/// instead of being tail-only.
const EVENTS_UNION_SQL: &str = "
    SELECT ts, 'process' AS kind, name AS tool, event||': '||COALESCE(NULLIF(cmdline,''),exe) AS detail, 0 AS flagged
      FROM process_events WHERE ts>=?1 AND ts<?2
    UNION ALL
    SELECT ts, 'command', parent_name, cmdline, flagged
      FROM child_events WHERE ts>=?1 AND ts<?2
    UNION ALL
    SELECT ts, 'network', name, COALESCE(NULLIF(rhost,''),raddr)||':'||rport, 0
      FROM net_events WHERE ts>=?1 AND ts<?2
    UNION ALL
    SELECT ts, 'device', app, device||CASE WHEN stopped='' THEN ' in use' ELSE ' used until '||stopped END, is_ai
      FROM device_events WHERE ts>=?1 AND ts<?2
";

fn parse_kind(s: &str) -> EventKind {
    match s {
        "command" => EventKind::Command,
        "network" => EventKind::Network,
        "device" => EventKind::Device,
        _ => EventKind::Process,
    }
}

/// Events in `[start, end)`, newest first, capped at `limit` — used for both
/// live-tail (`start` = last-seen ts, `end` = now) and historical scrub
/// (explicit bounded range) fetches.
pub fn events_in_range(conn: &Connection, start: &str, end: &str, limit: i64) -> Result<Vec<UnifiedEvent>> {
    let sql = format!("SELECT * FROM ({EVENTS_UNION_SQL}) ORDER BY ts DESC LIMIT ?3");
    let mut stmt = conn.prepare(&sql)?;
    let result = stmt
        .query_map(params![start, end, limit], |r| {
            Ok(UnifiedEvent {
                ts: r.get(0)?,
                kind: parse_kind(&r.get::<_, String>(1)?),
                tool: r.get(2)?,
                detail: r.get(3)?,
                flagged: r.get::<_, i64>(4)? != 0,
            })
        })?
        .collect();
    result
}

/// Oldest timestamp across all four tables, or `None` if the DB is empty.
/// Powers the history slider's left bound.
pub fn oldest_event_ts(conn: &Connection) -> Result<Option<String>> {
    let mut oldest: Option<String> = None;
    for table in ["process_events", "child_events", "net_events", "device_events"] {
        let v: Option<String> = conn.query_row(&format!("SELECT MIN(ts) FROM {table}"), [], |r| r.get(0))?;
        if let Some(v) = v {
            oldest = Some(match oldest {
                Some(cur) if cur <= v => cur,
                _ => v,
            });
        }
    }
    Ok(oldest)
}

/// AI tools with a start event at or before `as_of` and no matching stop yet
/// — only meaningful "as of now", so callers should only surface this when
/// the viewed window's end is effectively live.
pub fn running_now(conn: &Connection, as_of: &str) -> Result<Vec<RunningTool>> {
    let sql = "SELECT p.name, p.pid, MAX(p.ts) FROM process_events p
               WHERE p.event='start' AND p.ts<=?1
               AND NOT EXISTS (
                 SELECT 1 FROM process_events s
                 WHERE s.pid=p.pid AND s.event='stop' AND s.ts>=p.ts AND s.ts<=?1
               )
               GROUP BY p.pid";
    let mut stmt = conn.prepare(sql)?;
    let result = stmt
        .query_map(params![as_of], |r| Ok(RunningTool { name: r.get(0)?, pid: r.get(1)?, since: r.get(2)? }))?
        .collect();
    result
}

/// Devices with no recorded stop yet — inherently a "right now" concept,
/// same live-only caveat as `running_now`.
pub fn live_devices(conn: &Connection) -> Result<Vec<DeviceChip>> {
    let sql = "SELECT device, app FROM device_events d WHERE stopped=''
               AND ts=(SELECT MAX(ts) FROM device_events WHERE device=d.device AND app=d.app)";
    let mut stmt = conn.prepare(sql)?;
    let result = stmt.query_map([], |r| Ok(DeviceChip { device: r.get(0)?, app: r.get(1)? }))?.collect();
    result
}

pub fn counts_in_range(conn: &Connection, start: &str, end: &str) -> Result<Counts> {
    let launched = conn.query_row(
        "SELECT COUNT(*) FROM process_events WHERE event='start' AND ts>=?1 AND ts<?2",
        params![start, end],
        |r| r.get(0),
    )?;
    let commands = conn.query_row(
        "SELECT COUNT(*) FROM child_events WHERE ts>=?1 AND ts<?2",
        params![start, end],
        |r| r.get(0),
    )?;
    let flagged = conn.query_row(
        "SELECT COUNT(*) FROM child_events WHERE flagged=1 AND ts>=?1 AND ts<?2",
        params![start, end],
        |r| r.get(0),
    )?;
    let endpoints = conn.query_row(
        "SELECT COUNT(DISTINCT COALESCE(NULLIF(rhost,''),raddr)) FROM net_events WHERE ts>=?1 AND ts<?2",
        params![start, end],
        |r| r.get(0),
    )?;
    Ok(Counts { launched, commands, flagged, endpoints })
}

pub fn top_endpoints_in_range(conn: &Connection, start: &str, end: &str, limit: i64) -> Result<Vec<EndpointStat>> {
    let sql = "SELECT name, COALESCE(NULLIF(rhost,''),raddr) h, COUNT(*) n FROM net_events
               WHERE ts>=?1 AND ts<?2 GROUP BY 1,2 ORDER BY 3 DESC LIMIT ?3";
    let mut stmt = conn.prepare(sql)?;
    let result = stmt
        .query_map(params![start, end, limit], |r| Ok(EndpointStat { tool: r.get(0)?, host: r.get(1)?, n: r.get(2)? }))?
        .collect();
    result
}

/// A window's full summary. `is_live` (and therefore `running`/`devices`)
/// reflects whether `end` reaches the server's current time — a past window
/// has no "currently running" tools by definition.
pub fn state_for_range(conn: &Connection, start: &str, end: &str, now: &str) -> Result<StateSummary> {
    let is_live = end >= now;
    let (running, devices) =
        if is_live { (running_now(conn, now)?, live_devices(conn)?) } else { (Vec::new(), Vec::new()) };
    Ok(StateSummary {
        range_start: start.to_string(),
        range_end: end.to_string(),
        is_live,
        running,
        counts: counts_in_range(conn, start, end)?,
        devices,
        endpoints: top_endpoints_in_range(conn, start, end, 8)?,
    })
}

/// Per-tool, per-time-bucket activity counts for the river visualization.
/// Bucketing happens in Rust (not SQL) since it's a personal-scale dataset
/// and this keeps the query portable — no julianday gymnastics.
pub fn river_for_range(conn: &Connection, start: &str, end: &str, bucket_seconds: i64) -> Result<Vec<RiverLane>> {
    let Ok(start_dt) = NaiveDateTime::parse_from_str(start, TS_FMT) else {
        return Ok(Vec::new());
    };
    let bucket_seconds = bucket_seconds.max(1);

    let sql = format!("SELECT ts, tool, flagged FROM ({EVENTS_UNION_SQL}) ORDER BY ts");
    let mut stmt = conn.prepare(&sql)?;
    let rows: Vec<(String, String, i64)> =
        stmt.query_map(params![start, end], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_>>()?;

    let mut lanes: BTreeMap<String, BTreeMap<i64, (u32, bool)>> = BTreeMap::new();
    for (ts, tool, flagged) in rows {
        let Ok(t) = NaiveDateTime::parse_from_str(&ts, TS_FMT) else { continue };
        let idx = (t - start_dt).num_seconds().max(0) / bucket_seconds;
        let entry = lanes.entry(tool).or_default().entry(idx).or_insert((0, false));
        entry.0 += 1;
        entry.1 |= flagged != 0;
    }

    Ok(lanes
        .into_iter()
        .map(|(tool, buckets)| {
            let buckets = buckets
                .into_iter()
                .map(|(idx, (n, flagged))| RiverBucket {
                    t: (start_dt + Duration::seconds(idx * bucket_seconds)).format(TS_FMT).to_string(),
                    n,
                    flagged,
                })
                .collect();
            RiverLane { tool, buckets }
        })
        .collect())
}

/// Per-tool average CPU%/memory/GPU% over `[start, end)`. Used for BOTH the
/// live view (caller passes a narrow trailing window, e.g. the last ~8s) and
/// historical scrub (caller passes the full viewed window) — one function,
/// not two, so a tool that stopped ages out of the live view naturally
/// (no rows in a narrow recent window) instead of a "most recent row per
/// tool" query showing its last-ever reading forever.
pub fn perf_in_range(conn: &Connection, start: &str, end: &str) -> Result<Vec<ToolPerf>> {
    let sql = "SELECT tool, MAX(proc_count), AVG(cpu_pct), AVG(mem_bytes), AVG(gpu_pct)
               FROM perf_samples WHERE ts>=?1 AND ts<?2 GROUP BY tool";
    let mut stmt = conn.prepare(sql)?;
    let result = stmt
        .query_map(params![start, end], |r| {
            Ok(ToolPerf {
                tool: r.get(0)?,
                proc_count: r.get(1)?,
                cpu_pct: r.get(2)?,
                mem_bytes: r.get::<_, f64>(3)? as u64,
                gpu_pct: r.get(4)?,
            })
        })?
        .collect();
    result
}

// --- Daily report queries (aimon-report), typed ports of report.py's four
// ad hoc `ts LIKE '<day>%'` queries. ---

pub struct ProcessEventRow {
    pub ts: String,
    pub event: String,
    pub name: String,
    pub cmdline: String,
}

pub struct ChildEventRow {
    pub ts: String,
    pub parent_name: String,
    pub name: String,
    pub cmdline: String,
    pub flagged: bool,
}

pub struct NetSummaryRow {
    pub tool: String,
    pub host: String,
    pub port: u16,
    pub conns: u32,
    pub first: String,
    pub last: String,
}

pub struct DeviceEventRow {
    pub ts: String,
    pub device: String,
    pub app: String,
    pub started: String,
    pub stopped: String,
    pub is_ai: bool,
}

pub fn process_events_for_day(conn: &Connection, day: &str) -> Result<Vec<ProcessEventRow>> {
    let like = format!("{day}%");
    let mut stmt = conn.prepare("SELECT ts,event,name,cmdline FROM process_events WHERE ts LIKE ?1 ORDER BY ts")?;
    let result = stmt
        .query_map(params![like], |r| {
            Ok(ProcessEventRow { ts: r.get(0)?, event: r.get(1)?, name: r.get(2)?, cmdline: r.get(3)? })
        })?
        .collect();
    result
}

pub fn child_events_for_day(conn: &Connection, day: &str) -> Result<Vec<ChildEventRow>> {
    let like = format!("{day}%");
    let mut stmt =
        conn.prepare("SELECT ts,parent_name,name,cmdline,flagged FROM child_events WHERE ts LIKE ?1 ORDER BY ts")?;
    let result = stmt
        .query_map(params![like], |r| {
            Ok(ChildEventRow {
                ts: r.get(0)?,
                parent_name: r.get(1)?,
                name: r.get(2)?,
                cmdline: r.get(3)?,
                flagged: r.get::<_, i64>(4)? != 0,
            })
        })?
        .collect();
    result
}

pub fn net_summary_for_day(conn: &Connection, day: &str) -> Result<Vec<NetSummaryRow>> {
    let like = format!("{day}%");
    let sql = "SELECT name,COALESCE(NULLIF(rhost,''),raddr),rport,COUNT(*),MIN(ts),MAX(ts) FROM net_events
               WHERE ts LIKE ?1 GROUP BY 1,2,3 ORDER BY 4 DESC";
    let mut stmt = conn.prepare(sql)?;
    let result = stmt
        .query_map(params![like], |r| {
            Ok(NetSummaryRow {
                tool: r.get(0)?,
                host: r.get(1)?,
                port: r.get(2)?,
                conns: r.get(3)?,
                first: r.get(4)?,
                last: r.get(5)?,
            })
        })?
        .collect();
    result
}

pub fn device_events_for_day(conn: &Connection, day: &str) -> Result<Vec<DeviceEventRow>> {
    let like = format!("{day}%");
    let mut stmt = conn.prepare("SELECT ts,device,app,started,stopped,is_ai FROM device_events WHERE ts LIKE ?1 ORDER BY ts")?;
    let result = stmt
        .query_map(params![like], |r| {
            Ok(DeviceEventRow {
                ts: r.get(0)?,
                device: r.get(1)?,
                app: r.get(2)?,
                started: r.get(3)?,
                stopped: r.get(4)?,
                is_ai: r.get::<_, i64>(5)? != 0,
            })
        })?
        .collect();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn seeded_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        db::insert_process_event(&conn, "2026-09-19T10:00:00", "start", 100, "claude.exe", "C:\\claude.exe", "claude.exe").unwrap();
        db::insert_process_event(&conn, "2026-09-19T10:05:00", "stop", 100, "claude.exe", "C:\\claude.exe", "claude.exe").unwrap();
        db::insert_process_event(&conn, "2026-09-19T10:10:00", "start", 200, "ollama.exe", "C:\\ollama.exe", "ollama.exe").unwrap();
        db::insert_child_event(&conn, "2026-09-19T10:06:00", 100, "claude.exe", 101, "powershell.exe", "ls", true).unwrap();
        db::insert_child_event(&conn, "2026-09-19T10:11:00", 200, "ollama.exe", 201, "curl.exe", "curl x", true).unwrap();
        db::insert_net_event(&conn, "2026-09-19T10:12:00", 200, "ollama.exe", "1.2.3.4", 443, "", "ESTABLISHED").unwrap();
        db::insert_device_event(&conn, "2026-09-19T10:13:00", "microphone", "Teams", "2026-09-19T10:13:00", "", false).unwrap();
        conn
    }

    #[test]
    fn events_in_range_bounds_and_orders_correctly() {
        let conn = seeded_db();
        let events = events_in_range(&conn, "2026-09-19T10:05:00", "2026-09-19T10:12:00", 100).unwrap();
        // stop@10:05, child@10:06, start@10:10, child@10:11 — net@10:12 excluded (end exclusive)
        assert_eq!(events.len(), 4);
        assert_eq!(events[0].ts, "2026-09-19T10:11:00");
        assert_eq!(events.last().unwrap().ts, "2026-09-19T10:05:00");
    }

    #[test]
    fn oldest_event_ts_finds_the_true_minimum() {
        let conn = seeded_db();
        assert_eq!(oldest_event_ts(&conn).unwrap(), Some("2026-09-19T10:00:00".to_string()));
    }

    #[test]
    fn oldest_event_ts_none_for_empty_db() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        assert_eq!(oldest_event_ts(&conn).unwrap(), None);
    }

    #[test]
    fn running_now_excludes_stopped_tools() {
        let conn = seeded_db();
        let running = running_now(&conn, "2026-09-19T10:20:00").unwrap();
        assert_eq!(running.len(), 1);
        assert_eq!(running[0].name, "ollama.exe");
    }

    #[test]
    fn counts_in_range_matches_seeded_data() {
        let conn = seeded_db();
        let counts = counts_in_range(&conn, "2026-09-19T00:00:00", "2026-09-19T23:59:59").unwrap();
        assert_eq!(counts.launched, 2);
        assert_eq!(counts.commands, 2);
        assert_eq!(counts.flagged, 2);
        assert_eq!(counts.endpoints, 1);
    }

    #[test]
    fn state_for_range_hides_live_fields_when_not_live() {
        let conn = seeded_db();
        let past = state_for_range(&conn, "2026-09-19T10:00:00", "2026-09-19T10:15:00", "2026-09-19T12:00:00").unwrap();
        assert!(!past.is_live);
        assert!(past.running.is_empty());
        assert!(past.devices.is_empty());

        let live = state_for_range(&conn, "2026-09-19T10:00:00", "2026-09-19T12:00:00", "2026-09-19T12:00:00").unwrap();
        assert!(live.is_live);
        assert_eq!(live.running.len(), 1);
        assert_eq!(live.devices.len(), 1);
    }

    #[test]
    fn river_buckets_by_tool_and_time() {
        let conn = seeded_db();
        let lanes = river_for_range(&conn, "2026-09-19T10:00:00", "2026-09-19T10:15:00", 300).unwrap();
        let claude = lanes.iter().find(|l| l.tool == "claude.exe").unwrap();
        // start@10:00 (bucket 0), stop@10:05 + child@10:06 (bucket 1, 300s each)
        let total: u32 = claude.buckets.iter().map(|b| b.n).sum();
        assert_eq!(total, 3);
        assert!(claude.buckets.iter().any(|b| b.flagged));
    }

    #[test]
    fn report_queries_scope_to_the_given_day() {
        let conn = seeded_db();
        let procs = process_events_for_day(&conn, "2026-09-19").unwrap();
        assert_eq!(procs.len(), 3);
        assert_eq!(process_events_for_day(&conn, "2026-09-20").unwrap().len(), 0);

        let kids = child_events_for_day(&conn, "2026-09-19").unwrap();
        assert_eq!(kids.len(), 2);
        assert!(kids.iter().all(|k| k.flagged));

        let nets = net_summary_for_day(&conn, "2026-09-19").unwrap();
        assert_eq!(nets.len(), 1);
        assert_eq!(nets[0].conns, 1);

        let devs = device_events_for_day(&conn, "2026-09-19").unwrap();
        assert_eq!(devs.len(), 1);
        assert_eq!(devs[0].device, "microphone");
    }

    #[test]
    fn perf_in_range_averages_per_tool_and_excludes_out_of_range() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        db::insert_perf_sample(&conn, "2026-09-19T10:00:00", "claude.exe", 2, 10.0, 200_000_000, Some(1.0)).unwrap();
        db::insert_perf_sample(&conn, "2026-09-19T10:00:03", "claude.exe", 2, 20.0, 300_000_000, Some(3.0)).unwrap();
        db::insert_perf_sample(&conn, "2026-09-19T10:00:00", "ollama.exe", 1, 5.0, 100_000_000, None).unwrap();
        // Outside the queried window — must not affect the average.
        db::insert_perf_sample(&conn, "2026-09-19T12:00:00", "claude.exe", 1, 99.0, 999_000_000, Some(99.0)).unwrap();

        let rows = perf_in_range(&conn, "2026-09-19T09:00:00", "2026-09-19T11:00:00").unwrap();
        let claude = rows.iter().find(|r| r.tool == "claude.exe").unwrap();
        assert_eq!(claude.proc_count, 2);
        assert_eq!(claude.cpu_pct, 15.0);
        assert_eq!(claude.mem_bytes, 250_000_000);
        assert_eq!(claude.gpu_pct, Some(2.0));

        let ollama = rows.iter().find(|r| r.tool == "ollama.exe").unwrap();
        assert_eq!(ollama.gpu_pct, None);
    }

    #[test]
    fn perf_in_range_ages_out_stopped_tools() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        // A tool that stopped hours ago — its last row must not show up when
        // querying a narrow "live" window at a much later "now".
        db::insert_perf_sample(&conn, "2026-09-19T08:00:00", "old-tool.exe", 1, 50.0, 500_000_000, None).unwrap();
        db::insert_perf_sample(&conn, "2026-09-19T12:00:00", "claude.exe", 1, 5.0, 100_000_000, None).unwrap();

        let live_window = perf_in_range(&conn, "2026-09-19T11:59:52", "2026-09-19T12:00:01").unwrap();
        assert_eq!(live_window.len(), 1);
        assert_eq!(live_window[0].tool, "claude.exe");
    }
}
