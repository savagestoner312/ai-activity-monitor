//! AI Activity Monitor - live dashboard (localhost only).
//! Reads the same SQLite DB the collector writes. Replaces the Python
//! dashboard's tail-only `/api/events?after=` model with range-bounded
//! queries so the frontend can browse history, not just live-tail.
//!
//! No console window at logon, matching pythonw.exe's behavior in the
//! original — this also means stdout/println! go nowhere in practice.
#![windows_subsystem = "windows"]

use aimon_api_types::{PerfSummary, PerfTotal, RiverResponse, StateSummary, UnifiedEvent};
use aimon_core::{db, paths, queries};
use axum::{
    extract::Query, extract::State, http::StatusCode, http::Uri, response::IntoResponse, routing::get, Json, Router,
};
use chrono::{Duration, Local};
use rust_embed::RustEmbed;
use serde::Deserialize;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

/// The Leptos frontend, built by `trunk build --release` into
/// `frontend/aimon-ui/dist` and embedded into this binary at compile time —
/// deployment is just "copy the exe," no separate dist folder to manage.
#[derive(RustEmbed)]
#[folder = "../../frontend/aimon-ui/dist"]
struct Assets;

fn content_type_for(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "wasm" => "application/wasm",
        "css" => "text/css; charset=utf-8",
        _ => "application/octet-stream",
    }
}

async fn static_asset(uri: Uri) -> impl IntoResponse {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    match Assets::get(path) {
        Some(file) => ([("content-type", content_type_for(path))], file.data.into_owned()).into_response(),
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

struct AppState {
    db_path: PathBuf,
}

fn open_ro(state: &AppState) -> rusqlite::Result<rusqlite::Connection> {
    db::open_ro(&state.db_path)
}

fn hours_ago(hours: i64) -> String {
    (Local::now() - Duration::hours(hours)).format("%Y-%m-%dT%H:%M:%S").to_string()
}

fn seconds_ago(secs: i64) -> String {
    (Local::now() - Duration::seconds(secs)).format("%Y-%m-%dT%H:%M:%S").to_string()
}

#[derive(Deserialize)]
struct EventsQuery {
    start: Option<String>,
    end: Option<String>,
    limit: Option<i64>,
}

async fn get_meta(State(state): State<Arc<AppState>>) -> Json<aimon_api_types::MetaResponse> {
    let now = db::now_str();
    let oldest_ts = open_ro(&state).ok().and_then(|c| queries::oldest_event_ts(&c).ok().flatten());
    Json(aimon_api_types::MetaResponse { oldest_ts, now })
}

async fn get_events(State(state): State<Arc<AppState>>, Query(q): Query<EventsQuery>) -> Json<Vec<UnifiedEvent>> {
    let end = q.end.unwrap_or_else(db::now_str);
    let start = q.start.unwrap_or_else(|| hours_ago(1));
    let limit = q.limit.unwrap_or(300);
    let events = open_ro(&state).and_then(|c| queries::events_in_range(&c, &start, &end, limit)).unwrap_or_default();
    Json(events)
}

#[derive(Deserialize)]
struct StateQuery {
    start: Option<String>,
    end: Option<String>,
}

async fn get_state(State(state): State<Arc<AppState>>, Query(q): Query<StateQuery>) -> Json<StateSummary> {
    let now = db::now_str();
    let end = q.end.unwrap_or_else(|| now.clone());
    let start = q.start.unwrap_or_else(|| format!("{}T00:00:00", &now[..10]));
    let summary = open_ro(&state).and_then(|c| queries::state_for_range(&c, &start, &end, &now)).unwrap_or_else(|_| {
        StateSummary {
            range_start: start.clone(),
            range_end: end.clone(),
            is_live: false,
            running: Vec::new(),
            counts: Default::default(),
            devices: Vec::new(),
            endpoints: Vec::new(),
        }
    });
    Json(summary)
}

#[derive(Deserialize)]
struct RiverQuery {
    start: Option<String>,
    end: Option<String>,
    bucket: Option<i64>,
}

async fn get_river(State(state): State<Arc<AppState>>, Query(q): Query<RiverQuery>) -> Json<RiverResponse> {
    let end = q.end.unwrap_or_else(db::now_str);
    let start = q.start.unwrap_or_else(|| hours_ago(1));
    let bucket = q.bucket.unwrap_or(60);
    let lanes = open_ro(&state).and_then(|c| queries::river_for_range(&c, &start, &end, bucket)).unwrap_or_default();
    Json(RiverResponse { lanes })
}

#[derive(Deserialize)]
struct PerfQuery {
    start: Option<String>,
    end: Option<String>,
}

async fn get_perf(State(state): State<Arc<AppState>>, Query(q): Query<PerfQuery>) -> Json<PerfSummary> {
    let now = db::now_str();
    let end = q.end.unwrap_or_else(|| now.clone());
    let start = q.start.unwrap_or_else(|| hours_ago(1));
    let is_live = end >= now;
    // Live: a narrow trailing window (a couple of poll cycles) so a stopped
    // tool ages out naturally instead of showing its last-ever reading
    // forever. Historical: the full viewed window, averaged.
    let (qstart, qend) = if is_live { (seconds_ago(8), now.clone()) } else { (start, end.clone()) };
    let tools = open_ro(&state).and_then(|c| queries::perf_in_range(&c, &qstart, &qend)).unwrap_or_default();
    let gpu_available = tools.iter().any(|t| t.gpu_pct.is_some());
    let total = tools.iter().fold(PerfTotal::default(), |mut acc, t| {
        acc.proc_count += t.proc_count;
        acc.cpu_pct += t.cpu_pct;
        acc.mem_bytes += t.mem_bytes;
        acc.gpu_pct = match (acc.gpu_pct, t.gpu_pct) {
            (a, None) => a,
            (None, b) => b,
            (Some(a), Some(b)) => Some(a + b),
        };
        acc
    });
    Json(PerfSummary { ts: if is_live { now } else { end }, is_live, gpu_available, total, tools })
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let state = Arc::new(AppState { db_path: paths::db_path() });
    let app = Router::new()
        .route("/api/meta", get(get_meta))
        .route("/api/events", get(get_events))
        .route("/api/state", get(get_state))
        .route("/api/river", get(get_river))
        .route("/api/perf", get(get_perf))
        .fallback(static_asset)
        .with_state(state);

    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, 8765));
    println!("AI Activity Monitor dashboard: http://{addr}");
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
