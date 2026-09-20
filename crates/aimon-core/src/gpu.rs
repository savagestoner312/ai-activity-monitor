//! Per-process GPU utilization via Windows' "GPU Engine" performance counter
//! category — the same data source Task Manager's own GPU columns read.
//! sysinfo has no per-process GPU API at all; this is why PDH is used
//! directly instead. Degrades to `available = false` (never crashes the
//! collector, never blocks CPU/mem collection) if the counter category
//! isn't present on this machine.
#![cfg(windows)]

use std::collections::{HashMap, HashSet};
use windows::core::PCWSTR;
use windows::Win32::System::Performance::{
    PdhAddCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW, PdhOpenQueryW, PDH_FMT_COUNTERVALUE_ITEM_W,
    PDH_FMT_DOUBLE,
};

const COUNTER_PATH: &str = "\\GPU Engine(*)\\Utilization Percentage";
const PDH_SUCCESS: u32 = 0;

pub struct GpuSampler {
    query: isize,
    counter: isize,
    available: bool,
}

impl GpuSampler {
    pub fn new() -> Self {
        unsafe {
            let mut query: isize = 0;
            if PdhOpenQueryW(PCWSTR::null(), 0, &mut query) != PDH_SUCCESS {
                return Self::unavailable();
            }
            let path_wide: Vec<u16> = COUNTER_PATH.encode_utf16().chain(std::iter::once(0)).collect();
            let mut counter: isize = 0;
            if PdhAddCounterW(query, PCWSTR(path_wide.as_ptr()), 0, &mut counter) != PDH_SUCCESS {
                let _ = PdhCloseQuery(query);
                return Self::unavailable();
            }
            // Prime with one collect so the first real sample() call has a baseline
            // (this rate counter, like sysinfo's CPU%, needs two collects to mean anything).
            let _ = PdhCollectQueryData(query);
            Self { query, counter, available: true }
        }
    }

    fn unavailable() -> Self {
        Self { query: 0, counter: 0, available: false }
    }

    /// Summed GPU% per watched pid (across all of that pid's engine-type
    /// instances and GPUs), matching Task Manager's per-process GPU%
    /// convention. Empty map, no-op, if GPU sampling isn't available.
    pub fn sample(&mut self, watched: &HashSet<u32>) -> HashMap<u32, f32> {
        self.sample_all().into_iter().filter(|(pid, _)| watched.contains(pid)).collect()
    }

    /// Same as `sample`, but unfiltered — every pid the counter category
    /// currently reports a non-zero-status reading for. Used by the
    /// diagnostic example; production code should prefer `sample` so it
    /// isn't paying to parse/sum entries for processes nobody cares about.
    pub fn sample_all(&mut self) -> HashMap<u32, f32> {
        let mut out = HashMap::new();
        if !self.available {
            return out;
        }
        unsafe {
            if PdhCollectQueryData(self.query) != PDH_SUCCESS {
                return out;
            }

            let mut buffer_size: u32 = 0;
            let mut item_count: u32 = 0;
            // First call with no buffer to learn the required size (expected to
            // "fail" with PDH_MORE_DATA — that's not a real error here).
            let _ = PdhGetFormattedCounterArrayW(self.counter, PDH_FMT_DOUBLE, &mut buffer_size, &mut item_count, None);
            if buffer_size == 0 || item_count == 0 {
                return out;
            }

            let mut buf: Vec<u8> = vec![0u8; buffer_size as usize];
            let items_ptr = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
            if PdhGetFormattedCounterArrayW(self.counter, PDH_FMT_DOUBLE, &mut buffer_size, &mut item_count, Some(items_ptr))
                != PDH_SUCCESS
            {
                return out;
            }

            let items = std::slice::from_raw_parts(items_ptr, item_count as usize);
            for item in items {
                let Ok(name) = item.szName.to_string() else { continue };
                let Some(pid) = parse_pid(&name) else { continue };
                if item.FmtValue.CStatus != PDH_SUCCESS {
                    continue;
                }
                let value = item.FmtValue.Anonymous.doubleValue as f32;
                *out.entry(pid).or_insert(0.0) += value;
            }
        }
        out
    }
}

/// Pulls the pid out of instance names like
/// "pid_1234_luid_0x00000000_0x0000abcd_phys_0_eng_0_engtype_3D".
fn parse_pid(instance_name: &str) -> Option<u32> {
    let rest = instance_name.strip_prefix("pid_")?;
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

impl Drop for GpuSampler {
    fn drop(&mut self) {
        if self.available {
            unsafe {
                let _ = PdhCloseQuery(self.query);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_pid_extracts_digits() {
        assert_eq!(parse_pid("pid_1234_luid_0x00000000_0x0000abcd_phys_0_eng_0_engtype_3D"), Some(1234));
        assert_eq!(parse_pid("garbage"), None);
    }
}
