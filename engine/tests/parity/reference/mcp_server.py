#!/usr/bin/env python3
"""DesktopGlass MCP server (stdio, standard library only).

Gives any MCP client (Claude Code, Claude Desktop, Codex, Cursor, Gemini CLI, Copilot CLI, OpenCode, LM Studio, …)
read access to the organized Desktop: the summary index, each folder's description.md, search across blurbs,
open questions, and a way to start a re-index with the backend chosen in DesktopGlass Settings.
Everything stays inside ~/Desktop; nothing here moves, renames or deletes files.
"""
import contextlib
import json
import os
import re
import socket
import subprocess
import sys
import time
from pathlib import Path

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
import inventory as inv  # noqa: E402

ROOT = Path(os.environ.get("DESKTOP_GLASS_ROOT", Path.home() / "Desktop")).expanduser().resolve()
LOG = ROOT / inv.LOG_DIR
def _app_info():
    try:
        return json.loads((Path(__file__).resolve().parent / "app.json").read_text())
    except Exception:
        return {}


APP_INFO = _app_info()
VERSION = APP_INFO.get("version", "dev")
BUNDLE_ID = APP_INFO.get("bundleId", "com.mannyfowler.desktopglass")
CONTROL = Path.home() / "Library" / "Application Support" / "DesktopGlass" / "control.json"
APP_PATHS = [Path.home() / "Applications" / "DesktopGlass.app", Path("/Applications/DesktopGlass.app")]


class AppUnavailable(Exception):
    pass


def _control_info():
    try:
        info = json.loads(CONTROL.read_text())
        os.kill(int(info["pid"]), 0)          # still running?
        return info
    except Exception:
        return None


def ensure_app(launch=True):
    info = _control_info()
    if info or not launch:
        return info
    app = next((a for a in APP_PATHS if a.exists()), None)
    if not app:
        raise AppUnavailable("DesktopGlass isn't installed — run install.sh")
    subprocess.run(["open", "-g", str(app)], capture_output=True, timeout=15)
    for _ in range(50):
        time.sleep(0.2)
        info = _control_info()
        if info:
            time.sleep(0.3)
            return info
    raise AppUnavailable("DesktopGlass didn't start")


def app_call(method, params=None, timeout=120, launch=True):
    """Asks the running DesktopGlass app to do something (browser, schedule, re-index, preferences…)."""
    info = ensure_app(launch)
    if not info:
        raise AppUnavailable("DesktopGlass isn't running")
    try:
        with socket.create_connection(("127.0.0.1", int(info["port"])), timeout=5) as sock:
            sock.settimeout(timeout)
            sock.sendall((json.dumps({"token": info["token"], "method": method, "params": params or {}}) + "\n").encode())
            buf = b""
            while not buf.endswith(b"\n"):
                chunk = sock.recv(1 << 20)
                if not chunk:
                    break
                buf += chunk
    except OSError as e:
        raise AppUnavailable(f"Couldn't reach DesktopGlass ({e})")
    reply = json.loads(buf.decode() or "{}")
    if "error" in reply:
        raise RuntimeError(reply["error"])
    return reply.get("result")


def as_text(value):
    return value if isinstance(value, str) else json.dumps(value, indent=2, ensure_ascii=False)
SUPPORTED = ["2025-06-18", "2025-03-26", "2024-11-05"]

TOOLS = [
    {"name": "desktop_overview", "description": "The whole Desktop at a glance: every folder's summary and what's in it (desktop-index.md). Start here.",
     "inputSchema": {"type": "object", "properties": {}, "additionalProperties": False}},
    {"name": "list_folder", "description": "One Desktop folder: summary, tags and a blurb for every item inside. Path is Desktop-relative, e.g. 'Projects/MyApp'; empty = Desktop root.",
     "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}}, "additionalProperties": False}},
    {"name": "search_desktop", "description": "Find files, folders and projects on the Desktop by name, blurb or tag.",
     "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}, "limit": {"type": "integer", "minimum": 1, "maximum": 100}}, "additionalProperties": False, "required": ["query"]}},
    {"name": "get_description", "description": "The raw description.md of a Desktop folder (Desktop-relative path).",
     "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}}, "additionalProperties": False, "required": ["path"]}},
    {"name": "recent_changes", "description": "What changed on the Desktop recently: files and folders modified in the last N days, newest first, with their blurbs.",
     "inputSchema": {"type": "object", "properties": {"days": {"type": "number", "minimum": 0.01, "maximum": 365}, "limit": {"type": "integer", "minimum": 1, "maximum": 200}}, "additionalProperties": False}},
    {"name": "update_description", "description": "Improve a folder's description.md: set its summary, tags, notes, or individual item blurbs (only what you pass changes). Use after learning what something really is.",
     "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}, "summary": {"type": "string"}, "tags": {"type": "array", "items": {"type": "string"}}, "notes": {"type": "string"}, "blurbs": {"type": "object", "additionalProperties": {"type": "string"}, "description": "item name \u2192 blurb"}}, "additionalProperties": False, "required": ["path"]}},
    {"name": "list_questions", "description": "Open questions the indexer left about unclear folders.",
     "inputSchema": {"type": "object", "properties": {}, "additionalProperties": False}},
    {"name": "resolve_question", "description": "Answer one of the indexer's open questions. The answer is saved into that folder's Notes so future indexing uses it, and the question is checked off.",
     "inputSchema": {"type": "object", "properties": {"question": {"type": "string", "description": "Any distinctive part of the question text"}, "answer": {"type": "string"}}, "additionalProperties": False, "required": ["question", "answer"]}},
    {"name": "index_status", "description": "Backend, model, last run, schedule, problems (e.g. signed out), undescribed folders and open questions.",
     "inputSchema": {"type": "object", "properties": {}, "additionalProperties": False}},
    {"name": "reindex_desktop", "description": "Update the folder descriptions now with the backend chosen in DesktopGlass. Free when nothing changed. Set wait=true to wait for it to finish.",
     "inputSchema": {"type": "object", "properties": {"full": {"type": "boolean", "description": "Rewrite every description"}, "wait": {"type": "boolean"}}, "additionalProperties": False}},
    {"name": "get_schedule", "description": "When automatic re-indexing runs.",
     "inputSchema": {"type": "object", "properties": {}, "additionalProperties": False}},
    {"name": "set_schedule", "description": "Change automatic re-indexing: off, hourly, every_6_hours or daily (optionally at hour 0-23). Runs are skipped for free when nothing changed.",
     "inputSchema": {"type": "object", "properties": {"schedule": {"type": "string", "enum": ["off", "hourly", "every_6_hours", "daily"]}, "hour": {"type": "integer", "minimum": 0, "maximum": 23}}, "additionalProperties": False, "required": ["schedule"]}},
    {"name": "set_index_backend", "description": "Choose what writes the descriptions: claude, codex, gemini, antigravity, cursor, copilot, opencode (agent CLIs) or apple, mlx, ollama, lmstudio, llamacpp (on-device).",
     "inputSchema": {"type": "object", "properties": {"backend": {"type": "string"}, "model": {"type": "string"}, "endpoint": {"type": "string", "description": "Server URL for ollama/lmstudio/llamacpp"}}, "additionalProperties": False, "required": ["backend"]}},
    {"name": "set_preferences", "description": "Indexing preferences shared by every backend: blurb style, status flags, notes for the AI, and per-folder privacy (include, names_only, skip).",
     "inputSchema": {"type": "object", "properties": {"style": {"type": "string", "enum": ["terse", "sentence"]}, "flag_status": {"type": "boolean"}, "notes": {"type": "string"}, "folder": {"type": "string", "description": "Desktop-relative folder"}, "folder_mode": {"type": "string", "enum": ["include", "names_only", "skip"]}}, "additionalProperties": False}},
    {"name": "open_in_editor", "description": "Open a Desktop file or folder in the DesktopGlass editor (glass IDE with terminal).",
     "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}}, "additionalProperties": False, "required": ["path"]}},
    {"name": "show_overlay", "description": "Show the DesktopGlass overlay.",
     "inputSchema": {"type": "object", "properties": {}, "additionalProperties": False}},
    {"name": "show_artifact", "description": "Display something you made in the DesktopGlass artifact viewer: an HTML/SVG/PDF/image file, a localhost dev server, or a published artifact/canvas/preview URL. Local files live-reload when you rewrite them. Call this whenever you produce a visual artifact or start a web server.",
     "inputSchema": {"type": "object", "properties": {"target": {"type": "string", "description": "File path (absolute, ~/ or Desktop-relative) or URL"}, "title": {"type": "string"}}, "additionalProperties": False, "required": ["target"]}},
    {"name": "list_local_servers", "description": "Web servers currently running on this Mac (port, process, page title) \u2014 open one with show_artifact.",
     "inputSchema": {"type": "object", "properties": {}, "additionalProperties": False}},
    {"name": "browser_tabs", "description": "Pages open in the artifact viewer.",
     "inputSchema": {"type": "object", "properties": {}, "additionalProperties": False}},
    {"name": "browser_read", "description": "Read the page in the artifact viewer: visible text, links and form fields.",
     "inputSchema": {"type": "object", "properties": {"tab": {"type": "string", "description": "Tab id (default: the active tab)"}, "max_chars": {"type": "integer"}}, "additionalProperties": False}},
    {"name": "browser_screenshot", "description": "Screenshot of the page in the artifact viewer.",
     "inputSchema": {"type": "object", "properties": {"tab": {"type": "string"}}, "additionalProperties": False}},
    {"name": "browser_click", "description": "Click an element in the artifact viewer by CSS selector, or by visible text with 'text=Submit'.",
     "inputSchema": {"type": "object", "properties": {"selector": {"type": "string"}, "tab": {"type": "string"}}, "additionalProperties": False, "required": ["selector"]}},
    {"name": "browser_type", "description": "Type into an input/textarea in the artifact viewer (CSS selector); submit=true submits the form.",
     "inputSchema": {"type": "object", "properties": {"selector": {"type": "string"}, "text": {"type": "string"}, "submit": {"type": "boolean"}, "tab": {"type": "string"}}, "additionalProperties": False, "required": ["selector", "text"]}},
    {"name": "browser_eval", "description": "Run JavaScript in the artifact viewer's page and return the result (JSON).",
     "inputSchema": {"type": "object", "properties": {"script": {"type": "string"}, "tab": {"type": "string"}}, "additionalProperties": False, "required": ["script"]}},
    {"name": "browser_navigate", "description": "back, forward, reload or close the page, or go to another artifact URL/file.",
     "inputSchema": {"type": "object", "properties": {"action": {"type": "string", "enum": ["back", "forward", "reload", "close", "go"]}, "url": {"type": "string"}, "tab": {"type": "string"}}, "additionalProperties": False, "required": ["action"]}},
    {"name": "html_outline", "description": "Element tree of an HTML file (paths, tags, ids, classes, inline styles, text) plus its linked stylesheets. Use the paths with the other html_* tools.",
     "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}}, "additionalProperties": False, "required": ["path"]}},
    {"name": "html_set_style", "description": "Set CSS properties for an element. Written to the page's stylesheet (a rule for its class/id) unless to=inline or to=style_block. Empty value removes a property.",
     "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}, "element": {"type": "string", "description": "Path from html_outline (e.g. 1.0.2), #id, .class, tag or tag:N"}, "properties": {"type": "object", "additionalProperties": {"type": "string"}}, "selector": {"type": "string", "description": "Rule selector (.class or #id); default: the element's first class, else its id"}, "to": {"type": "string", "enum": ["stylesheet", "style_block", "inline"], "description": "Default: the page's linked stylesheet if it has one, else <style>"}, "stylesheet": {"type": "string", "description": "Which linked .css file, by name"}}, "additionalProperties": False, "required": ["path", "element", "properties"]}},
    {"name": "html_split_style", "description": "Give one element its own class split off a shared one, so it can be styled without changing the others. mode=copy copies the base class's styles into the new class and swaps it in; mode=variant keeps the base class and adds the new class for overrides.",
     "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}, "element": {"type": "string", "description": "Path from html_outline (e.g. 1.0.2), #id, .class, tag or tag:N"}, "new_class": {"type": "string"}, "base_class": {"type": "string"}, "mode": {"type": "string", "enum": ["copy", "variant"]}, "move_inline": {"type": "boolean", "description": "Move the element's inline styles into the new class (default True)"}, "to": {"type": "string", "enum": ["stylesheet", "style_block"]}, "stylesheet": {"type": "string"}}, "additionalProperties": False, "required": ["path", "element", "new_class"]}},
    {"name": "html_group_style", "description": "Make a new class shared by several elements: adds the class to each and writes the given properties into one rule in the stylesheet (or <style>).",
     "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}, "elements": {"type": "array", "items": {"type": "string", "description": "Path from html_outline (e.g. 1.0.2), #id, .class, tag or tag:N"}}, "class_name": {"type": "string"}, "properties": {"type": "object", "additionalProperties": {"type": "string"}}, "move_inline": {"type": "boolean", "description": "Remove those properties from the elements' inline styles (default True)"}, "to": {"type": "string", "enum": ["stylesheet", "style_block"]}, "stylesheet": {"type": "string"}}, "additionalProperties": False, "required": ["path", "elements", "class_name"]}},
    {"name": "html_set_text", "description": "Replace the text of an element that contains only text.",
     "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}, "element": {"type": "string", "description": "Path from html_outline (e.g. 1.0.2), #id, .class, tag or tag:N"}, "text": {"type": "string"}}, "additionalProperties": False, "required": ["path", "element", "text"]}},
    {"name": "html_set_html", "description": "Replace an element's text keeping inline formatting, e.g. 'Save <b>40%</b> with <a href=\"/plans\">annual plans</a>'. Only inline tags (b, strong, i, em, a, span, code, br, small, sup, sub, mark\u2026) are allowed, and only for elements that hold text and inline tags.",
     "inputSchema": {'type': 'object', 'properties': {'path': {'type': 'string'}, 'element': {'type': 'string', 'description': 'Path from html_outline, #id, .class, tag or tag:N'}, 'html': {'type': 'string'}}, 'required': ['path', 'element', 'html'], 'additionalProperties': False}},
    {"name": "html_duplicate_element", "description": "Duplicate an element right after itself (ids are dropped from the copy).",
     "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}, "element": {"type": "string", "description": "Path from html_outline (e.g. 1.0.2), #id, .class, tag or tag:N"}}, "additionalProperties": False, "required": ["path", "element"]}},
    {"name": "html_remove_element", "description": "Remove an element from an HTML file.",
     "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}, "element": {"type": "string", "description": "Path from html_outline (e.g. 1.0.2), #id, .class, tag or tag:N"}}, "additionalProperties": False, "required": ["path", "element"]}},
    {"name": "list_games", "description": "Mac games in the Desktop's Games folder: title, platform (always mac), path, and when it was last played. Filter by query (title).",
     "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}, "platform": {"type": "string", "description": "Only 'mac' is listed"}, "limit": {"type": "integer"}}, "additionalProperties": False}},
    {"name": "play_game", "description": "Start a Mac game from the Games folder. Use a title or path from list_games.",
     "inputSchema": {"type": "object", "properties": {"game": {"type": "string"}}, "additionalProperties": False, "required": ["game"]}},
]


def safe_path(rel):
    p = (ROOT / (rel or "").strip().strip("/")).resolve()
    if p != ROOT and ROOT not in p.parents:
        raise ValueError("Path must be inside the Desktop")
    return p


def tool_overview(_):
    index = LOG / "desktop-index.md"
    if index.exists():
        return index.read_text(encoding="utf-8", errors="replace")
    return "The Desktop hasn't been indexed yet. Run reindex_desktop, or open DesktopGlass and click Re-index."


def tool_list_folder(args):
    folder = safe_path(args.get("path", ""))
    if not folder.is_dir():
        raise ValueError(f"Not a folder: {args.get('path')}")
    d = inv.parse_desc(inv.read_head(folder / inv.DESC, 50000))
    rel = folder.relative_to(ROOT).as_posix() if folder != ROOT else ""
    lines = [f"# {d['title'] or folder.name}", f"Path: Desktop/{rel}".rstrip("/")]
    if d["summary"]:
        lines.append(f"Summary: {d['summary']}")
    if d["tags"]:
        lines.append("Tags: " + ", ".join(d["tags"]))
    lines.append("")
    for n in inv.listdir(folder):
        p = folder / n
        kind = inv.kind(p, len(Path(rel).parts) + 1 if rel else 1)
        b = d["blurbs"].get(n)
        lines.append(f"- {n}{'/' if inv.is_dir(p) else ''} [{kind}]" + (f" — {b}" if b else ""))
    if d["notes"]:
        lines += ["", "Notes: " + " ".join(d["notes"])]
    return "\n".join(lines)


def tool_search(args):
    terms = [t for t in str(args.get("query", "")).lower().split() if t]
    if not terms:
        raise ValueError("Empty query")
    limit = int(args.get("limit") or 30)
    hits = []
    skip, _ = inv.load_prefs(ROOT)
    for c, _depth in inv.containers(ROOT, skip):
        d = inv.parse_desc(inv.read_head(c / inv.DESC, 50000))
        rel = c.relative_to(ROOT).as_posix()
        folder_hay = " ".join([rel, d["summary"] or "", " ".join(d["tags"])]).lower()
        if all(t in folder_hay for t in terms):
            hits.append((3, f"{rel}/ — {d['summary'] or ''}"))
        for n in inv.listdir(c):
            b = d["blurbs"].get(n, "")
            hay = f"{n} {b}".lower()
            if all(t in hay for t in terms):
                score = 2 if terms[0] in n.lower() else 1
                hits.append((score, f"{rel}/{n}{'/' if inv.is_dir(c / n) else ''}" + (f" — {b}" if b else "")))
    hits.sort(key=lambda h: -h[0])
    if not hits:
        return f"Nothing on the Desktop matches “{args.get('query')}”."
    return "\n".join(f"- {h[1]}" for h in hits[:limit])


def tool_get_description(args):
    folder = safe_path(args["path"])
    f = folder / inv.DESC
    if not f.exists():
        return f"{args['path']} has no description.md yet."
    return f.read_text(encoding="utf-8", errors="replace")


def tool_status(_):
    status = inv.status_line(ROOT)
    index = LOG / "desktop-index.md"
    when = ""
    if index.exists():
        import time
        when = time.strftime("%b %d, %Y %I:%M %p", time.localtime(index.stat().st_mtime))
    skip, _ = inv.load_prefs(ROOT)
    missing = [c.relative_to(ROOT).as_posix() for c, _ in inv.containers(ROOT, skip)
               if not inv.parse_desc(inv.read_head(c / inv.DESC, 20000))["summary"]]
    questions = [l.strip() for l in inv.read_head(LOG / "questions.md", 100000).splitlines() if l.strip().startswith("- [ ]")]
    prefs = inv.load_prefs_full(ROOT)
    out = [status, f"Index rebuilt: {when or 'never'}"]
    if missing:
        out.append("Undescribed: " + ", ".join(missing[:30]))
    if questions:
        out += ["Open questions:"] + questions[:20]
    out.append("Preferences: " + json.dumps(prefs))
    return "\n".join(out)


def app_default(key):
    try:
        r = subprocess.run(["defaults", "read", BUNDLE_ID, key], capture_output=True, text=True, timeout=5)
        return r.stdout.strip() if r.returncode == 0 else ""
    except Exception:
        return ""


# ---- permissions (DesktopGlass → Settings → MCP). Reading is always allowed. ----
GATES = {
    "edit": ({"update_description", "resolve_question", "html_set_style", "html_split_style", "html_group_style",
              "html_set_text", "html_set_html", "html_duplicate_element", "html_remove_element"}, "Edit files"),
    "control": ({"reindex_desktop", "set_schedule", "set_index_backend", "set_preferences"},
                "Change settings and start re-indexing"),
    "script": ({"browser_click", "browser_type", "browser_eval"}, "Click, type and run JavaScript in the artifact viewer"),
    "play": ({"play_game"}, "Launch games from the Games folder"),
}


def _truthy(v):
    return str(v).strip().lower() in ("1", "true", "yes")


def blocked_by(tool):
    """The permission a tool needs that the user hasn't granted, or None."""
    for key, (tools, label) in GATES.items():
        if tool in tools and not _truthy(app_default(f"mcp.allow.{key}")):
            return label
    return None


def tool_reindex(args):
    script = Path(__file__).resolve().parent / "reindex.sh"
    harness = app_default("harness") or "claude"
    env = dict(os.environ)
    env["DESKTOP_INDEX_HARNESS"] = harness
    env["DESKTOP_INDEX_MODEL"] = app_default(f"model.{harness}") or ("haiku" if harness == "claude" else "")
    endpoint = app_default(f"endpoint.{harness}")
    if endpoint:
        env["DESKTOP_INDEX_ENDPOINT"] = endpoint
    app_bin = Path("/Applications/DesktopGlass.app/Contents/MacOS/DesktopGlass")
    for candidate in (Path.home() / "Applications/DesktopGlass.app/Contents/MacOS/DesktopGlass", app_bin):
        if candidate.exists():
            env["DESKTOP_GLASS_BIN"] = str(candidate)
            break
    env["DESKTOP_INDEX_REASON"] = "mcp"
    env["DESKTOP_INDEX_UNATTENDED"] = "1" if _truthy(app_default(f"unattended.{harness}")) else "0"
    sandbox = app_default("sandboxUnattended")
    env["DESKTOP_INDEX_SANDBOX"] = "1" if _truthy(sandbox) else "0"
    if args.get("full"):
        env["DESKTOP_INDEX_FULL"] = "1"
    r = subprocess.run(["/bin/zsh", "-l", str(script)], capture_output=True, text=True, env=env, timeout=1800)
    last = (r.stdout.strip().splitlines() or [""])[-1]
    codes = {0: "Indexed", 10: "Up to date", 75: "Already indexing", 76: "Usage limit", 77: "Not signed in",
             78: "Model not ready", 79: "Unattended runs not allowed", 127: "Backend not installed"}
    return f"{codes.get(r.returncode, f'Exit {r.returncode}')}: {last}"


def tool_recent(args):
    days = float(args.get("days") or 7)
    limit = int(args.get("limit") or 40)
    cutoff = time.time() - days * 86400
    skip, _ = inv.load_prefs(ROOT)
    rows = []
    for c, _depth in [(ROOT, 0)] + inv.containers(ROOT, skip):
        d = inv.parse_desc(inv.read_head(c / inv.DESC, 50000)) if c != ROOT else {"blurbs": {}}
        for n in inv.listdir(c):
            p = c / n
            try:
                m = p.stat().st_mtime
            except OSError:
                continue
            if m >= cutoff and not (c == ROOT and inv.kind(p, 1) == "group"):
                rel = p.relative_to(ROOT).as_posix()
                rows.append((m, rel + ("/" if inv.is_dir(p) else ""), d["blurbs"].get(n, "")))
    rows.sort(reverse=True)
    if not rows:
        return f"Nothing on the Desktop changed in the last {days:g} days."
    return "\n".join(f"- {time.strftime('%b %d %H:%M', time.localtime(m))}  {rel}" + (f" — {b}" if b else "")
                     for m, rel, b in rows[:limit])


def commit_quiet():
    # inventory.commit prints a status line; stdout belongs to the MCP protocol
    with contextlib.redirect_stdout(sys.stderr):
        inv.commit(ROOT)


def write_desc(folder, d):
    lines = [f"# {d['title'] or folder.name}", f"> {d['summary'] or '(?)'}"]
    if d["tags"]:
        lines.append("Tags: " + ", ".join(d["tags"]))
    lines += ["", "## Contents"]
    for n in inv.listdir(folder):
        b = d["blurbs"].get(n)
        lines.append(f"- **{n}{'/' if inv.is_dir(folder / n) else ''}**" + (f" — {b}" if b else ""))
    if d["notes"]:
        lines += ["", "## Notes"] + d["notes"]
    (folder / inv.DESC).write_text("\n".join(lines) + "\n", encoding="utf-8")


def tool_update_description(args):
    folder = safe_path(args["path"])
    rel = folder.relative_to(ROOT).as_posix() if folder != ROOT else ""
    if folder == ROOT or not folder.is_dir() or inv.kind(folder, len(Path(rel).parts)) != "group":
        raise ValueError(f"{args['path']} isn't a described Desktop folder (projects are described by their parent folder)")
    d = inv.parse_desc(inv.read_head(folder / inv.DESC, 50000))
    # Every value is forced onto one line: a newline in a summary or blurb would otherwise add headings or
    # bullets to description.md.
    if args.get("summary"):
        d["summary"] = inv.one_line(args["summary"], 160)
    if args.get("tags"):
        tags = [inv.one_line(t).lower().replace(",", " ").strip() for t in args["tags"]]
        d["tags"] = [t for t in tags if t][:6]
    if args.get("notes") is not None:
        d["notes"] = [l.strip() for l in str(args["notes"]).splitlines() if l.strip() and not l.lstrip().startswith("#")]
    names = set(inv.listdir(folder))
    unknown = []
    for name, blurb in (args.get("blurbs") or {}).items():
        key = name.rstrip("/")
        if key in names:
            d["blurbs"][key] = inv.one_line(blurb, 160)
        else:
            unknown.append(name)
    write_desc(folder, d)
    commit_quiet()
    note = f" (no such items: {', '.join(unknown)})" if unknown else ""
    return f"Updated {rel}/description.md{note}"


QUESTIONS = LOG / "questions.md"


def tool_list_questions(_):
    qs = [l.strip() for l in inv.read_head(QUESTIONS, 200000).splitlines() if l.strip().startswith("- [ ]")]
    return "\n".join(qs) if qs else "No open questions."


def tool_resolve_question(args):
    want = args["question"].strip().lower()
    lines = inv.read_head(QUESTIONS, 200000).splitlines()
    for i, line in enumerate(lines):
        if line.strip().startswith("- [ ]") and want in line.lower():
            answer = inv.one_line(args["answer"], 500)
            m = re.search(r"·\s*(.+?)\s+—\s+(.*)$", line)
            if m:
                folder_rel, question = m.group(1).strip().strip("/"), m.group(2).strip()
                try:
                    folder = safe_path(folder_rel)
                    target = folder if inv.is_dir(folder) and (folder / inv.DESC).exists() else folder.parent
                    d = inv.parse_desc(inv.read_head(target / inv.DESC, 50000))
                    d["notes"] = d["notes"] + [f"Q: {question} A: {answer}"]
                    if target.is_dir() and target != ROOT:
                        write_desc(target, d)
                except ValueError:
                    pass
            lines[i] = line.replace("- [ ]", "- [x]", 1) + f" → {answer}"
            QUESTIONS.write_text("\n".join(lines) + "\n", encoding="utf-8")
            commit_quiet()
            return "Saved. The answer is in that folder's Notes, so future indexing will use it."
    raise ValueError("No open question matches that text — call list_questions")


def via_app(method, params=None, timeout=120):
    try:
        return as_text(app_call(method, params, timeout=timeout))
    except AppUnavailable as e:
        raise RuntimeError(str(e))


def tool_reindex_app(args):
    try:
        res = app_call("index.run", {"full": bool(args.get("full")), "wait": bool(args.get("wait")), "timeout": 1500},
                       timeout=1600 if args.get("wait") else 30)
        return as_text(res)
    except AppUnavailable:
        return tool_reindex(args)   # app not available: run the script directly


def tool_status_full(args):
    text = tool_status(args)
    try:
        return text + "\n\nApp: " + as_text(app_call("app.status", launch=False, timeout=10))
    except Exception:
        return text


def tool_show_artifact(args):
    return via_app("browser.open", {"target": args["target"], "title": args.get("title")}, timeout=60)


def tool_screenshot(args):
    res = app_call("browser.screenshot", {"tab": args.get("tab")}, timeout=60)
    return [{"type": "image", "data": res["png"], "mimeType": "image/png"},
            {"type": "text", "text": f"{res.get('title', '')} — {res.get('url', '')}"}]


def tool_read_page(args):
    res = app_call("browser.read", {"tab": args.get("tab"), "max_chars": args.get("max_chars") or 20000}, timeout=60)
    out = [f"# {res.get('title', '')}", f"URL: {res.get('url', '')}", f"Tab: {res.get('tab', '')}", "", res.get("text", "")]
    if res.get("fields"):
        out += ["", "Form fields:"] + [f"- <{f['tag']}{(' type=' + f['type']) if f['type'] else ''}>"
                                       + (f" #{f['id']}" if f['id'] else "") + (f" name={f['name']}" if f['name'] else "")
                                       + (f" “{f['label']}”" if f['label'] else "") for f in res["fields"]]
    if res.get("links"):
        out += ["", "Links:"] + [f"- {l['text'] or '(no text)'} → {l['href']}" for l in res["links"][:50]]
    return "\n".join(out)


def tool_navigate(args):
    action = args.get("action")
    params = {"tab": args.get("tab"), "action": "" if action == "go" else action, "url": args.get("url")}
    return via_app("browser.navigate", params, timeout=60)


HANDLERS = {
    "desktop_overview": tool_overview, "list_folder": tool_list_folder, "search_desktop": tool_search,
    "get_description": tool_get_description, "recent_changes": tool_recent, "update_description": tool_update_description,
    "list_questions": tool_list_questions, "resolve_question": tool_resolve_question,
    "index_status": tool_status_full, "reindex_desktop": tool_reindex_app,
    "get_schedule": lambda a: via_app("schedule.get"),
    "set_schedule": lambda a: via_app("schedule.set", {"schedule": a["schedule"], **({"hour": a["hour"]} if "hour" in a else {})}),
    "set_index_backend": lambda a: via_app("backend.set", a),
    "set_preferences": lambda a: via_app("prefs.set", a),
    "open_in_editor": lambda a: via_app("editor.open", {"path": a["path"]}),
    "show_overlay": lambda a: via_app("overlay.show"),
    "show_artifact": tool_show_artifact, "list_local_servers": lambda a: via_app("servers.list", timeout=30),
    "browser_tabs": lambda a: via_app("browser.tabs"), "browser_read": tool_read_page,
    "browser_screenshot": tool_screenshot,
    "browser_click": lambda a: via_app("browser.click", a, timeout=60),
    "browser_type": lambda a: via_app("browser.type", a, timeout=60),
    "browser_eval": lambda a: via_app("browser.eval", a, timeout=60),
    "browser_navigate": tool_navigate,
    "html_outline": lambda a: via_app("html.outline", a),
    "html_set_style": lambda a: via_app("html.style", a),
    "html_split_style": lambda a: via_app("html.split_style", a),
    "html_group_style": lambda a: via_app("html.group_style", a),
    "html_set_text": lambda a: via_app("html.set_text", a),
    "html_set_html": lambda a: via_app("html.set_html", a),
    "html_duplicate_element": lambda a: via_app("html.duplicate", a),
    "html_remove_element": lambda a: via_app("html.remove", a),
    "list_games": lambda a: via_app("games.list", a, timeout=60),
    "play_game": lambda a: via_app("games.play", a, timeout=60),
}


def respond(msg_id, result=None, error=None):
    out = {"jsonrpc": "2.0", "id": msg_id}
    if error is not None:
        out["error"] = error
    else:
        out["result"] = result
    sys.stdout.write(json.dumps(out, ensure_ascii=False) + "\n")
    sys.stdout.flush()


def handle(msg):
    method, msg_id, params = msg.get("method"), msg.get("id"), msg.get("params") or {}
    if msg_id is None:  # notification (initialized, cancelled, …)
        return
    if method == "initialize":
        requested = params.get("protocolVersion")
        respond(msg_id, {
            "protocolVersion": requested if requested in SUPPORTED else SUPPORTED[0],
            "capabilities": {"tools": {"listChanged": False}},
            "serverInfo": {"name": "desktopglass", "version": VERSION},
            "instructions": "The user's organized Mac Desktop, maintained by DesktopGlass. Call desktop_overview first, then "
                            "list_folder / search_desktop / recent_changes to find things. Fix descriptions you learn are "
                            "wrong with update_description. Whenever you produce a visual artifact (HTML, SVG, a chart) or "
                            "start a web server, call show_artifact so the user sees it; use the browser_* tools to check it.",
        })
    elif method == "ping":
        respond(msg_id, {})
    elif method == "tools/list":
        respond(msg_id, {"tools": [t for t in TOOLS if not blocked_by(t["name"])]})
    elif method == "tools/call":
        name, args = params.get("name"), params.get("arguments") or {}
        fn = HANDLERS.get(name)
        needs = blocked_by(name) if fn else None
        if needs:
            respond(msg_id, {"content": [{"type": "text", "text": f"“{needs}” is turned off for AI apps. "
                                          "The user can allow it in DesktopGlass → Settings → MCP."}], "isError": True})
            return
        if not fn:
            respond(msg_id, error={"code": -32602, "message": f"Unknown tool: {name}"})
            return
        try:
            out = fn(args)
            content = out if isinstance(out, list) else [{"type": "text", "text": as_text(out)}]
            respond(msg_id, {"content": content, "isError": False})
        except Exception as e:
            respond(msg_id, {"content": [{"type": "text", "text": f"Error: {e}"}], "isError": True})
    elif method in ("resources/list", "prompts/list"):
        respond(msg_id, {"resources": []} if method == "resources/list" else {"prompts": []})
    else:
        respond(msg_id, error={"code": -32601, "message": f"Method not found: {method}"})


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except ValueError:
            respond(None, error={"code": -32700, "message": "Parse error"})
            continue
        for m in (msg if isinstance(msg, list) else [msg]):
            handle(m)


if __name__ == "__main__":
    main()
