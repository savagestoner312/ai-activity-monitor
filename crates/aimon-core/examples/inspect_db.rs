//! Dev tool: `cargo run -p aimon-core --example inspect_db -- <path-to-aimon.db>`
//! Dumps row counts and the most recent rows of each table.
use aimon_core::db;

fn main() {
    let path = std::env::args().nth(1).expect("usage: inspect_db <path>");
    let conn = db::open_ro(std::path::Path::new(&path)).expect("open db");

    for table in ["process_events", "child_events", "net_events", "device_events"] {
        let count: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        println!("== {table}: {count} rows ==");
        let cols: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM pragma_table_info('{table}')"), [], |r| r.get(0))
            .unwrap();
        let sql = format!("SELECT * FROM {table} ORDER BY ts DESC LIMIT 5");
        let mut stmt = conn.prepare(&sql).unwrap();
        let mut rows = stmt.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            let mut parts = Vec::new();
            for i in 0..cols {
                let v: String = row
                    .get::<_, Option<String>>(i as usize)
                    .ok()
                    .flatten()
                    .or_else(|| row.get::<_, Option<i64>>(i as usize).ok().flatten().map(|n| n.to_string()))
                    .unwrap_or_default();
                parts.push(v);
            }
            println!("  {}", parts.join(" | "));
        }
    }
}
