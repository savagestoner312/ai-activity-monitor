pub mod apps;
pub mod cmdsummary;
pub mod db;
pub mod netowner;
pub mod paths;
pub mod queries;
pub mod rules;

#[cfg(any(windows, target_os = "macos"))]
pub mod net;
#[cfg(any(windows, target_os = "macos"))]
pub mod process;

// GPU and mic/camera history read Windows-only sources (PDH counters, the
// CapabilityAccessManager registry keys). macOS gets stand-ins with the same
// API that report "unavailable" / nothing, so the collector runs unchanged.
#[cfg(windows)]
pub mod gpu;
#[cfg(target_os = "macos")]
#[path = "gpu_macos.rs"]
pub mod gpu;
#[cfg(windows)]
pub mod registry;
#[cfg(target_os = "macos")]
#[path = "registry_macos.rs"]
pub mod registry;
