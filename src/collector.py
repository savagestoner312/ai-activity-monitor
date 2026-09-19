"""
AI Activity Monitor - collector (Windows, home PC)
Polls every few seconds and logs to SQLite:
  - AI tool processes starting/stopping
  - child processes spawned by AI tools (commands agents run)
  - network connections made by AI tools
  - mic / camera usage by ANY app (from Windows CapabilityAccessManager)
Run:  pythonw collector.py      (pip install psutil)
"""
import os, sqlite3, time, datetime, socket, sys
import psutil

POLL_SECONDS = 3
DB_DIR = os.path.join(os.environ.get("LOCALAPPDATA", "."), "AIMonitor")
DB_PATH = os.path.join(DB_DIR, "aimon.db")

# Match on process name OR command line (lowercase substrings). Edit to taste.
AI_NAME_MATCH = ["copilot", "claude", "chatgpt", "ollama", "lm studio", "lmstudio",
                 "cursor", "windsurf", "gemini", "perplexity", "jan.exe", "gpt4all"]
AI_CMDLINE_MATCH = ["@github/copilot", "copilot-cli", "claude-code", "@anthropic-ai",
                    "openai", "ollama", "aider", "open-interpreter"]
SENSITIVE_CHILDREN = ["powershell", "pwsh", "cmd.exe", "wsl", "bash", "curl", "git",
                      "python", "node", "reg.exe", "schtasks", "net.exe"]

SCHEMA = """
CREATE TABLE IF NOT EXISTS process_events(ts TEXT, event TEXT, pid INT, name TEXT, exe TEXT, cmdline TEXT);
CREATE TABLE IF NOT EXISTS child_events(ts TEXT, parent_pid INT, parent_name TEXT, pid INT, name TEXT, cmdline TEXT, flagged INT);
CREATE TABLE IF NOT EXISTS net_events(ts TEXT, pid INT, name TEXT, raddr TEXT, rport INT, rhost TEXT, status TEXT);
CREATE TABLE IF NOT EXISTS device_events(ts TEXT, device TEXT, app TEXT, started TEXT, stopped TEXT, is_ai INT);
"""

def now(): return datetime.datetime.now().isoformat(timespec="seconds")

def is_ai(name, cmd):
    n, c = (name or "").lower(), (cmd or "").lower()
    return any(k in n for k in AI_NAME_MATCH) or any(k in c for k in AI_CMDLINE_MATCH)

def proc_info(p):
    try:
        return p.pid, p.name(), (p.exe() or ""), " ".join(p.cmdline())[:2000]
    except (psutil.NoSuchProcess, psutil.AccessDenied):
        return None

_dns = {}
def rhost(ip):
    if ip not in _dns:
        try: _dns[ip] = socket.gethostbyaddr(ip)[0]
        except Exception: _dns[ip] = ""
    return _dns[ip]

def filetime_to_iso(ft):
    if not ft: return ""
    return (datetime.datetime(1601, 1, 1) + datetime.timedelta(microseconds=ft // 10)).isoformat(timespec="seconds")

def read_device_usage():
    """Yield (device, app, start, stop) from HKCU ConsentStore (Windows 10/11)."""
    if sys.platform != "win32": return
    import winreg
    base = r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore"
    for device in ("microphone", "webcam"):
        for sub in ("", "\\NonPackaged"):
            try: key = winreg.OpenKey(winreg.HKEY_CURRENT_USER, base + "\\" + device + sub)
            except OSError: continue
            i = 0
            while True:
                try: app = winreg.EnumKey(key, i); i += 1
                except OSError: break
                if app == "NonPackaged": continue
                try:
                    k = winreg.OpenKey(key, app)
                    start = winreg.QueryValueEx(k, "LastUsedTimeStart")[0]
                    stop = winreg.QueryValueEx(k, "LastUsedTimeStop")[0]
                    yield device, app.replace("#", "\\"), filetime_to_iso(start), filetime_to_iso(stop)
                except OSError: continue

def main():
    os.makedirs(DB_DIR, exist_ok=True)
    db = sqlite3.connect(DB_PATH); db.executescript(SCHEMA)
    ai_pids, seen_children, seen_conns, seen_dev = {}, set(), set(), {}
    # Baseline device usage so we only log new sessions
    for d, a, s, e in read_device_usage(): seen_dev[(d, a)] = (s, e)

    while True:
        ts, current = now(), {}
        for p in psutil.process_iter():
            info = proc_info(p)
            if info and info[0] != os.getpid() and is_ai(info[1], info[3]): current[info[0]] = info

        for pid, (pid_, name, exe, cmd) in current.items():
            if pid not in ai_pids:
                db.execute("INSERT INTO process_events VALUES(?,?,?,?,?,?)", (ts, "start", pid, name, exe, cmd))
        for pid, (pid_, name, exe, cmd) in ai_pids.items():
            if pid not in current:
                db.execute("INSERT INTO process_events VALUES(?,?,?,?,?,?)", (ts, "stop", pid, name, exe, cmd))
        ai_pids = current

        # children: attribute each descendant to its NEAREST AI ancestor, log once
        me = os.getpid()
        for pid, (_, pname, _, _) in current.items():
            try: kids = psutil.Process(pid).children(recursive=True)
            except psutil.Error: kids = []
            for k in kids:
                if k.pid in current or k.pid == me or k.pid in seen_children: continue
                try:
                    anc = k.parent()
                    while anc and anc.pid not in current: anc = anc.parent()
                except psutil.Error: anc = None
                if not anc or anc.pid != pid: continue
                ki = proc_info(k)
                if not ki: continue
                seen_children.add(ki[0])
                flagged = int(any(x in ki[1].lower() for x in SENSITIVE_CHILDREN))
                db.execute("INSERT INTO child_events VALUES(?,?,?,?,?,?,?)", (ts, pid, pname, ki[0], ki[1], ki[3], flagged))
            # network
            try: conns = psutil.Process(pid).net_connections(kind="inet")
            except (psutil.Error, AttributeError): conns = []
            for c in conns:
                if not c.raddr: continue
                key = (pid, c.raddr.ip, c.raddr.port)
                if key in seen_conns: continue
                seen_conns.add(key)
                db.execute("INSERT INTO net_events VALUES(?,?,?,?,?,?,?)",
                           (ts, pid, pname, c.raddr.ip, c.raddr.port, rhost(c.raddr.ip), c.status))

        for d, a, s, e in read_device_usage():
            if seen_dev.get((d, a)) != (s, e):
                seen_dev[(d, a)] = (s, e)
                db.execute("INSERT INTO device_events VALUES(?,?,?,?,?,?)", (ts, d, a, s, e, int(is_ai(a, a))))

        db.commit()
        if len(seen_conns) > 50000: seen_conns.clear()
        if len(seen_children) > 50000: seen_children.clear()
        time.sleep(POLL_SECONDS)

if __name__ == "__main__":
    main()
