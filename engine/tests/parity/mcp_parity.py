#!/usr/bin/env python3
"""MCP parity: runs `python3 mcp_server.py` and `dg mcp` side by side and diffs every response byte for byte.

    mcp_parity.py /path/to/dg

Each side gets its own temporary HOME holding an identical copy of the golden Desktop (engine/tests/golden/desktops/
basic), its own fake DesktopGlass control server (control.json under that HOME, loopback, the app tools' requests are
recorded and diffed too) and the same permission settings:
  - dg reads them from DG_SETTINGS_FILE (dg_platform::read_setting);
  - mcp_server.py runs `defaults read <bundle id> <key>` through PATH, so a fake `defaults` first on PATH answers
    from the same JSON file. mcp_server.py itself is unchanged.
Messages are sent to both servers in lockstep (one message, both replies), so timestamps match. After each session
the two Desktops are diffed as well (description.md files, questions.md, desktop-index.md and .manifest.json with
their timestamps normalised). Nothing outside the temp folder is touched: no real Desktop, settings or app.
Exit 0 when everything matches. DG_PARITY_KEEP=1 keeps the work folder.
"""
import json
import os
import re
import shutil
import socketserver
import subprocess
import sys
import tempfile
import threading
from pathlib import Path

HERE = Path(__file__).resolve().parent
ENGINE = HERE.parent.parent
REPO = ENGINE.parent
SCRIPTS = Path(__file__).resolve().parent / "reference"
FIXTURE = ENGINE / "tests" / "golden" / "desktops" / "basic"
DG = Path(sys.argv[1] if len(sys.argv) > 1 else ENGINE / "target" / "release" / "dg").resolve()
TOKEN = "parity-token"

FAKE_DEFAULTS = r"""#!/usr/bin/env python3
import json, os, sys
# test-only stand-in for `defaults read <bundle id> <key>`, answering from $DG_SETTINGS_FILE
if len(sys.argv) != 4 or sys.argv[1] != "read":
    sys.exit(2)
try:
    data = json.load(open(os.environ["DG_SETTINGS_FILE"]))
except Exception:
    data = {}
if sys.argv[3] not in data:
    sys.exit(1)
v = data[sys.argv[3]]
print(("1" if v else "0") if isinstance(v, bool) else v)
"""


class FakeApp(socketserver.ThreadingTCPServer):
    """A stand-in for DesktopGlass's ControlServer: records each request line and answers per method."""
    allow_reuse_address = True
    daemon_threads = True

    def __init__(self):
        super().__init__(("127.0.0.1", 0), Handler)
        self.requests = []
        threading.Thread(target=self.serve_forever, daemon=True).start()


class Handler(socketserver.StreamRequestHandler):
    def handle(self):
        line = self.rfile.readline()
        self.server.requests.append(line.decode())
        req = json.loads(line)
        m, p = req.get("method"), req.get("params")
        if req.get("token") != TOKEN:
            reply = {"error": "bad token"}
        elif m == "browser.screenshot":
            reply = {"result": {"png": "iVBORw0KGgo=", "title": "Page", "url": "http://localhost:3000/"}}
        elif m == "browser.read":
            reply = {"result": {"title": "Shop", "url": "http://localhost:3000/", "tab": "t1", "text": "Hello\nworld",
                                "fields": [{"tag": "input", "type": "email", "id": "e", "name": "email", "label": "Email"},
                                           {"tag": "textarea", "type": "", "id": "", "name": "", "label": ""}],
                                "links": [{"text": "", "href": "/a"}, {"text": "Plans", "href": "/plans"}]}}
        elif m == "html.remove":
            reply = {"error": "No element matches 9.9"}
        elif m == "html.outline":
            reply = {"error": {"code": 3, "why": "missing"}}
        elif m == "app.status":
            reply = {"result": {"running": True, "version": "1.0.0", "indexing": False}}
        elif m == "schedule.get":
            reply = {"result": "Daily at 9:00"}
        elif m == "overlay.show":
            reply = {"result": None}
        elif m == "servers.list":
            reply = {"result": [{"port": 3000, "title": "Café ☕", "process": "node"}]}
        else:
            reply = {"result": {"method": m, "params": p, "ok": True}}
        self.wfile.write((json.dumps(reply) + "\n").encode())


class Side:
    def __init__(self, name, sess, work, cmd, settings):
        self.name = name
        self.home = sess / name
        self.home.mkdir()
        shutil.copytree(FIXTURE, self.home / "Desktop", symlinks=True, copy_function=shutil.copy2)
        # every fixture entry gets the same fixed mtime on both sides
        for p in [self.home / "Desktop", *sorted((self.home / "Desktop").rglob("*"))]:
            os.utime(p, (1767225600, 1767225600), follow_symlinks=False)   # 2026-01-01
        os.symlink(work / "outside" / "secret", self.home / "Desktop" / "Projects" / "link")
        self.app = FakeApp()
        support = self.home / "Library" / "Application Support" / "DesktopGlass"
        support.mkdir(parents=True)
        (support / "control.json").write_text(json.dumps({"port": self.app.server_address[1], "token": TOKEN, "pid": os.getpid()}))
        env = {"HOME": str(self.home), "PATH": f"{work / 'bin'}:/usr/bin:/bin", "DG_SETTINGS_FILE": str(settings),
               "LANG": "en_US.UTF-8", "TZ": os.environ.get("TZ", "")}
        env = {k: v for k, v in env.items() if v}
        self.proc = subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                     env=env, cwd=str(sess))

    def send(self, line, expect_reply):
        self.proc.stdin.write(line.encode() + b"\n")
        self.proc.stdin.flush()
        return [self.proc.stdout.readline().decode() for _ in range(expect_reply)]

    def close(self):
        self.proc.stdin.close()
        rest = self.proc.stdout.read().decode()
        self.proc.wait(timeout=10)
        self.app.shutdown()
        self.app.server_close()
        return rest


def norm(text):
    text = re.sub(r'"updated":"[^"]*"', '"updated":"T"', text)
    text = re.sub(r"_Updated [A-Z][a-z]{2} \d{2}, \d{4} \d{2}:\d{2} [AP]M · ", "_Updated T · ", text)
    return text


def tree(home):
    out = {}
    root = home / "Desktop"
    for p in sorted(root.rglob("*")):
        rel = p.relative_to(root).as_posix()
        if p.is_symlink():
            out[rel] = "-> " + os.readlink(p).replace(str(home), "~")
        elif p.is_file() and (p.name.endswith(".md") or p.name.endswith(".json")):
            out[rel] = norm(p.read_text(encoding="utf-8", errors="replace"))
        elif p.is_dir():
            out[rel] = "<dir>"
    return out


def call(i, name, args=None, **extra):
    msg = {"jsonrpc": "2.0", "id": i, "method": "tools/call", "params": {"name": name}}
    if args is not None:
        msg["params"]["arguments"] = args
    msg["params"].update(extra)
    return msg


def protocol_messages():
    m = [
        {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}},
        {"jsonrpc": "2.0", "method": "notifications/initialized"},
        {"jsonrpc": "2.0", "id": "a", "method": "initialize", "params": {"protocolVersion": "2025-03-26"}},
        {"jsonrpc": "2.0", "id": 2, "method": "initialize", "params": {"protocolVersion": "2024-11-05"}},
        {"jsonrpc": "2.0", "id": 3, "method": "initialize", "params": {"protocolVersion": "1999-01-01"}},
        {"jsonrpc": "2.0", "id": 4, "method": "initialize"},
        {"jsonrpc": "2.0", "id": None, "method": "ping"},
        {"jsonrpc": "2.0", "id": 5, "method": "ping"},
        {"jsonrpc": "2.0", "id": 6, "method": "tools/list"},
        {"jsonrpc": "2.0", "id": 7, "method": "resources/list"},
        {"jsonrpc": "2.0", "id": 8, "method": "prompts/list"},
        {"jsonrpc": "2.0", "id": 9, "method": "frobnicate"},
        {"jsonrpc": "2.0", "id": 10},
        call(11, "no_such_tool"),
        call(12, None),
        "this is not json",
        "",
        [{"jsonrpc": "2.0", "id": 13, "method": "ping"}, {"jsonrpc": "2.0", "method": "x"}, {"jsonrpc": "2.0", "id": 14, "method": "tools/list"}],
    ]
    return m


def file_messages(n=100):
    c = []
    add = lambda name, args=None, **kw: c.append(call(n + len(c), name, args, **kw))
    add("desktop_overview")
    for p in [None, "", "Projects", "/Projects/", "Projects/../Projects/web", "Docs/Taxes", "Games/Switch", "../", "..",
              "Projects/../../x", "../../../etc", "Projects/link", "Nope", "Loose note.txt", "Projects/Tool.app"]:
        add("list_folder", {} if p is None else {"path": p})
    add("list_folder", {"path": None})
    for args in [{"query": "web"}, {"query": "WEB  portfolio"}, {"query": "zzz"}, {"query": ""}, {"query": "   "}, {},
                 {"query": "a", "limit": 1}, {"query": "a", "limit": "2"}, {"query": "a", "limit": -1},
                 {"query": "a", "limit": 0}, {"query": "Projects"}, {"query": "taxes"}, {"query": 2024}]:
        add("search_desktop", args)
    for args in [{"path": "Projects"}, {"path": "Docs"}, {"path": "Docs/Taxes"}, {"path": "../../etc"}, {},
                 {"path": "Projects/link"}, {"path": ""}]:
        add("get_description", args)
    for args in [{}, {"days": 3650}, {"days": 3650, "limit": 3}, {"days": 0.5}, {"days": "3650", "limit": "2"},
                 {"days": "abc"}, {"limit": "x"}, {"days": 2}]:
        add("recent_changes", args)
    add("list_questions")
    add("index_status")
    add("index_status", [])
    return c


def edit_messages(n=300):
    c = []
    add = lambda name, args=None: c.append(call(n + len(c), name, args))
    add("update_description", {"path": "Projects", "summary": "ok\n## Contents\n- **evil.txt** — injected",
                               "tags": ["a,b", "two\nlines", ""], "blurbs": {"notes.txt": "fine\n# Title takeover"},
                               "notes": "keep this\n## Contents\n- **x** — y"})
    for p in ["", "Projects/web", "Projects/notes.txt", "Nope", "..", "../", "Projects/link"]:
        add("update_description", {"path": p, "summary": "x"})
    add("update_description", {"summary": "x"})
    add("update_description", {"path": "Projects", "blurbs": {"ghost.txt": "boo", "web/": "site"}})
    add("update_description", {"path": "Projects", "summary": "s" * 500, "tags": [f"t{i}" for i in range(10)]})
    add("update_description", {"path": "Docs", "summary": "Paperwork", "tags": "xy", "notes": None})
    add("update_description", {"path": "Games", "notes": "# heading\nreal note\n\n  indented  "})
    add("update_description", {"path": "Games", "blurbs": ["a"]})
    add("update_description", {"path": "Docs/Taxes", "blurbs": {"2024/": "Return", "2024": "", "nope/": 3}})
    add("get_description", {"path": "Projects"})
    add("get_description", {"path": "Docs"})
    add("list_questions")
    add("resolve_question", {"question": "what is games/switch", "answer": "Switch backups\n## Contents\n- **x**"})
    add("resolve_question", {"question": "is web live"})
    add("resolve_question", {"question": "nothing like this", "answer": "x"})
    add("resolve_question", {"answer": "x"})
    add("list_questions")
    add("desktop_overview")
    add("index_status")
    add("list_folder", {"path": "Projects"})
    add("search_desktop", {"query": "evil"})
    return c


def app_messages(n=500):
    c = []
    add = lambda name, args=None: c.append(call(n + len(c), name, args))
    add("get_schedule")
    add("set_schedule", {"schedule": "daily", "hour": 9})
    add("set_schedule", {"schedule": "off"})
    add("set_schedule", {})
    add("set_index_backend", {"backend": "ollama", "model": "qwen3", "endpoint": "http://127.0.0.1:11434"})
    add("set_preferences", {"style": "sentence", "folder": "Docs", "folder_mode": "skip"})
    add("open_in_editor", {"path": "Projects/notes.txt"})
    add("open_in_editor", {})
    add("show_overlay")
    add("show_artifact", {"target": "~/Desktop/page.html"})
    add("show_artifact", {"target": "http://localhost:3000", "title": "Dev"})
    add("list_local_servers")
    add("browser_tabs")
    add("browser_read")
    add("browser_read", {"tab": "t1", "max_chars": 500})
    add("browser_screenshot", {"tab": "t1"})
    add("browser_click", {"selector": "text=Submit"})
    add("browser_type", {"selector": "#e", "text": "a@b.c", "submit": True})
    add("browser_eval", {"script": "1+1"})
    add("browser_navigate", {"action": "go", "url": "http://localhost:3000/x"})
    add("browser_navigate", {"action": "back"})
    add("html_outline", {"path": "Projects/web/index.html"})
    add("html_set_style", {"path": "a.html", "element": "#x", "properties": {"color": "red"}})
    add("html_split_style", {"path": "a.html", "element": "1.0", "new_class": "hero-2"})
    add("html_group_style", {"path": "a.html", "elements": ["1", "2"], "class_name": "card"})
    add("html_set_text", {"path": "a.html", "element": "h1", "text": "Hi ☕"})
    add("html_set_html", {"path": "a.html", "element": "p", "html": "Save <b>40%</b>"})
    add("html_duplicate_element", {"path": "a.html", "element": "li:2"})
    add("html_remove_element", {"path": "a.html", "element": "9.9"})
    add("list_games", {"query": "zelda"})
    add("play_game", {"game": "Zelda"})
    add("reindex_desktop", {"full": True, "wait": True})
    add("reindex_desktop", {})
    add("index_status")
    return c


SESSIONS = [
    # (name, settings, messages)
    ("default (read only)", {}, lambda: protocol_messages() + file_messages() + edit_messages() + app_messages()),
    ("all allowed", {"mcp.allow.edit": True, "mcp.allow.control": True, "mcp.allow.script": True, "mcp.allow.play": True},
     lambda: protocol_messages() + file_messages() + edit_messages() + app_messages()),
    ("edit only (string setting)", {"mcp.allow.edit": "YES", "mcp.allow.control": "0", "mcp.allow.script": False},
     lambda: protocol_messages()[8:9] + edit_messages() + app_messages()),
    ("script + play", {"mcp.allow.script": "true", "mcp.allow.play": 1},
     lambda: protocol_messages()[8:9] + edit_messages()[:3] + app_messages()),
]


def expected_replies(msg):
    if isinstance(msg, str):
        return 1 if msg.strip() and not _is_json(msg) else (0 if not msg.strip() else 1)
    msgs = msg if isinstance(msg, list) else [msg]
    return sum(1 for m in msgs if m.get("id") is not None)


def _is_json(s):
    try:
        json.loads(s)
        return True
    except ValueError:
        return False


def install_test(work):
    """mcp_install.py's own self-test (test_server) pointed at `dg mcp`, with HOME in the temp folder."""
    home = work / "install-home"
    shutil.copytree(FIXTURE, home / "Desktop")
    code = ("import json, sys; sys.dont_write_bytecode = True; sys.path.insert(0, sys.argv[1]); import mcp_install as m; "
            "m.COMMAND, m.ARGS = sys.argv[2], ['mcp']; print(json.dumps(m.test_server()))")
    env = {"HOME": str(home), "PATH": f"{work / 'bin'}:/usr/bin:/bin", "DG_SETTINGS_FILE": str(work / "none.json")}
    r = subprocess.run([sys.executable, "-I", "-c", code, str(SCRIPTS), str(DG)], capture_output=True, text=True, env=env, timeout=60)
    return json.loads(r.stdout)


def main():
    if not DG.exists():
        print(f"mcp-parity: no dg binary at {DG} (cargo build --release)")
        return 2
    work = Path(tempfile.mkdtemp(prefix="dg-mcp-parity.")).resolve()
    checks = fails = 0
    try:
        (work / "bin").mkdir()
        (work / "bin" / "defaults").write_text(FAKE_DEFAULTS)
        (work / "bin" / "defaults").chmod(0o755)
        (work / "outside" / "secret").mkdir(parents=True)
        (work / "outside" / "secret" / "description.md").write_text("# Secret\n> do not touch\n")
        (work / "none.json").write_text("{}")
        for si, (title, settings, messages) in enumerate(SESSIONS):
            print(f"== {title}")
            sfile = work / f"settings{si}.json"
            sfile.write_text(json.dumps(settings))
            sess = work / f"s{si}"
            sess.mkdir()
            py = Side("python", sess, work, [sys.executable, "-I", str(SCRIPTS / "mcp_server.py")], sfile)
            dg = Side("dg", sess, work, [str(DG), "mcp"], sfile)
            for msg in messages():
                line = msg if isinstance(msg, str) else json.dumps(msg)
                n = expected_replies(msg)
                a, b = py.send(line, n), dg.send(line, n)
                checks += 1
                label = line[:110]
                if a == b:
                    continue
                fails += 1
                print(f"✗ {label}\n  python: {''.join(a).strip()[:800]}\n  dg:     {''.join(b).strip()[:800]}")
            ra, rb = py.close(), dg.close()
            checks += 1
            if ra != rb:
                fails += 1
                print(f"✗ trailing output differs:\n  python: {ra[:300]}\n  dg:     {rb[:300]}")
            checks += 1
            if py.app.requests != dg.app.requests:
                fails += 1
                print("✗ control-channel requests differ")
                for x, y in zip(py.app.requests, dg.app.requests):
                    if x != y:
                        print(f"  python: {x.strip()}\n  dg:     {y.strip()}")
                print(f"  counts: python={len(py.app.requests)} dg={len(dg.app.requests)}")
            checks += 1
            ta, tb = tree(py.home), tree(dg.home)
            if ta != tb:
                fails += 1
                print("✗ Desktop contents differ after the session")
                for k in sorted(set(ta) | set(tb)):
                    if ta.get(k) != tb.get(k):
                        print(f"  {k}:\n    python: {ta.get(k)!r}\n    dg:     {tb.get(k)!r}")
            checks += 1
            if "do not touch" not in (work / "outside" / "secret" / "description.md").read_text():
                fails += 1
                print("✗ a file outside the Desktop was written")
            print(f"   {len(py.app.requests)} control-channel requests, {len(messages())} messages")
        print("== mcp_install.py test_server() against dg mcp")
        r = install_test(work)
        checks += 1
        ok = r.get("ok") and len(r.get("tools", [])) == 18 and r.get("status", "").startswith("Indexed ")
        if ok:
            print(f"✓ ok={r['ok']} tools={len(r['tools'])} status={r['status']!r} ({r['ms']} ms)")
        else:
            fails += 1
            print(f"✗ {r}")
    finally:
        if os.environ.get("DG_PARITY_KEEP") == "1":
            print(f"kept {work}")
        else:
            shutil.rmtree(work, ignore_errors=True)
    print(f"\nmcp parity: {checks - fails}/{checks} matched")
    return 0 if fails == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
