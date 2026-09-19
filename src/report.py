"""
AI Activity Monitor - daily report. Builds an HTML summary for a day and opens it.
Usage:  python report.py            (today)
        python report.py 2026-09-17 (specific day)
"""
import os, sqlite3, sys, datetime, html, webbrowser
from collections import Counter

DB_PATH = os.path.join(os.environ.get("LOCALAPPDATA", "."), "AIMonitor", "aimon.db")
OUT_DIR = os.path.join(os.environ.get("LOCALAPPDATA", "."), "AIMonitor", "reports")

def table(title, headers, rows):
    if not rows: return f"<h2>{title}</h2><p class=muted>Nothing recorded.</p>"
    th = "".join(f"<th>{h}</th>" for h in headers)
    trs = "".join("<tr>" + "".join(f"<td>{html.escape(str(v))}</td>" for v in r) + "</tr>" for r in rows)
    return f"<h2>{title}</h2><div class=wrap><table><tr>{th}</tr>{trs}</table></div>"

def main():
    day = sys.argv[1] if len(sys.argv) > 1 else datetime.date.today().isoformat()
    db = sqlite3.connect(DB_PATH); q = lambda s: db.execute(s, (day + "%",)).fetchall()
    procs = q("SELECT ts,event,name,cmdline FROM process_events WHERE ts LIKE ? ORDER BY ts")
    kids = q("SELECT ts,parent_name,name,cmdline,flagged FROM child_events WHERE ts LIKE ? ORDER BY ts")
    nets = q("SELECT name,COALESCE(NULLIF(rhost,''),raddr),rport,COUNT(*),MIN(ts),MAX(ts) FROM net_events WHERE ts LIKE ? GROUP BY 1,2,3 ORDER BY 4 DESC")
    devs = q("SELECT ts,device,app,started,stopped,is_ai FROM device_events WHERE ts LIKE ? ORDER BY ts")

    tools = Counter(r[2] for r in procs if r[1] == "start")
    flagged = [k for k in kids if k[4]]
    cards = [("AI tools launched", sum(tools.values())), ("Commands run by AI", len(kids)),
             ("Flagged commands", len(flagged)), ("Remote endpoints", len(nets)),
             ("Mic/camera sessions", len(devs))]
    card_html = "".join(f"<div class=card><b>{v}</b><span>{k}</span></div>" for k, v in cards)

    body = f"""<h1>AI Activity — {day}</h1><div class=cards>{card_html}</div>
    {table("Flagged commands (shells, scripts, git, curl...)", ["Time","AI tool","Process","Command"], [k[:4] for k in flagged])}
    {table("AI tools started / stopped", ["Time","Event","Process","Command line"], procs)}
    {table("Network endpoints contacted", ["AI tool","Host / IP","Port","Conns","First","Last"], nets)}
    {table("Microphone / camera use (all apps)", ["Logged","Device","App","Started","Stopped","AI?"], devs)}
    {table("All child processes spawned by AI tools", ["Time","AI tool","Process","Command","Flagged"], kids)}"""

    css = """:root{--bg:#fff;--fg:#1a1a1a;--muted:#666;--line:#e3e3e3;--card:#f5f6f8;--acc:#2f6fed}
    @media (prefers-color-scheme:dark){:root{--bg:#141517;--fg:#e8e8e8;--muted:#999;--line:#2c2e33;--card:#1d1f23;--acc:#6d9bff}}
    body{font-family:Segoe UI,system-ui,sans-serif;background:var(--bg);color:var(--fg);max-width:1200px;margin:2rem auto;padding:0 1rem}
    h2{margin-top:2rem;font-size:1.1rem}.muted{color:var(--muted)}
    .cards{display:flex;gap:.75rem;flex-wrap:wrap}.card{background:var(--card);border-radius:10px;padding:1rem;min-width:150px}
    .card b{display:block;font-size:1.8rem;color:var(--acc)}.card span{color:var(--muted);font-size:.85rem}
    .wrap{overflow-x:auto}table{border-collapse:collapse;width:100%;font-size:.82rem}
    td,th{border-bottom:1px solid var(--line);padding:.35rem .5rem;text-align:left;vertical-align:top;max-width:600px;word-break:break-word}"""
    os.makedirs(OUT_DIR, exist_ok=True)
    out = os.path.join(OUT_DIR, f"ai-activity-{day}.html")
    with open(out, "w", encoding="utf-8") as f:
        f.write(f"<!doctype html><meta charset=utf-8><title>AI Activity {day}</title><style>{css}</style>{body}")
    print(out); webbrowser.open(out)

if __name__ == "__main__":
    main()
