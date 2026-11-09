# DesktopGlass

A native macOS overlay for your Desktop. Press a hotkey and your folders appear as glass cards, each with a short
description of what's inside. The descriptions are written by an AI of your choice — a coding-agent CLI or a model
running on your Mac — and kept up to date without re-reading what hasn't changed.

The overlay also opens into a small glass IDE (editor, terminal, Markdown preview, visual HTML/CSS editor), has an
artifact viewer for things AI tools produce, and ships an MCP server so AI apps can search and work with your Desktop.

> **Status:** 1.0, a developer release. Build from source; the app isn't notarized yet.
> Requires macOS 13 or later. Liquid Glass on macOS 26, frosted blur on 13–15.

## Contents

- [Requirements](#requirements) · [Install](#install) · [First run](#first-run) · [Uninstall](#uninstall)
- [Features](#features) · [Keyboard shortcuts](#keyboard-shortcuts)
- [AI backends](#ai-backends) · [MCP server](#mcp-server)
- [Safety](#safety) · [Privacy](#privacy) · [Troubleshooting](#troubleshooting)
- [Development](#development) · [License](#license)
- How it works inside: **[ARCHITECTURE.md](ARCHITECTURE.md)**

## Requirements

| | |
|---|---|
| macOS | 13 Ventura or later (Apple silicon or Intel; on-device models need Apple silicon) |
| Toolchain | Xcode 15+ or the Command Line Tools (`xcode-select --install`) for Swift 5.9, and **Rust** ([rustup.rs](https://rustup.rs)) for the `dg` engine; add the `aarch64-apple-darwin` and `x86_64-apple-darwin` targets for a universal build. Python isn't needed (except by MLX itself) |
| At least one AI backend | an agent CLI (Claude Code, Codex, Gemini CLI, Antigravity CLI, Cursor CLI, GitHub Copilot CLI, OpenCode) **or** an on-device runtime (Apple Foundation Models, MLX, Ollama, LM Studio, llama.cpp) — see [AI backends](#ai-backends) |
| Network (build only) | Swift Package Manager fetches [SwiftTerm](https://github.com/migueldeicaza/SwiftTerm) and Cargo fetches the engine's crates on the first build |

## Install

```bash
git clone <this repo> DesktopGlass
cd DesktopGlass
./install.sh
```

`install.sh`:

1. copies the `desktopglass-index` skill (`SKILL.md` only) to `~/.claude/skills/desktopglass-index` and removes the
   Python/zsh scripts older versions put there
2. builds `DesktopGlass.app` (`swift build -c release` plus the Rust engine `dg`, ad-hoc signed) and copies it to
   `~/Applications`
3. links `~/.local/bin/dg` to the engine inside the app, and writes a `~/.local/bin/dgview` wrapper that runs `dg view`
4. runs `dg mcp-install refresh`, which points existing MCP registrations and hooks at `dg` (including ones made by
   the old Python installer)
5. launches it — look for the grid icon in the menu bar

Re-run it after pulling changes. Your settings and descriptions are kept.

## First run

1. Press **⌥⌘D**. macOS asks whether DesktopGlass may read your Desktop — allow it.
2. Open **⚙️ Settings**:
   - **Re-indexing → Written by:** pick a backend (anything not installed is marked *not found*), a model, and a schedule.
   - **Indexing Preferences:** blurb style, notes for the AI, and per-folder **Include / Names only / Skip**.
   - **MCP → Install in All Apps** (optional): registers the MCP server with the AI apps on this Mac and tells their
     CLIs about DesktopGlass, so what they make shows up in the artifact viewer.
3. Click **Re-index**. The first run describes every folder; later runs only touch what changed and are free when nothing did.

The app never moves, renames or deletes your files. It writes a `description.md` into each described folder and keeps
its own files in `~/Desktop/_Organize Log/`.

## Uninstall

```bash
./uninstall.sh              # removes the app, skill, dg, dgview, MCP registrations (dg mcp-install uninstall) and settings
./uninstall.sh --purge      # also deletes the description.md files and _Organize Log from your Desktop
```

## Features

**Island**
- A glass pill at the top of your desktop, always there when you're looking at the desktop. Click it, swipe down
  with two fingers at the top edge of the screen, or press ⌘D in any app and it drops down into the overlay
  (while ⌘D is on, other apps don't get it; Settings → Island turns it off)
- ⌘= / ⌘- step the open island through Standard, Medium, Large and Full Screen
- **Wallpaper** (the picture button on the pill, or Settings → Island): an image or a looping, muted video, from a
  file or a link; files are copied to `~/Library/Application Support/DesktopGlass/Wallpapers`. YouTube links play
  through YouTube's embedded player (needs a connection), with an optional fallback image or video for when it can't
- **DesktopGlass mode** (the switch on the pill): hides Finder's desktop icons, shows your wallpaper and auto-hides
  the Dock. Turning it off brings back the icons, your normal wallpaper and your own Dock setting. Your files stay on
  the Desktop and show in the overlay
- ⌘D and the top-edge swipe need Accessibility permission; clicking the pill and swiping over it don't

**Overlay**
- Folders as glass cards with summaries and per-item blurbs; projects and plain folders open in place
- Search across names, blurbs and tags; keyboard navigation (arrows / WASD, Return, Tab to go back)
- Hover actions: edit in DesktopGlass, open, show in Finder, open in your editor
- The same docked agents terminal as the IDE, on the right; it starts collapsed to a slim rail — click the rail or
  press ⌃` to open it
- A collapsible, searchable **Apps** dropdown in the sidebar

**Editor (the overlay morphs into it)**
- File tree, tabs with per-file undo, syntax highlighting for ~20 languages, line numbers, auto-indent
- Quick Open (⌘P), find & replace, reload-on-external-change
- Terminal panel with your default CLI already running in the project; collapses to a rail
- **Past conversations** (clock button): the folder's earlier conversations for Claude Code, Codex, Gemini CLI,
  Antigravity CLI, Copilot CLI and OpenCode, newest first — click one to resume it, or *Continue Last* / the CLI's own chooser
  (Cursor's chats are only reachable that way). Local models (Ollama, MLX, LM Studio) don't save conversations
- **Markdown:** Source ⇄ Preview, where Preview is a live-styled editor you can keep typing in — Mermaid diagrams,
  images (local, relative, web, SVG and badges) and ```` ```svg ```` blocks render inline; click one to edit its source. Fenced code blocks are syntax-highlighted
  (by their ```` ```lang ```` tag) on a darker translucent panel with a hairline border
- **HTML (vanilla HTML/CSS/JS):** Source ⇄ Visual
  - click elements to edit layout, spacing, size, type, colour, border and effects in an inspector
  - styles go to the linked stylesheet by default, or to `<style>` / inline if you choose
  - **Split Style** gives one element its own copy of a shared class; **Group** gives several elements a new shared class
  - double-click text to edit it in place (inline formatting kept)
  - every change is an exact edit of the source files, undoable with ⌘Z; no AI involved

**Artifact viewer**
- A glass browser limited to AI output: local HTML/SVG/PDF/images (live-reload), web servers running on this Mac, and
  published artifact/canvas/preview hosts. Other web pages open in your default browser; files and app links it
  can't show are refused rather than opened, so a link can never launch an app or script
- Lists local web servers (found with `lsof`, checked with a real request) so dev servers are one click away
- Open things with `dgview <file-or-url>`, `desktopglass://open?url=…`, or the `show_artifact` MCP tool. Anything
  opened this way — including any web page — opens in the viewer, and that tab keeps its links. Pages you didn't
  ask for that aren't artifacts still go to your browser
- **New Tab** (the **+** in the editor's tabs or the viewer window): a blank tab with an address bar — type a URL,
  `localhost:port` or a file path — and a list of the web servers running on this Mac
- **While the editor is open, pages open as tabs next to your code** (back/forward/reload in the tab bar). The **+**
  at the end of the tabs opens a new tab, a preview of the current file, or a local server. Right-click a
  page tab → *Open in Viewer Window* to pop it out; the window's split button moves it back into the editor
- **Accounts** (Settings → Accounts, or the person button on a page): sign in once to Claude, ChatGPT/Codex, Google,
  Vercel or GitHub and their artifacts open signed in right away. The site's own page does the sign-in (with its
  popups, e.g. *Continue with Google*); DesktopGlass keeps no passwords or tokens, and *Sign Out* clears that site
  from the viewer only
- **Codex visualizations:** Codex can only draw its visualizations in the Codex app (elsewhere: "Visualization
  unavailable on this device"). DesktopGlass watches `~/.codex/visualizations` and the open project's
  `.codex/visualizations` and shows each new one as a page — next to your code when the editor is open. Toggle in
  Settings → Accounts

**Games library (Games folder only)**
- In a top-level folder named *Games* (or one you pick: right-click it in the sidebar → *Use as Games Folder*), a
  **Library | Folder** switch turns the folder into a library of native Mac games: `.app` bundles and Finder
  aliases/symlinks to them. Click or Return launches the game
- **Find Games…** searches this Mac for apps marked as games and Mac games in Steam libraries, and adds shortcuts
  (Finder aliases) to them in the Games folder — nothing is moved or copied
- Sort by name or recently played
- **Controller support:** with the overlay open, a game controller's D-pad/stick moves, A plays or opens, B goes back,
  Y searches. The overlay hides when a game starts, and the controller is ignored while it's hidden
- Emulation was removed; games for other platforms will come back as DesktopGlass apps (see `plans/`)

**Indexing**
- Seven agent CLIs and five on-device backends, chosen in Settings; the schedule is checked every minute
- Change detection by size and mtime: unchanged folders are never sent; no change means no AI call at all
- Sign-in and "model not running" detection, with scheduled runs paused until fixed
- Questions the AI couldn't answer are queued in `_Organize Log/questions.md`

## Keyboard shortcuts

| Where | Keys | Action |
|---|---|---|
| Anywhere | ⌘D · two-finger swipe down at the top edge | Open / close the island |
| | ⌥⌘D (configurable) | Show / hide the overlay |
| Island | ⌘= · ⌘- | Bigger · smaller (Standard → Medium → Large → Full Screen) |
| Overlay | ↑ ↓ ← → or W A S D | Move the highlight |
| | Return / ⌥Return / ⌘Return | Open · open in the editor · show in Finder |
| | Tab or Delete | Back one level |
| | `/`, ⌘F, or any letter | Search |
| | Esc | Clear search, then close |
| Editor | ⌘P · ⌘F · ⌥⌘F | Go to file · find · find & replace |
| | ⌘S · ⌥⌘S · ⌘W | Save · save all · close tab |
| | ⌘B · ⌃\` · ⇧⌘V | Sidebar · terminal · Preview/Visual |
| | ⇧⌘D · ⇧⌘A | Back to the overlay · artifact viewer |
| Visual HTML | double-click · Return · Esc · ⇧Return | Edit text · save · cancel · line break |

## AI backends

**Agent CLIs** (opt-in for unattended runs — see [Safety](#safety)) run tools themselves and follow [`skill/desktopglass-index/SKILL.md`](skill/desktopglass-index/SKILL.md):

| CLI | Headless command used | Instructions |
|---|---|---|
| Claude Code | `claude -p --permission-mode acceptEdits --allowedTools …` | installed as a skill |
| Codex CLI | `codex exec --full-auto -C ~/Desktop` | SKILL.md as the prompt |
| Gemini CLI | `gemini -p --approval-mode yolo` | SKILL.md as the prompt |
| Antigravity CLI | `agy -p --dangerously-skip-permissions` | SKILL.md as the prompt |
| Cursor CLI | `cursor-agent -p --force --trust` | SKILL.md as the prompt |
| GitHub Copilot CLI | `copilot -p --no-ask-user --allow-tool 'read, write, shell(~/.local/bin/dg:*)'` | SKILL.md as the prompt |
| OpenCode | `opencode run` | SKILL.md as the prompt |

**On-device** backends can't run tools, so `dg reindex` ([`engine/crates/dg-index/src/local.rs`](engine/crates/dg-index/src/local.rs))
does the agent's job and validates every reply. Local servers are reached over plain `http://` (endpoints and live
model lists alike):

| Backend | Setup |
|---|---|
| Apple Foundation Models | macOS 26 with Apple Intelligence on |
| MLX | `pip install mlx-lm`; DesktopGlass starts `mlx_lm.server` itself (or start one: `mlx_lm.server --model <model>`). Models download from Hugging Face on first use |
| Ollama | Ollama running, a model pulled (`ollama pull llama3.2`) |
| LM Studio | local server started with a model loaded |
| llama.cpp | `llama-server -m model.gguf --port 8080` |

A first full index of a busy Desktop is roughly 7k input tokens; later runs are usually a few hundred, or zero.

## MCP server

`dg mcp` — the engine's MCP server (stdio, [`engine/crates/dg-mcp`](engine/crates/dg-mcp)), 35 tools:

| Group | Tools |
|---|---|
| Desktop | `desktop_overview`, `list_folder`, `search_desktop`, `get_description`, `recent_changes`, `update_description`, `list_questions`, `resolve_question` |
| Indexing | `index_status`, `reindex_desktop`, `get_schedule`, `set_schedule`, `set_index_backend`, `set_preferences` |
| Artifact viewer | `show_artifact`, `list_local_servers`, `browser_tabs`, `browser_read`, `browser_screenshot`, `browser_click`, `browser_type`, `browser_eval`, `browser_navigate` |
| HTML styling | `html_outline`, `html_set_style`, `html_split_style`, `html_group_style`, `html_set_text`, `html_set_html`, `html_duplicate_element`, `html_remove_element` |
| Windows | `open_in_editor`, `show_overlay` |
| Games | `list_games`, `play_game` (needs *Launch games* allowed in Settings → MCP) |

Register it with **Settings → MCP → Install in All Apps** (Claude Code, Claude Desktop, Codex, Cursor, Gemini CLI,
Copilot CLI, OpenCode, Antigravity, LM Studio, Windsurf, VS Code; each config is backed up once), or add it by hand:

```json
{
  "mcpServers": {
    "desktopglass": {
      "command": "/Users/you/.local/bin/dg",
      "args": ["mcp"]
    }
  }
}
```

(Use the absolute path; **Copy JSON** in Settings fills it in. `~/.local/bin/dg` is a link to the engine inside the app.)

**So agents actually use it.** Registering a server doesn't make a model reach for it, so *Install in All Apps* also:
- adds a short, marked DesktopGlass section to each CLI's global instructions — `~/.claude/CLAUDE.md`,
  `~/.codex/AGENTS.md`, `~/.gemini/GEMINI.md` (Gemini CLI and Antigravity), `~/.copilot/copilot-instructions.md`, `~/.config/opencode/AGENTS.md`
  — pointing it at the Desktop index tools (`desktop_overview`, `search_desktop`). Nothing else in those files
  changes, and uninstalling removes only that section;
- for **Claude Code**, adds a `PostToolUse` hook (`dg hook autoshow`) so every HTML or SVG file it writes opens in the
  viewer without the model having to remember — only in sessions started from DesktopGlass's terminal
  (`TERM_PROGRAM=DesktopGlass`). Pages inside a JS project are skipped (they need the dev server), and
  `DESKTOPGLASS_AUTOSHOW=0` turns it off for a session;
- for **every agent CLI**, adds a session-start hook (`dg hook session --format <cli>`) that tells the agent it's
  running inside DesktopGlass and how to show what it makes, again only in sessions started from DesktopGlass's
  terminal; elsewhere it adds nothing. `DESKTOPGLASS_SESSION_CONTEXT=0` turns it off for a session. `install.sh`
  (`dg mcp-install refresh`) updates it wherever the MCP server is already registered.

  | CLI | Where | Notes |
  |---|---|---|
  | Claude Code | `SessionStart` in `~/.claude/settings.json` | |
  | Codex | `SessionStart` in `~/.codex/hooks.json` | Codex asks you to trust the hook once |
  | Gemini CLI | `SessionStart` in `~/.gemini/settings.json` | |
  | Antigravity CLI | `desktopglass` entry in `~/.gemini/config/hooks.json` | |
  | Copilot CLI | `~/.copilot/hooks/desktopglass.json` | |
  | Cursor CLI | `sessionStart` in `~/.cursor/hooks.json` | Cursor has an open bug where it sometimes drops hook context |
  | OpenCode | plugin `~/.config/opencode/plugins/desktopglass.js` | uses OpenCode's experimental system-prompt hook |
- for **Codex**, registers through `codex mcp add` when the Codex CLI or app is installed, so the Codex app's own
  config handling keeps the entry.

Restart the CLI (or start a new Codex/Claude session) after installing.

## Safety

DesktopGlass hands AI tools real permissions only when you say so.

**Unattended runs are opt-in, per CLI.** A headless re-index has no one approving individual steps, so agent CLIs
don't run until you allow it for that CLI (the first Re-index asks, or Settings → Re-indexing → *Unattended runs*).
The dialog shows exactly what that CLI may do. Until then, scheduled runs are skipped and MCP-triggered runs are
refused. On-device backends need no opt-in: they never run commands — DesktopGlass writes the files itself.

**Each CLI's own permission controls apply** — the flags DesktopGlass passes are listed below. Claude Code, Codex and
Copilot run with restricted tool sets or their own sandbox; Gemini, Cursor and OpenCode auto-approve in headless mode.

| CLI | What it may do in an unattended run |
|---|---|
| Claude Code | tool allow-list: Skill, Read, Write, Edit, `dg inventory` (`--allowedTools`), edits auto-accepted |
| Codex CLI | Codex's own sandbox (`--full-auto -C ~/Desktop`): writes only inside the Desktop, no network for commands |
| Gemini CLI | auto-approves everything (`--approval-mode yolo`) |
| Antigravity CLI | auto-approves everything (`--dangerously-skip-permissions`) — headless `agy` silently skips any tool that needs approval otherwise |
| Cursor CLI | auto-approves commands (`--force`) |
| GitHub Copilot CLI | `--allow-tool 'read, write, shell(~/.local/bin/dg:*)'` |
| OpenCode | auto-approves (`opencode run`) |

**Optional extra sandbox.** Settings → Re-indexing → *Extra sandbox* (off by default) wraps agent CLIs in a macOS
sandbox profile (`sandbox_profile` in `engine/crates/dg-index/src/agent.rs`) that allows writes only to
`description.md` files and `_Organize Log` on the Desktop, temp folders and the CLI's own session/cache folders;
write-protects CLI settings, hooks, skills, MCP lists and DesktopGlass's skill; and blocks reading `~/.ssh`, `~/.aws`, `~/.gnupg` and other
credential folders. Codex keeps its own sandbox (macOS sandboxes can't be nested).

**MCP is read-only by default.** AI apps can always read the index and open the artifact viewer. Three groups are off
until you tick them in Settings → MCP, and are enforced in the MCP server, the engine and the app:

- *Edit files* — `update_description`, `resolve_question`, `html_*` edits
- *Change settings and start re-indexing* — `set_schedule`, `set_index_backend`, `set_preferences`, `reindex_desktop`
- *Click, type and run JavaScript* — `browser_click`, `browser_type`, `browser_eval`

Disabled tools aren't listed to AI apps and are refused if called anyway. File paths are confined to `~/Desktop`
(`resolve_question` included: it never writes outside it).

**The control socket** the MCP server uses to drive the app is opened by the engine (`dg engine`, which the app
starts). It listens on `127.0.0.1` only and requires a random token stored in `~/Library/Application Support/DesktopGlass/control.json` (mode 600). Any process running as your
user can read that file, as with any per-user secret.

**The artifact viewer** loads remote pages only from a fixed list of artifact/preview hosts and localhost. Until
the engine has sent those lists at startup, it allows nothing.

## Privacy

- With an **agent CLI**, each changed folder's file names, sizes, dates, project stack hints, the first line or two
  of READMEs and the first line of small text files are sent to that CLI's provider. Beyond that, the AI may read
  the start of up to 5 files per run when a name says too little.
- Folders set to **Names only** send names only; **Skip** sends nothing.
- With an **on-device** backend nothing leaves your Mac.
- DesktopGlass itself has no telemetry and makes no network requests except to the backend you choose, local
  servers you open, and artifact hosts you visit.

## Troubleshooting

| Problem | Fix |
|---|---|
| Overlay is empty | System Settings → Privacy & Security → Files and Folders → allow DesktopGlass to read the Desktop |
| "Not signed in to …" | Click **Sign In**, finish the CLI's login in Terminal, then **Check again** |
| "… isn't ready" (Ollama/LM Studio) | Start the app/server, load a model, then **Check model** in Settings |
| A re-index failed | Settings → Files → **Re-index log** (`_Organize Log/reindex.log`) |
| Hotkey does nothing | Another app owns it — pick another under Settings → General → Shortcut |
| macOS says the app is damaged / unverified | Right-click `~/Applications/DesktopGlass.app` → **Open** (the build is ad-hoc signed) |
| MCP tools that drive the app fail | Make sure DesktopGlass is running; **Test** in Settings → MCP shows what the server sees |
| Overlay stays empty, Settings can't find backends | The app needs its bundled engine (`DesktopGlass.app/Contents/MacOS/dg`); install Rust and re-run `./install.sh` |

## Development

```bash
swift build                    # debug build
./scripts/build-app.sh         # release .app in build/
./install.sh                   # build + install skill + app + dg
scripts/test.sh                # Rust tests, parity checks, swift test (scripts/test.sh rust|parity|swift for one)
engine/target/release/dg schema   # the app ↔ engine protocol (methods, notifications, shared settings)
```

- App identity and version live in **`app.conf`** (`VERSION`, `BUILD`, `BUNDLE_ID`) — the only place to change them.
- Project layout and data flow: **[ARCHITECTURE.md](ARCHITECTURE.md)**.
- Everything that isn't UI is in the Rust engine under `engine/` (one binary, `dg`: `inventory`, `reindex`, `ask`,
  `login`, `mcp`, `mcp-install`, `hook`, `view`, `engine`, `schema`). Folder rules live once, in
  `engine/crates/dg-core/src/rules.rs`.
- JavaScript injected into WebViews lives in `shared/web/*.js`; `scripts/build-app.sh` copies it into the app.
- The parity checks compare `dg inventory` and `dg mcp` with the old Python scripts kept in
  `engine/tests/parity/reference/` (not installed); they're skipped if `python3` is missing.

**Known limitations in 1.0:** not notarized; the visual HTML editor targets vanilla HTML/CSS/JS (framework-rendered
pages are viewable, with limited editing); CLI flags for agents other than Claude Code are taken from their docs and
less tested; Swift 6 strict-concurrency warnings remain in WebKit delegates.

## License

DesktopGlass is **source-available and free for noncommercial use** under the
[PolyForm Noncommercial License 1.0.0](LICENSE.md): personal use, study, hobby projects and research, and use by
charities, schools and public institutions, are all allowed — including modifying and sharing it. Commercial use
needs a separate license from the author; open an issue to ask.

Bundled third-party software keeps its own license — see [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
