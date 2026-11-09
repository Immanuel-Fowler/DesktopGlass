# Harbor: ideation brief

**Status:** for discussion. Nothing here is built or scheduled.
**Depends on:** the Rust engine plan ([README.md](README.md)), mainly Phase 5 (`dg engine`), and the app standard
([APP_STANDARD.md](APP_STANDARD.md)).

---

## 1. Vision

Harbor is where your agents and your local programs dock. From the terminal's program menu you pick **Harbor**
instead of a single CLI, and get one pane that runs several agents at once: Claude Code, Codex, Gemini and the
others next to on-device models (Ollama, LM Studio, llama.cpp, MLX, Apple Foundation Models). The pane routes work
between them and keeps every session. OrchestrateLang scripts the coordination: who works on what, in parallel, with
restarts and budgets. The same engine also **ships** local programs. You point Harbor at a script or project folder
and get a sealed, versioned bundle that runs, stops and logs the same way on any Mac, and later on Linux and Windows
through your translation layer. Think Docker, but with no VM and no images, just pinned toolchains plus an OS sandbox.
`.dgapp` extensions can use both halves through declared permissions.

### In scope for v1
- A `dg engine` subsystem, `harbor`, that spawns, supervises and records agent sessions over each CLI's
  **structured** protocol (not screen-scraping), plus local models over HTTP.
- A **Harbor** entry in the terminal program menu, which opens a multi-agent pane.
- Routing by explicit choice or simple rules, plus fan-out ("ask three agents, compare").
- Harbor scripts: a declarative manifest, with OrchestrateLang as the logic layer (see §4b).
- A shipper for **scripts and single-process projects** on macOS: pinned runtimes, a Seatbelt sandbox, and
  run/stop/logs.
- `.dgapp` manifest entries so apps can start agent sessions and ship/run Harbor bundles.

### Out of scope for v1
- Real cross-OS execution. Abstractions only (§4e).
- Multi-service stacks with networking between services (a compose equivalent). Next step after v1.
- Remote or cloud execution, and syncing across machines.
- A public registry or store. Sideload and local folders only.
- A GUI chat client competing with T3 Code. Harbor's UI is a terminal-first pane plus a small session list.

---

## 2. Findings

### 2.1 "t3 backend" = T3 Code

**What "t3" most likely means: T3 Code** (`github.com/pingdotgg/t3code`, ~50k stars, **MIT**, "Copyright (c) 2026
T3 Tools Inc."). Its README calls it an "agent harness control surface". It drives the agent CLIs already installed
on your machine, using your own subscriptions, from web, Electron, iOS and Android clients, plus a headless server
CLI (`npx t3`, `t3 service install`). That is exactly a "meta harness backend".

Other meanings ruled out:
- **t3.chat** is the same team's closed-source chat product. It has no backend to study, and we would build any
  equivalent from scratch.
- **create-t3-app** is an MIT Next.js/tRPC scaffold. It is unrelated to agents.

Everything below comes from reading the public source at commit `b90e554` (2026-11-06). Nothing was run.

**Architecture summary**

| Concern | T3 Code |
|---|---|
| Stack | TypeScript on Node in a pnpm monorepo. **Effect-TS v4** everywhere (services, Schema, `effect/rpc`, `effect/sql`). `node:sqlite`, `node-pty`. UI: React + Vite. Small Rust helpers in `native/`. |
| Layout | `apps/server` (the backend, npm `t3`), `apps/{web,desktop,mobile}`, and `packages/contracts` (the whole RPC and schema contract). Agent protocol clients: `packages/effect-codex-app-server`, `packages/effect-acp`. Providers: `packages/provider-core` and `provider-*`. Also `source-control-*` and `ssh` / `tailscale` packages. |
| Adapter abstraction | **`ProviderAdapterV2`** (`packages/provider-core/src/server/ProviderAdapter.ts`) has `getCapabilities()` and `openSession()`. `openSession` returns a scoped runtime with an **`events` stream** and `ensureThread`, `resumeThread`, `startTurn`, `steerTurn`, `interruptTurn`, `respondToRuntimeRequest`, `rollbackThread`, `forkThread`, plus optional compact, stop-subagent and similar methods. Features are gated on **capability flags, never provider names**. A *driver* is a kind of integration; an *instance* is one configured account. |
| Codex | spawns **`codex app-server`** and speaks JSON-RPC over stdio: `thread/start`, `thread/resume`, `turn/start`, `turn/interrupt`, and approval server-requests. The schema is generated. |
| Claude | **`@anthropic-ai/claude-agent-sdk`** `query()` pointed at the user's own `claude` binary, with `canUseTool` for approvals, `resume` and `forkSession`. The SDK is TypeScript-only. |
| Others | **ACP over stdio** (Agent Client Protocol) for Grok, Kiro, fx, Antigravity and any ACP-Registry agent, all through one generic driver. OpenCode through `opencode serve` (HTTP). Cursor and Muse through their own SDKs. Pi through `--mode rpc`. **No Gemini CLI or Copilot driver.** |
| Data model | Project → AppThread → **Run** (one user turn) → a tree of ExecutionNodes (tool, approval, subagent). **App-owned IDs**; provider thread and session IDs are references only. |
| Approvals | persisted `RuntimeRequest` entities answered with `runtime-request.respond`. It never fakes a permission mode by auto-answering. |
| Server ↔ UI | **one WebSocket `/ws` carrying Effect RPC (JSON)**. Namespaced methods (`terminal.*`, `vcs.*`, `git.*`, `projects.*`…) plus subscriptions. Clients get a **snapshot at sequence N, then the events after N**. |
| Core pattern | **Event sourcing + CQRS.** A pure, serial `Orchestrator` decides events from commands. `EventSink` commits events, projections, the command receipt and the outbox in **one SQLite transaction**. `EffectWorker` does the provider and filesystem I/O and feeds the results back as commands. Receipts make retries idempotent. |
| Persistence | `~/.t3/userdata/statev2.sqlite` (WAL, ~80 migrations). Raw provider NDJSON logs go in `logs/provider/`. The event log is pruned to about a day, so **projections are the source of truth**. |
| Git | worktrees under `~/.t3/worktrees`. **Checkpoints as hidden git refs** (`refs/t3/orchestration-v2/checkpoints`) drive diffs and rollback. Rich PR support across forges. |
| Terminals | server-owned PTYs with capped, persisted history (5k lines / 8 MiB). Clients render with libghostty-vt compiled to WASM. |
| Extras | a **per-thread MCP server injected into each agent**, so agents can spawn subagents on other providers, steer threads and schedule work. Remote access through Tailscale, SSH or the "T3 Connect" relay. Scheduled tasks. Usage-limit tracking. Replay fixtures recorded from real CLIs. |

**What Harbor should adopt** (ideas, re-implemented in Rust inside `dg engine`):
1. The **adapter trait shape** (session runtime + one normalized event enum + capability flags).
2. **Three transport families cover almost everything:** Codex app-server JSON-RPC, **generic ACP**, and Claude
   stream-json. The Claude Agent SDK is TypeScript-only, so Rust uses the CLI's `--input-format/--output-format
   stream-json` and handles permission prompts itself.
3. The **durable command → event → projection → outbox** loop in one SQLite transaction, with snapshot-plus-sequence
   streaming to the Swift UI. `dg-proto` JSON-RPC notifications map onto it directly.
4. **App-owned IDs** and **approvals as persisted requests** that survive an engine restart.
5. **Worktrees for parallel agents, and hidden-ref checkpoints** for diff and rollback.
6. **The injected MCP back-channel.** DesktopGlass already has `dg mcp`, so add `harbor_*` tools to it and agents can
   start or steer other agents.
7. **Replay-fixture tests** per CLI, which matches the reindex plan's captured-output fixtures.

**What not to adopt:** the Effect-TS runtime, the WebSocket/web-client split (DesktopGlass's UI is native and talks
over stdio), the remote relay and mobile clients, and the forge and PR machinery (v1).

**License implications.** MIT allows copying code, not just ideas. The only obligation is to keep the copyright and
permission notice in copies or substantial portions, and a close Rust translation counts as a derivative. Practically,
the code is so Effect-specific that **we port ideas and protocol knowledge, not code**. If any file is translated
closely, add a `THIRD_PARTY_NOTICES` entry for T3 Tools Inc. Check the upstream Codex app-server and ACP schema
licenses separately, and don't use the "T3" name or marks. There is nothing proprietary to reverse-engineer: the
backend is fully open, so "reverse engineering" here means reading the source.

### 2.2 OrchestrateLang: what it is today

**Where it lives**

| What | Path / repo |
|---|---|
| Compiler repo | `~/Desktop/Projects/OrchestrateLang/OrchestrateLang`, public at `github.com/Immanuel-Fowler/OrchestrateLang`. MIT. `main` is level with `origin/main` at `3f2827b` (2026-10-27), tagged `v0.15.1-57`. |
| Demo programs | `~/Desktop/Projects/OrchestrateLang/orchestratelang_demo_apps`: backtest-engine, robot-coordinator, **workflow-runtime**, sim-engine (built for a research paper) |
| Website | `~/Desktop/Projects/OrchestrateLang/orhcestratelangwebsite` (static HTML) |
| Out-of-repo bug list | `~/Desktop/Projects/OrchestrateLang/OrchestrateLang-bugs-and-problems.md` (as of 0.15.1) |
| Adopter | Ackee game engine: `Ackee-Game-Engine/sdk/rust/ackee_orchestrate`, `crates/ackee_cli/src/orchestrate.rs`, ADR 0045 |
| Installed binaries | `~/.cargo/bin/orchestrate` (v0.15.1, commit 278c61c), `orchestrate-lsp`, `cargo-orch(estrate)`. **`~/.local/bin/orchestrate` is an older June build and comes first on `PATH`.** It prints the old usage text, so `orchestrate` in a shell is not the current compiler. `~/.orchestrate/registry.toml` (PROM) still points at the pre-move path `~/Desktop/OrchestrateLang/...`. |

**What it is.** A compiled, async orchestration DSL (`.orch`) written in Rust (~17k lines in `src/`: lexer, parser,
typechecker, codegen, LSP). It **transpiles to Rust + Tokio and builds with Cargo**. There is no VM or interpreter.
`orchestrate run | build | check | check-foreign | build --lib | build --guest | prom`. There is a VS Code extension,
an LSP, 50 golden programs, and 121 diagnostics cases, all rejected by `check`.

**Core constructs**, which map directly onto agent orchestration:

| Construct | Meaning | Harbor reading |
|---|---|---|
| `orchestrator main(workers: process[...])` | owns lifecycle and the worker set; `trigger update_orchestrator([...])` swaps workers live | the session or run supervisor |
| `automatic(restart: 3 \| always \| never, backoff:, every:) { } on_crash e { }` | a supervised loop | a long-running agent or watcher with a restart policy |
| `on name(args) { }` / `trigger name(args)` | typed multicast events | "agent finished", "tests failed", "approval needed" |
| `serverlet X { on h(..) -> result<T> }`, `start X()` returns a typed client | an actor with state | **an agent as a typed service** |
| boundaries: in-process, `secret` (child exe), `via python(...)`, `via typescript(...)`, `sandbox(memory_limit:, timeout:)` (wasmtime) | where a body runs; same call site | where an agent adapter or tool runs |
| `budget: "2ms"`, `late: "drop"/"latest"` | per-call latency bounds | turn timeouts |
| `grant call world.record` | the only way a sandbox or landline reaches the host | **Harbor's capability model** |
| `parallel`, `\|>`, `shared let`, `shared queue (capacity:, full:)` | joins, pipelines, atomic shared state, bounded queues | fan-out, work queues |
| `start Model from path` (Unreleased) | load a `.wasm` guest at run time, checked against a declared interface | load user Harbor scripts into a running engine |
| `build --lib` → `Host` trait, `tick`, `trigger_<event>`, deterministic mode | embed in a Rust host | **embed in `dg engine`** |
| `measure ...` / `stats()` | p50/p90/p99 per handler | per-agent latency and cost dashboards |

**Gaps that matter for Harbor**

1. **No I/O in the language.** It has no file, network, process-spawn or env builtins. A script cannot launch
   `claude` or read a file by itself. Everything goes through foreign code, landlines or host grants. That suits
   Harbor (the engine does the I/O and the script asks through grants), but it means Harbor must define that host
   API.
2. **Running a script means compiling it with Cargo.** The first build is slow and the `.orch_cache` is about
   100 MB (400–600 MB with wasmtime). End users would need a Rust toolchain. That rules it out as an "edit and run in
   a second" scripting language unless Harbor precompiles or adds an interpreter.
3. **The sandbox ABI is narrow.** It has no `string[]`, no strings inside structs, and no maps across the wasm
   boundary. Agent messages are exactly that shape (lists of strings, structs with text), so a sandboxed Harbor
   script would need JSON-in-a-string or ABI work in the language.
4. `option`/enums/closures don't cross process boundaries, and a top-level `trigger update_orchestrator` before
   start is dropped (a known gap in the workflow-runtime demo).
5. Cross-compiling foreign code is limited, and the host target is the main focus. This matters for the
   cross-platform goal.
6. `via tcp` remote serverlets are **proposed only**. TypeScript in-process via scriptc is a plan.

**Most relevant prior work:** `orchestratelang_demo_apps/workflow-runtime` is a multi-tenant runtime that already
runs untrusted per-tenant steps in per-tenant wasm sandboxes with budgets, retries, dead-lettering and live
enable/disable. That is close to the shape of a Harbor run supervisor.

Also related: the private repo `Immanuel-Fowler/ackee-ide` (`~/Desktop/Projects/Ackee/ackee-ide`) is a Rust core
(`ackeed`) plus an Electron IDE whose README says "agent harness: not built yet". If Harbor's harness lives in a crate
with no DesktopGlass dependencies, Ackee could reuse it later.

### 2.3 Prior art for a local shipper

| Tool | Model | Take for Harbor |
|---|---|---|
| **Dev Containers** (`devcontainer.json`) | Docker image + features + lifecycle hooks | Lifecycle hook names (`postCreate`, `postStart`) and composable "features". Leave out the VM. |
| **Nix / devbox / flox** | content-addressed, fully pinned packages; `devbox.json` lists packages | The gold standard for reproducibility, but heavy and weak on Windows. Worth offering as an optional backend, not the default. |
| **mise** (`mise.toml`) | per-project tool versions (node, python, go…) + env + tasks; native on macOS/Linux/Windows | **Closest match.** Toolchain pinning, `[env]` and `[tasks]` are the right vocabulary, and it already runs on all three OSes. Could be the toolchain resolver. |
| **pkgx** | runs any tool on demand without installing it | Lazily provisioning runtimes on first run. |
| **Homebrew Brewfile** | declarative system packages | "Host requirements" that Harbor checks but doesn't install. |
| **uv + PEP 723** | inline `# /// script` dependency metadata in a single `.py`; `uv run` builds a cached venv | **The model for single-file Harbor scripts:** metadata in a header comment, so one file is the whole package. |
| **Deno / Bun `compile`** | a script compiled into one self-contained binary | The "freeze" output: ship a binary, not a runtime. |
| **process-compose / Procfile / overmind** | YAML for local processes with dependencies, readiness probes, restart policies and a TUI | The run/stop/logs UX and the multi-process model (v2). |
| **Tauri / Electron builders**, **AppImage / Flatpak**, **MSIX** | per-OS app packaging and signing | Final "export as a real app" targets, one per OS, behind the translation layer. |
| **Docker** itself | images, layers, `run/stop/logs/ps`, Compose | Keep the verbs and the mental model (`build → run → ps → logs → stop`). Drop the daemon-in-a-VM. |

**Takeaway.** Nobody combines (a) uv-style single-file metadata, (b) mise-style toolchain pinning, (c) a real OS
sandbox derived from declared permissions, and (d) a desktop UI for run/stop/logs. That combination is Harbor's niche.

---

## 3. Where Harbor sits in DesktopGlass

```
 Swift UI ── terminal dock: program menu [Shell | Claude Code | Codex | … | ⚓ Harbor]
    │                         Harbor pane: session list · active agent PTY/stream · router bar
    │ JSON-RPC (dg-proto)     Harbor sheet: shipped bundles · run/stop · logs
    ▼
 dg engine ─┬─ core / index / mcp      (existing plan)
            ├─ apphost                 (APP_STANDARD: .dgapp)
            └─ harbor  ← new crate family
                 ├─ harbor-agents   provider adapters, sessions, event log (SQLite)
                 ├─ harbor-script   manifest parser + OrchestrateLang host (grants)
                 ├─ harbor-ship     bundle build, toolchains, sandbox, supervisor
                 └─ harbor-platform → dg-platform traits (sandbox, process, paths)
            ▲
 dg harbor … one-shot CLI (run, ps, logs, stop, pack, agents)  ← also usable by agents themselves
```

**Fit with the existing plans**
- `Harness.swift` + `HarnessDetector` already detect all 12 backends, and `dg-index/harness.rs` (Phase 2) ports
  that. Harbor builds on the same catalogue rather than starting a second one.
- `TerminalProgram` (`Sources/DesktopGlass/Editor/Terminal.swift`) is `shell | harness(Harness)`. Harbor becomes a
  third case, `.harbor`, in the same `Menu`, so the user flow is unchanged.
- `ConversationHistory` (`Editor/Conversations.swift`) already reads each CLI's on-disk sessions for resume. Harbor
  sessions add a native record on top of that.
- The Seatbelt profile generator that runs agent CLIs during reindex (Phase 2, `dg-platform`) is the sandbox the
  shipper reuses.
- `.dgapp` apps already run as separate processes under per-app Seatbelt, with `terminal/open` and `mcp/register`.
  Harbor is just more host services on the same `dgapp/1` socket.

---

## 4. Components

### 4a. Meta harness

**What it does:** one supervisor for many agents, so the user (or a script) can talk to any of them, route work
between them and see everything in one place.

**Adapters (one Rust trait, one impl per backend):**

```rust
trait AgentAdapter {
    fn id(&self) -> &'static str;                 // "codex", "claude", "ollama", …
    fn capabilities(&self) -> Caps;               // resume, approvals, images, tools, cost reporting
    async fn start(&self, spec: SessionSpec) -> Result<Session>;   // cwd, model, sandbox, env
    async fn send(&self, s: &Session, turn: Turn) -> Result<()>;   // user message / attachments
    async fn interrupt(&self, s: &Session) -> Result<()>;
    async fn respond(&self, s: &Session, approval: ApprovalReply) -> Result<()>;
    fn events(&self, s: &Session) -> BoxStream<AgentEvent>;        // normalized: text delta, tool call, diff, approval req, done, error, usage
    async fn stop(&self, s: &Session) -> Result<()>;
}
```

| Backend | Transport Harbor would use | Notes |
|---|---|---|
| Codex | `codex app-server` (JSON-RPC over stdio) | richest structured protocol; T3 Code's primary path |
| Claude Code | `claude -p --output-format stream-json --input-format stream-json` (or Agent SDK) | resume via `--resume`; permission prompts via the permission-prompt tool |
| Antigravity, Gemini, other ACP agents | **one generic ACP (Agent Client Protocol) adapter** over stdio, where the CLI speaks ACP | T3 Code covers Antigravity/Grok/Kiro this way; verify each CLI's ACP flag at H1 |
| OpenCode | `opencode serve` (HTTP) | as T3 Code does |
| Copilot, Cursor | headless/JSON mode if usable from Rust (Cursor's is an SDK); otherwise **PTY mode** | PTY mode = today's terminal, recorded but not parsed |
| Ollama, LM Studio, llama.cpp, MLX | OpenAI-compatible HTTP (already the plan in Phase 2) | Harbor supplies the agent loop (tools = MCP `dg mcp` + a small file/shell toolset, sandboxed) |
| Apple Foundation Models | the app binary's `--foundation-models` entry point | Mac only |

**Two modes per session, so nothing is lost:**
- **Structured.** Harbor parses events, and the pane renders a compact transcript with tool calls, diffs and
  approvals.
- **Raw PTY.** The CLI's own TUI runs in a pane exactly as today, and Harbor only records start/stop/cwd/resume ID.
  This is the fallback for CLIs with no headless protocol, and for users who prefer the native TUI.

**Sessions.** A session is (adapter, model, cwd, worktree?, sandbox, parent run?). Events are appended to a SQLite
event log in app support (`harbor.db`) and projected into "threads". Resume uses the CLI's own resume ID, which
`ConversationResume` already models.

**Routing.**
1. **Manual:** pick an agent per message (`@codex`, `@claude`, `@local`).
2. **Rules:** a small table in settings (task type, privacy flag or folder → agent). For example, anything in a
   folder marked private goes only to on-device models, which reuses the index's `private` preference.
3. **Scripted:** an OrchestrateLang program decides (§4b).
4. **Fan-out / compare:** send one prompt to N agents in separate git worktrees, then diff or merge the results.

**Harbor pane (Swift):** a left rail of sessions (agent icon, state, cost), a center that shows the active session
(structured transcript or SwiftTerm PTY), and a bottom router bar (`@agent` picker, fan-out toggle, approval chips).
The pane is a `TerminalDock` client like today's panel, so it docks and collapses the same way.

### 4b. Harbor script: three options

The real decision is **how much OrchestrateLang is the script itself and how much it is a layer underneath.**

#### Option A: Manifest-first (`harbor.toml`), OrchestrateLang optional
A declarative file covers 80% of cases: which agents, which steps, the order, and the policies. An `.orch` file is
added only when logic is needed.

```toml
# review.harbor.toml
[harbor]
name = "pr-review"
version = 1

[agents]
fast  = { use = "claude", model = "sonnet" }
deep  = { use = "codex",  model = "gpt-5.5" }
local = { use = "ollama", model = "qwen2.5:7b" }   # private files only

[[step]]
id = "triage"
agent = "fast"
prompt = "Summarize the diff in {{git.diff}} and list risky files."

[[step]]
id = "review"
needs = ["triage"]
fan_out = ["fast", "deep"]          # both, in separate worktrees
prompt = "Review {{triage.risky_files}} for bugs."
restart = 1
timeout = "10m"

[[step]]
id = "merge"
needs = ["review"]
agent = "deep"
prompt = "Merge these reviews into one list: {{review.*}}"
```
- **Pros:** instant to run (no compile), easy for UI and agents to generate, and a good fit with `.dgapp` manifests.
- **Cons:** a second mini-language for conditions creeps in over time, and it leaves out the user's own language.

#### Option B: OrchestrateLang-first (`.orch` with a `harbor` module)
Harbor publishes an OrchestrateLang module (`use module harbor: prom "harbor"`). Agents are serverlets whose bodies
the engine implements. The script is compiled once and cached.

```orchestrate
use module harbor: prom "harbor"

serverlet Claude via harbor(agent: "claude", model: "sonnet", budget: "10m") {
    on ask(prompt: string) -> result<string>
}
serverlet Codex via harbor(agent: "codex") {
    on ask(prompt: string) -> result<string>
}

let reviewer = on diff_ready(diff: string) {
    let c = start Claude()
    let x = start Codex()
    let (a, b) = parallel { c.ask("Review:\n{diff}"), x.ask("Review:\n{diff}") }
    trigger reviewed(harbor.merge_reviews([a?, b?]))
}

orchestrator main() {
    trigger diff_ready(harbor.git_diff())
}
```
- `via harbor(...)` would be a **new serverlet boundary** in the language: a landline whose far end is `dg engine`.
  Until that exists, it can be done with a `secret`/Rust `load_foreign` shim calling `dg harbor` over the control
  socket.
- **Pros:** real types, supervision, `restart:`, `budget:`, `measure` and `grant call`, all already built and
  tested. It uses the user's language, and it improves the language too.
- **Cons:** needs Cargo plus a Rust toolchain and a slow first build, so it isn't "scripting" for end users. It also
  needs a language change (`via harbor`) or a shim.

#### Option C: Hybrid (recommended for discussion)
`harbor.toml` is the **package and permission manifest** (what the shipper and `.dgapp` read). Logic is either
**inline steps** (Option A) or **`logic = "flow.orch"`**, which DesktopGlass precompiles to a **wasm guest**
(`orchestrate build --guest`) and loads into `dg engine` with OrchestrateLang's own `start X from path` model. The
engine is the host, and every Harbor capability is a `grant call`: `grant call harbor.ask`, `grant call
harbor.git_diff`.
- Script authors need the toolchain once at build time (`dg harbor pack`). Users who **run** a shipped Harbor bundle
  don't need it, because the `.wasm` is in the bundle and it is cross-platform by construction.
- The grants are the permission list, so the manifest and the code agree, and `orchestrate check` already refuses an
  ungranted call.
- **Blocker:** the sandbox ABI's lack of `string[]` and strings in structs (gap 3). Either close it in
  OrchestrateLang (best for the language anyway) or pass JSON strings through grants for v1.
- Also needs the guest host side (the wasmtime linker with OrchestrateLang's `orch_interface`) reimplemented or
  exposed as a crate that `dg engine` can link. That means OrchestrateLang ships a small `orchestrate-guest-host`
  crate.

### 4c. Shipper ("Docker, but local")

**Unit:** a **Harbor bundle**, `Name.harbor/`, a folder like `.dgapp`. A single-file script is a valid bundle when
its metadata sits in a header comment (PEP 723 style).

```toml
# harbor.toml  (bundle root)
[bundle]
id       = "com.manny.photo-sorter"
name     = "Photo Sorter"
version  = "0.3.0"
kind     = "service"        # "script" (runs to completion) | "service" (long-running) | "app" (has a UI → .dgapp)

[runtime]                   # pinned toolchains, resolved per-OS by the toolchain provider (mise / uv / pkgx / nix)
python = "3.12.7"
node   = "22.11.0"
lock   = "harbor.lock"      # exact URLs + sha256 per OS/arch, written by `dg harbor lock`

[deps]
python = "requirements.txt" # or pyproject / uv.lock
node   = "package-lock.json"

[run]
cmd        = ["python", "-m", "sorter", "--watch", "{{data}}/inbox"]
cwd        = "."
env        = { SORTER_MODE = "fast" }
restart    = "on-failure"    # never | on-failure | always   (maps to OrchestrateLang restart policies)
ready      = { http = "http://127.0.0.1:{{port.web}}/health" }
ports      = { web = "auto" }   # Harbor assigns; shown in DesktopGlass's local-servers list

[hooks]
build = ["pip install -r requirements.txt"]
pre_start = []

[permissions]               # same vocabulary as .dgapp (APP_STANDARD §7); compiled to Seatbelt / later AppContainer / Landlock
network = ["api.openai.com"]
files   = ["pick", "desktop:read"]
data    = true              # a private writable dir, ~/Library/Application Support/DesktopGlass/Harbor/data/<id>
agents  = ["claude", "local"]   # may start agent sessions (§4d)

[platforms]                 # what this bundle claims to support; the translation layer fills gaps later
macos   = "native"
linux   = "native"
windows = "translate"       # needs the translation layer
```

**Isolation model: layered, with no VM**

| Layer | v1 (macOS) | Later |
|---|---|---|
| Filesystem | the bundle is read-only; a private `data/` dir; `files:` grants | Linux: Landlock + mount namespace; Windows: AppContainer |
| Network | Seatbelt allow-list from `network:` | Linux: netns + proxy; Windows: AppContainer capabilities |
| Toolchain | pinned, content-hashed, shared cache (`Harbor/toolchains/<name>-<ver>-<sha>`) | the same cache per OS |
| Dependencies | per-bundle env (venv, `node_modules`), built once from the lockfile | optional Nix backend for full reproducibility |
| Process | supervised by `dg engine` (restart policy, ready probe, log ring buffer) | cgroups / Job Objects for CPU and memory limits |
| Strongest | `kind = "sandbox"` for **wasm** bundles (wasmtime, which OrchestrateLang already wraps) | the only truly portable tier: the same file on all three OSes |

**CLI / UX (Docker verbs, deliberately)**

| Command | Does |
|---|---|
| `dg harbor init [path]` | writes `harbor.toml`, detecting the runtime from `package.json`, `pyproject.toml`, `Cargo.toml` and friends (the scanner already reads these hints) |
| `dg harbor lock` | resolves toolchains and deps to exact hashes for every target platform |
| `dg harbor build` | provisions the toolchain and installs deps into the bundle's env |
| `dg harbor run [--detach]` / `stop` / `restart` | supervised run |
| `dg harbor ps` / `logs [-f]` | status and logs (also in the Harbor sheet and the terminal pane) |
| `dg harbor pack` → `Name.harbor.zip` | source plus lockfile plus precompiled `.wasm` logic; **no toolchain inside**, which keeps it small |
| `dg harbor pack --standalone` | also embeds toolchains for one OS/arch (bigger, but it works offline) |
| `dg harbor install <zip\|url\|git>` | permission sheet, then install, same as `.dgapp` |

**UI:** a Harbor sheet in the island lists installed bundles as cards (status dot, port link, CPU/memory, last log
line) with Run / Stop / Logs / Open. Running services also appear in the existing local-servers list, and logs open
in the terminal dock. Dropping a project folder on the sheet runs `dg harbor init`.

**"Just works" means:** (1) the lockfile pins everything per platform, (2) the toolchain is fetched on first run and
cached, (3) ports and data dirs are assigned by Harbor rather than hardcoded, (4) a readiness check means the UI's
"Open" button only lights up when the app is really up, and (5) a `dg harbor doctor` explains anything missing in
plain words.

### 4d. Extensions (`.dgapp`) using Harbor

Consistent with APP_STANDARD.md: new **capabilities** for what an app may ask for and new **permissions** for what
the user approves, all enforced by the host and never by the SDK.

```toml
# dgapp.toml additions
[capabilities]
harbor_agents = true        # may open agent sessions
harbor_ship   = true        # may run/stop bundles it ships or the user picks

[permissions.harbor]
agents   = ["claude", "codex", "local"]  # "local" = any on-device backend; "*" asks per session
spend    = "ask"                         # "ask" | "never" | { daily_usd = 2 } — gates paid CLIs
cwd      = ["pick", "app-data"]          # where agent sessions may run
bundles  = ["embedded"]                  # "embedded" = only bundles inside this .dgapp; "pick" = user-chosen

[[harbor.bundle]]          # an app can carry Harbor bundles (its backend) inside its package
path = "harbor/backend"    # → started with the app, stopped with it, sandboxed by its own harbor.toml
```

**New `dgapp/1` methods** (types in `dg-proto`, next to APP_STANDARD §3):

| Call | Notes |
|---|---|
| `harbor/session.open { agent, model?, cwd, mode: "structured"\|"pty", pane? }` → `{ session }` | `pane` puts it in a terminal tab owned by the app (APP_STANDARD §3.3) |
| `harbor/session.send { session, text, attachments? }`, `.interrupt`, `.stop` | |
| event `harbor/session.event { session, event }` | normalized `AgentEvent` |
| `harbor/approval.request` → host UI | **approvals always go to the user**, never to the app |
| `harbor/bundle.run { id }`, `.stop`, `.status`, `.logs` | only bundles allowed by `permissions.harbor.bundles` |
| `harbor/script.run { path, args }` | runs a Harbor script under the app's grants ∩ the script's |

**Rules**
- An app never sees CLI credentials. Sessions use the user's own logins inside the engine, which answers APP_STANDARD
  open question 2 ("Agents + apps").
- Every paid-agent session started by an app shows a badge with the app's name and a running cost.
- `.dgapp` apps whose backend is a Harbor bundle make "ship a local app that just works" concrete: the UI is the
  `.dgapp` and the backend is a sandboxed Harbor bundle with pinned runtimes.

### 4e. Cross-platform and the translation layer

The translation layer lives **under** Harbor, not inside it. Harbor talks only to traits, and the translation layer
is one more implementation.

| Trait (in `dg-platform` / `harbor-platform`) | macOS v1 | Linux | Windows | Translation layer hook |
|---|---|---|---|---|
| `Sandbox::spawn(profile, cmd)` | Seatbelt | Landlock + namespaces / bubblewrap | AppContainer / restricted token | runs a foreign-OS binary inside the target's sandbox |
| `Toolchain::resolve(name, ver, target)` | mise/uv/pkgx download | same | same (`.exe`, `PATHEXT`) | maps "needs macOS tool X" to an equivalent |
| `Paths` (data, cache, desktop) | `~/Library/…` | XDG | Known Folders | — |
| `Process` (signals, groups, PTY) | `posix_spawn`, `forkpty` | same | Job Objects, ConPTY | — |
| `Ports` / `Servers` | `lsof` | `/proc/net` | `GetExtendedTcpTable` | — |
| `Exec format` | Mach-O | ELF | PE | **the main hook:** `[platforms] windows = "translate"` routes the run through it |

**What to abstract now (cheap) so it's possible later (expensive):**
1. Never write `/bin/zsh`, `open`, `lsof` or `sandbox-exec` in Harbor code. Use traits only.
2. `harbor.toml` paths are POSIX-relative and normalized per OS. Commands are **argv arrays, not shell strings**.
3. The lockfile is keyed by `os-arch` triples (`aarch64-apple-darwin`, `x86_64-pc-windows-msvc`, …).
4. Permissions are a platform-neutral vocabulary (already true in APP_STANDARD), compiled per OS.
5. Treat **wasm bundles** as the first portable tier. They work on every OS today with no translation.
6. A bundle declares `[platforms]` honestly (`native` / `translate` / `unsupported`), so the translation layer has
   one switch point.

---

## 5. Phases

| Phase | What | Depends on | Size |
|---|---|---|---|
| **H0 Spec** | `harbor.toml` schema, `AgentEvent` and session types in `dg-proto`, this doc → reference | Rust Phase 0 | 2–3 days |
| **H1 Harness core** | `harbor-agents`: adapter trait; **Codex app-server** + **Claude stream-json** + **generic ACP** + one OpenAI-HTTP local adapter; SQLite command/event/projection store (T3 pattern); `dg harbor agents` CLI usable before the daemon exists | Rust Phase 2 (`harness.rs`, CLI discovery, preflight) | 6–8 days |
| **H2 Harbor pane** | `TerminalProgram.harbor`, session rail, structured transcript, PTY fallback for the other CLIs, approvals, manual `@agent` routing | **Rust Phase 5** (`dg engine`, `EngineClient`) | 5–7 days |
| **H3 Script v1** | inline steps (Option A), fan-out with git worktrees, rules routing | H1 | 4–5 days |
| **H4 Shipper v1 (macOS)** | bundles, `init/lock/build/run/stop/ps/logs/pack/install`, toolchain cache (wrap mise/uv), Seatbelt from permissions, Harbor sheet | Rust Phase 2 Seatbelt in `dg-platform`; Phase 5 for UI | 8–10 days |
| **H5 OrchestrateLang logic** | Option C: guest-host crate, `grant call harbor.*`, `dg harbor pack` precompiles `.wasm`; *or* Option B's `via harbor` boundary | OrchestrateLang work (ABI strings; host crate) | 6–10 days (+ language work) |
| **H6 Extensions** | `harbor_*` capabilities and permissions, `harbor/*` methods, embedded bundles in `.dgapp` | APP_STANDARD A1–A2, A6 | 4–5 days |
| **H7 Linux/Windows** | `harbor-platform` impls; wasm bundles first; translation-layer hook | Rust Phase 6, WINDOWS_UI | follows those plans |

**macOS total, H0–H6:** roughly **7–9 weeks** for one developer, after Rust Phase 5. H1 and H3 can start right
after Rust Phase 2 as CLI-only features (`dg harbor …`), so they're useful from a terminal before the pane exists.

---

## 6. Risks

| Risk | Mitigation |
|---|---|
| Agent CLIs change their headless protocols often (Codex app-server is young; Claude stream-json evolves) | adapter per CLI with captured-output fixtures (the same approach as the reindex preflight tables), PTY fallback always available, and the CLI version pinned in tests |
| Running paid agents in parallel burns money silently | per-session cost from usage events, a spend cap per session/script/app, fan-out confirmation |
| Two agents editing one checkout | fan-out always uses separate git worktrees; one writer per worktree |
| Unattended agents doing damage | reuse the reindex consent model + Seatbelt; approvals always surface in the UI |
| OrchestrateLang's compile model makes scripts feel slow | Option C (precompiled wasm), or inline steps for quick scripts |
| "Like Docker" sets expectations of perfect isolation | say plainly what the sandbox does and doesn't contain (OrchestrateLang's "be honest about guarantees" principle); wasm tier for real containment |
| Toolchain downloads (supply chain) | hashes in the lockfile; only known mirrors; show what's downloaded |
| Scope creep into a compose/k8s clone | v1 is single-process bundles; multi-service is a separate, later decision |
| Copying T3 Code's code without care | see §2.1 license notes; port ideas, keep attribution for anything copied |

---

## 7. Open questions for you

1. **What is Harbor's center of gravity:** the multi-agent harness or the shipper? They share an engine, but v1 can
   only lead with one. My lean is the harness, because the shipper is more valuable once agents can use it.
2. **Structured or PTY first?** Do you want Harbor to *replace* each CLI's own TUI with a unified transcript
   (T3 Code style), or keep the native TUIs and add orchestration around them? This decides most of the UI work.
3. **How central is OrchestrateLang?** (a) the only script format, (b) the logic layer under a TOML manifest
   (Option C), or (c) an optional power-user path? And are you willing to change the language for Harbor (a
   `via harbor` boundary, strings in the sandbox ABI, a reusable guest-host crate)?
4. **Who writes Harbor scripts?** You, other developers, agents themselves (agents writing scripts that run agents),
   or non-coders through a UI? This decides between TOML/visual and `.orch`.
5. **What does "reverse-engineer T3" mean to you?** Port its *architecture* (adapters, event log, protocol) into
   Rust, reuse its MIT-licensed TypeScript server as a sidecar, or match its *features* (worktrees, diffs,
   checkpoints)?
6. **Shipper target user:** shipping *your own* projects between your machines, or handing a bundle to someone who
   has never seen it? The second needs `--standalone` bundles, signing and a much better `doctor`.
7. **Is Docker-level isolation a requirement or a nice-to-have?** If it's required, the wasm tier (or a real
   VM/container backend) becomes the default rather than Seatbelt-plus-pinned-toolchains.
8. **Toolchain provider:** wrap existing tools (mise/uv/pkgx, fast, with external dependencies) or own the
   resolver (slower, fully controlled, needed for the translation layer anyway)?
9. **The translation layer:** what does it translate? Binaries (a Rosetta/Wine-like layer), APIs (a portability
   library apps link against), or Harbor scripts/manifests? Where it plugs in (§4e) depends on that answer.
10. **Should Harbor stay reusable outside DesktopGlass,** for example in Ackee's IDE, which plans an agent harness?
    If so, the `harbor-*` crates must not depend on `dg-*`.
11. **Spend and approvals:** what should an unattended Harbor script be allowed to do without asking, across paid
    CLIs, file writes, network and git push?
