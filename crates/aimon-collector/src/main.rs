//! AI Activity Monitor - collector (Windows, home PC).
//! Polls every few seconds and logs to SQLite: AI tool processes starting/
//! stopping, child processes they spawn, network connections they make, and
//! mic/camera usage by any app. Direct port of the Python collector's main().
use aimon_core::{db, net::NetTracker, paths, process::ProcSnapshot, registry, rules};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

const POLL_SECONDS: u64 = 3;
const CACHE_CLEAR_THRESHOLD: usize = 50_000;

fn main() {
    let db_dir = paths::app_dir();
    std::fs::create_dir_all(&db_dir).expect("create AIMonitor dir");
    let conn = db::open_rw(&paths::db_path()).expect("open aimon.db for writing");

    let my_pid = std::process::id();
    let mut ai_pids: HashMap<u32, aimon_core::process::ProcInfo> = HashMap::new();
    let mut seen_children: HashSet<u32> = HashSet::new();
    let mut net = NetTracker::new();
    let mut seen_dev: HashMap<(String, String), (String, String)> = HashMap::new();

    // Baseline device usage so we only log new sessions from here on.
    for d in registry::read_device_usage() {
        seen_dev.insert((d.device, d.app), (d.started, d.stopped));
    }

    println!("AI Activity Monitor collector started. DB: {:?}", paths::db_path());

    loop {
        let ts = db::now_str();
        conn.execute_batch("BEGIN;").ok();

        let snap = ProcSnapshot::refresh();
        let current = snap.current_ai_processes(my_pid);

        for (pid, info) in &current {
            if !ai_pids.contains_key(pid) {
                let _ = db::insert_process_event(&conn, &ts, "start", info.pid, &info.name, &info.exe, &info.cmdline);
            }
        }
        for (pid, info) in &ai_pids {
            if !current.contains_key(pid) {
                let _ = db::insert_process_event(&conn, &ts, "stop", info.pid, &info.name, &info.exe, &info.cmdline);
            }
        }
        ai_pids = current;

        for (&pid, info) in &ai_pids {
            for child_pid in snap.descendants(pid) {
                if ai_pids.contains_key(&child_pid) || child_pid == my_pid || seen_children.contains(&child_pid) {
                    continue;
                }
                if snap.nearest_ai_ancestor(child_pid, &ai_pids) != Some(pid) {
                    continue;
                }
                if let Some(ci) = snap.info(child_pid) {
                    seen_children.insert(ci.pid);
                    let flagged = rules::is_sensitive_child(&ci.name);
                    let _ = db::insert_child_event(&conn, &ts, pid, &info.name, ci.pid, &ci.name, &ci.cmdline, flagged);
                }
            }
        }

        let ai_pid_list: Vec<u32> = ai_pids.keys().copied().collect();
        for (nc, host) in net.poll_new_connections(&ai_pid_list) {
            if let Some(info) = ai_pids.get(&nc.pid) {
                let _ = db::insert_net_event(&conn, &ts, nc.pid, &info.name, &nc.raddr.to_string(), nc.rport, &host, &nc.status);
            }
        }

        for d in registry::read_device_usage() {
            let key = (d.device.clone(), d.app.clone());
            let val = (d.started.clone(), d.stopped.clone());
            if seen_dev.get(&key) != Some(&val) {
                seen_dev.insert(key, val);
                let is_ai_app = rules::is_ai(&d.app, &d.app);
                let _ = db::insert_device_event(&conn, &ts, &d.device, &d.app, &d.started, &d.stopped, is_ai_app);
            }
        }

        conn.execute_batch("COMMIT;").ok();

        net.clear_if_large();
        if seen_children.len() > CACHE_CLEAR_THRESHOLD {
            seen_children.clear();
        }

        std::thread::sleep(Duration::from_secs(POLL_SECONDS));
    }
}
