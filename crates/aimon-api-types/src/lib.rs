//! Wire types shared between `aimon-dashboard` (server) and `aimon-ui` (Leptos/wasm frontend).
//! Must stay free of native-only dependencies so it compiles for wasm32-unknown-unknown.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EventKind {
    Process,
    Command,
    Network,
    Device,
}

/// One feed row. `detail` is the full text (command line, `host:port`,
/// device state); `summary` is what the row shows before it's expanded.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnifiedEvent {
    pub ts: String,
    pub kind: EventKind,
    /// The raw process name (the parent's, for a command).
    pub tool: String,
    pub detail: String,
    pub flagged: bool,
    /// The app `tool` belongs to (see `aimon_core::apps`).
    #[serde(default)]
    pub app: String,
    #[serde(default)]
    pub summary: String,
    /// A routine command (keep-awake, pipeline plumbing), hidden by default.
    #[serde(default)]
    pub routine: bool,
    /// Process rows: `start`, `baseline` (already running when the collector
    /// started) or `stop`.
    #[serde(default)]
    pub action: Option<String>,
    /// Command rows: the name of the process that was spawned.
    #[serde(default)]
    pub child: Option<String>,
    #[serde(default)]
    pub pid: Option<i64>,
    /// Process rows: the executable path.
    #[serde(default)]
    pub exe: Option<String>,
    /// Stop rows: how long the process ran, when its start is known.
    #[serde(default)]
    pub duration_s: Option<i64>,
    /// Network rows: best-effort owner of the endpoint.
    #[serde(default)]
    pub owner: Option<String>,
}

/// An app with at least one process running now. `since` is when its
/// longest-running process started.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunningApp {
    pub app: String,
    pub procs: u32,
    pub since: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Counts {
    /// Processes started while the collector was watching.
    pub launched: u32,
    pub commands: u32,
    pub flagged: u32,
    /// Distinct `host:port` endpoints.
    pub endpoints: u32,
    /// Processes that were already running when the collector started.
    #[serde(default)]
    pub already_running: u32,
    #[serde(default)]
    pub routine_commands: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceChip {
    pub device: String,
    pub app: String,
}

/// Connections to one `host:port` in a window, across every app.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndpointStat {
    /// Reverse-DNS name, or the IP when there is none.
    pub host: String,
    pub ip: String,
    pub port: u16,
    /// Best-effort, from a hand-kept table (see `aimon_core::netowner`).
    pub owner: Option<String>,
    pub apps: Vec<String>,
    pub n: u32,
    pub first: String,
    pub last: String,
}

/// Per-app activity in a window. `running`/`procs`/`since` describe now, and
/// are only set on a live window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSummary {
    pub app: String,
    pub running: bool,
    pub procs: u32,
    pub since: Option<String>,
    pub launches: u32,
    pub commands: u32,
    pub flagged: u32,
    pub endpoints: u32,
}

/// Summary for a window `[range_start, range_end)`. `running`/`devices` are only
/// meaningful (non-empty) when the window's end is effectively "now" — the
/// live-window case — since nothing is "currently running" in a past window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateSummary {
    pub range_start: String,
    pub range_end: String,
    pub is_live: bool,
    pub running: Vec<RunningApp>,
    pub counts: Counts,
    pub devices: Vec<DeviceChip>,
    /// The busiest endpoints; `counts.endpoints` is the full total.
    pub endpoints: Vec<EndpointStat>,
    #[serde(default)]
    pub apps: Vec<AppSummary>,
}

/// Bounds for the history slider: the oldest recorded event and the server's
/// current wall-clock time (same timestamp format as everything else).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetaResponse {
    pub oldest_ts: Option<String>,
    pub now: String,
    /// The collector's platform, `std::env::consts::OS` ("macos", "windows").
    #[serde(default)]
    pub os: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiverBucket {
    pub t: String,
    pub n: u32,
    pub flagged: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiverLane {
    pub tool: String,
    pub buckets: Vec<RiverBucket>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiverResponse {
    pub lanes: Vec<RiverLane>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolPerf {
    pub tool: String,
    pub proc_count: u32,
    pub cpu_pct: f32,
    pub mem_bytes: u64,
    pub gpu_pct: Option<f32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PerfTotal {
    pub proc_count: u32,
    pub cpu_pct: f32,
    pub mem_bytes: u64,
    pub gpu_pct: Option<f32>,
}

/// `tools`/`total` hold time-averaged values on a non-live window (unlike
/// StateSummary's running/devices, which are empty when not live) — "average
/// load in this window" is well-defined for the past, so the frontend swaps
/// rendering mode (animated vs. static) rather than hiding the panel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerfSummary {
    pub ts: String,
    pub is_live: bool,
    pub gpu_available: bool,
    pub total: PerfTotal,
    pub tools: Vec<ToolPerf>,
}
