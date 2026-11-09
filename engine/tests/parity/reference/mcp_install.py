#!/usr/bin/env python3
"""One-click registration of the DesktopGlass MCP server with every AI app found on this Mac.

Usage: mcp_install.py status | install | uninstall | refresh | test   (prints JSON for the DesktopGlass app)
refresh: re-registers only where it is already installed, pointing at this copy of mcp_server.py.
Edits each app's own MCP config (backing it up first) or uses its CLI. Never touches an app that isn't installed,
and skips any config file it can't parse rather than overwrite it.

Agent CLIs also get a short, marked block in their global instructions file (CLAUDE.md, AGENTS.md, GEMINI.md, …)
telling them DesktopGlass exists and to use the Desktop index. Showing output in the viewer is left to the
session-start hook below, so it only happens in sessions started from DesktopGlass. Everything outside
the markers is left exactly as it was; uninstall removes only the block.

Every agent CLI also gets a session-start hook (session_context.py; for OpenCode a small plugin) that, only in
sessions started from DesktopGlass's terminal, tells the agent it's running inside DesktopGlass. Uninstall removes
only that hook.
"""
import json
import os
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path

sys.dont_write_bytecode = True
HOME = Path.home()
NAME = "desktopglass"
SERVER = str(Path(__file__).resolve().parent / "mcp_server.py")
PYTHON = "/usr/bin/python3" if Path("/usr/bin/python3").exists() else (shutil.which("python3") or "python3")
COMMAND, ARGS = PYTHON, [SERVER]
EXTRA_PATH = [str(HOME / ".local/bin"), str(HOME / ".claude/local"), "/opt/homebrew/bin", "/usr/local/bin",
              str(HOME / ".npm-global/bin"), str(HOME / ".bun/bin"), str(HOME / ".opencode/bin")]


def which(cmd):
    return shutil.which(cmd, path=os.pathsep.join(EXTRA_PATH + [os.environ.get("PATH", "")]))


# ---------- global agent instructions ----------

BEGIN, END = "<!-- desktopglass:start -->", "<!-- desktopglass:end -->"
GUIDE = f"""{BEGIN}
## DesktopGlass

This Mac runs DesktopGlass, which keeps an organized index of ~/Desktop available through the `desktopglass` MCP
server. To find something on the Desktop, use `desktop_overview` and `search_desktop` before listing folders.
{END}"""


def guide_block(text):
    """(start, end) of the marked block in text, or None."""
    a = text.find(BEGIN)
    if a < 0:
        return None
    b = text.find(END, a)
    return (a, b + len(END)) if b >= 0 else None


def with_guide(text):
    span = guide_block(text)
    if span:
        return text[:span[0]] + GUIDE + text[span[1]:]
    return (text.rstrip("\n") + "\n\n" if text.strip() else "") + GUIDE + "\n"


def without_guide(text):
    span = guide_block(text)
    if not span:
        return text
    before, after = text[:span[0]].rstrip("\n"), text[span[1]:].lstrip("\n")
    return (before + ("\n\n" + after if after else "\n")) if before else after


class Guide:
    """The DesktopGlass block in one agent's global instructions file."""

    def __init__(self, path):
        self.path = path

    def installed(self):
        return self.path.exists() and guide_block(self.path.read_text(encoding="utf-8", errors="replace")) is not None

    def install(self):
        text = self.path.read_text(encoding="utf-8") if self.path.exists() else ""
        new = with_guide(text)
        if new != text:
            self.path.parent.mkdir(parents=True, exist_ok=True)
            self.path.write_text(new, encoding="utf-8")

    def uninstall(self):
        if not self.path.exists():
            return
        text = self.path.read_text(encoding="utf-8")
        new = without_guide(text)
        if new == text:
            return
        if new.strip():
            self.path.write_text(new, encoding="utf-8")
        else:
            self.path.unlink()          # the file only ever held our block


# ---------- JSON config helpers ----------

def read_json(path):
    if not path.exists() or not path.read_text().strip():
        return {}
    return json.loads(path.read_text())  # raises on comments/invalid → caller reports "skipped"


def write_json(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists():
        backup = path.with_name(path.name + ".desktopglass-backup")
        if not backup.exists():
            shutil.copy2(path, backup)
    tmp = path.with_name(path.name + ".tmp")
    tmp.write_text(json.dumps(data, indent=2) + "\n")
    tmp.replace(path)


class JsonClient:
    def __init__(self, cid, name, path, detect, key="mcpServers", entry=None):
        self.id, self.name, self.path, self.detect_paths, self.key = cid, name, path, detect, key
        self.entry = entry or {"command": COMMAND, "args": ARGS}

    def detected(self):
        return any(p.exists() for p in self.detect_paths)

    def installed(self):
        try:
            return NAME in (read_json(self.path).get(self.key) or {})
        except ValueError:
            return False

    def install(self):
        data = read_json(self.path)
        data.setdefault(self.key, {})[NAME] = self.entry
        write_json(self.path, data)

    def uninstall(self):
        data = read_json(self.path)
        if NAME in (data.get(self.key) or {}):
            del data[self.key][NAME]
            write_json(self.path, data)


class ClaudeCode:
    id, name = "claude-code", "Claude Code"

    def detected(self):
        return which("claude") is not None

    def _run(self, *args):
        return subprocess.run([which("claude"), "mcp", *args], capture_output=True, text=True, timeout=30)

    def installed(self):
        return self.detected() and self._run("get", NAME).returncode == 0

    def install(self):
        if self.installed():
            return
        r = self._run("add", "--scope", "user", NAME, "--", COMMAND, *ARGS)
        if r.returncode != 0:
            raise RuntimeError((r.stderr or r.stdout).strip()[:200])

    def uninstall(self):
        if self.installed():
            self._run("remove", "--scope", "user", NAME)


CODEX_HOME = Path(os.environ.get("CODEX_HOME") or HOME / ".codex")
CODEX_BINARIES = ["/Applications/Codex.app/Contents/Resources/codex", "/Applications/ChatGPT.app/Contents/Resources/codex"]


def codex_binary():
    return which("codex") or next((b for b in CODEX_BINARIES if os.access(b, os.X_OK)), None)


class Codex:
    id, name = "codex", "Codex"
    path = CODEX_HOME / "config.toml"
    header = f"[mcp_servers.{NAME}]"

    def detected(self):
        return codex_binary() is not None or CODEX_HOME.exists()

    def installed(self):
        return self.path.exists() and self.header in self.path.read_text()

    def install(self):
        if self.installed():
            return
        # Codex's own command keeps its config format right (the Codex app rewrites config.toml itself).
        binary = codex_binary()
        if binary:
            try:
                r = subprocess.run([binary, "mcp", "add", NAME, "--", COMMAND, *ARGS],
                                   capture_output=True, text=True, timeout=30)
                if r.returncode == 0 and self.installed():
                    return
            except (OSError, subprocess.SubprocessError):
                pass
        self.path.parent.mkdir(parents=True, exist_ok=True)
        text = self.path.read_text() if self.path.exists() else ""
        if text:
            shutil.copy2(self.path, self.path.with_name("config.toml.desktopglass-backup"))
        block = f'\n{self.header}\ncommand = {json.dumps(COMMAND)}\nargs = {json.dumps(ARGS)}\n'
        self.path.write_text(text.rstrip("\n") + "\n" + block)

    def uninstall(self):
        if not self.installed():
            return
        out, skipping = [], False
        for line in self.path.read_text().splitlines():
            if line.strip() == self.header:
                skipping = True
                continue
            if skipping and re.match(r"^\s*\[", line):
                skipping = False
            if not skipping:
                out.append(line)
        self.path.write_text("\n".join(out).rstrip("\n") + "\n")


AUTOSHOW = str(Path(__file__).resolve().parent / "autoshow.py")
HOOK_COMMAND = f"{PYTHON} {json.dumps(AUTOSHOW)}"


class ClaudeHook:
    """Claude Code PostToolUse hook that shows HTML/SVG files Claude writes (autoshow.py) in the viewer."""

    def __init__(self, path=None):
        self.path = path or HOME / ".claude" / "settings.json"

    @staticmethod
    def ours(hook):
        return "autoshow.py" in str(hook.get("command", "")) and "desktopglass" in str(hook.get("command", ""))

    def _groups(self, data):
        return (data.get("hooks") or {}).get("PostToolUse") or []

    def installed(self):
        try:
            return any(self.ours(h) for g in self._groups(read_json(self.path)) for h in g.get("hooks") or [])
        except ValueError:
            return False

    def install(self):
        data = read_json(self.path)
        self._remove(data)
        data.setdefault("hooks", {}).setdefault("PostToolUse", []).append(
            {"matcher": "Write|Edit|MultiEdit", "hooks": [{"type": "command", "command": HOOK_COMMAND, "timeout": 10}]})
        write_json(self.path, data)

    def uninstall(self):
        if not self.path.exists():
            return
        data = read_json(self.path)
        if self._remove(data):
            write_json(self.path, data)

    def _remove(self, data):
        groups = self._groups(data)
        kept, changed = [], False
        for g in groups:
            hooks = [h for h in g.get("hooks") or [] if not self.ours(h)]
            changed |= len(hooks) != len(g.get("hooks") or [])
            if hooks:
                kept.append({**g, "hooks": hooks})
        if changed:
            data["hooks"]["PostToolUse"] = kept
            if not kept:
                del data["hooks"]["PostToolUse"]
            if not data["hooks"]:
                del data["hooks"]
        return changed


SESSION_SCRIPT = str(Path(__file__).resolve().parent / "session_context.py")
SESSION_COMMAND = f"{PYTHON} {json.dumps(SESSION_SCRIPT)}"


class SessionContext:
    """Base for the per-CLI session-start hooks: install.sh adds them wherever the MCP server is registered."""


class SessionHook(SessionContext):
    """SessionStart hook (session_context.py) for CLIs with Claude Code's hook layout,
    {"hooks": {"SessionStart": [{"hooks": [...]}]}}: Claude Code (~/.claude/settings.json), Codex (~/.codex/hooks.json;
    it asks the user to trust a new hook once) and Gemini CLI (~/.gemini/settings.json; timeout in milliseconds)."""

    EVENT = "SessionStart"

    def __init__(self, path, timeout=5):
        self.path = path
        self.timeout = timeout

    @staticmethod
    def ours(hook):
        return "session_context.py" in str(hook.get("command", "")) and "desktopglass" in str(hook.get("command", ""))

    def _groups(self, data):
        return (data.get("hooks") or {}).get(self.EVENT) or []

    def installed(self):
        try:
            return any(self.ours(h) for g in self._groups(read_json(self.path)) for h in g.get("hooks") or [])
        except ValueError:
            return False

    def install(self):
        data = read_json(self.path)
        self._remove(data)
        data.setdefault("hooks", {}).setdefault(self.EVENT, []).append(
            {"hooks": [{"type": "command", "command": SESSION_COMMAND, "timeout": self.timeout}]})
        write_json(self.path, data)

    def uninstall(self):
        if not self.path.exists():
            return
        data = read_json(self.path)
        if self._remove(data):
            if data:
                write_json(self.path, data)
            elif self.path.name == "hooks.json":
                self.path.unlink()      # Codex's hooks.json only ever held our hook
            else:
                write_json(self.path, data)

    def _remove(self, data):
        kept, changed = [], False
        for g in self._groups(data):
            hooks = [h for h in g.get("hooks") or [] if not self.ours(h)]
            changed |= len(hooks) != len(g.get("hooks") or [])
            if hooks:
                kept.append({**g, "hooks": hooks})
        if changed:
            data["hooks"][self.EVENT] = kept
            if not kept:
                del data["hooks"][self.EVENT]
            if not data["hooks"]:
                del data["hooks"]
        return changed


def session_command(fmt):
    return SESSION_COMMAND + ("" if fmt == "claude" else f" --format {fmt}")


def _ours_command(cmd):
    return "session_context.py" in str(cmd) and "desktopglass" in str(cmd)


class AntigravityHook(SessionContext):
    """Antigravity CLI: ~/.gemini/config/hooks.json maps hook names to events — {"desktopglass": {"SessionStart": [...]}}.
    (Hooks in antigravity-cli/settings.json aren't loaded.)"""

    NAME = "desktopglass"

    def __init__(self, path):
        self.path = path

    def installed(self):
        try:
            entry = read_json(self.path).get(self.NAME) or {}
        except ValueError:
            return False
        return any(_ours_command(h.get("command")) for h in entry.get("SessionStart") or [])

    def install(self):
        data = read_json(self.path)
        data[self.NAME] = {"SessionStart": [{"type": "command", "command": session_command("antigravity"), "timeout": 5}]}
        write_json(self.path, data)

    def uninstall(self):
        if not self.path.exists():
            return
        data = read_json(self.path)
        if self.NAME in data:
            del data[self.NAME]
            if data:
                write_json(self.path, data)
            else:
                self.path.unlink()


class OwnFileHook(SessionContext):
    """A hook kept in a file of DesktopGlass's own (removed on uninstall): Copilot CLI's ~/.copilot/hooks/*.json,
    or OpenCode's plugin in ~/.config/opencode/plugins/."""

    def __init__(self, path, content):
        self.path = path
        self.content = content

    def installed(self):
        return self.path.exists() and "session_context" in self.path.read_text(errors="replace")

    def install(self):
        self.path.parent.mkdir(parents=True, exist_ok=True)
        if not self.path.exists() or self.path.read_text(errors="replace") != self.content:
            self.path.write_text(self.content)

    def uninstall(self):
        if self.installed():
            self.path.unlink()


def copilot_hook():
    home = Path(os.environ.get("COPILOT_HOME") or HOME / ".copilot")
    content = json.dumps({"version": 1, "hooks": {"sessionStart": [
        {"type": "command", "bash": session_command("copilot"), "timeoutSec": 5}]}}, indent=2) + "\n"
    return OwnFileHook(home / "hooks" / "desktopglass.json", content)


def opencode_plugin():
    import session_context
    text = session_context.context({"TERM_PROGRAM": "DesktopGlass"})
    js = f"""// DesktopGlass (session_context): tells OpenCode when it was started from DesktopGlass's terminal.
// Written by DesktopGlass's MCP installer; removed when you uninstall. Prints nothing anywhere else.
export const DesktopGlass = async () => {{
  if (process.env.TERM_PROGRAM !== "DesktopGlass" || process.env.DESKTOPGLASS_SESSION_CONTEXT === "0") return {{}}
  return {{
    "experimental.chat.system.transform": async (_input, output) => {{
      output.system.push({json.dumps(text)})
    }},
  }}
}}
"""
    return OwnFileHook(HOME / ".config" / "opencode" / "plugins" / "desktopglass.js", js)


class CursorHook(SessionContext):
    """Cursor CLI: ~/.cursor/hooks.json — {"version": 1, "hooks": {"sessionStart": [{"command": …}]}}."""

    def __init__(self, path):
        self.path = path

    def _entries(self, data):
        return (data.get("hooks") or {}).get("sessionStart") or []

    def installed(self):
        try:
            return any(_ours_command(h.get("command")) for h in self._entries(read_json(self.path)))
        except ValueError:
            return False

    def install(self):
        data = read_json(self.path)
        data.setdefault("version", 1)
        entries = [h for h in self._entries(data) if not _ours_command(h.get("command"))]
        data.setdefault("hooks", {})["sessionStart"] = entries + [{"command": session_command("cursor")}]
        write_json(self.path, data)

    def uninstall(self):
        if not self.path.exists():
            return
        data = read_json(self.path)
        entries = self._entries(data)
        kept = [h for h in entries if not _ours_command(h.get("command"))]
        if len(kept) == len(entries):
            return
        if kept:
            data["hooks"]["sessionStart"] = kept
        else:
            del data["hooks"]["sessionStart"]
            if not data["hooks"]:
                del data["hooks"]
        if set(data) <= {"version"}:
            self.path.unlink()                    # the file only ever held our hook
        else:
            write_json(self.path, data)


def session_hook_for(client):
    """The session-start hook for an agent CLI, in that CLI's own format (None for apps that aren't agent CLIs)."""
    makers = {
        "claude-code": lambda: SessionHook(HOME / ".claude" / "settings.json"),
        "codex": lambda: SessionHook(CODEX_HOME / "hooks.json"),
        "gemini": lambda: SessionHook(HOME / ".gemini" / "settings.json", timeout=5000),
        "antigravity": lambda: AntigravityHook(HOME / ".gemini" / "config" / "hooks.json"),
        "copilot": copilot_hook,
        "cursor": lambda: CursorHook(HOME / ".cursor" / "hooks.json"),
        "opencode": opencode_plugin,
    }
    make = makers.get(client.id)
    return make() if make else None


GUIDES = {
    "claude-code": HOME / ".claude" / "CLAUDE.md",
    "codex": CODEX_HOME / "AGENTS.md",
    "gemini": HOME / ".gemini" / "GEMINI.md",
    "antigravity": HOME / ".gemini" / "GEMINI.md",     # Antigravity reads Gemini's global rules file
    "copilot": HOME / ".copilot" / "copilot-instructions.md",
    "opencode": HOME / ".config" / "opencode" / "AGENTS.md",
}


def guide_for(client):
    path = GUIDES.get(client.id)
    return Guide(path) if path else None


def extras_for(client):
    """What else an app gets with the MCP server: its instructions block, the DesktopGlass session-start hook (every
    agent CLI), and for Claude Code the autoshow hook."""
    out = [x for x in [guide_for(client), session_hook_for(client)] if x]
    if client.id == "claude-code":
        out.append(ClaudeHook())
    return out


def clients():
    sup = HOME / "Library" / "Application Support"
    return [
        ClaudeCode(),
        JsonClient("claude-desktop", "Claude Desktop", sup / "Claude" / "claude_desktop_config.json",
                   [sup / "Claude", Path("/Applications/Claude.app")]),
        Codex(),
        JsonClient("cursor", "Cursor", HOME / ".cursor" / "mcp.json", [HOME / ".cursor", Path("/Applications/Cursor.app")]),
        JsonClient("gemini", "Gemini CLI", HOME / ".gemini" / "settings.json", [HOME / ".gemini"]),
        JsonClient("antigravity", "Antigravity", HOME / ".gemini" / "config" / "mcp_config.json",
                   [HOME / ".gemini" / "antigravity-cli", HOME / ".gemini" / "antigravity", HOME / ".local" / "bin" / "agy",
                    Path("/Applications/Antigravity.app")]),
        JsonClient("copilot", "GitHub Copilot CLI", HOME / ".copilot" / "mcp-config.json", [HOME / ".copilot"],
                   entry={"type": "local", "command": COMMAND, "args": ARGS, "tools": ["*"]}),
        JsonClient("opencode", "OpenCode", HOME / ".config" / "opencode" / "opencode.json",
                   [HOME / ".config" / "opencode", HOME / ".opencode"], key="mcp",
                   entry={"type": "local", "command": [COMMAND, *ARGS], "enabled": True}),
        JsonClient("lmstudio", "LM Studio", HOME / ".lmstudio" / "mcp.json",
                   [HOME / ".lmstudio", Path("/Applications/LM Studio.app")]),
        JsonClient("windsurf", "Windsurf", HOME / ".codeium" / "windsurf" / "mcp_config.json",
                   [HOME / ".codeium" / "windsurf", Path("/Applications/Windsurf.app")]),
        JsonClient("vscode", "VS Code", sup / "Code" / "User" / "mcp.json",
                   [sup / "Code" / "User", Path("/Applications/Visual Studio Code.app")], key="servers",
                   entry={"type": "stdio", "command": COMMAND, "args": ARGS}),
    ]


def status_of(c):
    row = {"id": c.id, "name": c.name, "detected": False, "installed": False}
    try:
        row["detected"] = c.detected()
        row["installed"] = row["detected"] and c.installed()
        g = guide_for(c)
        if g:
            row["instructions"] = row["detected"] and g.installed()
        s = session_hook_for(c)
        if s:
            row["sessionHook"] = row["detected"] and s.installed()
        if c.id == "claude-code":
            row["autoshow"] = row["detected"] and ClaudeHook().installed()
    except Exception as e:
        row["note"] = str(e)[:160]
    return row


def test_server():
    started = time.time()
    msgs = [{"jsonrpc": "2.0", "id": 1, "method": "initialize",
             "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}}},
            {"jsonrpc": "2.0", "method": "notifications/initialized"},
            {"jsonrpc": "2.0", "id": 2, "method": "tools/list"},
            {"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "index_status", "arguments": {}}}]
    r = subprocess.run([COMMAND, *ARGS], input="\n".join(json.dumps(m) for m in msgs) + "\n",
                       capture_output=True, text=True, timeout=30)
    replies = {}
    for line in r.stdout.splitlines():
        try:
            m = json.loads(line)
            replies[m.get("id")] = m
        except ValueError:
            pass
    tools = [t["name"] for t in replies.get(2, {}).get("result", {}).get("tools", [])]
    status = replies.get(3, {}).get("result", {}).get("content", [{}])[0].get("text", "").splitlines()
    ok = bool(replies.get(1, {}).get("result")) and len(tools) > 0
    return {"ok": ok, "tools": tools, "status": status[0] if status else "",
            "ms": int((time.time() - started) * 1000), "error": "" if ok else (r.stderr.strip()[:300] or "no reply")}


def main():
    cmd = sys.argv[1] if len(sys.argv) > 1 else "status"
    result = {"server": {"command": COMMAND, "args": ARGS}}
    if cmd == "refresh":
        done, errors = [], []
        for c in clients():
            try:
                registered = c.detected() and c.installed()
                if registered:
                    c.uninstall()
                    c.install()
                    done.append(c.name)
                for extra in extras_for(c):
                    # brings an older block or hook path up to date; the SessionStart hook is also added wherever
                    # the server is registered, so re-running install.sh gives existing users the hook
                    if extra.installed() or (registered and isinstance(extra, SessionContext)):
                        extra.install()
            except Exception as e:
                errors.append(f"{c.name}: {str(e)[:160]}")
        result.update({"done": done, "errors": errors})
    if cmd in ("install", "uninstall"):
        done, errors = [], []
        for c in clients():
            try:
                if not c.detected():
                    continue
                (c.install if cmd == "install" else c.uninstall)()
                for extra in extras_for(c):
                    (extra.install if cmd == "install" else extra.uninstall)()
                done.append(c.name)
            except Exception as e:
                errors.append(f"{c.name}: {str(e)[:160]}" if not isinstance(e, ValueError)
                              else f"{c.name}: skipped (its config file isn't plain JSON)")
        result.update({"done": done, "errors": errors})
    if cmd == "test":
        result["test"] = test_server()
    result["clients"] = [status_of(c) for c in clients()]
    print(json.dumps(result))


if __name__ == "__main__":
    main()
