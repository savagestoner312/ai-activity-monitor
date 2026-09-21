//! Read-side queries backing the dashboard API and the report generator.
//! `events_in_range`/`state_for_range` generalize the Python dashboard's
//! tail-only `events(after)` / hardcoded-to-"today" `state()` into arbitrary
//! `[start, end)` windows, which is what makes history browsing possible.
use crate::{cmdsummary, netowner, rules};
use aimon_api_types::{
    AppSummary, Counts, DeviceChip, EndpointStat, EventKind, RiverBucket, RiverLane, RunningApp, StateSummary,
    ToolPerf, UnifiedEvent,
};
use chrono::{Duration, NaiveDateTime};
use rusqlite::{params, Connection, Result, Row};
use std::collections::BTreeMap;

const TS_FMT: &str = "%Y-%m-%dT%H:%M:%S";

/// Every loggable event, normalized to one shape. Mirrors the Python
/// dashboard's `events()` UNION-ALL query, parameterized by an end bound
/// instead of being tail-only. Columns: ts, kind, tool, detail, flagged, app,
/// child, pid, action, exe, began (a stop's matching start), ip.
const EVENTS_UNION_SQL: &str = "
    SELECT ts, 'process' AS kind, name AS tool, COALESCE(NULLIF(cmdline,''),exe) AS detail, 0 AS flagged,
           COALESCE(app,name) AS app, NULL AS child, pid, event AS action, exe,
           CASE WHEN event='stop' THEN (
             SELECT COALESCE(s.started,s.ts) FROM process_events s
             WHERE s.pid=p.pid AND s.event IN ('start','baseline') AND s.ts<=p.ts ORDER BY s.ts DESC LIMIT 1
           ) END AS began,
           NULL AS ip
      FROM process_events p WHERE event<>'session' AND ts>=?1 AND ts<?2
    UNION ALL
    SELECT ts, 'command', parent_name, cmdline, flagged, COALESCE(parent_app,parent_name), name, pid, NULL, NULL, NULL, NULL
      FROM child_events WHERE ts>=?1 AND ts<?2
    UNION ALL
    SELECT ts, 'network', name, COALESCE(NULLIF(rhost,''),raddr)||':'||rport, 0, COALESCE(app,name), NULL, pid, NULL, NULL, NULL, raddr
      FROM net_events WHERE ts>=?1 AND ts<?2
    UNION ALL
    SELECT ts, 'device', app, device||CASE WHEN stopped='' THEN ' in use' ELSE ' used until '||stopped END, is_ai,
           app, NULL, NULL, NULL, NULL, NULL, NULL
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

fn seconds_between(from: &str, to: &str) -> Option<i64> {
    let from = NaiveDateTime::parse_from_str(from, TS_FMT).ok()?;
    let to = NaiveDateTime::parse_from_str(to, TS_FMT).ok()?;
    Some((to - from).num_seconds().max(0))
}

fn event_from_row(r: &Row) -> Result<UnifiedEvent> {
    let ts: String = r.get(0)?;
    let kind = parse_kind(&r.get::<_, String>(1)?);
    let detail: String = r.get::<_, Option<String>>(3)?.unwrap_or_default();
    let child: Option<String> = r.get(6)?;
    let began: Option<String> = r.get(10)?;
    let ip: Option<String> = r.get(11)?;
    let (summary, routine, owner) = match kind {
        EventKind::Process | EventKind::Command => (
            cmdsummary::summarize(&detail),
            kind == EventKind::Command && child.as_deref().is_some_and(rules::is_routine_child),
            None,
        ),
        EventKind::Network => {
            let host = detail.rsplit_once(':').map_or(detail.as_str(), |(h, _)| h);
            (detail.clone(), false, netowner::owner(host, ip.as_deref().unwrap_or("")).map(str::to_string))
        }
        EventKind::Device => (detail.clone(), false, None),
    };
    Ok(UnifiedEvent {
        duration_s: began.as_deref().and_then(|b| seconds_between(b, &ts)),
        ts,
        kind,
        tool: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
        detail,
        flagged: r.get::<_, i64>(4)? != 0,
        app: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
        summary,
        routine,
        action: r.get(8)?,
        child,
        pid: r.get(7)?,
        exe: r.get::<_, Option<String>>(9)?.filter(|e| !e.is_empty()),
        owner,
    })
}

/// Events in `[start, end)`, newest first, capped at `limit` — used for both
/// live-tail (`start` = last-seen ts, `end` = now) and historical scrub
/// (explicit bounded range) fetches.
pub fn events_in_range(conn: &Connection, start: &str, end: &str, limit: i64) -> Result<Vec<UnifiedEvent>> {
    let sql = format!("SELECT * FROM ({EVENTS_UNION_SQL}) ORDER BY ts DESC LIMIT ?3");
    let mut stmt = conn.prepare(&sql)?;
    stmt.query_map(params![start, end, limit], event_from_row)?.collect()
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

/// Apps with a process started (or found already running) at or before
/// `as_of` and not stopped since, grouped by app. Only rows from the latest
/// collector session count: that session's `baseline` rows re-establish
/// whatever survived a collector restart, so a process that exited while the
/// collector was down doesn't linger. Only meaningful "as of now", so callers
/// should only surface this when the viewed window's end is effectively live.
pub fn running_now(conn: &Connection, as_of: &str) -> Result<Vec<RunningApp>> {
    let sql = "SELECT COALESCE(p.app,p.name), MAX(p.ts), p.started FROM process_events p
               WHERE p.event IN ('start','baseline') AND p.ts<=?1
               AND p.ts>=(SELECT COALESCE(MAX(ts),'') FROM process_events WHERE event='session' AND ts<=?1)
               AND NOT EXISTS (
                 SELECT 1 FROM process_events s
                 WHERE s.pid=p.pid AND s.event='stop' AND s.ts>=p.ts AND s.ts<=?1
               )
               GROUP BY p.pid";
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt
        .query_map(params![as_of], |r| {
            let ts: String = r.get(1)?;
            let started: Option<String> = r.get(2)?;
            Ok((r.get::<_, String>(0)?, started.unwrap_or(ts)))
        })?
        .collect::<Result<Vec<_>>>()?;
    let mut by_app: BTreeMap<String, RunningApp> = BTreeMap::new();
    for (app, since) in rows {
        let entry = by_app.entry(app.clone()).or_insert_with(|| RunningApp { app, procs: 0, since: since.clone() });
        entry.procs += 1;
        if since < entry.since {
            entry.since = since;
        }
    }
    let mut out: Vec<RunningApp> = by_app.into_values().collect();
    out.sort_by(|a, b| b.procs.cmp(&a.procs).then_with(|| a.app.cmp(&b.app)));
    Ok(out)
}

/// Devices with no recorded stop yet — inherently a "right now" concept,
/// same live-only caveat as `running_now`.
pub fn live_devices(conn: &Connection) -> Result<Vec<DeviceChip>> {
    let sql = "SELECT device, app FROM device_events d WHERE stopped=''
               AND ts=(SELECT MAX(ts) FROM device_events WHERE device=d.device AND app=d.app)";
    let mut stmt = conn.prepare(sql)?;
    stmt.query_map([], |r| Ok(DeviceChip { device: r.get(0)?, app: r.get(1)? }))?.collect()
}

fn count(conn: &Connection, sql: &str, start: &str, end: &str) -> Result<u32> {
    conn.query_row(sql, params![start, end], |r| r.get(0))
}

pub fn counts_in_range(conn: &Connection, start: &str, end: &str) -> Result<Counts> {
    let child_names = conn
        .prepare("SELECT name FROM child_events WHERE ts>=?1 AND ts<?2")?
        .query_map(params![start, end], |r| Ok(r.get::<_, Option<String>>(0)?.unwrap_or_default()))?
        .collect::<Result<Vec<_>>>()?;
    Ok(Counts {
        launched: count(conn, "SELECT COUNT(*) FROM process_events WHERE event='start' AND ts>=?1 AND ts<?2", start, end)?,
        already_running: count(
            conn,
            // Each collector restart re-baselines everything still running;
            // count the processes, not the restarts.
            "SELECT COUNT(DISTINCT pid) FROM process_events WHERE event='baseline' AND ts>=?1 AND ts<?2",
            start,
            end,
        )?,
        commands: child_names.len() as u32,
        routine_commands: child_names.iter().filter(|n| rules::is_routine_child(n)).count() as u32,
        flagged: count(conn, "SELECT COUNT(*) FROM child_events WHERE flagged=1 AND ts>=?1 AND ts<?2", start, end)?,
        endpoints: count(
            conn,
            "SELECT COUNT(DISTINCT COALESCE(NULLIF(rhost,''),raddr)||':'||rport) FROM net_events WHERE ts>=?1 AND ts<?2",
            start,
            end,
        )?,
    })
}

/// Endpoints contacted in `[start, end)`, one per `host:port` across every
/// app, busiest first.
pub fn endpoints_in_range(conn: &Connection, start: &str, end: &str, limit: usize) -> Result<Vec<EndpointStat>> {
    let sql = "SELECT COALESCE(NULLIF(rhost,''),raddr), MAX(raddr), rport, COALESCE(app,name), COUNT(*), MIN(ts), MAX(ts)
               FROM net_events WHERE ts>=?1 AND ts<?2 GROUP BY 1,3,4";
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(params![start, end], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, u16>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, u32>(4)?,
            r.get::<_, String>(5)?,
            r.get::<_, String>(6)?,
        ))
    })?;
    let mut by_endpoint: BTreeMap<(String, u16), EndpointStat> = BTreeMap::new();
    for row in rows {
        let (host, ip, port, app, n, first, last) = row?;
        let e = by_endpoint.entry((host.clone(), port)).or_insert_with(|| EndpointStat {
            owner: netowner::owner(&host, &ip).map(str::to_string),
            host,
            ip,
            port,
            apps: Vec::new(),
            n: 0,
            first: first.clone(),
            last: last.clone(),
        });
        e.apps.push(app);
        e.n += n;
        e.first = e.first.clone().min(first);
        e.last = e.last.clone().max(last);
    }
    let mut out: Vec<EndpointStat> = by_endpoint.into_values().collect();
    for e in &mut out {
        e.apps.sort();
    }
    out.sort_by(|a, b| b.n.cmp(&a.n).then_with(|| b.last.cmp(&a.last)));
    out.truncate(limit);
    Ok(out)
}

/// One row per app active in `[start, end)` or in `running`: what it
/// launched, ran and contacted. Running apps first, then the busiest.
pub fn apps_in_range(conn: &Connection, start: &str, end: &str, running: &[RunningApp]) -> Result<Vec<AppSummary>> {
    let mut apps: BTreeMap<String, AppSummary> = BTreeMap::new();
    let blank = |app: &str| AppSummary {
        app: app.to_string(),
        running: false,
        procs: 0,
        since: None,
        launches: 0,
        commands: 0,
        flagged: 0,
        endpoints: 0,
    };

    let mut stmt = conn.prepare(
        "SELECT COALESCE(app,name), SUM(event='start') FROM process_events
         WHERE event<>'session' AND ts>=?1 AND ts<?2 GROUP BY 1",
    )?;
    for row in stmt.query_map(params![start, end], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u32>(1)?)))? {
        let (app, launches) = row?;
        apps.entry(app.clone()).or_insert_with(|| blank(&app)).launches = launches;
    }

    let mut stmt = conn.prepare(
        "SELECT COALESCE(parent_app,parent_name), COUNT(*), SUM(flagged) FROM child_events
         WHERE ts>=?1 AND ts<?2 GROUP BY 1",
    )?;
    for row in stmt.query_map(params![start, end], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, u32>(1)?, r.get::<_, u32>(2)?))
    })? {
        let (app, commands, flagged) = row?;
        let e = apps.entry(app.clone()).or_insert_with(|| blank(&app));
        e.commands = commands;
        e.flagged = flagged;
    }

    let mut stmt = conn.prepare(
        "SELECT COALESCE(app,name), COUNT(DISTINCT COALESCE(NULLIF(rhost,''),raddr)||':'||rport) FROM net_events
         WHERE ts>=?1 AND ts<?2 GROUP BY 1",
    )?;
    for row in stmt.query_map(params![start, end], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u32>(1)?)))? {
        let (app, endpoints) = row?;
        apps.entry(app.clone()).or_insert_with(|| blank(&app)).endpoints = endpoints;
    }

    for r in running {
        let e = apps.entry(r.app.clone()).or_insert_with(|| blank(&r.app));
        e.running = true;
        e.procs = r.procs;
        e.since = Some(r.since.clone());
    }

    let mut out: Vec<AppSummary> = apps.into_values().collect();
    out.sort_by_key(|a| (!a.running, std::cmp::Reverse(a.launches + a.commands + a.endpoints)));
    Ok(out)
}

/// A window's full summary. `is_live` (and therefore `running`/`devices`)
/// reflects whether `end` reaches the server's current time — a past window
/// has no "currently running" tools by definition.
pub fn state_for_range(conn: &Connection, start: &str, end: &str, now: &str) -> Result<StateSummary> {
    let is_live = end >= now;
    let (running, devices) =
        if is_live { (running_now(conn, now)?, live_devices(conn)?) } else { (Vec::new(), Vec::new()) };
    let apps = apps_in_range(conn, start, end, &running)?;
    Ok(StateSummary {
        range_start: start.to_string(),
        range_end: end.to_string(),
        is_live,
        running,
        counts: counts_in_range(conn, start, end)?,
        devices,
        endpoints: endpoints_in_range(conn, start, end, 10)?,
        apps,
    })
}

/// Per-app, per-time-bucket activity counts for the river visualization.
/// Bucketing happens in Rust (not SQL) since it's a personal-scale dataset
/// and this keeps the query portable — no julianday gymnastics.
pub fn river_for_range(conn: &Connection, start: &str, end: &str, bucket_seconds: i64) -> Result<Vec<RiverLane>> {
    let Ok(start_dt) = NaiveDateTime::parse_from_str(start, TS_FMT) else {
        return Ok(Vec::new());
    };
    let bucket_seconds = bucket_seconds.max(1);

    let sql = format!("SELECT ts, app, flagged FROM ({EVENTS_UNION_SQL}) ORDER BY ts");
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
    
    stmt
        .query_map(params![start, end], |r| {
            Ok(ToolPerf {
                tool: r.get(0)?,
                proc_count: r.get(1)?,
                cpu_pct: r.get(2)?,
                mem_bytes: r.get::<_, f64>(3)? as u64,
                gpu_pct: r.get(4)?,
            })
        })?
        .collect()
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
    
    stmt
        .query_map(params![like], |r| {
            Ok(ProcessEventRow { ts: r.get(0)?, event: r.get(1)?, name: r.get(2)?, cmdline: r.get(3)? })
        })?
        .collect()
}

pub fn child_events_for_day(conn: &Connection, day: &str) -> Result<Vec<ChildEventRow>> {
    let like = format!("{day}%");
    let mut stmt =
        conn.prepare("SELECT ts,parent_name,name,cmdline,flagged FROM child_events WHERE ts LIKE ?1 ORDER BY ts")?;
    
    stmt
        .query_map(params![like], |r| {
            Ok(ChildEventRow {
                ts: r.get(0)?,
                parent_name: r.get(1)?,
                name: r.get(2)?,
                cmdline: r.get(3)?,
                flagged: r.get::<_, i64>(4)? != 0,
            })
        })?
        .collect()
}

pub fn net_summary_for_day(conn: &Connection, day: &str) -> Result<Vec<NetSummaryRow>> {
    let like = format!("{day}%");
    let sql = "SELECT name,COALESCE(NULLIF(rhost,''),raddr),rport,COUNT(*),MIN(ts),MAX(ts) FROM net_events
               WHERE ts LIKE ?1 GROUP BY 1,2,3 ORDER BY 4 DESC";
    let mut stmt = conn.prepare(sql)?;
    
    stmt
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
        .collect()
}

pub fn device_events_for_day(conn: &Connection, day: &str) -> Result<Vec<DeviceEventRow>> {
    let like = format!("{day}%");
    let mut stmt = conn.prepare("SELECT ts,device,app,started,stopped,is_ai FROM device_events WHERE ts LIKE ?1 ORDER BY ts")?;
    
    stmt
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
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn seeded_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        db::migrate(&conn).unwrap();
        db::insert_process_event(&conn, "2026-09-19T10:00:00", "start", 100, "claude.exe", "C:\\claude.exe", "claude.exe", "Claude Code", None).unwrap();
        db::insert_process_event(&conn, "2026-09-19T10:05:00", "stop", 100, "claude.exe", "C:\\claude.exe", "claude.exe", "Claude Code", None).unwrap();
        db::insert_process_event(&conn, "2026-09-19T10:10:00", "start", 200, "ollama.exe", "C:\\ollama.exe", "ollama.exe", "ollama", None).unwrap();
        db::insert_child_event(&conn, "2026-09-19T10:06:00", 100, "claude.exe", "Claude Code", 101, "powershell.exe", "ls", true).unwrap();
        db::insert_child_event(&conn, "2026-09-19T10:11:00", 200, "ollama.exe", "ollama", 201, "curl.exe", "curl x", true).unwrap();
        db::insert_net_event(&conn, "2026-09-19T10:12:00", 200, "ollama.exe", "ollama", "1.2.3.4", 443, "", "ESTABLISHED").unwrap();
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
        assert_eq!(running[0].app, "ollama");
    }

    #[test]
    fn running_now_groups_by_app_and_forgets_previous_sessions() {
        let conn = seeded_db();
        // Session 1 saw pid 300 start; the collector then restarted and its
        // baseline only found pids 400 and 401, both Claude.
        db::insert_process_event(&conn, "2026-09-19T10:14:00", "start", 300, "gone", "", "", "gone", None).unwrap();
        db::insert_session_marker(&conn, "2026-09-19T10:15:00", 1).unwrap();
        for pid in [400, 401] {
            db::insert_process_event(&conn, "2026-09-19T10:15:00", "baseline", pid, "Claude Helper", "", "", "Claude", Some("2026-09-19T09:00:00")).unwrap();
        }
        let running = running_now(&conn, "2026-09-19T10:20:00").unwrap();
        assert_eq!(running.len(), 1);
        assert_eq!(running[0].app, "Claude");
        assert_eq!(running[0].procs, 2);
        assert_eq!(running[0].since, "2026-09-19T09:00:00");

        let counts = counts_in_range(&conn, "2026-09-19T00:00:00", "2026-09-19T23:59:59").unwrap();
        assert_eq!(counts.launched, 3);
        assert_eq!(counts.already_running, 2);
    }

    #[test]
    fn events_carry_app_summary_and_duration() {
        let conn = seeded_db();
        db::insert_child_event(&conn, "2026-09-19T10:07:00", 100, "claude.exe", "Claude Code", 102, "caffeinate", "caffeinate -i", false).unwrap();
        let events = events_in_range(&conn, "2026-09-19T00:00:00", "2026-09-19T23:59:59", 100).unwrap();
        let stop = events.iter().find(|e| e.action.as_deref() == Some("stop")).unwrap();
        assert_eq!(stop.app, "Claude Code");
        assert_eq!(stop.duration_s, Some(300));
        let routine = events.iter().find(|e| e.child.as_deref() == Some("caffeinate")).unwrap();
        assert!(routine.routine);
        let net = events.iter().find(|e| e.kind == EventKind::Network).unwrap();
        assert_eq!(net.app, "ollama");
        assert_eq!(net.summary, "1.2.3.4:443");
    }

    #[test]
    fn endpoints_merge_across_apps() {
        let conn = seeded_db();
        db::insert_net_event(&conn, "2026-09-19T10:13:00", 100, "claude.exe", "Claude Code", "1.2.3.4", 443, "", "ESTABLISHED").unwrap();
        db::insert_net_event(&conn, "2026-09-19T10:14:00", 100, "claude.exe", "Claude Code", "160.79.104.10", 443, "", "ESTABLISHED").unwrap();
        let eps = endpoints_in_range(&conn, "2026-09-19T00:00:00", "2026-09-19T23:59:59", 10).unwrap();
        assert_eq!(eps.len(), 2);
        assert_eq!(eps[0].host, "1.2.3.4");
        assert_eq!(eps[0].n, 2);
        assert_eq!(eps[0].apps, ["Claude Code", "ollama"]);
        assert_eq!(eps[1].owner.as_deref(), Some("Anthropic"));

        let apps = apps_in_range(&conn, "2026-09-19T00:00:00", "2026-09-19T23:59:59", &[]).unwrap();
        let claude = apps.iter().find(|a| a.app == "Claude Code").unwrap();
        assert_eq!((claude.launches, claude.commands, claude.flagged, claude.endpoints), (1, 1, 1, 2));
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
        let claude = lanes.iter().find(|l| l.tool == "Claude Code").unwrap();
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
