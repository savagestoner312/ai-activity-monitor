//! Reads mic/camera usage history from Windows' own privacy records
//! (HKCU CapabilityAccessManager ConsentStore). Direct port of the Python
//! collector's `read_device_usage()` / `filetime_to_iso()`.
#![cfg(windows)]

use chrono::{Duration, NaiveDate};
use winreg::enums::HKEY_CURRENT_USER;
use winreg::RegKey;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceUsage {
    pub device: String,
    pub app: String,
    pub started: String,
    pub stopped: String,
}

const BASE: &str = r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore";

/// FILETIME (100ns ticks since 1601-01-01 UTC) -> the same local-time ISO
/// string format used everywhere else. Matches Python's `filetime_to_iso`.
fn filetime_to_iso(ft: u64) -> String {
    if ft == 0 {
        return String::new();
    }
    let epoch = NaiveDate::from_ymd_opt(1601, 1, 1).unwrap().and_hms_opt(0, 0, 0).unwrap();
    let dt = epoch + Duration::microseconds((ft / 10) as i64);
    dt.format("%Y-%m-%dT%H:%M:%S").to_string()
}

/// Yields (device, app, started, stopped) for every app with a recorded
/// mic/camera session, across both packaged and non-packaged (Win32) apps.
pub fn read_device_usage() -> Vec<DeviceUsage> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let mut out = Vec::new();

    for device in ["microphone", "webcam"] {
        for suffix in ["", "\\NonPackaged"] {
            let path = format!("{BASE}\\{device}{suffix}");
            let Ok(key) = hkcu.open_subkey(&path) else { continue };

            for app in key.enum_keys().flatten() {
                if app == "NonPackaged" {
                    continue;
                }
                let Ok(app_key) = key.open_subkey(&app) else { continue };
                let (Ok(start), Ok(stop)) = (
                    app_key.get_value::<u64, _>("LastUsedTimeStart"),
                    app_key.get_value::<u64, _>("LastUsedTimeStop"),
                ) else {
                    continue;
                };
                out.push(DeviceUsage {
                    device: device.to_string(),
                    app: app.replace('#', "\\"),
                    started: filetime_to_iso(start),
                    stopped: filetime_to_iso(stop),
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filetime_zero_is_empty() {
        assert_eq!(filetime_to_iso(0), "");
    }

    #[test]
    fn filetime_shape_matches_iso_format() {
        // 2024-01-01T00:00:00 UTC, in 100ns ticks since 1601-01-01.
        let ft: u64 = 133_484_832_000_000_000;
        let s = filetime_to_iso(ft);
        assert_eq!(s.len(), 19);
        assert_eq!(s.as_bytes()[10], b'T');
    }

    #[test]
    fn read_device_usage_runs_without_panicking() {
        // Smoke test only — actual mic/camera data depends on the machine's
        // real usage history, verified separately on Windows.
        let _ = read_device_usage();
    }
}
