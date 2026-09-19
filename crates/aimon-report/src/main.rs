//! AI Activity Monitor - daily report. Builds an HTML summary for a day and
//! opens it. Direct port of report.py.
//! Usage:  aimon-report            (today)
//!         aimon-report 2026-09-17 (specific day)
use aimon_core::{db, paths, queries};

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#x27;")
}

fn table_html(title: &str, headers: &[&str], rows: &[Vec<String>]) -> String {
    if rows.is_empty() {
        return format!("<h2>{title}</h2><p class=muted>Nothing recorded.</p>");
    }
    let th: String = headers.iter().map(|h| format!("<th>{h}</th>")).collect();
    let trs: String = rows
        .iter()
        .map(|r| {
            let tds: String = r.iter().map(|v| format!("<td>{}</td>", esc(v))).collect();
            format!("<tr>{tds}</tr>")
        })
        .collect();
    format!("<h2>{title}</h2><div class=wrap><table><tr>{th}</tr>{trs}</table></div>")
}

const CSS: &str = ":root{--bg:#fff;--fg:#1a1a1a;--muted:#666;--line:#e3e3e3;--card:#f5f6f8;--acc:#2f6fed}
@media (prefers-color-scheme:dark){:root{--bg:#141517;--fg:#e8e8e8;--muted:#999;--line:#2c2e33;--card:#1d1f23;--acc:#6d9bff}}
body{font-family:Segoe UI,system-ui,sans-serif;background:var(--bg);color:var(--fg);max-width:1200px;margin:2rem auto;padding:0 1rem}
h2{margin-top:2rem;font-size:1.1rem}.muted{color:var(--muted)}
.cards{display:flex;gap:.75rem;flex-wrap:wrap}.card{background:var(--card);border-radius:10px;padding:1rem;min-width:150px}
.card b{display:block;font-size:1.8rem;color:var(--acc)}.card span{color:var(--muted);font-size:.85rem}
.wrap{overflow-x:auto}table{border-collapse:collapse;width:100%;font-size:.82rem}
td,th{border-bottom:1px solid var(--line);padding:.35rem .5rem;text-align:left;vertical-align:top;max-width:600px;word-break:break-word}";

fn main() {
    let day = std::env::args().nth(1).unwrap_or_else(|| chrono::Local::now().date_naive().to_string());
    let conn = db::open_ro(&paths::db_path()).expect("open aimon.db");

    let procs = queries::process_events_for_day(&conn, &day).unwrap_or_default();
    let kids = queries::child_events_for_day(&conn, &day).unwrap_or_default();
    let nets = queries::net_summary_for_day(&conn, &day).unwrap_or_default();
    let devs = queries::device_events_for_day(&conn, &day).unwrap_or_default();

    let tools_launched = procs.iter().filter(|p| p.event == "start").count();
    let flagged: Vec<_> = kids.iter().filter(|k| k.flagged).collect();

    let cards = [
        ("AI tools launched", tools_launched),
        ("Commands run by AI", kids.len()),
        ("Flagged commands", flagged.len()),
        ("Remote endpoints", nets.len()),
        ("Mic/camera sessions", devs.len()),
    ];
    let card_html: String =
        cards.iter().map(|(k, v)| format!("<div class=card><b>{v}</b><span>{k}</span></div>")).collect();

    let flagged_rows: Vec<Vec<String>> =
        flagged.iter().map(|k| vec![k.ts.clone(), k.parent_name.clone(), k.name.clone(), k.cmdline.clone()]).collect();
    let proc_rows: Vec<Vec<String>> =
        procs.iter().map(|p| vec![p.ts.clone(), p.event.clone(), p.name.clone(), p.cmdline.clone()]).collect();
    let net_rows: Vec<Vec<String>> = nets
        .iter()
        .map(|n| vec![n.tool.clone(), n.host.clone(), n.port.to_string(), n.conns.to_string(), n.first.clone(), n.last.clone()])
        .collect();
    let dev_rows: Vec<Vec<String>> = devs
        .iter()
        .map(|d| {
            vec![
                d.ts.clone(),
                d.device.clone(),
                d.app.clone(),
                d.started.clone(),
                d.stopped.clone(),
                if d.is_ai { "yes" } else { "no" }.to_string(),
            ]
        })
        .collect();
    let kid_rows: Vec<Vec<String>> = kids
        .iter()
        .map(|k| {
            vec![
                k.ts.clone(),
                k.parent_name.clone(),
                k.name.clone(),
                k.cmdline.clone(),
                if k.flagged { "yes" } else { "no" }.to_string(),
            ]
        })
        .collect();

    let body = format!(
        "<h1>AI Activity \u{2014} {day}</h1><div class=cards>{card_html}</div>{}{}{}{}{}",
        table_html("Flagged commands (shells, scripts, git, curl...)", &["Time", "AI tool", "Process", "Command"], &flagged_rows),
        table_html("AI tools started / stopped", &["Time", "Event", "Process", "Command line"], &proc_rows),
        table_html("Network endpoints contacted", &["AI tool", "Host / IP", "Port", "Conns", "First", "Last"], &net_rows),
        table_html("Microphone / camera use (all apps)", &["Logged", "Device", "App", "Started", "Stopped", "AI?"], &dev_rows),
        table_html("All child processes spawned by AI tools", &["Time", "AI tool", "Process", "Command", "Flagged"], &kid_rows),
    );

    let reports_dir = paths::reports_dir();
    std::fs::create_dir_all(&reports_dir).expect("create reports dir");
    let out = reports_dir.join(format!("ai-activity-{day}.html"));
    let html = format!("<!doctype html><meta charset=utf-8><title>AI Activity {day}</title><style>{CSS}</style>{body}");
    std::fs::write(&out, html).expect("write report");

    println!("{}", out.display());
    let _ = webbrowser::open(&out.to_string_lossy());
}
