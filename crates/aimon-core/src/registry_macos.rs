//! macOS stand-in for the Windows mic/camera history reader. Windows keeps a
//! per-app last-used record in the registry; macOS keeps no equivalent
//! history (TCC records permission grants, not sessions), so no device
//! events are logged on macOS.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceUsage {
    pub device: String,
    pub app: String,
    pub started: String,
    pub stopped: String,
}

pub fn read_device_usage() -> Vec<DeviceUsage> {
    Vec::new()
}
