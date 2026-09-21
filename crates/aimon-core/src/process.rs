//! Process enumeration and AI-tool matching, ported from the Python
//! collector's `psutil`-based process walk.
#![cfg(any(windows, target_os = "macos"))]

use crate::rules::is_ai;
use std::collections::HashMap;
use sysinfo::{Pid, ProcessRefreshKind, System};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcInfo {
    pub pid: u32,
    pub name: String,
    pub exe: String,
    pub cmdline: String,
    /// See `crate::apps::app_for`.
    pub app: String,
    /// When the process itself started (local time, the DB's timestamp
    /// format), if the OS reports it.
    pub started: Option<String>,
}

pub struct ProcSnapshot {
    sys: System,
    /// parent pid -> direct child pids, precomputed once per refresh so
    /// descendant walks are O(subtree size) instead of O(n) per node.
    children_by_parent: HashMap<u32, Vec<u32>>,
    num_cpus: usize,
}

impl ProcSnapshot {
    /// Creates a persistent snapshot. Must be reused across poll cycles (call
    /// `refresh()` each cycle, not `new()`) — `Process::cpu_usage()` is a
    /// delta since the last refresh *on the same `System` instance*, so
    /// recreating a fresh `System` every cycle (the old `refresh() -> Self`
    /// design) means CPU% would read as garbage forever, not just on the
    /// first cycle.
    pub fn new() -> Self {
        let sys = System::new();
        // std's count, not sysinfo's `cpus()` list — that requires its own
        // refresh_cpu_list() call to be populated and we don't otherwise need
        // per-core detail, just the logical core count to normalize cpu_usage().
        let num_cpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
        let mut snap = Self { sys, children_by_parent: HashMap::new(), num_cpus };
        snap.refresh();
        snap
    }

    pub fn refresh(&mut self) {
        // sysinfo's default refresh skips reading cmdline/exe for performance;
        // this tool's whole point is showing what AI tools run, so request it
        // explicitly. Without this, packaged/Electron apps (Claude, ChatGPT)
        // come back with an empty cmdline where psutil could read it.
        let refresh_kind = ProcessRefreshKind::everything();
        self.sys.refresh_processes_specifics(sysinfo::ProcessesToUpdate::All, true, refresh_kind);

        self.children_by_parent.clear();
        for (pid, proc) in self.sys.processes() {
            if let Some(parent) = proc.parent() {
                self.children_by_parent.entry(parent.as_u32()).or_default().push(pid.as_u32());
            }
        }
    }

    /// Percent of *total system capacity* (0..~100, comparable to Task
    /// Manager), not percent-of-one-core (sysinfo's raw `cpu_usage()` can
    /// read >100% on a busy multi-core process, classic `top` semantics).
    pub fn cpu_percent(&self, pid: u32) -> Option<f32> {
        self.sys.process(Pid::from_u32(pid)).map(|p| p.cpu_usage() / self.num_cpus as f32)
    }

    pub fn mem_bytes(&self, pid: u32) -> Option<u64> {
        self.sys.process(Pid::from_u32(pid)).map(|p| p.memory())
    }

    fn info_for(&self, pid: Pid, proc: &sysinfo::Process) -> ProcInfo {
        let name = proc.name().to_string_lossy().to_string();
        let exe = proc.exe().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
        let mut cmdline = proc
            .cmd()
            .iter()
            .map(|s| s.to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join(" ");
        truncate_at_char_boundary(&mut cmdline, 2000);
        let app = crate::apps::app_for(&name, &exe, &cmdline);
        let started = match proc.start_time() {
            0 => None,
            secs => chrono::DateTime::from_timestamp(secs as i64, 0)
                .map(|t| t.with_timezone(&chrono::Local).format("%Y-%m-%dT%H:%M:%S").to_string()),
        };
        ProcInfo { pid: pid.as_u32(), name, exe, cmdline, app, started }
    }

    pub fn info(&self, pid: u32) -> Option<ProcInfo> {
        let p = Pid::from_u32(pid);
        self.sys.process(p).map(|proc| self.info_for(p, proc))
    }

    /// All currently-running processes matched by `rules::is_ai`, excluding `exclude_pid`
    /// (the collector's own pid — mirrors Python's `info[0] != os.getpid()`).
    pub fn current_ai_processes(&self, exclude_pid: u32) -> HashMap<u32, ProcInfo> {
        let mut out = HashMap::new();
        for (pid, proc) in self.sys.processes() {
            if pid.as_u32() == exclude_pid {
                continue;
            }
            let info = self.info_for(*pid, proc);
            if is_ai(&info.name, &info.cmdline) {
                out.insert(info.pid, info);
            }
        }
        out
    }

    fn parent_of(&self, pid: u32) -> Option<u32> {
        self.sys.process(Pid::from_u32(pid))?.parent().map(|p| p.as_u32())
    }

    /// All descendants of `pid` (recursive), matching psutil's
    /// `Process.children(recursive=True)`.
    pub fn descendants(&self, pid: u32) -> Vec<u32> {
        let mut out = Vec::new();
        let mut stack: Vec<u32> = self.direct_children(pid).to_vec();
        while let Some(p) = stack.pop() {
            out.push(p);
            stack.extend_from_slice(self.direct_children(p));
        }
        out
    }

    fn direct_children(&self, pid: u32) -> &[u32] {
        self.children_by_parent.get(&pid).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Walks up from `start`'s parent until it finds a pid present in
    /// `ai_pids`, matching Python's nearest-AI-ancestor attribution logic.
    /// Returns `None` if no ancestor is currently a tracked AI process.
    pub fn nearest_ai_ancestor(&self, start: u32, ai_pids: &HashMap<u32, ProcInfo>) -> Option<u32> {
        let mut cur = self.parent_of(start)?;
        loop {
            if ai_pids.contains_key(&cur) {
                return Some(cur);
            }
            cur = self.parent_of(cur)?;
        }
    }
}

/// `String::truncate` panics mid-character; command lines are arbitrary UTF-8.
fn truncate_at_char_boundary(s: &mut String, max: usize) {
    if s.len() > max {
        let mut end = max;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
    }
}
