//! AI Activity Monitor - Leptos frontend.
//! Phase 4 ported the original dashboard.html to live-parity. Phase 5 adds
//! the history-browsing feature: a timeframe dropdown (window size) plus a
//! look-back slider (window position in history). While the slider sits at
//! "now" the view live-tails exactly like the original; dragging it back
//! freezes the view on a fixed historical window and stops polling until
//! the slider moves again.
use aimon_api_types::{EventKind, MetaResponse, PerfSummary, StateSummary, UnifiedEvent};
use gloo_net::http::Request;
use gloo_timers::future::TimeoutFuture;
use leptos::prelude::*;
use leptos::task::spawn_local;
use std::mem;

const POLL_MS: u32 = 2000;
const META_REFRESH_MS: u32 = 30_000;
const SCRUB_DEBOUNCE_MS: u32 = 200;
const LIVE_THRESHOLD: f64 = 0.999;
const SLIDER_STEPS: i64 = 1000;

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

/// Inverse of `js_time_ms` — formats milliseconds back to the same
/// `YYYY-MM-DDTHH:MM:SS` local-time shape the server writes and expects.
fn ms_to_iso_local(ms: f64) -> String {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ms));
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        d.get_full_year() as u32,
        d.get_month() as u32 + 1,
        d.get_date() as u32,
        d.get_hours() as u32,
        d.get_minutes() as u32,
        d.get_seconds() as u32
    )
}

fn fmt_local(ms: f64) -> String {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ms));
    let date = format!("{}/{}/{}", d.get_month() as u32 + 1, d.get_date() as u32, d.get_full_year() as u32);
    let time = String::from(d.to_locale_time_string("en-US"));
    format!("{date} {time}")
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
enum WindowSize {
    M15,
    H1,
    H6,
    H24,
    D7,
}

impl WindowSize {
    const ALL: [WindowSize; 5] = [WindowSize::M15, WindowSize::H1, WindowSize::H6, WindowSize::H24, WindowSize::D7];

    fn ms(self) -> f64 {
        const MINUTE: f64 = 60_000.0;
        const HOUR: f64 = 60.0 * MINUTE;
        const DAY: f64 = 24.0 * HOUR;
        match self {
            WindowSize::M15 => 15.0 * MINUTE,
            WindowSize::H1 => HOUR,
            WindowSize::H6 => 6.0 * HOUR,
            WindowSize::H24 => DAY,
            WindowSize::D7 => 7.0 * DAY,
        }
    }

    fn label(self) -> &'static str {
        match self {
            WindowSize::M15 => "15m",
            WindowSize::H1 => "1h",
            WindowSize::H6 => "6h",
            WindowSize::H24 => "24h",
            WindowSize::D7 => "7d",
        }
    }

    fn from_label(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|w| w.label() == s)
    }
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

// ============ Settings: theme, layout, background ============
// All persisted client-side via localStorage — nothing here touches the
// backend, there's no server-side concept of "your theme."

const SETTINGS_KEY: &str = "aimon.settings.v1";
const BACKGROUND_KEY: &str = "aimon.settings.background.v1";

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
enum PresetId {
    Default,
    GreenCrt,
    Amber,
    Synthwave,
}

impl PresetId {
    const ALL: [PresetId; 4] = [PresetId::Default, PresetId::GreenCrt, PresetId::Amber, PresetId::Synthwave];

    fn label(self) -> &'static str {
        match self {
            PresetId::Default => "Default",
            PresetId::GreenCrt => "Green CRT",
            PresetId::Amber => "Amber",
            PresetId::Synthwave => "Synthwave",
        }
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct ThemeColors {
    bg: String,
    panel: String,
    line: String,
    fg: String,
    muted: String,
    cmd: String,
    net: String,
    proc: String,
    dev: String,
    flag: String,
    vu_lo: String,
    vu_mid: String,
    vu_hi: String,
}

impl ThemeColors {
    fn preset(id: PresetId) -> Self {
        fn c(bg: &str, panel: &str, line: &str, fg: &str, muted: &str, cmd: &str, net: &str, proc: &str, dev: &str, flag: &str, vu_lo: &str, vu_mid: &str, vu_hi: &str) -> ThemeColors {
            ThemeColors {
                bg: bg.into(), panel: panel.into(), line: line.into(), fg: fg.into(), muted: muted.into(),
                cmd: cmd.into(), net: net.into(), proc: proc.into(), dev: dev.into(), flag: flag.into(),
                vu_lo: vu_lo.into(), vu_mid: vu_mid.into(), vu_hi: vu_hi.into(),
            }
        }
        match id {
            PresetId::Default => c("#16202e", "#1c2838", "#2a3a50", "#dfe7f1", "#8a9bb0", "#f2a541", "#5bc0eb", "#b9a7ff", "#7ee0b5", "#ff6b7d", "#3ddc73", "#f2c14e", "#ff5c5c"),
            PresetId::GreenCrt => c("#0a1408", "#0f1f0c", "#1d3a17", "#8cff9b", "#4f8a58", "#c9ff5c", "#5cffb8", "#9dff70", "#c8ffb0", "#ff5c5c", "#39ff6a", "#c9ff3d", "#ff4f4f"),
            PresetId::Amber => c("#1a1006", "#26180a", "#4a2f12", "#ffcf7a", "#b8863f", "#ffb347", "#ffdf91", "#ff9d4d", "#ffe8b0", "#ff5c5c", "#c9d64a", "#ffb347", "#ff5c3d"),
            PresetId::Synthwave => c("#180a2e", "#22103f", "#3d1e63", "#f4e8ff", "#a082c4", "#ff9f1c", "#3ff5ff", "#ff5cd8", "#5cffb8", "#ff2d75", "#3ffce0", "#ff9f1c", "#ff2d75"),
        }
    }

    /// (CSS custom property name, value) for every themable variable —
    /// applying all of these to :root is the entire runtime reskin mechanism,
    /// since every rule in index.html already references var(--...) with no
    /// hardcoded literals outside the :root block itself.
    fn entries(&self) -> [(&'static str, &str); 13] {
        [
            ("--bg", &self.bg), ("--panel", &self.panel), ("--line", &self.line),
            ("--fg", &self.fg), ("--muted", &self.muted), ("--cmd", &self.cmd),
            ("--net", &self.net), ("--proc", &self.proc), ("--dev", &self.dev),
            ("--flag", &self.flag), ("--vu-lo", &self.vu_lo), ("--vu-mid", &self.vu_mid),
            ("--vu-hi", &self.vu_hi),
        ]
    }
}

/// One entry per swatch in the "customize colors" grid — lets the UI iterate
/// all 13 fields generically instead of writing 13 near-identical handlers.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ColorField {
    Bg, Panel, Line, Fg, Muted, Cmd, Net, Proc, Dev, Flag, VuLo, VuMid, VuHi,
}

impl ColorField {
    const ALL: [(ColorField, &'static str); 13] = [
        (ColorField::Bg, "Background"), (ColorField::Panel, "Panel"), (ColorField::Line, "Border"),
        (ColorField::Fg, "Text"), (ColorField::Muted, "Muted text"), (ColorField::Cmd, "Command"),
        (ColorField::Net, "Network"), (ColorField::Proc, "Process"), (ColorField::Dev, "Device"),
        (ColorField::Flag, "Flagged"), (ColorField::VuLo, "VU low"), (ColorField::VuMid, "VU mid"),
        (ColorField::VuHi, "VU high"),
    ];

    fn get(self, t: &ThemeColors) -> String {
        match self {
            ColorField::Bg => t.bg.clone(), ColorField::Panel => t.panel.clone(), ColorField::Line => t.line.clone(),
            ColorField::Fg => t.fg.clone(), ColorField::Muted => t.muted.clone(), ColorField::Cmd => t.cmd.clone(),
            ColorField::Net => t.net.clone(), ColorField::Proc => t.proc.clone(), ColorField::Dev => t.dev.clone(),
            ColorField::Flag => t.flag.clone(), ColorField::VuLo => t.vu_lo.clone(), ColorField::VuMid => t.vu_mid.clone(),
            ColorField::VuHi => t.vu_hi.clone(),
        }
    }

    fn set(self, t: &mut ThemeColors, v: String) {
        match self {
            ColorField::Bg => t.bg = v, ColorField::Panel => t.panel = v, ColorField::Line => t.line = v,
            ColorField::Fg => t.fg = v, ColorField::Muted => t.muted = v, ColorField::Cmd => t.cmd = v,
            ColorField::Net => t.net = v, ColorField::Proc => t.proc = v, ColorField::Dev => t.dev = v,
            ColorField::Flag => t.flag = v, ColorField::VuLo => t.vu_lo = v, ColorField::VuMid => t.vu_mid = v,
            ColorField::VuHi => t.vu_hi = v,
        }
    }
}

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
struct SectionVisibility {
    perf: bool,
    river: bool,
    feed: bool,
    window_counts: bool,
    endpoints: bool,
    // TimeControls is intentionally not included — it's the primary time-
    // navigation control, not a content section; hiding it would strand
    // the user with no way to change the viewed window.
}

impl Default for SectionVisibility {
    fn default() -> Self {
        Self { perf: true, river: true, feed: true, window_counts: true, endpoints: true }
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct BackgroundImage {
    data_url: String,
    /// 0.0 = image fully visible, 1.0 = fully hidden behind the dim layer.
    dim: f32,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct SettingsBlob {
    theme: ThemeColors,
    theme_preset: PresetId,
    sections: SectionVisibility,
}

fn local_storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok()?
}

fn load_settings_blob() -> Option<SettingsBlob> {
    let raw = local_storage()?.get_item(SETTINGS_KEY).ok()??;
    serde_json::from_str(&raw).ok()
}

fn save_settings_blob(blob: &SettingsBlob) {
    let Some(storage) = local_storage() else { return };
    if let Ok(json) = serde_json::to_string(blob) {
        let _ = storage.set_item(SETTINGS_KEY, &json);
    }
}

fn load_background() -> Option<BackgroundImage> {
    let raw = local_storage()?.get_item(BACKGROUND_KEY).ok()??;
    serde_json::from_str(&raw).ok()
}

fn save_background(bg: Option<&BackgroundImage>) {
    let Some(storage) = local_storage() else { return };
    match bg {
        Some(b) => {
            if let Ok(json) = serde_json::to_string(b) {
                let _ = storage.set_item(BACKGROUND_KEY, &json);
            }
        }
        None => {
            let _ = storage.remove_item(BACKGROUND_KEY);
        }
    }
}

/// The entire runtime reskin mechanism: overwrite the 13 CSS custom
/// properties on the document root. Inline element styles win the cascade
/// over stylesheet rules (including the @media prefers-color-scheme block),
/// so this correctly overrides OS light/dark preference with an explicit
/// user theme choice — that's intentional, not an oversight.
fn apply_theme(theme: &ThemeColors) {
    use wasm_bindgen::JsCast;
    let Some(document) = web_sys::window().and_then(|w| w.document()) else { return };
    let Some(el) = document.document_element() else { return };
    let Ok(html_el) = el.dyn_into::<web_sys::HtmlElement>() else { return };
    let style = html_el.style();
    for (name, value) in theme.entries() {
        let _ = style.set_property(name, value);
    }
}

#[derive(Clone, Copy)]
struct SettingsState {
    theme: RwSignal<ThemeColors>,
    theme_preset: RwSignal<PresetId>,
    sections: RwSignal<SectionVisibility>,
    background: RwSignal<Option<BackgroundImage>>,
    panel_open: RwSignal<bool>,
}

#[derive(Clone, Copy)]
struct SharedState {
    events: RwSignal<Vec<UnifiedEvent>>,
    state: RwSignal<Option<StateSummary>>,
    perf: RwSignal<Option<PerfSummary>>,
    connected: RwSignal<bool>,
    error_msg: RwSignal<Option<String>>,
}

#[component]
fn App() -> impl IntoView {
    let shared = SharedState {
        events: RwSignal::new(Vec::new()),
        state: RwSignal::new(None),
        perf: RwSignal::new(None),
        connected: RwSignal::new(true),
        error_msg: RwSignal::new(None),
    };
    let filter = RwSignal::new(FeedFilter::All);
    let clock = RwSignal::new(String::new());

    let loaded_settings = load_settings_blob();
    let settings = SettingsState {
        theme: RwSignal::new(loaded_settings.as_ref().map(|b| b.theme.clone()).unwrap_or_else(|| ThemeColors::preset(PresetId::Default))),
        theme_preset: RwSignal::new(loaded_settings.as_ref().map(|b| b.theme_preset).unwrap_or(PresetId::Default)),
        sections: RwSignal::new(loaded_settings.as_ref().map(|b| b.sections).unwrap_or_default()),
        background: RwSignal::new(load_background()),
        panel_open: RwSignal::new(false),
    };

    // Apply immediately on startup and whenever the theme changes.
    Effect::new(move |_| apply_theme(&settings.theme.get()));

    // Persist theme+preset+layout as one small blob whenever any of them change.
    Effect::new(move |_| {
        let blob =
            SettingsBlob { theme: settings.theme.get(), theme_preset: settings.theme_preset.get(), sections: settings.sections.get() };
        save_settings_blob(&blob);
    });

    // Background lives in its own key so a large-image write/quota error can
    // never corrupt or block the much smaller, more important settings blob.
    Effect::new(move |_| save_background(settings.background.get().as_ref()));

    let window_size = RwSignal::new(WindowSize::H1);
    let slider_pos = RwSignal::new(1.0_f64);
    let meta = RwSignal::new(None::<MetaResponse>);
    let is_live = Memo::new(move |_| slider_pos.get() >= LIVE_THRESHOLD);

    // The window's right edge: real wall-clock "now" while live (so the
    // river/labels advance in real time), or frozen at the slider's
    // position once scrubbed away from live.
    let view_end_ms = Memo::new(move |_| {
        if is_live.get() {
            return js_sys::Date::now();
        }
        let Some(m) = meta.get() else { return js_sys::Date::now() };
        let now_ms = js_time_ms(&m.now);
        let oldest_ms = m.oldest_ts.as_deref().map(js_time_ms).unwrap_or(now_ms);
        let span = (now_ms - oldest_ms).max(1.0);
        oldest_ms + slider_pos.get() * span
    });
    let view_start_ms = Memo::new(move |_| view_end_ms.get() - window_size.get().ms());

    // Clock ticks every second, independent of the data poll.
    spawn_local(async move {
        loop {
            clock.set(String::from(js_sys::Date::new_0().to_locale_time_string("en-US")));
            TimeoutFuture::new(1000).await;
        }
    });

    // Slider bounds: refresh /api/meta periodically (oldest_ts barely
    // moves; now advances, which matters for scaling the slider correctly).
    spawn_local(async move {
        loop {
            if let Some(m) = fetch_json::<MetaResponse>("/api/meta").await {
                meta.set(Some(m));
            }
            TimeoutFuture::new(META_REFRESH_MS).await;
        }
    });

    // Fetch orchestration: an epoch counter lets a fresh run (triggered by
    // window_size/slider_pos/meta changing) invalidate whatever the
    // previous run is doing, without needing real task cancellation.
    let epoch = RwSignal::new(0_u64);
    Effect::new(move |_| {
        let ws = window_size.get();
        let pos = slider_pos.get();
        let meta_val = meta.get();
        epoch.update(|e| *e += 1);
        let my_epoch = epoch.get_untracked();
        let is_current = move || epoch.get_untracked() == my_epoch;

        spawn_local(async move {
            let live = pos >= LIVE_THRESHOLD;

            if live {
                // Live-tail loop, generalized from phase 4's hardcoded 1h
                // window to whatever the dropdown currently selects.
                let mut last_ts: Option<String> = None;
                loop {
                    if !is_current() {
                        return;
                    }
                    let end_now = js_sys::Date::now();
                    let start_ms = end_now - ws.ms();
                    let (events_url, incremental) = match &last_ts {
                        Some(ts) => (format!("/api/events?start={}&limit=2000", urlenc_ts(ts)), true),
                        None => (
                            format!(
                                "/api/events?start={}&end={}&limit=2000",
                                urlenc_ts(&ms_to_iso_local(start_ms)),
                                urlenc_ts(&ms_to_iso_local(end_now))
                            ),
                            false,
                        ),
                    };
                    let state_url = format!(
                        "/api/state?start={}&end={}",
                        urlenc_ts(&ms_to_iso_local(start_ms)),
                        urlenc_ts(&ms_to_iso_local(end_now))
                    );
                    let ev = fetch_json::<Vec<UnifiedEvent>>(&events_url).await;
                    let st = fetch_json::<StateSummary>(&state_url).await;
                    let pf = fetch_json::<PerfSummary>("/api/perf").await;
                    if !is_current() {
                        return;
                    }
                    match (ev, st) {
                        (Some(mut new_events), Some(st_val)) => {
                            shared.connected.set(true);
                            shared.error_msg.set(None);
                            if let Some(first) = new_events.first() {
                                last_ts = Some(first.ts.clone());
                            }
                            if incremental {
                                shared.events.update(|all| {
                                    let old = mem::take(all);
                                    new_events.extend(old);
                                    new_events.truncate(2000);
                                    *all = new_events;
                                });
                            } else {
                                shared.events.set(new_events);
                            }
                            shared.state.set(Some(st_val));
                            if pf.is_some() {
                                shared.perf.set(pf);
                            }
                        }
                        _ => {
                            shared.connected.set(false);
                            shared.error_msg.set(Some("Dashboard can't reach the collector data".to_string()));
                        }
                    }
                    TimeoutFuture::new(POLL_MS).await;
                }
            } else {
                // Historical scrub: debounce so dragging doesn't spam the
                // API, then one fetch for the fixed window — no polling
                // until the slider moves again.
                TimeoutFuture::new(SCRUB_DEBOUNCE_MS).await;
                if !is_current() {
                    return;
                }
                let Some(m) = meta_val else { return };
                let now_ms = js_time_ms(&m.now);
                let oldest_ms = m.oldest_ts.as_deref().map(js_time_ms).unwrap_or(now_ms);
                let span = (now_ms - oldest_ms).max(1.0);
                let end_ms = oldest_ms + pos * span;
                let start_ms = end_ms - ws.ms();
                let start = ms_to_iso_local(start_ms);
                let end = ms_to_iso_local(end_ms);
                let events_url = format!("/api/events?start={}&end={}&limit=5000", urlenc_ts(&start), urlenc_ts(&end));
                let state_url = format!("/api/state?start={}&end={}", urlenc_ts(&start), urlenc_ts(&end));
                let perf_url = format!("/api/perf?start={}&end={}", urlenc_ts(&start), urlenc_ts(&end));
                let ev = fetch_json::<Vec<UnifiedEvent>>(&events_url).await;
                let st = fetch_json::<StateSummary>(&state_url).await;
                let pf = fetch_json::<PerfSummary>(&perf_url).await;
                if !is_current() {
                    return;
                }
                match (ev, st) {
                    (Some(new_events), Some(st_val)) => {
                        shared.connected.set(true);
                        shared.error_msg.set(None);
                        shared.events.set(new_events);
                        shared.state.set(Some(st_val));
                        shared.perf.set(pf);
                    }
                    _ => {
                        shared.connected.set(false);
                        shared.error_msg.set(Some("Dashboard can't reach the collector data".to_string()));
                    }
                }
            }
        });
    });

    let window_label = Memo::new(move |_| {
        if is_live.get() {
            format!("Live \u{2022} last {}", window_size.get().label())
        } else {
            format!("{} \u{2013} {}", fmt_local(view_start_ms.get()), fmt_local(view_end_ms.get()))
        }
    });

    view! {
        <Header shared=shared clock=clock settings=settings/>
        <main>
            <TimeControls
                window_size=window_size
                slider_pos=slider_pos
                meta=meta
                is_live=is_live
                window_label=window_label
            />
            <PerfMeters perf=shared.perf/>
            <RiverLanesView
                events=shared.events
                window_size=window_size
                is_live=is_live
                view_end_ms=view_end_ms
            />
            <LiveFeed events=shared.events filter=filter/>
            <aside>
                <WindowCounts state=shared.state window_label=window_label/>
                <EndpointsChart state=shared.state/>
            </aside>
        </main>
    }
}

#[component]
fn Header(shared: SharedState, clock: RwSignal<String>, settings: SettingsState) -> impl IntoView {
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
            <SettingsPanel settings=settings/>
        </header>
    }
}

#[component]
fn SettingsPanel(settings: SettingsState) -> impl IntoView {
    view! {
        <button class="gear-btn" on:click=move |_| settings.panel_open.update(|o| *o = !*o) title="Settings">"\u{2699}"</button>
        <div
            class="overlay"
            style:display=move || if settings.panel_open.get() { "flex" } else { "none" }
            on:click=move |_| settings.panel_open.set(false)
        >
            <div class="overlay-panel" on:click=|ev| ev.stop_propagation()>
                <div class="row">
                    <h2>"Settings"</h2>
                    <button class="overlay-close" on:click=move |_| settings.panel_open.set(false) title="Close">"\u{2715}"</button>
                </div>

                <h3 class="settings-h3">"Theme"</h3>
                <div class="preset-buttons">
                    {PresetId::ALL.iter().map(|&id| {
                        view! {
                            <button
                                class="preset-btn"
                                class:active=move || settings.theme_preset.get() == id
                                on:click=move |_| {
                                    settings.theme_preset.set(id);
                                    settings.theme.set(ThemeColors::preset(id));
                                }
                            >{id.label()}</button>
                        }
                    }).collect_view()}
                </div>
                <div class="swatch-grid">
                    {ColorField::ALL.iter().map(|&(field, label)| {
                        view! {
                            <label class="swatch-row">
                                <span>{label}</span>
                                <input
                                    type="color"
                                    prop:value=move || field.get(&settings.theme.get())
                                    on:input=move |ev| {
                                        let v = event_target_value(&ev);
                                        settings.theme.update(|t| field.set(t, v));
                                    }
                                />
                            </label>
                        }
                    }).collect_view()}
                </div>
                <button
                    class="reset-btn"
                    on:click=move |_| settings.theme.set(ThemeColors::preset(settings.theme_preset.get_untracked()))
                >"Reset to preset"</button>
            </div>
        </div>
    }
}

#[component]
fn TimeControls(
    window_size: RwSignal<WindowSize>,
    slider_pos: RwSignal<f64>,
    meta: RwSignal<Option<MetaResponse>>,
    is_live: Memo<bool>,
    window_label: Memo<String>,
) -> impl IntoView {
    view! {
        <section id="time-controls" style="grid-column:1/-1">
            <div class="row" style="flex-wrap:wrap;gap:.75rem">
                <select
                    prop:value=move || window_size.get().label()
                    on:change=move |ev| {
                        let val = event_target_value(&ev);
                        if let Some(ws) = WindowSize::from_label(&val) {
                            window_size.set(ws);
                        }
                    }
                >
                    {WindowSize::ALL.iter().map(|&w| {
                        let label = w.label();
                        view! { <option value=label selected=move || window_size.get() == w>{label}</option> }
                    }).collect_view()}
                </select>
                <input
                    type="range"
                    min="0"
                    max=SLIDER_STEPS.to_string()
                    step="1"
                    style="flex:1;min-width:150px"
                    disabled=move || meta.get().and_then(|m| m.oldest_ts).is_none()
                    prop:value=move || (slider_pos.get() * SLIDER_STEPS as f64).round().to_string()
                    on:input=move |ev| {
                        let raw = event_target_value(&ev);
                        if let Ok(v) = raw.parse::<f64>() {
                            slider_pos.set((v / SLIDER_STEPS as f64).clamp(0.0, 1.0));
                        }
                    }
                />
                <button
                    class:hot=move || !is_live.get()
                    on:click=move |_| slider_pos.set(1.0)
                    title="Jump back to live"
                >
                    "Live"
                </button>
                <span class="sub">{move || window_label.get()}</span>
            </div>
        </section>
    }
}

#[component]
fn RiverLanesView(
    events: RwSignal<Vec<UnifiedEvent>>,
    window_size: RwSignal<WindowSize>,
    is_live: Memo<bool>,
    view_end_ms: Memo<f64>,
) -> impl IntoView {
    view! {
        <section id="river">
            <div class="row">
                <h2>{move || format!("Last {}", window_size.get().label())} <span class="sub">"one row per AI tool"</span></h2>
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
                    let now = view_end_ms.get();
                    let span_ms = window_size.get().ms();
                    let all = events.get();
                    // One tick per raw event within the window, colored by its
                    // actual kind — the same live event list the feed uses,
                    // not a lossy aggregated view.
                    let mut lanes: std::collections::BTreeMap<String, Vec<(f64, &str, bool)>> = Default::default();
                    for e in &all {
                        let t = js_time_ms(&e.ts);
                        if now - t > span_ms || t > now {
                            continue;
                        }
                        lanes.entry(nice_name(&e.tool)).or_default().push((t, kind_color_var(e.kind), e.flagged));
                    }
                    if lanes.is_empty() {
                        let msg = format!("No AI activity in the last {}.", window_size.get().label());
                        view! { <div class="empty">{msg}</div> }.into_any()
                    } else {
                        lanes.into_iter().map(|(name, marks_data)| {
                            let marks: Vec<_> = marks_data.into_iter().map(|(t, color, flagged)| {
                                let x = 100.0 - (now - t) / span_ms * 100.0;
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
                <div>
                    <span>{move || format!("{} ago", window_size.get().label())}</span>
                    <span>"75%"</span><span>"50%"</span><span>"25%"</span>
                    <span>{move || if is_live.get() { "Now".to_string() } else { "".to_string() }}</span>
                </div>
            </div>
        </section>
    }
}

const VU_SEGMENTS: usize = 12;
/// Bar-fill reference ceiling only ("visually full") — the true byte value
/// is always printed underneath, this constant never hides or misrepresents
/// it, just scales the ladder.
const MEM_CEILING_BYTES: f64 = 4.0 * 1024.0 * 1024.0 * 1024.0;

fn fmt_mem(bytes: f64) -> String {
    if bytes >= 1024.0 * 1024.0 * 1024.0 {
        format!("{:.2} GB", bytes / 1024.0 / 1024.0 / 1024.0)
    } else {
        format!("{:.0} MB", bytes / 1024.0 / 1024.0)
    }
}

/// A small vertical stack of fixed-height segments (bottom-to-top in DOM
/// order, `column-reverse` in CSS draws them growing up from the bottom
/// like a real VU meter). Segment color is fixed by position (green/amber/
/// red bands); the reading decides how many are "lit" — a period-accurate
/// LED-ladder look, not a single bar whose fill color shifts with the value.
fn vu_ladder(pct: f64) -> Vec<impl IntoView> {
    let pct = pct.clamp(0.0, 100.0);
    let lit_count = ((pct / 100.0) * VU_SEGMENTS as f64).ceil() as usize;
    (0..VU_SEGMENTS)
        .map(|j| {
            let color = if j < VU_SEGMENTS * 6 / 10 {
                "var(--vu-lo)"
            } else if j < VU_SEGMENTS * 85 / 100 {
                "var(--vu-mid)"
            } else {
                "var(--vu-hi)"
            };
            let cls = if j < lit_count { "vu-seg lit" } else { "vu-seg" };
            let style = format!("background:{color};color:{color}");
            view! { <span class={cls} style={style}></span> }
        })
        .collect()
}

fn perf_row(name: &str, proc_count: u32, cpu_pct: f64, mem_bytes: f64, gpu_pct: Option<f64>, live: bool, gpu_available: bool, total: bool) -> impl IntoView {
    let name_owned = name.to_string();
    let row_class = if total { "perf-row total" } else { "perf-row" };

    if live {
        let mem_fill_pct = (mem_bytes / MEM_CEILING_BYTES * 100.0).min(100.0);
        let gpu_block = if !gpu_available {
            view! {}.into_any()
        } else if let Some(g) = gpu_pct {
            view! {
                <div class="vu-metric">
                    <div class="vu-ladder">{vu_ladder(g)}</div>
                    <span class="vu-label">{format!("GPU {g:.1}%")}</span>
                </div>
            }
            .into_any()
        } else {
            view! { <div class="vu-metric"><span class="vu-label muted">"GPU n/a"</span></div> }.into_any()
        };
        view! {
            <div class={row_class}>
                <b title={name_owned.clone()}>{name_owned.clone()}</b>
                <span class="perf-count">{format!("{proc_count}p")}</span>
                <div class="vu-metric">
                    <div class="vu-ladder">{vu_ladder(cpu_pct)}</div>
                    <span class="vu-label">{format!("CPU {cpu_pct:.1}%")}</span>
                </div>
                <div class="vu-metric">
                    <div class="vu-ladder">{vu_ladder(mem_fill_pct)}</div>
                    <span class="vu-label">{fmt_mem(mem_bytes)}</span>
                </div>
                {gpu_block}
            </div>
        }
        .into_any()
    } else {
        let gpu_text = if !gpu_available {
            String::new()
        } else {
            match gpu_pct {
                Some(g) => format!("  GPU {g:.1}%"),
                None => "  GPU n/a".to_string(),
            }
        };
        let text = format!("CPU {cpu_pct:.1}%  MEM {}{}", fmt_mem(mem_bytes), gpu_text);
        view! {
            <div class={row_class}>
                <b title={name_owned.clone()}>{name_owned.clone()}</b>
                <span class="perf-count">{format!("{proc_count}p")}</span>
                <code class="perf-avg" style="grid-column:3/-1">{text}</code>
            </div>
        }
        .into_any()
    }
}

fn fmt_compact_total(p: &PerfSummary) -> String {
    let gpu = if !p.gpu_available {
        String::new()
    } else {
        match p.total.gpu_pct {
            Some(g) => format!(" \u{b7} GPU {g:.1}%"),
            None => String::new(),
        }
    };
    format!("{}p \u{b7} CPU {:.1}% \u{b7} MEM {}{}", p.total.proc_count, p.total.cpu_pct, fmt_mem(p.total.mem_bytes as f64), gpu)
}

fn perf_detail_rows(p: &PerfSummary) -> impl IntoView {
    let live = p.is_live;
    let label = if live { "Live" } else { "Avg over this window" };
    let total_row =
        perf_row("Total AI load", p.total.proc_count, p.total.cpu_pct as f64, p.total.mem_bytes as f64, p.total.gpu_pct.map(|g| g as f64), live, p.gpu_available, true);
    let tool_rows: Vec<_> = p
        .tools
        .iter()
        .map(|t| perf_row(&nice_name(&t.tool), t.proc_count, t.cpu_pct as f64, t.mem_bytes as f64, t.gpu_pct.map(|g| g as f64), live, p.gpu_available, false))
        .collect();
    view! {
        <div class="perf-rows">
            <span class="sub perf-mode-label">{label}</span>
            {total_row}
            <div class="perf-divider"></div>
            {tool_rows}
        </div>
    }
}

/// Collapsed by default (a compact one-line strip so it doesn't dominate the
/// page), expanding into a large overlay on click — the full per-tool VU
/// meter grid needs more room than a permanently-inline panel should claim.
#[component]
fn PerfMeters(perf: RwSignal<Option<PerfSummary>>) -> impl IntoView {
    let expanded = RwSignal::new(false);
    view! {
        <section id="perf" style="grid-column:1/-1">
            <button
                class="toggle-strip"
                aria-expanded=move || expanded.get().to_string()
                on:click=move |_| expanded.update(|e| *e = !*e)
            >
                <span class="toggle-strip-title">
                    <span class="toggle-chevron">{move || if expanded.get() { "\u{25be}" } else { "\u{25b8}" }}</span>
                    "AI resource use"
                </span>
                <span class="sub toggle-strip-summary">
                    {move || match perf.get() {
                        None => "Waiting for data\u{2026}".to_string(),
                        Some(p) => fmt_compact_total(&p),
                    }}
                </span>
            </button>
            <div
                class="overlay"
                style:display=move || if expanded.get() { "flex" } else { "none" }
                on:click=move |_| expanded.set(false)
            >
                <div class="overlay-panel" on:click=|ev| ev.stop_propagation()>
                    <div class="row">
                        <h2>"AI resource use"</h2>
                        {move || {
                            if perf.get().map(|p| !p.gpu_available).unwrap_or(false) {
                                view! { <span class="sub">"GPU: n/a on this PC"</span> }.into_any()
                            } else {
                                view! {}.into_any()
                            }
                        }}
                        <button class="overlay-close" on:click=move |_| expanded.set(false) title="Close">"\u{2715}"</button>
                    </div>
                    {move || match perf.get() {
                        None => view! { <div class="empty">"Waiting for data\u{2026}"</div> }.into_any(),
                        Some(p) => perf_detail_rows(&p).into_any(),
                    }}
                </div>
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
fn WindowCounts(state: RwSignal<Option<StateSummary>>, window_label: Memo<String>) -> impl IntoView {
    view! {
        <section>
            <h2>{move || window_label.get()}</h2>
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
                        view! { <div class="empty">"No connections in this window."</div> }.into_any()
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
