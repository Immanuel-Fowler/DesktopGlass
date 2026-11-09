#!/usr/bin/env python3
"""Compact, token-cheap Desktop inventory for the desktopglass-index skill.

Usage:
  inventory.py DESKTOP scan [--all]   compact JSON of folders whose description.md needs work
  inventory.py DESKTOP check          exit 0 if nothing changed (skip Claude), 1 if work is needed
  inventory.py DESKTOP commit         save the change manifest + rebuild _Organize Log/desktop-index.md
  inventory.py DESKTOP status         one-line status

Standard library only. Reads the Desktop; writes only inside _Organize Log/.
Folder rules mirror the DesktopGlass app (Sources/DesktopGlass/Models.swift).
"""
import json
import os
import re
import sys
import time
from collections import Counter
from pathlib import Path

LOG_DIR = "_Organize Log"
DESC = "description.md"
MAX_DEPTH = 3          # Desktop/A/B/C can be a described folder; deeper ones are just listed
MAX_ENTRIES = 40       # entries sent to Claude per folder; the rest are summarized as a count
IGNORED = {".DS_Store", "desktop.ini", ".localized", "node_modules", ".venv", "venv", "__pycache__",
           ".git", "dist", "build", ".next", DESC, LOG_DIR, "Icon\r"}
PACKAGE_EXT = {"app", "band", "bundle", "xcodeproj", "xcworkspace", "photoslibrary", "logicx",
               "framework", "playground", "rtfd", "pages", "numbers", "key"}
MARKERS = {".git", "package.json", "pyproject.toml", "requirements.txt", "setup.py", "Package.swift",
           "Cargo.toml", "go.mod", "pom.xml", "build.gradle", "Gemfile", "composer.json", "main.py",
           "app.py", "index.html", "Podfile", "pubspec.yaml", "deno.json", "README.md"}
MARKER_SUFFIXES = (".xcodeproj", ".xcworkspace", ".sln", ".csproj")
TEXT_EXT = {".md", ".txt", ".py", ".js", ".ts", ".tsx", ".jsx", ".swift", ".json", ".csv", ".html",
            ".css", ".sh", ".yaml", ".yml", ".toml", ".sql", ".rs", ".go", ".java", ".c", ".cpp",
            ".h", ".rb", ".php", ".ipynb"}
NOTABLE = {"next", "react", "vue", "svelte", "express", "fastify", "vite", "electron", "@tauri-apps/api",
           "expo", "react-native", "tailwindcss", "typescript", "prisma", "@supabase/supabase-js",
           "firebase", "three", "socket.io", "@anthropic-ai/sdk", "openai", "stripe", "plaid",
           "mongoose", "drizzle-orm", "@modelcontextprotocol/sdk"}
BULLET = re.compile(r"^[-*]\s+\*\*(.+?)\*\*\s*(?:[—–:-]\s*)?(.*)$")


# ---------- filesystem helpers ----------

def visible(name):
    return not name.startswith((".", "~$")) and name not in IGNORED


def listdir(p):
    try:
        return sorted((n for n in os.listdir(p) if visible(n)), key=str.lower)
    except OSError:
        return []


def is_dir(p):
    return p.is_dir() and not p.is_symlink()


def is_project(p):
    try:
        names = os.listdir(p)
    except OSError:
        return False
    return any(n in MARKERS or n.endswith(MARKER_SUFFIXES) for n in names)


def kind(p, depth):
    if not is_dir(p):
        return "file"
    if p.suffix.lower().lstrip(".") in PACKAGE_EXT:
        return "package"
    if is_project(p):
        return "project"
    return "group" if depth <= MAX_DEPTH else "folder"


def read_head(p, limit=2000):
    try:
        with open(p, "r", encoding="utf-8", errors="ignore") as f:
            return f.read(limit)
    except OSError:
        return ""


def first_lines(text, n=2, width=160):
    out = []
    for raw in text.splitlines():
        line = raw.strip().strip("#*>`_ \t").strip()
        if len(line) < 4 or line.startswith(("![", "[!", "<", "---", "|", "import ", "from ", "#!", "{", "}", "\"")):
            continue
        out.append(line)
        if len(out) >= n:
            break
    return " / ".join(out)[:width]


def one_line(text, limit=None):
    """Collapses whitespace (newlines included) so a value can't break description.md's line-based format."""
    s = " ".join(str(text or "").split())
    return s[:limit] if limit else s


def human(n):
    for unit in ("B", "KB", "MB", "GB"):
        if n < 1024:
            return f"{n:.0f}{unit}"
        n /= 1024
    return f"{n:.1f}TB"


def entry_sig(p):
    try:
        st = p.stat()
    except OSError:
        return "x"
    if is_dir(p):
        return f"d{int(st.st_mtime)}:{len(listdir(p))}"
    return f"f{st.st_size}:{int(st.st_mtime)}"


# ---------- description.md ----------

def parse_desc(text):
    d = {"title": None, "summary": None, "tags": [], "blurbs": {}, "notes": []}
    section = ""
    for raw in text.splitlines():
        line = raw.strip()
        if line.startswith("## "):
            section = line[3:].strip().lower()
            continue
        if line.startswith("# ") and d["title"] is None:
            d["title"] = line[2:].strip()
            continue
        if line.startswith("> ") and d["summary"] is None:
            d["summary"] = line[2:].strip()
            continue
        if line.lower().startswith("tags:"):
            d["tags"] = [t.strip().lower() for t in line[5:].split(",") if t.strip()]
            continue
        if section == "contents":
            m = BULLET.match(line)
            if m:
                d["blurbs"][m.group(1).strip().rstrip("/")] = m.group(2).strip()
                continue
        if section == "notes" and line:
            d["notes"].append(line)
    return d


DEFAULT_PREFS = {"version": 1, "style": "terse", "private": [], "skip": [], "flagStatus": True, "notes": ""}


def load_prefs_full(root):
    """_Organize Log/preferences.json, written by DesktopGlass Settings. Falls back to an old preferences.md."""
    prefs = dict(DEFAULT_PREFS)
    raw = read_head(root / LOG_DIR / "preferences.json", 200000)
    if raw.strip():
        try:
            data = json.loads(raw)
            if isinstance(data, dict):
                prefs.update({k: v for k, v in data.items() if k in DEFAULT_PREFS})
        except ValueError:
            pass
    else:
        for line in read_head(root / LOG_DIR / "preferences.md", 20000).splitlines():
            m = re.match(r"\s*[-*]?\s*(skip|private|style|notes)\s*:\s*(.*)", line, re.I)
            if not m:
                continue
            key, val = m.group(1).lower(), m.group(2).strip()
            if key in ("skip", "private"):
                prefs[key] = [v.strip().strip("`").strip("/") for v in val.split(",") if v.strip()]
            else:
                prefs[key] = val
    prefs["private"] = [str(v).strip("/") for v in prefs.get("private") or [] if str(v).strip()]
    prefs["skip"] = [str(v).strip("/") for v in prefs.get("skip") or [] if str(v).strip()]
    return prefs


def load_prefs(root):
    p = load_prefs_full(root)
    return set(p["skip"]), set(p["private"])


def under(rel, paths):
    return any(rel == p or rel.startswith(p + "/") for p in paths)


def containers(root, skip=frozenset()):
    out = []

    def walk(d, depth):
        for n in listdir(d):
            p = d / n
            rel = p.relative_to(root).as_posix()
            if under(rel, skip):
                continue
            if kind(p, depth) == "group":
                out.append((p, depth))
                walk(p, depth + 1)

    walk(root, 1)
    return out


def load_manifest(root):
    try:
        return json.loads(read_head(root / LOG_DIR / ".manifest.json", 5_000_000) or "{}")
    except ValueError:
        return {}


def open_questions(root):
    return sum(1 for l in read_head(root / LOG_DIR / "questions.md", 200000).splitlines()
               if l.strip().startswith("- [ ]"))


# ---------- hints for Claude ----------

def project_hints(p):
    try:
        names = set(os.listdir(p))
    except OSError:
        return {}
    h, stack = {}, []
    if "package.json" in names:
        try:
            pj = json.loads(read_head(p / "package.json", 100000))
        except ValueError:
            pj = {}
        deps = {**(pj.get("dependencies") or {}), **(pj.get("devDependencies") or {})}
        hits = sorted(d for d in deps if d in NOTABLE)[:6]
        stack.append("node" + (f"[{','.join(hits)}]" if hits else ""))
        if isinstance(pj.get("description"), str) and pj["description"].strip():
            h["desc"] = pj["description"].strip()[:120]
    if names & {"requirements.txt", "pyproject.toml", "setup.py", "main.py", "app.py"}:
        reqs = [re.split(r"[<>=~!\[;\s]", l.strip())[0]
                for l in read_head(p / "requirements.txt", 4000).splitlines()
                if l.strip() and not l.lstrip().startswith(("#", "-"))]
        stack.append("python" + (f"[{','.join(reqs[:6])}]" if reqs else ""))
    if "Package.swift" in names or any(n.endswith((".xcodeproj", ".xcworkspace")) for n in names):
        stack.append("swift")
    for marker, label in (("Cargo.toml", "rust"), ("go.mod", "go"), ("pubspec.yaml", "flutter"),
                          ("pom.xml", "java"), ("build.gradle", "gradle")):
        if marker in names:
            stack.append(label)
    if any(n.endswith((".sln", ".csproj")) for n in names):
        stack.append("dotnet")
    if "index.html" in names and "package.json" not in names:
        stack.append("static-web")
    if stack:
        h["stack"] = " ".join(stack)
    if ".git" in names:
        h["git"] = 1
    for r in ("README.md", "readme.md", "README.txt", "README"):
        if r in names:
            line = first_lines(read_head(p / r))
            if line:
                h["readme"] = line
            break
    h["top"] = ",".join(sorted((n for n in names if visible(n)), key=str.lower)[:8])[:160]
    return h


def describe_entry(p, depth, private=False):
    k = kind(p, depth)
    e = {"n": p.name, "k": k}
    if private:
        return e
    try:
        st = p.stat()
    except OSError:
        return e
    e["mod"] = time.strftime("%Y-%m", time.localtime(st.st_mtime))
    if k == "file":
        e["size"] = human(st.st_size)
        if p.suffix.lower() in TEXT_EXT and st.st_size < 2_000_000:
            line = first_lines(read_head(p), 1, 120)
            if line:
                e["head"] = line
    elif k == "project":
        e.update(project_hints(p))
    elif k in ("group", "folder"):
        kids = listdir(p)
        e["items"] = len(kids)
        summary = parse_desc(read_head(p / DESC, 20000))["summary"] if k == "group" else None
        if summary:
            e["summary"] = summary
        else:
            e["top"] = ",".join(kids[:8])[:160]
    return e


# ---------- commands ----------

def scan(root, force=False):
    old_all = load_manifest(root).get("containers", {})
    skip, private = load_prefs(root)
    out, seen = {"containers": []}, set()
    for c, depth in containers(root, skip):
        rel = c.relative_to(root).as_posix()
        seen.add(rel)
        is_private = under(rel, private)
        names = listdir(c)
        sigs = {n: entry_sig(c / n) for n in names}
        old = old_all.get(rel, {}).get("entries", {})
        desc_text = read_head(c / DESC, 20000)
        existing = parse_desc(desc_text) if desc_text.strip() else None
        changed = {n for n in names if sigs[n] != old.get(n)}
        gone = sorted(set(old) - set(sigs))
        missing = {n for n in names[:MAX_ENTRIES] if existing and n not in existing["blurbs"]}
        if not force and existing and not changed and not gone and not missing:
            continue

        item = {"path": rel, "why": "new" if not existing else ("all" if force else "changed")}
        if is_private:
            item["private"] = 1
        if existing:
            item["summary"] = existing["summary"]
            if existing["tags"]:
                item["tags"] = ",".join(existing["tags"])
            if existing["notes"]:
                item["notes"] = " ".join(existing["notes"])[:300]
        order = sorted(names, key=lambda n: (n not in changed and n not in missing, n.lower()))
        entries = []
        for n in order[:MAX_ENTRIES]:
            blurb = existing["blurbs"].get(n) if existing else None
            if blurb and n not in changed and not force:
                entries.append({"n": n, "keep": blurb})
                continue
            e = describe_entry(c / n, depth + 1, is_private)
            if blurb:
                e["old"] = blurb
            entries.append(e)
        item["entries"] = entries
        rest = order[MAX_ENTRIES:]
        if rest:
            exts = Counter((Path(n).suffix.lower() or "folder") for n in rest)
            item["more"] = f"+{len(rest)} more: " + ", ".join(f"{v} {k}" for k, v in exts.most_common(6))
        if gone:
            item["removed"] = gone
        out["containers"].append(item)
    removed = sorted(set(old_all) - seen)
    if removed:
        out["removed_folders"] = removed
    prefs = load_prefs_full(root)
    out["prefs"] = {"style": prefs["style"], "flagStatus": prefs["flagStatus"]}
    if str(prefs.get("notes") or "").strip():
        out["prefs"]["notes"] = str(prefs["notes"]).strip()[:600]
    q = open_questions(root)
    if q:
        out["open_questions"] = q
    return out


def status_line(root):
    conts = containers(root, load_prefs(root)[0])
    missing = sum(1 for c, _ in conts if not parse_desc(read_head(c / DESC, 20000))["summary"])
    q = open_questions(root)
    parts = [f"Indexed {len(conts)} folders"]
    if missing:
        parts.append(f"{missing} still undescribed")
    if q:
        parts.append(f"{q} question{'s' if q != 1 else ''} for you")
    return " · ".join(parts)


def commit(root):
    skip, _ = load_prefs(root)
    conts = containers(root, skip)
    manifest = {"version": 1, "updated": time.strftime("%Y-%m-%dT%H:%M:%S"), "containers": {}}
    body, total = [], 0
    for c, depth in conts:
        rel = c.relative_to(root).as_posix()
        names = listdir(c)
        total += len(names)
        manifest["containers"][rel] = {"entries": {n: entry_sig(c / n) for n in names}}
        d = parse_desc(read_head(c / DESC, 20000))
        body += ["", f"{'#' * min(depth + 1, 4)} {rel}/", f"> {d['summary'] or '_No description yet._'}"]
        if d["tags"]:
            body.append(f"Tags: {', '.join(d['tags'])}")
        body.append("")
        for n in names:
            blurb = d["blurbs"].get(n)
            body.append(f"- **{n}{'/' if is_dir(c / n) else ''}**" + (f" — {blurb}" if blurb else ""))
        if d["notes"]:
            body += [""] + d["notes"]
    loose = [n for n in listdir(root) if kind(root / n, 1) != "group"]
    q = open_questions(root)
    head = ["# Desktop Index", "",
            f"_Updated {time.strftime('%b %d, %Y %I:%M %p')} · {len(conts)} folders · {total} items"
            + (f" · {q} open question{'s' if q != 1 else ''} in questions.md" if q else "")
            + " · assembled from each folder's description.md_"]
    if loose:
        head += ["", "## Loose on the Desktop", ""] + [f"- {n}" for n in loose]
    log = root / LOG_DIR
    log.mkdir(exist_ok=True)
    (log / "desktop-index.md").write_text("\n".join(head + body) + "\n", encoding="utf-8")
    (log / ".manifest.json").write_text(json.dumps(manifest, separators=(",", ":")), encoding="utf-8")
    print(status_line(root))


def main():
    if len(sys.argv) < 3:
        print(__doc__)
        sys.exit(2)
    root, cmd = Path(sys.argv[1]).expanduser().resolve(), sys.argv[2]
    if cmd == "scan":
        print(json.dumps(scan(root, "--all" in sys.argv), separators=(",", ":"), ensure_ascii=False))
    elif cmd == "check":
        r = scan(root)
        sys.exit(1 if r["containers"] or r.get("removed_folders") else 0)
    elif cmd == "commit":
        commit(root)
    elif cmd == "status":
        print(status_line(root))
    else:
        print(__doc__)
        sys.exit(2)


if __name__ == "__main__":
    main()
