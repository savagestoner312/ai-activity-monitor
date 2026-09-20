//! Phase-2 verification tool for the performance-meters feature.
//! `cargo run -p aimon-core --example inspect_gpu`
//! Samples ALL currently-running pids (not just AI-matched ones, to make it
//! easy to see real values from any GPU-using app) every 3s and prints
//! non-zero GPU% readings. Cross-check against Task Manager's GPU column.
use aimon_core::{gpu::GpuSampler, process::ProcSnapshot};
use std::time::Duration;

fn main() {
    let mut gpu = GpuSampler::new();
    let mut snap = ProcSnapshot::new();
    println!("Sampling GPU Engine counters every 3s. Ctrl+C to stop.\n");
    loop {
        std::thread::sleep(Duration::from_secs(3));
        snap.refresh();

        let readings = gpu.sample_all();
        if readings.is_empty() {
            println!("(no non-zero GPU engine readings this cycle)");
            continue;
        }
        let mut rows: Vec<_> = readings.into_iter().collect();
        rows.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        for (pid, pct) in rows.into_iter().take(10) {
            let name = snap.info(pid).map(|i| i.name).unwrap_or_else(|| "?".to_string());
            println!("pid={pid:<7} gpu={pct:6.2}%  {name}");
        }
        println!();
    }
}
