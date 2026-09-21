//! macOS stand-in for the Windows PDH GPU sampler. macOS exposes no
//! unprivileged per-process GPU% the way Windows' "GPU Engine" counters do
//! (`powermetrics` needs root), so this always reports unavailable: perf
//! samples get `gpu_pct = None` and the dashboard hides its GPU column.

use std::collections::{HashMap, HashSet};

pub struct GpuSampler;

impl GpuSampler {
    pub fn new() -> Self {
        Self
    }

    pub fn sample(&mut self, _watched: &HashSet<u32>) -> HashMap<u32, f32> {
        HashMap::new()
    }

    pub fn sample_all(&mut self) -> HashMap<u32, f32> {
        HashMap::new()
    }
}
