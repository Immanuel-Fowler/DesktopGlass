# Plan: move the DesktopGlass engine to Rust

**Goal:** one cross-platform engine, written in Rust, that does everything that isn't UI. Each platform then gets a
native UI that is a thin client of that engine: SwiftUI/AppKit on macOS (today's app) and WinUI 3 on Windows (see
[WINDOWS_UI.md](WINDOWS_UI.md)).

## Status

**Phases 0–5 are done (2026-11-06). Phase 6 (Windows platform layer) has not started.** The rest of this document
is the original plan, kept as written; [ARCHITECTURE.md](../ARCHITECTURE.md) describes the code as it is now.

- `dg` (`engine/`: `dg-core`, `dg-proto`, `dg-platform`, `dg-index`, `dg-mcp`, `dg-install`, `dg-engine`) does all
  non-UI work: `inventory`, `reindex`, `ask`, `login`, `mcp`, `mcp-install`, `hook`, `view`, `engine`, `schema`.
- No Python or zsh ships any more: the skill folder holds only `SKILL.md`, and `install.sh` links `~/.local/bin/dg`
  and runs `dg mcp-install refresh` to move old registrations and hooks to `dg`. The Python scripts survive only in
  `engine/tests/parity/reference/` for the parity harnesses (`parity.sh`, `mcp-parity.sh`); `Tests/python` is gone.
- The app spawns `dg engine` (`EngineClient.swift`), which owns the control channel, `settings.json` and FSEvents
  watching; `ControlServer.swift` and the Swift scanner, schedule, backend detection, HTML ops, artifact rules, game
  finder and local-server scan are gone. Embedded JavaScript moved to `shared/web/*.js`.

**Deliberate behaviour differences** from the Python/Swift code:

- MLX runs as `mlx_lm.server` over HTTP instead of through Python imports; `dg` can start the server itself.
- Local model endpoints and live model lists are `http://` only.
- The re-index schedule is checked every 60 s (was every 10 minutes).
- A re-index can be cancelled (`index/cancel` in the engine); the app has no button for it yet.
- `localizedStandardCompare` and Finder's package detection are approximated in Rust: sorting can differ in rare
  cases (ß, ø, combining marks), and unusual package extensions may not be treated as packages.
- `resolve_question` refuses to write outside the Desktop (fixes a Python bug that could write `~/description.md`).
- The artifact policy is deny-all until the engine has sent its tables at startup.
- The app needs the bundled engine; there is no in-app fallback.

---

**Rule for the whole migration:** the Mac app keeps working after every phase. Each phase replaces one piece, proves
it matches the old behaviour with tests, and only then deletes the old code.

---

## 1. Why this change

Today the non-UI logic is split across three languages, and two of them don't exist on a stock Windows machine:

| Where | What | Lines |
|---|---|---|
| Python, `skill/desktopglass-index/scripts/*.py` | inventory/change detection, on-device indexing, MCP server (35 tools), MCP installer, hooks | ~2,050 |
| zsh, `reindex.sh`, `common.sh`, `ask.sh`, `login.sh` | reindex entry point, lock, CLI discovery, sign-in preflight, failure classification | ~430 |
| Swift, mixed into the app | Desktop scanner, `description.md` parser, search, schedule, backend detection, preferences model, local server discovery, artifact policy, game detection, HTML source ops | ~3,000 of 11,577 |

Some rules are written twice and must be kept in sync by hand. The group/project rules live in `Models.swift` **and**
`inventory.py`, and the `description.md` parser exists as both `FolderDescription.parse` and `parse_desc`. The MCP
server reads settings with `defaults read`, which only works on macOS.

Rust gives one static binary with no runtime to install, good cross-platform libraries for the parts that differ by
OS, and an easy path to call it from both Swift and C#.

## 2. Target architecture

One executable, `dg` (`dg.exe` on Windows), with subcommands. It ships inside the app bundle / MSIX package.

```
                ┌──────────── UI process (per platform) ────────────┐
                │ macOS: SwiftUI + AppKit   Windows: WinUI 3 (C#)   │
                │ windows · glass · hotkey · editor · terminal ·    │
                │ WebView (artifact viewer, visual HTML editor)     │
                └──────────────┬────────────────────────▲───────────┘
          spawns `dg engine`;  │ JSON-RPC 2.0 requests  │ notifications + UI-bound calls
          stdio, NDJSON        ▼                        │ (browser_*, html_*, show_artifact)
                ┌──────────────────────────────────────────────────┐
                │ dg engine   (long-running, one per user session) │
                │  core: scan · rules · description.md · manifest  │
                │        index assembly · search · preferences     │
                │  index: lock · schedule · backends · preflight   │
                │  watch: file-system events → "desktop changed"   │
                │  control: loopback listener + token (control.json)│
                └──────────────▲───────────────────────────────────┘
                               │ token-checked loopback TCP
                ┌──────────────┴──────────┐   ┌─────────────────────────────┐
                │ dg mcp  (stdio, per     │   │ dg reindex · dg inventory · │
                │ MCP client)             │   │ dg view · dg hook · dg mcp- │
                │ file tools work without │   │ install  (one-shot CLIs,    │
                │ the engine running      │   │ used by agents and scripts) │
                └─────────────────────────┘   └─────────────────────────────┘
```

**Why a separate process instead of a linked library:**
- It matches what exists today. `mcp_server.py` already reaches the app over a control channel, and the app already
  runs `reindex.sh` as a subprocess.
- No FFI bindings to generate or keep in sync. Each UI is just a JSON client.
- An engine crash (or a hung backend CLI) can't take the UI down; the UI restarts it.
- If a hot path ever needs to be in-process (for example search-as-you-type over a huge Desktop), add
  [UniFFI](https://mozilla.github.io/uniffi-rs/) bindings for that one call later.

**The control channel moves into the engine.** Today `ControlServer.swift` listens and `AppControl.swift` handles
everything. After the move, the engine owns the listener, token and `control.json`. It answers file, index and
settings methods itself and forwards only UI-bound methods (`browser_*`, `html_*`, `show_artifact`, `open_in_editor`,
`show_overlay`, `play_game`) to the UI over the stdio link. Both UIs then implement the same small set of handlers.

**Protocol types are defined once.** Request/response structs live in Rust with `serde` + `schemars`. `dg schema`
prints a JSON Schema that generates Swift (`quicktype`) and C# (`NJsonSchema` or `quicktype`) types at build time,
so a field rename breaks the build instead of failing at runtime.

## 3. Repository layout

```
engine/
├── Cargo.toml                 workspace
├── crates/
│   ├── dg-core/               no I/O side effects beyond the file system; no async
│   │   ├── rules.rs           group / project / package classification (single source of truth)
│   │   ├── description.rs     description.md parse + render (byte-compatible with today's files)
│   │   ├── scan.rs            Desktop walk, hints (package.json, requirements.txt, README first line), caps, skips
│   │   ├── manifest.rs        .manifest.json signatures, change detection
│   │   ├── assemble.rs        desktop-index.md
│   │   ├── prefs.rs           preferences.json (style, private, skip, flagStatus, notes)
│   │   ├── questions.rs       questions.md
│   │   ├── search.rs          search over names + descriptions
│   │   ├── html.rs            HTMLSource / HTMLOps port (outline, set style/text, split, group…)
│   │   └── paths.rs           confinement to the Desktop root, symlink resolution
│   ├── dg-index/              reindex pipeline
│   │   ├── lock.rs            directory lock, 30-minute staleness
│   │   ├── harness.rs         backend catalogue, detection, models (Harness.swift + common.sh)
│   │   ├── agent.rs           headless CLI runs, SKILL.md → prompt, unattended consent, sandbox
│   │   ├── local.rs           on-device pipeline (local_index.py): Ollama, LM Studio, llama.cpp, MLX server
│   │   ├── preflight.rs       sign-in checks, classify_failure
│   │   └── exit.rs            exit codes 0/10/75/76/77/78/79/127 (unchanged)
│   ├── dg-mcp/                MCP server: protocol negotiation, 35 tools, permission groups
│   ├── dg-platform/           everything that differs by OS, behind traits (see §6)
│   └── dg-proto/              engine ↔ UI and control-channel message types (+ JSON Schema export)
├── dg/                        the binary: clap subcommands wiring the crates together
└── tests/
    ├── golden/                fixture Desktops + expected outputs (shared with Python during migration)
    └── parity/                runs Python and Rust on the same fixtures and diffs the results
```

**Key crates:** `clap` (CLI), `serde`/`serde_json`, `schemars`, `tokio` (processes, timers, sockets), `reqwest`
(local model HTTP), `notify` (FSEvents / ReadDirectoryChangesW), `dirs` + `known-folders` (Desktop/OneDrive
location), `which` (CLI discovery), `sha2`, `walkdir`, `keyring` (credential presence checks only), `tracing` for logs.

## 4. Phases

Each phase lists what moves, how parity is proven, and when it's done.

### Phase 0: Scaffolding (½ day)
- Create the `engine/` workspace and the `dg` binary with `dg --version` reading `app.conf`.
- CI: `cargo test` + `clippy` on macOS (arm64, x86_64) and Windows runners; the existing `scripts/test.sh` keeps
  running.
- `scripts/build-app.sh` builds `dg` as a universal binary (`lipo`) and copies it to `DesktopGlass.app/Contents/MacOS/dg`.
- Turn `Tests/DesktopGlassTests/Fixtures/descriptions` into the start of `engine/tests/golden/`.

**Done when:** the app bundle contains `dg`, and CI is green on both OSes.

### Phase 1: Core and `dg inventory` (replaces `inventory.py`, 423 lines)
- Port `rules`, `description`, `scan`, `manifest`, `assemble`, `prefs`, `paths`.
- `dg inventory scan | check | commit | status` with the **same arguments, JSON output and exit codes** as
  `inventory.py`. The compact scan format (`{"n": name, "keep": blurb}`, 40-entry cap, skip list) must match exactly,
  because `SKILL.md` and every agent depend on it.
- Parity test: for each golden Desktop, run `python3 inventory.py X` and `dg inventory X` and diff the stdout,
  `.manifest.json` and `desktop-index.md`.
- Switch `reindex.sh` and `SKILL.md` to call `dg inventory`. Keep `inventory.py` one release as a fallback.

**Done when:** parity tests pass on all fixtures plus a copy of a real Desktop, and `test_inventory.py` is ported to
Rust tests.

### Phase 2: Reindex pipeline (replaces `reindex.sh`, `common.sh`, `local_index.py`)
- `dg reindex` reads the same inputs (`HARNESS`, `MODEL`, `ENDPOINT`, `REASON`, `FULL` env vars, or flags) and returns
  the **same exit codes**, so `Reindexer.swift` only changes the command it runs.
- Port CLI discovery, including the macOS GUI-app PATH problem that `common.sh` solves (resolve the login-shell PATH once
  and cache it).
- Port `preflight_auth` and `classify_failure` with their pattern tables as data, unit-tested against captured CLI output.
- Port the on-device pipeline: per-folder prompts, batching, `<think>`/fence stripping, parse-and-retry-once, verbatim
  "keep" blurbs. Talk to every local runtime over HTTP. MLX runs as `mlx_lm.server` (OpenAI-compatible) instead of
  through Python imports.
- Apple Foundation Models stays a Mac-only backend: `dg` calls the app binary's existing `--foundation-models` entry
  point, gated by `cfg(target_os = "macos")`.
- Seatbelt sandbox (`sandbox-exec` profile) stays macOS-only behind `dg-platform` (see §6 for Windows).

**Done when:** every backend in Settings re-indexes through `dg reindex`, every exit code has a test, and the
shell scripts are deleted.

### Phase 3: MCP server (replaces `mcp_server.py`, 573 lines)
- `dg mcp` speaks JSON-RPC 2.0 over stdio. It negotiates protocol versions 2024-11-05 → 2025-06-18 and never writes
  anything but protocol messages to stdout.
- Port all 35 tools with identical names, schemas and result shapes. File tools work with no engine running. App tools
  go through the control channel and launch the app if needed (`open -g` on macOS, a protocol/AUMID launch on Windows).
- Permission groups (`read` by default; `edit`, `control`, `script`, `play` opt-in) **move out of UserDefaults** into a
  shared `settings.json` (see Phase 5) so they work on Windows. Until then, read them with `defaults read` on macOS.
- Port `test_mcp_security.py` cases one-for-one: path escapes, symlinks, one-line enforcement, hidden tools, refused
  calls.
- Use the official `rmcp` crate if it covers the version negotiation we need; otherwise keep a small hand-written loop
  as the Python one is today.

**Done when:** `mcp_install.py test` passes against `dg mcp`, and Claude Code, Codex and Gemini each complete a
smoke session.

### Phase 4: Installer and hooks (replaces `mcp_install.py`, `autoshow.py`, `session_context.py`, `dgview`, `ask.sh`, `login.sh`)
- `dg mcp-install install | uninstall | test`. Keep the per-client config writers (JSON `mcpServers`/`servers`/`mcp`,
  Codex TOML, `claude mcp add`), the one-time backup and the skip-on-parse-failure rule. Make config paths per-OS.
- `dg hook autoshow` and `dg hook session --format <cli>` replace the two Python hooks; `dg view <path|url>` replaces
  `dgview`.
- `install.sh` stops copying Python scripts. The skill folder keeps only `SKILL.md`, which now calls `dg`.

**Done when:** a clean Mac with **no Python** installs and runs everything, and `Tests/python/` is gone.

### Phase 5: The engine daemon (moves non-UI logic out of Swift)
- `dg engine`: long-running; spawned and supervised by the UI; JSON-RPC over stdio.
- Move from Swift:

  | Swift file | Moves | Stays in UI |
  |---|---|---|
  | `Models.swift` | Scanner, `FolderDescription`, group rules | layout constants |
  | `Store.swift` | search, desktop model | selection, keyboard state |
  | `Reindexer.swift` | `Schedule`, run + exit-code interpretation, `IndexProblem` | the problem chip |
  | `Harness.swift` | detection, `ModelCatalog` | labels |
  | `IndexPreferences.swift` | the model | the Settings section |
  | `ControlServer.swift`, `AppControl.swift` | listener, token, non-UI methods | handlers for UI-bound methods |
  | `LocalServers.swift` | server discovery (`lsof` → platform trait) | — |
  | `ArtifactPolicy.swift` | the allow-list rules | applying them in the WebView |
  | `Games/GameFinder`, `GameCatalog`, `GameLibrary` scan | game detection | `GamesLibraryView` |
  | `HTML/HTMLSource`, `HTMLOps` | all of it | `HTMLVisualSession`, `HTMLVisualEditor` |

- Settings that both the engine and the UI need (backend, models, endpoints, schedule, `mcp.allow.*`, `unattended.*`)
  move from UserDefaults to `settings.json` in the app-support folder, owned by the engine, with a one-time migration.
  Pure UI settings (window size, editor font) stay in UserDefaults.
- File watching via `notify` replaces polling. The engine pushes `desktop/changed`, `index/progress` and
  `index/problem` notifications.
- Add a Swift `EngineClient` (spawn, restart with backoff, typed calls from generated types).
- Extract the JavaScript embedded in Swift strings (`HTMLVisualSession.swift`, `AppControl.swift`,
  `MarkdownVisuals.swift`) into `shared/web/*.js` files so the Windows WebView2 can reuse them unchanged.

**Done when:** the Mac app has no file-system logic of its own, the shared Swift/Python fixtures run only against Rust,
and Swift tests cover only UI.

### Phase 6: Windows platform layer
Implement every `dg-platform` trait for Windows and run the full Rust test suite on Windows CI. After this phase the
engine is ready for the WinUI app.

## 5. Compatibility guarantees

These are on-disk and on-wire formats that users, agents and other apps already depend on. They must not change
during the migration:
- `description.md` format, byte for byte, for unchanged content
- `_Organize Log/` contents: `desktop-index.md`, `.manifest.json`, `preferences.json`, `questions.md`,
  `reindex.log`, `.reindex.lock/`
- `inventory scan` JSON, which is the contract with `SKILL.md` and every agent
- reindex exit codes
- MCP tool names, input schemas and result shapes
- `control.json` (`{port, token, pid}`, mode 600) and the newline-delimited control protocol

## 6. Platform differences (`dg-platform`)

| Concern | macOS | Windows |
|---|---|---|
| Desktop folder | `~/Desktop` | `FOLDERID_Desktop` via `SHGetKnownFolderPath`; often redirected into OneDrive |
| App data | `~/Library/Application Support/DesktopGlass` | `%APPDATA%\DesktopGlass` |
| Launch the app | `open -g -b <bundle id>` | protocol activation (`desktopglass://`) or AUMID |
| Find local dev servers | `lsof -iTCP -sTCP:LISTEN` | `GetExtendedTcpTable` (`netstat2` / `listeners` crate) |
| Sign-in checks | credential files, Keychain item presence | credential files, Credential Manager presence (`keyring`) |
| Sandbox for agent CLIs | Seatbelt `sandbox-exec` profile | none in v1, so the CLI's own permission controls apply and Settings says so; later: AppContainer or a restricted token |
| CLI discovery | login-shell PATH | `PATH` + `PATHEXT` (`.cmd`/`.ps1` shims from npm) |
| File quirks | packages (`.app`, `.band`) are leaves | skip `desktop.ini`, hidden/system attributes; treat `.lnk` as entries; long paths (`\\?\`); case-insensitive names |
| Apple Foundation Models, MLX | available | not offered |

## 7. Risks

| Risk | Mitigation |
|---|---|
| Subtle parse/scan differences silently rewrite users' `description.md` | parity tests against Python on golden fixtures **and** a real Desktop copy before each switch-over; dry-run mode that diffs instead of writing |
| Agent prompts depend on exact `inventory scan` output | the golden tests include agent-facing JSON; `SKILL.md` is changed only to swap the command name |
| PATH/auth detection regressions per CLI | captured-output fixtures per CLI; manual smoke matrix before each release |
| HTML ops port changes users' pages | port `HTMLOpsTests.swift`/`HTMLSourceTests.swift` first, then the code |
| Windows has no Seatbelt equivalent in v1 | unattended runs stay opt-in per CLI; the consent dialog states that no sandbox is active on Windows |

## 8. Rough effort

| Phase | Estimate |
|---|---|
| 0 Scaffolding | ½ day |
| 1 Core + inventory | 3–4 days |
| 2 Reindex pipeline | 4–5 days |
| 3 MCP server | 3–4 days |
| 4 Installer + hooks | 2–3 days |
| 5 Engine daemon + Swift client | 5–7 days |
| 6 Windows platform layer | 3–4 days |
| **Total** | **~4–5 weeks** for one developer |

The Windows UI can start in parallel once Phase 5's protocol is defined. See [WINDOWS_UI.md](WINDOWS_UI.md).

Once Phase 5 lands, the engine also hosts third-party apps. See [APP_STANDARD.md](APP_STANDARD.md).
