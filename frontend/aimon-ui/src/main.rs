//! AI Activity Monitor - Leptos frontend, live-parity port of dashboard.html.
//! Slider/dropdown history browsing lands in phase 5 — this phase only
//! reproduces the original always-live-tail behavior against the new API.
use aimon_api_types::{EventKind, StateSummary, UnifiedEvent};
use gloo_net::http::Request;
use gloo_timers::future::TimeoutFuture;
use leptos::prelude::*;
use leptos::task::spawn_local;
use std::mem;

const POLL_MS: u32 = 2000;
const RIVER_SPAN_MS: f64 = 3_600_000.0; // last 60 minutes, matches the original hardcoded window

fn main() {
    console_error_panic_hook::set_once();
    _ = console_log::init_with_level(log::Level::Info);
    leptos::mount::mount_to_body(App);
}

/// Strips a Windows path down to the filename, then strips a trailing
/// `_<13 lowercase-alphanumeric>` suffix — port of the original JS `nice()`.
fn nice_name(tool: &str) -> String {
    let last = tool.rsplit('\\').next().unwrap_or(tool);
    if last.len() > 14 {
        let (head, tail) = last.split_at(last.len() - 14);
        if tail.starts_with('_') && tail[1..].chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()) {
            return head.to_string();
        }
    }
    last.to_string()
}

/// Parses the server's local-time-no-offset timestamp the same way the
/// browser's own `Date` parser would (matches the original JS behavior
/// exactly, since it also did `new Date(e.ts)` on this same format).
fn js_time_ms(ts: &str) -> f64 {
    js_sys::Date::new(&wasm_bindgen::JsValue::from_str(ts)).get_time()
}

fn kind_color_var(kind: EventKind) -> &'static str {
    match kind {
        EventKind::Command => "var(--cmd)",
        EventKind::Network => "var(--net)",
        EventKind::Process => "var(--proc)",
        EventKind::Device => "var(--dev)",
    }
}

async fn fetch_json<T: for<'de> serde::Deserialize<'de>>(url: &str) -> Option<T> {
    let resp = Request::get(url).send().await.ok()?;
    if !resp.ok() {
        return None;
    }
    resp.json::<T>().await.ok()
}

fn urlenc_ts(s: &str) -> String {
    s.replace(':', "%3A")
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FeedFilter {
    All,
    Flagged,
    Command,
    Network,
    Device,
    Process,
}

impl FeedFilter {
    fn matches(self, e: &UnifiedEvent) -> bool {
        match self {
            FeedFilter::All => true,
            FeedFilter::Flagged => e.flagged,
            FeedFilter::Command => e.kind == EventKind::Command,
            FeedFilter::Network => e.kind == EventKind::Network,
            FeedFilter::Device => e.kind == EventKind::Device,
            FeedFilter::Process => e.kind == EventKind::Process,
        }
    }

    fn label(self) -> &'static str {
        match self {
            FeedFilter::All => "Everything",
            FeedFilter::Flagged => "Flagged",
            FeedFilter::Command => "Commands",
            FeedFilter::Network => "Network",
            FeedFilter::Device => "Mic/camera",
            FeedFilter::Process => "Start/stop",
        }
    }
}

const ALL_FILTERS: [FeedFilter; 6] =
    [FeedFilter::All, FeedFilter::Flagged, FeedFilter::Command, FeedFilter::Network, FeedFilter::Device, FeedFilter::Process];

#[derive(Clone, Copy)]
struct SharedState {
    events: RwSignal<Vec<UnifiedEvent>>,
    state: RwSignal<Option<StateSummary>>,
    connected: RwSignal<bool>,
    error_msg: RwSignal<Option<String>>,
}

#[component]
fn App() -> impl IntoView {
    let shared = SharedState {
        events: RwSignal::new(Vec::new()),
        state: RwSignal::new(None),
        connected: RwSignal::new(true),
        error_msg: RwSignal::new(None),
    };
    let filter = RwSignal::new(FeedFilter::All);
    let last_ts = RwSignal::new(None::<String>);
    let clock = RwSignal::new(String::new());

    // Clock ticks every second, independent of the data poll.
    spawn_local(async move {
        loop {
            clock.set(String::from(js_sys::Date::new_0().to_locale_time_string("en-US")));
            TimeoutFuture::new(1000).await;
        }
    });

    // Data poll — mirrors the original dashboard.html's tick()/setInterval(tick,2000):
    // fetch incremental events + full state + river every 2s, prepend new events.
    spawn_local(async move {
        loop {
            let after = last_ts.get_untracked();
            let events_url = match &after {
                Some(ts) => format!("/api/events?start={}", urlenc_ts(ts)),
                None => "/api/events".to_string(),
            };
            let ev = fetch_json::<Vec<UnifiedEvent>>(&events_url).await;
            let st = fetch_json::<StateSummary>("/api/state").await;

            match (ev, st) {
                (Some(mut new_events), Some(st_val)) => {
                    shared.connected.set(true);
                    shared.error_msg.set(None);
                    if let Some(first) = new_events.first() {
                        last_ts.set(Some(first.ts.clone()));
                    }
                    shared.events.update(|all| {
                        let old = mem::take(all);
                        new_events.extend(old);
                        new_events.truncate(2000);
                        *all = new_events;
                    });
                    shared.state.set(Some(st_val));
                }
                _ => {
                    shared.connected.set(false);
                    shared.error_msg.set(Some("Dashboard can't reach the collector data".to_string()));
                }
            }
            TimeoutFuture::new(POLL_MS).await;
        }
    });

    view! {
        <Header shared=shared clock=clock/>
        <main>
            <RiverLanesView events=shared.events/>
            <LiveFeed events=shared.events filter=filter/>
            <aside>
                <WindowCounts state=shared.state/>
                <EndpointsChart state=shared.state/>
            </aside>
        </main>
    }
}

#[component]
fn Header(shared: SharedState, clock: RwSignal<String>) -> impl IntoView {
    view! {
        <header>
            <span class="pulse" class:off=move || !shared.connected.get()></span>
            <h1>"AI activity on this PC"</h1>
            <span id="clock">{move || clock.get()}</span>
            <div id="live" aria-live="polite">
                {move || {
                    if let Some(err) = shared.error_msg.get() {
                        view! { <span class="chip hot">{err}</span> }.into_any()
                    } else {
                        match shared.state.get() {
                            None => view! {}.into_any(),
                            Some(st) => {
                                let dev_chips: Vec<_> = st.devices.iter().map(|d| {
                                    let label = if d.device == "webcam" { "Camera" } else { "Mic" };
                                    let text = format!("{label} in use: {}", nice_name(&d.app));
                                    view! { <span class="chip hot">{text}</span> }
                                }).collect();
                                let running_view = if st.running.is_empty() {
                                    view! { <span class="chip">"No AI tools running"</span> }.into_any()
                                } else {
                                    let chips: Vec<_> = st.running.iter()
                                        .map(|r| view! { <span class="chip">{format!("{} running", r.name)}</span> })
                                        .collect();
                                    chips.into_view().into_any()
                                };
                                (dev_chips.into_view(), running_view).into_any()
                            }
                        }
                    }
                }}
            </div>
        </header>
    }
}

#[component]
fn RiverLanesView(events: RwSignal<Vec<UnifiedEvent>>) -> impl IntoView {
    view! {
        <section id="river">
            <div class="row">
                <h2>"Last 60 minutes" <span class="sub">"one row per AI tool"</span></h2>
                <div class="legend">
                    <span><i style="background:var(--cmd)"></i>"Command"</span>
                    <span><i style="background:var(--net)"></i>"Network"</span>
                    <span><i style="background:var(--proc)"></i>"Start/stop"</span>
                    <span><i style="background:var(--dev)"></i>"Mic/camera"</span>
                    <span><i style="background:var(--flag)"></i>"Flagged"</span>
                </div>
            </div>
            <div id="lanes">
                {move || {
                    let now = js_sys::Date::now();
                    let all = events.get();
                    // One tick per raw event within the window, colored by its
                    // actual kind — matches the original exactly (it plotted
                    // the same live event list used by the feed, not an
                    // aggregated view).
                    let mut lanes: std::collections::BTreeMap<String, Vec<(f64, &str, bool)>> = Default::default();
                    for e in &all {
                        let t = js_time_ms(&e.ts);
                        if now - t > RIVER_SPAN_MS || t > now {
                            continue;
                        }
                        lanes.entry(nice_name(&e.tool)).or_default().push((t, kind_color_var(e.kind), e.flagged));
                    }
                    if lanes.is_empty() {
                        view! { <div class="empty">"No AI activity in the last hour."</div> }.into_any()
                    } else {
                        lanes.into_iter().map(|(name, marks_data)| {
                            let marks: Vec<_> = marks_data.into_iter().map(|(t, color, flagged)| {
                                let x = 100.0 - (now - t) / RIVER_SPAN_MS * 100.0;
                                let cls = if flagged { "mark flag" } else { "mark" };
                                let bg = if flagged { "var(--flag)" } else { color };
                                let style = format!("left:{x:.2}%;background:{bg}");
                                view! { <span class={cls} style={style}></span> }
                            }).collect();
                            view! {
                                <div class="lane">
                                    <b title={name.clone()}>{name.clone()}</b>
                                    <div class="track">{marks}</div>
                                </div>
                            }
                        }).collect_view().into_any()
                    }
                }}
            </div>
            <div class="axis">
                <span></span>
                <div><span>"60 min ago"</span><span>"45"</span><span>"30"</span><span>"15"</span><span>"Now"</span></div>
            </div>
        </section>
    }
}

#[component]
fn LiveFeed(events: RwSignal<Vec<UnifiedEvent>>, filter: RwSignal<FeedFilter>) -> impl IntoView {
    view! {
        <section>
            <h2>"Live feed"</h2>
            <div id="filters" role="group" aria-label="Filter feed">
                {ALL_FILTERS.iter().map(|&f| {
                    view! {
                        <button
                            aria-pressed=move || (filter.get() == f).to_string()
                            on:click=move |_| filter.set(f)
                        >{f.label()}</button>
                    }
                }).collect_view()}
            </div>
            <ul id="feed">
                {move || {
                    let all = events.get();
                    let f = filter.get();
                    let rows: Vec<_> = all.iter().filter(|e| f.matches(e)).take(250).collect();
                    if rows.is_empty() {
                        view! { <li class="empty">"Nothing here yet. Open an AI tool and activity will show up within a few seconds."</li> }.into_any()
                    } else {
                        rows.into_iter().map(|e| {
                            let li_class = if e.flagged { "flag" } else { "" };
                            let time = e.ts.get(11..).unwrap_or(&e.ts).to_string();
                            let dot_color = if e.flagged { "var(--flag)".to_string() } else { kind_color_var(e.kind).to_string() };
                            let tool_name = nice_name(&e.tool);
                            let tool_full = e.tool.clone();
                            let detail = e.detail.clone();
                            view! {
                                <li class={li_class}>
                                    <time>{time}</time>
                                    <span class="dot" style={format!("background:{dot_color}")}></span>
                                    <span class="tool" title={tool_full}>{tool_name}</span>
                                    <code>{detail}</code>
                                </li>
                            }
                        }).collect_view().into_any()
                    }
                }}
            </ul>
        </section>
    }
}

#[component]
fn WindowCounts(state: RwSignal<Option<StateSummary>>) -> impl IntoView {
    view! {
        <section>
            <h2>"Today"</h2>
            <dl>
                {move || {
                    let c = state.get().map(|s| s.counts).unwrap_or_default();
                    view! {
                        <dt>"AI tools launched"</dt><dd>{c.launched}</dd>
                        <dt>"Commands run by AI"</dt><dd>{c.commands}</dd>
                        <dt>"Flagged commands"</dt><dd class=(c.flagged > 0).then_some("warn")>{c.flagged}</dd>
                        <dt>"Endpoints contacted"</dt><dd>{c.endpoints}</dd>
                    }
                }}
            </dl>
        </section>
    }
}

#[component]
fn EndpointsChart(state: RwSignal<Option<StateSummary>>) -> impl IntoView {
    view! {
        <section>
            <h2>"Who they're talking to"</h2>
            <div>
                {move || {
                    let endpoints = state.get().map(|s| s.endpoints).unwrap_or_default();
                    if endpoints.is_empty() {
                        view! { <div class="empty">"No connections yet today."</div> }.into_any()
                    } else {
                        let max = endpoints.iter().map(|e| e.n).max().unwrap_or(1).max(1);
                        endpoints.into_iter().map(|e| {
                            let pct = (e.n as f64 / max as f64) * 100.0;
                            view! {
                                <div class="ep">
                                    <span><span>{e.host}</span><span>{e.n}</span></span>
                                    <div class="bar"><i style={format!("width:{pct}%")}></i></div>
                                </div>
                            }
                        }).collect_view().into_any()
                    }
                }}
            </div>
        </section>
    }
}
