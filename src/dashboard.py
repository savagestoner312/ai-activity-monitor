"""
AI Activity Monitor - live dashboard (localhost only).
Run:  python dashboard.py      then open http://127.0.0.1:8765
Reads the same SQLite DB the collector writes. No extra packages needed.
"""
import os, sqlite3, json, datetime
from http.server import ThreadingHTTPServer, BaseHTTPRequestHandler
from urllib.parse import urlparse, parse_qs

PORT = 8765
BASE = os.path.join(os.environ.get("LOCALAPPDATA", "."), "AIMonitor")
DB_PATH = os.path.join(BASE, "aimon.db")
HERE = os.path.dirname(os.path.abspath(__file__))

def q(sql, args=()):
    db = sqlite3.connect(f"file:{DB_PATH}?mode=ro", uri=True, timeout=5)
    try: return db.execute(sql, args).fetchall()
    finally: db.close()

def events(after, limit=300):
    rows = q("""
      SELECT * FROM (
        SELECT ts,'process' kind,name tool,event||': '||COALESCE(NULLIF(cmdline,''),exe) detail,0 flagged FROM process_events WHERE ts>?
        UNION ALL SELECT ts,'command',parent_name,cmdline,flagged FROM child_events WHERE ts>?
        UNION ALL SELECT ts,'network',name,COALESCE(NULLIF(rhost,''),raddr)||':'||rport,0 FROM net_events WHERE ts>?
        UNION ALL SELECT ts,'device',app,device||CASE WHEN stopped='' THEN ' in use' ELSE ' used until '||stopped END,is_ai FROM device_events WHERE ts>?
      ) ORDER BY ts DESC LIMIT ?""", (after,)*4 + (limit,))
    return [dict(zip(["ts","kind","tool","detail","flagged"], r)) for r in rows]

def state():
    day = datetime.date.today().isoformat() + "%"
    running = q("""SELECT p.name, p.pid, MAX(p.ts) FROM process_events p
                   WHERE p.event='start' AND NOT EXISTS (SELECT 1 FROM process_events s
                   WHERE s.pid=p.pid AND s.event='stop' AND s.ts>=p.ts) GROUP BY p.pid""")
    counts = dict(
        launched=q("SELECT COUNT(*) FROM process_events WHERE event='start' AND ts LIKE ?", (day,))[0][0],
        commands=q("SELECT COUNT(*) FROM child_events WHERE ts LIKE ?", (day,))[0][0],
        flagged=q("SELECT COUNT(*) FROM child_events WHERE flagged=1 AND ts LIKE ?", (day,))[0][0],
        endpoints=q("SELECT COUNT(DISTINCT COALESCE(NULLIF(rhost,''),raddr)) FROM net_events WHERE ts LIKE ?", (day,))[0][0])
    live_dev = q("""SELECT device, app FROM device_events d WHERE stopped='' AND ts=(SELECT MAX(ts) FROM device_events
                    WHERE device=d.device AND app=d.app)""")
    endpoints = q("""SELECT name, COALESCE(NULLIF(rhost,''),raddr) h, COUNT(*) FROM net_events WHERE ts LIKE ?
                     GROUP BY 1,2 ORDER BY 3 DESC LIMIT 8""", (day,))
    return dict(running=[{"name":n,"pid":p,"since":t} for n,p,t in running], counts=counts,
                devices=[{"device":d,"app":a} for d,a in live_dev],
                endpoints=[{"tool":t,"host":h,"n":n} for t,h,n in endpoints],
                now=datetime.datetime.now().isoformat(timespec="seconds"))

class H(BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def send(self, body, ctype="application/json"):
        b = body.encode() if isinstance(body, str) else body
        self.send_response(200); self.send_header("Content-Type", ctype)
        self.send_header("Cache-Control", "no-store"); self.end_headers(); self.wfile.write(b)
    def do_GET(self):
        u = urlparse(self.path); qs = parse_qs(u.query)
        try:
            if u.path == "/": self.send(open(os.path.join(HERE, "dashboard.html"), "rb").read(), "text/html; charset=utf-8")
            elif u.path == "/api/state": self.send(json.dumps(state()))
            elif u.path == "/api/events":
                after = qs.get("after", [(datetime.datetime.now()-datetime.timedelta(hours=1)).isoformat(timespec="seconds")])[0]
                self.send(json.dumps(events(after)))
            else: self.send_error(404)
        except sqlite3.OperationalError as e:
            self.send(json.dumps({"error": f"Database not found yet. Start the collector first. ({e})"}))

if __name__ == "__main__":
    print(f"AI Activity Monitor dashboard: http://127.0.0.1:{PORT}")
    ThreadingHTTPServer(("127.0.0.1", PORT), H).serve_forever()
