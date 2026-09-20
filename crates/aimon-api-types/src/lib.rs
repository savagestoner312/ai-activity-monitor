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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnifiedEvent {
    pub ts: String,
    pub kind: EventKind,
    pub tool: String,
    pub detail: String,
    pub flagged: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunningTool {
    pub name: String,
    pub pid: i64,
    pub since: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Counts {
    pub launched: u32,
    pub commands: u32,
    pub flagged: u32,
    pub endpoints: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceChip {
    pub device: String,
    pub app: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndpointStat {
    pub tool: String,
    pub host: String,
    pub n: u32,
}

/// Summary for a window `[range_start, range_end)`. `running`/`devices` are only
/// meaningful (non-empty) when the window's end is effectively "now" — the
/// live-window case — since nothing is "currently running" in a past window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateSummary {
    pub range_start: String,
    pub range_end: String,
    pub is_live: bool,
    pub running: Vec<RunningTool>,
    pub counts: Counts,
    pub devices: Vec<DeviceChip>,
    pub endpoints: Vec<EndpointStat>,
}

/// Bounds for the history slider: the oldest recorded event and the server's
/// current wall-clock time (same timestamp format as everything else).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetaResponse {
    pub oldest_ts: Option<String>,
    pub now: String,
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
