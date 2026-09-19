// Poll loop lands in phase 2 (collector port).
fn main() {
    println!("aimon-collector scaffold — db at {:?}", aimon_core::paths::db_path());
}
