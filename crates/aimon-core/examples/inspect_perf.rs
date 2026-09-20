//! Phase-1 verification tool for the performance-meters feature.
//! `cargo run -p aimon-core --example inspect_perf`
//! Prints pid/name/cpu%/mem every 3s for currently-matched AI processes.
//! Run side-by-side with Task Manager's Details tab (Ctrl+Shift+Esc) while
//! an AI tool is busy, and check the numbers land in the same ballpark.
use aimon_core::process::ProcSnapshot;
use std::time::Duration;

fn main() {
    let mut snap = ProcSnapshot::new();
    let my_pid = std::process::id();
    println!("Watching AI processes. Ctrl+C to stop.\n");
    loop {
        std::thread::sleep(Duration::from_secs(3));
        snap.refresh();
        let current = snap.current_ai_processes(my_pid);
        if current.is_empty() {
            println!("(no AI processes matched)");
            continue;
        }
        let mut rows: Vec<_> = current.into_iter().collect();
        rows.sort_by_key(|(pid, _)| *pid);
        for (pid, info) in rows {
            let cpu = snap.cpu_percent(pid).unwrap_or(-1.0);
            let mem_mb = snap.mem_bytes(pid).unwrap_or(0) as f64 / 1_048_576.0;
            println!("pid={pid:<7} cpu={cpu:6.2}%  mem={mem_mb:8.1}MB  {}", info.name);
        }
        println!();
    }
}
