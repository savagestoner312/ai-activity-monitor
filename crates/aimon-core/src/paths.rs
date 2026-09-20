use std::path::PathBuf;

/// `%LOCALAPPDATA%\AIMonitor`, falling back to `.` if the env var is unset —
/// mirrors Python's `os.environ.get("LOCALAPPDATA", ".")`.
pub fn app_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").unwrap_or_else(|| ".".into());
    PathBuf::from(base).join("AIMonitor")
}

pub fn db_path() -> PathBuf {
    app_dir().join("aimon.db")
}

pub fn reports_dir() -> PathBuf {
    app_dir().join("reports")
}

pub fn poll_seconds_path() -> PathBuf {
    app_dir().join("poll_seconds.txt")
}

const DEFAULT_POLL_SECONDS: u64 = 3;
const MIN_POLL_SECONDS: u64 = 1;
const MAX_POLL_SECONDS: u64 = 30;

/// The collector's poll interval, in seconds. Reads a single integer from
/// `poll_seconds.txt` in the app dir if present and valid (clamped to
/// [MIN_POLL_SECONDS, MAX_POLL_SECONDS] so a typo can't turn into a runaway
/// busy-loop or a de-facto-frozen collector); falls back to
/// `DEFAULT_POLL_SECONDS` if the file is missing, empty, or unparseable.
/// Trading a lower interval for less chance of missing a short-lived command
/// costs more CPU (a full process-table walk with cmdline reads every
/// cycle) — this is a manual text file rather than a live-reloadable setting
/// so that choice is deliberate, not something that silently drifts.
pub fn poll_seconds() -> u64 {
    std::fs::read_to_string(poll_seconds_path())
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .map(|v| v.clamp(MIN_POLL_SECONDS, MAX_POLL_SECONDS))
        .unwrap_or(DEFAULT_POLL_SECONDS)
}
