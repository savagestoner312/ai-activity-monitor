//! Phase-3 verification tool: dump perf_samples rows from a DB.
//! `cargo run -p aimon-core --example inspect_perf_db -- <path-to-aimon.db>`
use aimon_core::db;

fn main() {
    let path = std::env::args().nth(1).expect("usage: inspect_perf_db <path>");
    let conn = db::open_ro(std::path::Path::new(&path)).expect("open db");

    let count: i64 = conn.query_row("SELECT COUNT(*) FROM perf_samples", [], |r| r.get(0)).unwrap();
    println!("perf_samples: {count} rows\n");

    let mut stmt = conn
        .prepare("SELECT ts, tool, proc_count, cpu_pct, mem_bytes, gpu_pct FROM perf_samples ORDER BY ts DESC LIMIT 20")
        .unwrap();
    let mut rows = stmt.query([]).unwrap();
    while let Some(row) = rows.next().unwrap() {
        let ts: String = row.get(0).unwrap();
        let tool: String = row.get(1).unwrap();
        let proc_count: i64 = row.get(2).unwrap();
        let cpu_pct: f64 = row.get(3).unwrap();
        let mem_bytes: i64 = row.get(4).unwrap();
        let gpu_pct: Option<f64> = row.get(5).unwrap();
        println!(
            "{ts} | {tool:<20} | procs={proc_count} | cpu={cpu_pct:6.2}% | mem={:8.1}MB | gpu={}",
            mem_bytes as f64 / 1_048_576.0,
            gpu_pct.map(|g| format!("{g:.2}%")).unwrap_or_else(|| "n/a".to_string())
        );
    }
}
