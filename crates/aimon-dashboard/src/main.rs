// Full API (/api/meta, /api/events, /api/state, /api/river) and embedded
// frontend land in phase 3. This scaffold just proves the server binds to
// 127.0.0.1 only, per the hard localhost-only constraint.
use axum::{routing::get, Router};
use std::net::{Ipv4Addr, SocketAddr};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let app = Router::new().route("/", get(|| async { "aimon-dashboard scaffold" }));
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, 8765));
    println!("AI Activity Monitor dashboard: http://{addr}");
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
