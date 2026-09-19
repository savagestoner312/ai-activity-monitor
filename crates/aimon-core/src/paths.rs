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
