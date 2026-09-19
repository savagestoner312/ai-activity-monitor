//! Process enumeration and AI-tool matching, ported from the Python
//! collector's `psutil`-based process walk.
#![cfg(windows)]

use crate::rules::is_ai;
use std::collections::HashMap;
use sysinfo::{Pid, ProcessRefreshKind, System};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcInfo {
    pub pid: u32,
    pub name: String,
    pub exe: String,
    pub cmdline: String,
}

pub struct ProcSnapshot {
    sys: System,
    /// parent pid -> direct child pids, precomputed once per refresh so
    /// descendant walks are O(subtree size) instead of O(n) per node.
    children_by_parent: HashMap<u32, Vec<u32>>,
}

impl ProcSnapshot {
    pub fn refresh() -> Self {
        let mut sys = System::new();
        // sysinfo's default refresh skips reading cmdline/exe for performance;
        // this tool's whole point is showing what AI tools run, so request it
        // explicitly. Without this, packaged/Electron apps (Claude, ChatGPT)
        // come back with an empty cmdline where psutil could read it.
        let refresh_kind = ProcessRefreshKind::everything();
        sys.refresh_processes_specifics(sysinfo::ProcessesToUpdate::All, true, refresh_kind);

        let mut children_by_parent: HashMap<u32, Vec<u32>> = HashMap::new();
        for (pid, proc) in sys.processes() {
            if let Some(parent) = proc.parent() {
                children_by_parent.entry(parent.as_u32()).or_default().push(pid.as_u32());
            }
        }
        Self { sys, children_by_parent }
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
        cmdline.truncate(2000);
        ProcInfo { pid: pid.as_u32(), name, exe, cmdline }
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
