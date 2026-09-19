//! AI Activity Monitor - live dashboard (localhost only).
//! Reads the same SQLite DB the collector writes. Replaces the Python
//! dashboard's tail-only `/api/events?after=` model with range-bounded
//! queries so the frontend can browse history, not just live-tail.
use aimon_api_types::{RiverResponse, StateSummary, UnifiedEvent};
use aimon_core::{db, paths, queries};
use axum::{extract::Query, extract::State, routing::get, Json, Router};
use chrono::{Duration, Local};
use serde::Deserialize;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

struct AppState {
    db_path: PathBuf,
}

fn open_ro(state: &AppState) -> rusqlite::Result<rusqlite::Connection> {
    db::open_ro(&state.db_path)
}

fn hours_ago(hours: i64) -> String {
    (Local::now() - Duration::hours(hours)).format("%Y-%m-%dT%H:%M:%S").to_string()
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

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let state = Arc::new(AppState { db_path: paths::db_path() });
    let app = Router::new()
        .route("/", get(|| async { "AI Activity Monitor — frontend lands in phase 4" }))
        .route("/api/meta", get(get_meta))
        .route("/api/events", get(get_events))
        .route("/api/state", get(get_state))
        .route("/api/river", get(get_river))
        .with_state(state);

    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, 8765));
    println!("AI Activity Monitor dashboard: http://{addr}");
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
