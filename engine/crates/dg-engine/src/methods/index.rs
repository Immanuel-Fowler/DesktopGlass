//! Re-indexing and backends (was Reindexer.swift, the schedule timer in AppDelegate.swift, and the detection /
//! model-list half of Harness.swift).
//!
//! Runs are `dg reindex` child processes (this same binary), so a run's stdout status lines, `reindex.log`, the
//! lock and the exit codes stay exactly what Phase 2 defined, and a run can be cancelled by killing its process
//! group. The engine interprets the exit code into a status line and an [`Problem`], keeps
//! `lastReindexRun` / `lastReindexSuccess` / `signedOutHarness` in settings.json, and runs the schedule itself.
//!
//! **UI link methods**
//! - `index/status {}` → `{running, checkingAuth, status, problem, scheduleBlocked, lastRun, lastSuccess}`
//! - `index/run {reason, full?, allowUnattended?}` → `{started, running, status, problem}`; `allowUnattended`:
//!   the user just agreed in the app's consent dialog, so `unattended.<backend>` is turned on first
//! - `index/check_auth {}` → same shape (runs `dg reindex --auth-only`)
//! - `index/cancel {}` → `{cancelled: bool}`
//! - `backends/detect {appleAvailable?, glassBin?}` → `{installed: {backend: path}}`
//! - `models/list {harness, force?, binary?}` → `{harness, models: [..], live: bool}`
//! - `models/presets {}` → `{backend: [..]}`
//!
//! **Notifications** — `index/progress {running, checkingAuth, status}`, `index/problem {problem}` and
//! `index/finished {code, status, authOnly}`; `problem` is `null` or `{kind, harness?, message}` with kind one of
//! signedOut, notInstalled, limited, notRunning, needsConsent, failed.
//!
//! **Control methods** (same params, results and errors as AppControl.swift had): `app.status`, `schedule.get`,
//! `schedule.set`, `index.run`, `backend.set`.

pub mod backends;
pub mod schedule;

use crate::Engine;
use dg_index::exit;
use schedule::Schedule;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

/// UI-link method names.
pub mod ui {
    pub const STATUS: &str = "index/status";
    pub const RUN: &str = "index/run";
    pub const CHECK_AUTH: &str = "index/check_auth";
    pub const CANCEL: &str = "index/cancel";
    pub const DETECT: &str = "backends/detect";
    pub const MODELS: &str = "models/list";
    pub const PRESETS: &str = "models/presets";
}

/// Notification names.
pub mod notify {
    pub const PROGRESS: &str = "index/progress";
    pub const PROBLEM: &str = "index/problem";
    pub const FINISHED: &str = "index/finished";
}

/// Reasons that come from the user clicking something (the app asks for consent itself before these).
pub const INTERACTIVE_REASONS: &[&str] = &["button", "menu", "settings", "settings-rewrite"];

/// What went wrong on the last run, if anything the user should act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    SignedOut(String),
    NotInstalled(String),
    Limited(String),
    NotRunning(String),
    NeedsConsent(String),
    Failed(String),
}

impl Problem {
    pub fn message(&self) -> String {
        match self {
            Problem::SignedOut(h) => format!("Not signed in to {}", backends::label(h)),
            Problem::NotInstalled(h) => format!("{} isn't installed", backends::label(h)),
            Problem::Limited(h) => format!("{} hit a usage limit", backends::label(h)),
            Problem::NotRunning(h) => format!("{} isn't ready", backends::label(h)),
            Problem::NeedsConsent(h) => format!("Unattended runs with {} are off", backends::label(h)),
            Problem::Failed(m) => m.clone(),
        }
    }

    pub fn to_json(&self) -> Value {
        let (kind, h) = match self {
            Problem::SignedOut(h) => ("signedOut", Some(h)),
            Problem::NotInstalled(h) => ("notInstalled", Some(h)),
            Problem::Limited(h) => ("limited", Some(h)),
            Problem::NotRunning(h) => ("notRunning", Some(h)),
            Problem::NeedsConsent(h) => ("needsConsent", Some(h)),
            Problem::Failed(_) => ("failed", None),
        };
        let mut o = json!({"kind": kind, "message": self.message()});
        if let Some(h) = h {
            o["harness"] = json!(h);
        }
        o
    }
}

/// How runs are started (tests swap in a fake `dg reindex`).
#[derive(Debug, Clone)]
pub struct Config {
    /// The program and leading arguments: `<current exe> reindex` by default.
    pub program: PathBuf,
    pub args: Vec<String>,
    /// Extra environment for runs.
    pub env: Vec<(String, String)>,
    /// The Desktop (for "has it ever been indexed" and open questions).
    pub desktop: PathBuf,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            program: std::env::current_exe().unwrap_or_else(|_| PathBuf::from("dg")),
            args: vec!["reindex".into()],
            env: vec![],
            desktop: std::env::var_os("DESKTOP_INDEX_DESKTOP").map(PathBuf::from).unwrap_or_else(dg_platform::desktop),
        }
    }
}

#[derive(Default)]
struct RunState {
    running: bool,
    checking_auth: bool,
    status: String,
    problem: Option<Problem>,
    child: Option<i32>,
    cancelled: bool,
}

/// One engine's re-indexing state.
pub struct Indexer {
    cfg: Mutex<Config>,
    st: Mutex<RunState>,
    done: Condvar,
    /// Live model lists read so far.
    catalog: Mutex<HashMap<String, Vec<String>>>,
    /// The app binary that runs Apple Foundation Models (told by the app in `backends/detect`).
    glass_bin: Mutex<Option<String>>,
}

type Registry = Mutex<Vec<(Weak<Engine>, Arc<Indexer>)>>;

fn registry() -> &'static Registry {
    static R: OnceLock<Registry> = OnceLock::new();
    R.get_or_init(Default::default)
}

/// This engine's indexer (created on first use, with the sign-in problem that survives restarts).
pub fn indexer(engine: &Arc<Engine>) -> Arc<Indexer> {
    let mut reg = registry().lock().unwrap();
    reg.retain(|(w, _)| w.strong_count() > 0);
    if let Some((_, ix)) = reg.iter().find(|(w, _)| w.as_ptr() == Arc::as_ptr(engine)) {
        return ix.clone();
    }
    let mut st = RunState::default();
    if let Some(h) = engine.settings.string("signedOutHarness").filter(|h| backends::is_known(h)) {
        st.status = Problem::SignedOut(h.clone()).message();
        st.problem = Some(Problem::SignedOut(h));
    }
    let ix = Arc::new(Indexer {
        cfg: Mutex::new(Config::default()),
        st: Mutex::new(st),
        done: Condvar::new(),
        catalog: Mutex::new(HashMap::new()),
        glass_bin: Mutex::new(None),
    });
    reg.push((Arc::downgrade(engine), ix.clone()));
    ix
}

/// Replaces how runs are started (tests).
pub fn configure(engine: &Arc<Engine>, cfg: Config) {
    *indexer(engine).cfg.lock().unwrap() = cfg;
}

// ---------- settings ----------

fn put(engine: &Arc<Engine>, values: Vec<(&str, Value)>) {
    let map: Map<String, Value> = values.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
    match engine.settings.set(&map) {
        Ok(changed) if !changed.is_empty() => {
            engine.link.notify(dg_proto::notify::SETTINGS_CHANGED, json!({"keys": changed}));
        }
        Ok(_) => {}
        Err(e) => engine.link.notify(dg_proto::notify::LOG, json!({"message": e})),
    }
}

pub fn current_harness(engine: &Engine) -> String {
    engine.settings.string("harness").filter(|h| backends::is_known(h)).unwrap_or_else(|| "claude".into())
}

pub fn model(engine: &Engine, h: &str) -> String {
    engine.settings.string(&format!("model.{h}")).unwrap_or_else(|| backends::default_model(h).into())
}

pub fn endpoint(engine: &Engine, h: &str) -> String {
    engine
        .settings
        .string(&format!("endpoint.{h}"))
        .unwrap_or_else(|| backends::default_endpoint(h).unwrap_or("").into())
}

pub fn unattended_allowed(engine: &Engine, h: &str) -> bool {
    backends::is_local(h) || engine.settings.bool(&format!("unattended.{h}"))
}

fn current_schedule(engine: &Engine) -> (Schedule, u32) {
    (
        Schedule::from_stored(engine.settings.string("schedule").as_deref()),
        schedule::hour_setting(engine.settings.value("scheduleHour")),
    )
}

fn iso_setting(engine: &Engine, key: &str) -> Value {
    schedule::date_setting(engine.settings.value(key)).map(|t| json!(schedule::iso(t))).unwrap_or(Value::Null)
}

/// Scheduled runs pause while the selected CLI is known to be signed out, or isn't allowed to run unattended.
pub fn schedule_blocked(engine: &Arc<Engine>) -> bool {
    let h = current_harness(engine);
    if !unattended_allowed(engine, &h) {
        return true; // never ask from a scheduled run
    }
    matches!(&indexer(engine).st.lock().unwrap().problem, Some(Problem::SignedOut(p)) if *p == h)
}

// ---------- state + notifications ----------

fn problem_json(p: &Option<Problem>) -> Value {
    p.as_ref().map(|p| p.to_json()).unwrap_or(Value::Null)
}

fn send_progress(engine: &Arc<Engine>, st: &RunState) {
    engine.link.notify(
        notify::PROGRESS,
        json!({"running": st.running, "checkingAuth": st.checking_auth, "status": st.status}),
    );
}

fn set_problem(engine: &Arc<Engine>, st: &mut RunState, p: Option<Problem>) {
    match &p {
        Some(Problem::SignedOut(h)) => put(engine, vec![("signedOutHarness", json!(h))]),
        _ => put(engine, vec![("signedOutHarness", Value::Null)]),
    }
    st.problem = p;
    engine.link.notify(notify::PROBLEM, json!({"problem": problem_json(&st.problem)}));
}

pub fn status_json(engine: &Arc<Engine>) -> Value {
    let blocked = schedule_blocked(engine);
    let ix = indexer(engine);
    let st = ix.st.lock().unwrap();
    json!({
        "running": st.running,
        "checkingAuth": st.checking_auth,
        "status": st.status,
        "problem": problem_json(&st.problem),
        "scheduleBlocked": blocked,
        "lastRun": iso_setting(engine, "lastReindexRun"),
        "lastSuccess": iso_setting(engine, "lastReindexSuccess"),
    })
}

/// Turns a run's exit code and last output line into status + problem (Reindexer.swift's `handle`).
fn interpret(engine: &Arc<Engine>, st: &mut RunState, h: &str, code: i32, line: &str, auth_only: bool) {
    let or = |m: String| if line.is_empty() { m } else { line.to_string() };
    let now = json!(schedule::iso(schedule::now()));
    match code {
        exit::OK => {
            set_problem(engine, st, None);
            st.status = if line.is_empty() {
                if auth_only { format!("Signed in to {}", backends::label(h)) } else { "Indexed".into() }
            } else {
                line.to_string()
            };
            if !auth_only {
                put(engine, vec![("lastReindexSuccess", now)]);
            }
        }
        exit::UP_TO_DATE => {
            st.status = or("Up to date".into());
            put(engine, vec![("lastReindexSuccess", now)]);
        }
        exit::BUSY => st.status = "Already indexing".into(),
        _ => {
            let p = match code {
                exit::LIMITED => Problem::Limited(h.into()),
                exit::NEEDS_CONSENT => Problem::NeedsConsent(h.into()),
                exit::NOT_READY => Problem::NotRunning(h.into()),
                exit::SIGNED_OUT => Problem::SignedOut(h.into()),
                exit::NOT_INSTALLED => Problem::NotInstalled(h.into()),
                _ => Problem::Failed(or(format!("Failed ({code}) — see reindex.log"))),
            };
            st.status = or(p.message());
            set_problem(engine, st, Some(p));
        }
    }
}

// ---------- runs ----------

/// Starts `dg reindex` (or `--auth-only`) for the current backend; returns (exit code, last output line).
fn launch(engine: &Arc<Engine>, ix: &Indexer, h: &str, extra: &[&str], reason: &str, full: bool) -> (i32, String) {
    let cfg = ix.cfg.lock().unwrap().clone();
    let mut c = Command::new(&cfg.program);
    c.args(&cfg.args).args(extra);
    c.env("DESKTOP_INDEX_HARNESS", h)
        .env("DESKTOP_INDEX_MODEL", backends::sanitize_model(&model(engine, h)))
        .env("DESKTOP_INDEX_REASON", reason)
        .env("DESKTOP_INDEX_UNATTENDED", if unattended_allowed(engine, h) { "1" } else { "0" })
        .env("DESKTOP_INDEX_SANDBOX", if engine.settings.bool("sandboxUnattended") { "1" } else { "0" });
    if backends::uses_server(h) {
        c.env("DESKTOP_INDEX_ENDPOINT", backends::sanitize_endpoint(&endpoint(engine, h)));
    }
    if let Some(g) = ix.glass_bin.lock().unwrap().clone() {
        c.env("DESKTOP_GLASS_BIN", g);
    }
    if full {
        c.env("DESKTOP_INDEX_FULL", "1");
    }
    c.envs(cfg.env.iter().map(|(k, v)| (k, v)));
    // the CLI's own output goes to reindex.log; only status lines come back, stdout and stderr together
    let (mut reader, writer) = match std::io::pipe() {
        Ok(p) => p,
        Err(e) => return (126, format!("Couldn't start: {e}")),
    };
    let Ok(w2) = writer.try_clone() else { return (126, "Couldn't start: no pipe".into()) };
    c.stdin(Stdio::null()).stdout(writer).stderr(w2);
    {
        use std::os::unix::process::CommandExt;
        c.process_group(0); // so a cancel reaches the agent CLI too
    }
    let child = c.spawn();
    drop(c); // closes our copies of the pipe's write end
    let mut child = match child {
        Ok(ch) => ch,
        Err(e) => return (126, format!("Couldn't start: {e}")),
    };
    ix.st.lock().unwrap().child = Some(child.id() as i32);
    let mut out = Vec::new();
    let _ = reader.read_to_end(&mut out);
    let code = child.wait().map(dg_index::process::shell_code).unwrap_or(1);
    ix.st.lock().unwrap().child = None;
    let text = String::from_utf8_lossy(&out);
    let last = text.split('\n').filter(|l| !l.is_empty()).last().unwrap_or("").to_string();
    (code, last)
}

fn finish(engine: &Arc<Engine>, ix: &Indexer, code: i32, auth_only: bool) {
    let st = ix.st.lock().unwrap();
    send_progress(engine, &st);
    engine.link.notify(notify::FINISHED, json!({"code": code, "status": st.status, "authOnly": auth_only}));
    ix.done.notify_all();
}

/// Starts a re-index in the background (Reindexer.swift's `run(reason:full:)`). Returns false if a run or sign-in
/// check is already going. A backend that needs consent for unattended runs gets the needsConsent problem instead.
pub fn run(engine: &Arc<Engine>, reason: &str, full: bool) -> bool {
    let ix = indexer(engine);
    let h = current_harness(engine);
    {
        let mut st = ix.st.lock().unwrap();
        if st.running || st.checking_auth {
            return false;
        }
        if !unattended_allowed(engine, &h) {
            // Schedule and MCP runs never ask; the app asks before interactive ones (allowUnattended).
            let p = Problem::NeedsConsent(h.clone());
            st.status = p.message();
            set_problem(engine, &mut st, Some(p));
            drop(st);
            finish(engine, &ix, exit::NEEDS_CONSENT, false);
            return true;
        }
        st.running = true;
        st.cancelled = false;
        st.status = "Indexing…".into();
        send_progress(engine, &st);
    }
    let (engine, reason) = (engine.clone(), reason.to_string());
    std::thread::spawn(move || {
        let ix = indexer(&engine);
        let (code, line) = launch(&engine, &ix, &h, &[], &reason, full);
        {
            let mut st = ix.st.lock().unwrap();
            st.running = false;
            put(&engine, vec![("lastReindexRun", json!(schedule::iso(schedule::now())))]);
            if st.cancelled {
                st.status = "Cancelled".into();
            } else {
                interpret(&engine, &mut st, &h, code, &line, false);
            }
        }
        finish(&engine, &ix, code, false);
    });
    true
}

/// Verifies the selected backend is signed in / ready (`dg reindex --auth-only`), in the background.
pub fn check_auth(engine: &Arc<Engine>) -> bool {
    let ix = indexer(engine);
    let h = current_harness(engine);
    {
        let mut st = ix.st.lock().unwrap();
        if st.running || st.checking_auth {
            return false;
        }
        st.checking_auth = true;
        st.cancelled = false;
        st.status = "Checking sign-in…".into();
        send_progress(engine, &st);
    }
    let engine = engine.clone();
    std::thread::spawn(move || {
        let ix = indexer(&engine);
        let (code, line) = launch(&engine, &ix, &h, &["--auth-only"], "auth-check", false);
        {
            let mut st = ix.st.lock().unwrap();
            st.checking_auth = false;
            if st.cancelled {
                st.status = "Cancelled".into();
            } else {
                interpret(&engine, &mut st, &h, code, &line, true);
            }
        }
        finish(&engine, &ix, code, true);
    });
    true
}

/// Stops the current run or check (its whole process group).
pub fn cancel(engine: &Arc<Engine>) -> bool {
    let ix = indexer(engine);
    let mut st = ix.st.lock().unwrap();
    let Some(pid) = st.child else { return false };
    st.cancelled = true;
    unsafe { libc::kill(-pid, libc::SIGTERM) };
    true
}

/// Waits until no run is going (or the timeout passes).
pub fn wait_idle(engine: &Arc<Engine>, timeout: Duration) {
    let ix = indexer(engine);
    let deadline = Instant::now() + timeout;
    let mut st = ix.st.lock().unwrap();
    while st.running {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        st = ix.done.wait_timeout(st, left.min(Duration::from_millis(500))).unwrap().0;
    }
}

// ---------- schedule ----------

/// One scheduler check: runs a re-index if one is due and nothing blocks it.
pub fn tick(engine: &Arc<Engine>, now: f64) -> bool {
    if schedule_blocked(engine) {
        return false; // signed out / no consent: wait for the user
    }
    let (s, hour) = current_schedule(engine);
    let last = schedule::date_setting(engine.settings.value("lastReindexRun"));
    s.is_due(last, now, hour) && run(engine, "schedule", false)
}

/// At launch: migrate old schedule values and count an existing index as a first run (so the schedule starts).
pub fn launch_bookkeeping(engine: &Arc<Engine>) {
    let stored = engine.settings.string("schedule");
    let normalized = Schedule::from_stored(stored.as_deref());
    if stored.as_deref() != Some(normalized.raw()) {
        put(engine, vec![("schedule", json!(normalized.raw()))]);
    }
    let desktop = indexer(engine).cfg.lock().unwrap().desktop.clone();
    let index = desktop.join(dg_core::rules::LOG_DIR).join("desktop-index.md");
    if engine.settings.value("lastReindexRun").is_none() && index.exists() {
        put(engine, vec![("lastReindexRun", json!(schedule::iso(schedule::now())))]);
    }
}

pub fn start(engine: &Arc<Engine>) {
    launch_bookkeeping(engine);
    let weak = Arc::downgrade(engine);
    std::thread::spawn(move || {
        // A minute is fine-grained enough, and catches up within a minute after the Mac wakes.
        let mut first = true;
        loop {
            if !first {
                std::thread::sleep(Duration::from_secs(60));
            }
            first = false;
            let Some(engine) = weak.upgrade() else { return };
            tick(&engine, schedule::now());
        }
    });
}

// ---------- control methods (AppControl.swift's answers) ----------

fn schedule_info(engine: &Arc<Engine>) -> Value {
    let (s, hour) = current_schedule(engine);
    json!({
        "schedule": s.raw(),
        "label": s.label(hour),
        "hour": hour,
        "paused_until_sign_in": schedule_blocked(engine),
        "last_run": iso_setting(engine, "lastReindexRun"),
    })
}

fn app_status(engine: &Arc<Engine>) -> Value {
    let h = current_harness(engine);
    let ix = indexer(engine);
    let (running, status, problem, desktop) = {
        let st = ix.st.lock().unwrap();
        let desktop = ix.cfg.lock().unwrap().desktop.clone();
        (st.running, st.status.clone(), st.problem.as_ref().map(|p| json!(p.message())).unwrap_or(Value::Null), desktop)
    };
    json!({
        "backend": h,
        "backend_label": backends::label(&h),
        "model": model(engine, &h),
        "running": running,
        "status": status,
        "problem": problem,
        "last_success": iso_setting(engine, "lastReindexSuccess"),
        "open_questions": dg_core::scan::open_questions(&desktop),
        "schedule": schedule_info(engine),
    })
}

fn run_result(engine: &Arc<Engine>, started: bool) -> Value {
    let ix = indexer(engine);
    let st = ix.st.lock().unwrap();
    json!({"started": started, "running": st.running, "status": st.status,
           "problem": st.problem.as_ref().map(|p| json!(p.message())).unwrap_or(Value::Null)})
}

fn int(v: Option<&Value>) -> Option<i64> {
    let v = v?;
    v.as_i64().or_else(|| v.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64))
}

fn control(engine: &Arc<Engine>, method: &str, p: &Value) -> Option<Result<Value, String>> {
    Some(match method {
        "app.status" => Ok(app_status(engine)),
        "schedule.get" => Ok(schedule_info(engine)),
        "schedule.set" => {
            let mut values = vec![];
            if let Some(raw) = p.get("schedule").and_then(|s| s.as_str()) {
                let Some(s) = Schedule::parse_alias(raw) else {
                    return Some(Err("Schedule must be off, hourly, every_6_hours or daily".into()));
                };
                values.push(("schedule", json!(s.raw())));
            }
            if let Some(h) = int(p.get("hour")) {
                values.push(("scheduleHour", json!(h.clamp(0, 23))));
            }
            put(engine, values);
            Ok(schedule_info(engine))
        }
        "index.run" => {
            if indexer(engine).st.lock().unwrap().running {
                return Some(Ok(json!({"started": false, "status": "Already indexing"})));
            }
            run(engine, "mcp", p.get("full").and_then(|f| f.as_bool()).unwrap_or(false));
            if p.get("wait").and_then(|w| w.as_bool()).unwrap_or(false) {
                let secs = int(p.get("timeout")).unwrap_or(600).max(0) as u64;
                wait_idle(engine, Duration::from_secs(secs));
            }
            Ok(run_result(engine, true))
        }
        "backend.set" => {
            let raw = p.get("backend").and_then(|b| b.as_str()).unwrap_or("").to_lowercase();
            let h = backends::ALL
                .iter()
                .find(|h| **h == raw)
                .or_else(|| backends::ALL.iter().find(|h| backends::label(h).to_lowercase() == raw));
            let Some(h) = h else {
                return Some(Err(format!("Backend must be one of: {}", backends::ALL.join(", "))));
            };
            let mut values = vec![("harness", json!(h))];
            let (mk, ek) = (format!("model.{h}"), format!("endpoint.{h}"));
            if let Some(m) = p.get("model").and_then(|m| m.as_str()) {
                values.push((mk.as_str(), json!(backends::sanitize_model(m))));
            }
            if let Some(e) = p.get("endpoint").and_then(|e| e.as_str()) {
                if backends::uses_server(h) {
                    values.push((ek.as_str(), json!(backends::sanitize_endpoint(e))));
                }
            }
            put(engine, values);
            Ok(app_status(engine))
        }
        _ => return None,
    })
}

// ---------- UI methods ----------

fn ui_method(engine: &Arc<Engine>, method: &str, p: &Value) -> Option<Result<Value, String>> {
    let ix = indexer(engine);
    Some(match method {
        ui::STATUS => Ok(status_json(engine)),
        ui::RUN => {
            let reason = p.get("reason").and_then(|r| r.as_str()).unwrap_or("button").to_string();
            if p.get("allowUnattended").and_then(|a| a.as_bool()).unwrap_or(false) {
                let h = current_harness(engine);
                put(engine, vec![(format!("unattended.{h}").as_str(), json!(true))]);
            }
            let started = run(engine, &reason, p.get("full").and_then(|f| f.as_bool()).unwrap_or(false));
            Ok(run_result(engine, started))
        }
        ui::CHECK_AUTH => {
            let started = check_auth(engine);
            Ok(run_result(engine, started))
        }
        ui::CANCEL => Ok(json!({"cancelled": cancel(engine)})),
        ui::DETECT => {
            let glass = p.get("glassBin").and_then(|g| g.as_str()).filter(|g| !g.is_empty()).map(String::from);
            if glass.is_some() {
                *ix.glass_bin.lock().unwrap() = glass.clone();
            }
            let apple_ok = p.get("appleAvailable").and_then(|a| a.as_bool()).unwrap_or(false);
            let apple = if apple_ok { glass.or_else(|| dg_index::harness::apple_bin(None)) } else { None };
            let home = dg_platform::home();
            let path = backends::search_path(&home);
            Ok(json!({"installed": backends::detect(&path, &home, apple.as_deref())}))
        }
        ui::MODELS => {
            let Some(h) = p.get("harness").and_then(|h| h.as_str()).filter(|h| backends::is_known(h)) else {
                return Some(Err(format!("harness must be one of: {}", backends::ALL.join(", "))));
            };
            let force = p.get("force").and_then(|f| f.as_bool()).unwrap_or(false);
            let cached = ix.catalog.lock().unwrap().get(h).cloned();
            let live = match cached {
                Some(list) if !force => list,
                _ => {
                    let home = dg_platform::home();
                    let binary = p.get("binary").and_then(|b| b.as_str()).map(String::from).or_else(|| {
                        let path = backends::search_path(&home);
                        let d = dg_index::harness::Discovery { path: &path, home: &home, glass_bin: None, python: None, mlx_server_up: false };
                        backends::list_args(h).and_then(|_| dg_index::harness::find_bin(h, &d))
                    });
                    let list = backends::live_models(h, binary.as_deref(), &endpoint(engine, h), &home);
                    if !list.is_empty() {
                        ix.catalog.lock().unwrap().insert(h.to_string(), list.clone());
                    }
                    list
                }
            };
            let is_live = !live.is_empty();
            let models = if is_live { json!(live) } else { json!(backends::presets(h)) };
            Ok(json!({"harness": h, "models": models, "live": is_live}))
        }
        ui::PRESETS => Ok(backends::presets_json()),
        _ => return None,
    })
}

pub fn handle(engine: &Arc<Engine>, method: &str, params: &Value) -> Option<Result<Value, String>> {
    if method.contains('/') {
        ui_method(engine, method, params)
    } else {
        control(engine, method, params)
    }
}

#[cfg(test)]
mod tests;
