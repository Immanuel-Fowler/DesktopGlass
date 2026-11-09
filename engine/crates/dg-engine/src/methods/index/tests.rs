//! Runs go to a fake `dg reindex` (a shell script), settings to a temp file; nothing real is indexed or launched.

use super::*;
use crate::settings::{tests::temp_home, Settings};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;

#[derive(Clone, Default)]
struct Buf(Arc<Mutex<Vec<u8>>>);
impl Write for Buf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl Buf {
    fn notes(&self, method: &str) -> Vec<Value> {
        String::from_utf8(self.0.lock().unwrap().clone())
            .unwrap()
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter(|m| m["method"] == json!(method))
            .map(|m| m["params"].clone())
            .collect()
    }
}

const FAKE: &str = r#"#!/bin/sh
echo "$DESKTOP_INDEX_HARNESS|$DESKTOP_INDEX_REASON|$DESKTOP_INDEX_MODEL|$DESKTOP_INDEX_UNATTENDED|$DESKTOP_INDEX_FULL|$DESKTOP_INDEX_ENDPOINT|$*" >> "$FAKE_LOG"
[ -n "$FAKE_SLEEP" ] && sleep "$FAKE_SLEEP"
[ -n "$FAKE_LINE" ] && { echo "Scanning…"; echo "$FAKE_LINE" >&2; }
exit "${FAKE_CODE:-0}"
"#;

struct T {
    engine: Arc<Engine>,
    buf: Buf,
    home: crate::settings::tests::TempHome,
}

impl T {
    fn new(tag: &str, code: i32, line: &str) -> T {
        Self::with_env(tag, &[("FAKE_CODE", &code.to_string()), ("FAKE_LINE", line)])
    }

    fn with_env(tag: &str, env: &[(&str, &str)]) -> T {
        let home = temp_home(&format!("index-{tag}"));
        let buf = Buf::default();
        let engine = Engine::with_settings(Box::new(buf.clone()), Settings::open_at(home.path().join("settings.json")));
        let script = home.path().join("fake-dg");
        std::fs::write(&script, FAKE).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut envs: Vec<(String, String)> = env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        envs.push(("FAKE_LOG".into(), home.path().join("calls.log").to_string_lossy().into_owned()));
        std::fs::create_dir_all(home.path().join("Desktop")).unwrap();
        configure(&engine, Config { program: script, args: vec![], env: envs, desktop: home.path().join("Desktop") });
        let t = T { engine, buf, home };
        t.set(&[("mcp.allow.control", json!(true))]);
        t
    }

    fn set(&self, kv: &[(&str, Value)]) {
        let m: Map<String, Value> = kv.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
        self.engine.settings.set(&m).unwrap();
    }

    fn control(&self, method: &str, params: Value) -> Result<Value, String> {
        self.engine.handle_control(method, &params)
    }

    fn ui(&self, method: &str, params: Value) -> Result<Value, String> {
        self.engine.handle_ui(method, &params)
    }

    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.home.path().join("calls.log")).unwrap_or_default().lines().map(String::from).collect()
    }
}

#[test]
fn index_run_waits_and_interprets_success() {
    let t = T::new("ok", 0, "Indexed 3 folders");
    t.set(&[("harness", json!("ollama")), ("model.ollama", json!("qwen2.5:7b; rm"))]);
    let r = t.control("index.run", json!({"wait": true, "timeout": 20, "full": true})).unwrap();
    assert_eq!(r, json!({"started": true, "running": false, "status": "Indexed 3 folders", "problem": null}));
    assert_eq!(t.calls(), vec!["ollama|mcp|qwen2.5:7brm|1|1|http://localhost:11434|"]);
    let st = t.ui("index/status", json!({})).unwrap();
    assert!(st["lastRun"].as_str().unwrap().ends_with('Z') && st["lastSuccess"].is_string());
    // notifications: Indexing… then the result, and a finished event
    let progress = t.buf.notes("index/progress");
    assert_eq!(progress[0]["status"], json!("Indexing…"));
    assert_eq!(progress.last().unwrap()["running"], json!(false));
    assert_eq!(t.buf.notes("index/finished")[0]["code"], json!(0));
    assert!(t.buf.notes("settings/changed").iter().any(|n| n["keys"].to_string().contains("lastReindexSuccess")));
}

#[test]
fn exit_codes_become_problems() {
    for (code, line, status, problem) in [
        (10, "", "Up to date", Value::Null),
        (75, "whatever", "Already indexing", Value::Null),
        (76, "", "Ollama hit a usage limit", json!("Ollama hit a usage limit")),
        (78, "Ollama isn't running — open it", "Ollama isn't running — open it", json!("Ollama isn't ready")),
        (127, "", "Ollama isn't installed", json!("Ollama isn't installed")),
        (3, "", "Failed (3) — see reindex.log", json!("Failed (3) — see reindex.log")),
        (1, "boom", "boom", json!("boom")),
    ] {
        let t = T::new(&format!("code{code}"), code, line);
        t.set(&[("harness", json!("ollama"))]);
        let r = t.control("index.run", json!({"wait": true})).unwrap();
        assert_eq!(r["status"], json!(status), "exit {code}");
        assert_eq!(r["problem"], problem, "exit {code}");
        assert_eq!(t.engine.settings.value("lastReindexSuccess").is_some(), code == 10, "exit {code}");
        assert!(t.engine.settings.value("lastReindexRun").is_some());
    }
}

#[test]
fn signed_out_pauses_schedule_and_survives_restart() {
    let t = T::new("signedout", 77, "");
    t.set(&[("harness", json!("codex")), ("unattended.codex", json!(true)), ("schedule", json!("hourly"))]);
    let r = t.control("index.run", json!({"wait": true})).unwrap();
    assert_eq!(r["problem"], json!("Not signed in to Codex CLI"));
    assert_eq!(t.engine.settings.string("signedOutHarness").as_deref(), Some("codex"));
    let s = t.control("schedule.get", json!({})).unwrap();
    assert_eq!(s["paused_until_sign_in"], json!(true));
    assert!(!tick(&t.engine, schedule::now() + 10.0 * 3600.0), "blocked while signed out");
    let p = t.buf.notes("index/problem");
    assert_eq!(p.last().unwrap()["problem"], json!({"kind": "signedOut", "harness": "codex", "message": "Not signed in to Codex CLI"}));

    // a new engine on the same settings starts with the problem
    let again = Engine::with_settings(Box::new(std::io::sink()), Settings::open_at(t.home.path().join("settings.json")));
    let st = again.handle_ui("index/status", &json!({})).unwrap();
    assert_eq!(st["problem"]["kind"], json!("signedOut"));
    assert_eq!(st["status"], json!("Not signed in to Codex CLI"));

    // signing in (auth check passes) clears it
    let mut env = indexer(&t.engine).cfg.lock().unwrap().env.clone();
    env.retain(|(k, _)| k != "FAKE_CODE");
    indexer(&t.engine).cfg.lock().unwrap().env = env;
    let r = t.ui("index/check_auth", json!({})).unwrap();
    assert_eq!(r["started"], json!(true));
    let deadline = Instant::now() + Duration::from_secs(10);
    while indexer(&t.engine).st.lock().unwrap().checking_auth && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let st = t.ui("index/status", json!({})).unwrap();
    assert_eq!(st["problem"], Value::Null);
    assert_eq!(st["status"], json!("Signed in to Codex CLI"));
    assert!(t.engine.settings.value("signedOutHarness").is_none());
    assert!(t.calls().last().unwrap().ends_with("|auth-check|||||--auth-only") || t.calls().last().unwrap().contains("--auth-only"));
}

#[test]
fn agent_without_consent_never_runs() {
    let t = T::new("consent", 0, "");
    t.set(&[("harness", json!("gemini"))]);
    let r = t.control("index.run", json!({"wait": true})).unwrap();
    assert_eq!(r, json!({"started": true, "running": false, "status": "Unattended runs with Gemini CLI are off",
                         "problem": "Unattended runs with Gemini CLI are off"}));
    assert!(t.calls().is_empty());
    assert!(t.control("schedule.get", json!({})).unwrap()["paused_until_sign_in"] == json!(true));
    // the app got the user's OK: allowUnattended turns it on and runs
    let r = t.ui("index/run", json!({"reason": "button", "allowUnattended": true})).unwrap();
    assert_eq!(r["started"], json!(true));
    wait_idle(&t.engine, Duration::from_secs(10));
    assert!(t.engine.settings.bool("unattended.gemini"));
    assert_eq!(t.calls().len(), 1);
    assert!(t.calls()[0].starts_with("gemini|button||1|"));
}

#[test]
fn busy_cancel_and_gate() {
    let t = T::with_env("cancel", &[("FAKE_SLEEP", "20")]);
    t.set(&[("harness", json!("llamacpp"))]);
    assert_eq!(t.control("index.run", json!({})).unwrap()["started"], json!(true));
    assert_eq!(t.control("index.run", json!({})).unwrap(), json!({"started": false, "status": "Already indexing"}));
    assert_eq!(t.control("app.status", json!({})).unwrap()["running"], json!(true));
    // wait for the child to exist, then cancel the whole group
    let deadline = Instant::now() + Duration::from_secs(5);
    while indexer(&t.engine).st.lock().unwrap().child.is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let started = Instant::now();
    assert_eq!(t.ui("index/cancel", json!({})).unwrap(), json!({"cancelled": true}));
    wait_idle(&t.engine, Duration::from_secs(10));
    assert!(started.elapsed() < Duration::from_secs(10));
    let st = t.ui("index/status", json!({})).unwrap();
    assert_eq!((st["running"].clone(), st["status"].clone()), (json!(false), json!("Cancelled")));
    assert_eq!(t.ui("index/cancel", json!({})).unwrap(), json!({"cancelled": false}));

    // MCP: gated, and UI-link methods aren't reachable at all
    t.set(&[("mcp.allow.control", json!(false))]);
    assert!(t.control("index.run", json!({})).unwrap_err().contains("Change settings and start re-indexing"));
    assert_eq!(t.control("index/run", json!({})).unwrap_err(), "Unknown method index/run");
    assert!(t.control("app.status", json!({})).is_ok(), "reading is always allowed");
}

#[test]
fn schedule_get_and_set() {
    let t = T::new("schedule", 0, "");
    let s = t.control("schedule.get", json!({})).unwrap();
    assert_eq!(s, json!({"schedule": "daily", "label": "Daily at 3 AM", "hour": 3, "paused_until_sign_in": true, "last_run": null}));
    let s = t.control("schedule.set", json!({"schedule": "Every_6_Hours", "hour": 30})).unwrap();
    assert_eq!((s["schedule"].clone(), s["hour"].clone(), s["label"].clone()), (json!("every6h"), json!(23), json!("Every 6 hours")));
    assert_eq!(t.engine.settings.string("schedule").as_deref(), Some("every6h"));
    let s = t.control("schedule.set", json!({"schedule": "daily", "hour": 15})).unwrap();
    assert_eq!(s["label"], json!("Daily at 3 PM"));
    assert_eq!(t.control("schedule.set", json!({"schedule": "weekly"})).unwrap_err(), "Schedule must be off, hourly, every_6_hours or daily");
    t.set(&[("lastReindexRun", json!("2026-11-06 16:00:00 +0000"))]);
    assert_eq!(t.control("schedule.get", json!({})).unwrap()["last_run"], json!("2026-11-06T16:00:00Z"));
}

#[test]
fn scheduler_ticks_and_launch_bookkeeping() {
    let t = T::new("tick", 10, "");
    t.set(&[("harness", json!("mlx")), ("schedule", json!("Every 6 hours"))]);
    // never indexed: nothing to catch up on, and no index file means no first run is recorded
    launch_bookkeeping(&t.engine);
    assert_eq!(t.engine.settings.string("schedule").as_deref(), Some("every6h"), "old values are migrated");
    assert!(t.engine.settings.value("lastReindexRun").is_none());
    assert!(!tick(&t.engine, schedule::now()));
    // an existing index counts as a first run
    let log = t.home.path().join("Desktop/_Organize Log");
    std::fs::create_dir_all(&log).unwrap();
    std::fs::write(log.join("desktop-index.md"), "# index\n").unwrap();
    launch_bookkeeping(&t.engine);
    assert!(t.engine.settings.value("lastReindexRun").is_some());
    assert!(!tick(&t.engine, schedule::now() + 3600.0), "not due yet");
    assert!(tick(&t.engine, schedule::now() + 7.0 * 3600.0), "due after six hours");
    wait_idle(&t.engine, Duration::from_secs(10));
    assert!(t.calls()[0].starts_with("mlx|schedule|"));
    assert_eq!(t.ui("index/status", json!({})).unwrap()["status"], json!("Up to date"));
    t.set(&[("schedule", json!("off"))]);
    assert!(!tick(&t.engine, schedule::now() + 100.0 * 3600.0));
}

#[test]
fn backend_set_and_app_status() {
    let t = T::new("backend", 0, "");
    let log = t.home.path().join("Desktop/_Organize Log");
    std::fs::create_dir_all(&log).unwrap();
    std::fs::write(log.join("questions.md"), "- [ ] one?\n- [x] done\n  - [ ] two?\n").unwrap();
    let s = t.control("app.status", json!({})).unwrap();
    assert_eq!(s["backend"], json!("claude"));
    assert_eq!(s["model"], json!("haiku"));
    assert_eq!(s["open_questions"], json!(2));
    assert_eq!(s["schedule"]["schedule"], json!("daily"));

    let s = t.control("backend.set", json!({"backend": "LM Studio", "model": "qwen/qwen3 8b", "endpoint": "http://127.0.0.1:9999"})).unwrap();
    assert_eq!((s["backend"].clone(), s["backend_label"].clone(), s["model"].clone()), (json!("lmstudio"), json!("LM Studio"), json!("qwen/qwen38b")));
    assert_eq!(t.engine.settings.string("endpoint.lmstudio").as_deref(), Some("http://127.0.0.1:9999"));
    t.control("backend.set", json!({"backend": "codex", "endpoint": "http://x"})).unwrap();
    assert!(t.engine.settings.value("endpoint.codex").is_none(), "endpoints only for servers");
    let e = t.control("backend.set", json!({"backend": "gpt"})).unwrap_err();
    assert_eq!(e, "Backend must be one of: claude, codex, gemini, antigravity, cursor, copilot, opencode, apple, mlx, ollama, lmstudio, llamacpp");
    assert!(t.control("backend.set", json!({})).is_err());
}

#[test]
fn detection_and_models() {
    let t = T::new("models", 0, "");
    let bin = t.home.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let cursor = bin.join("cursor-agent");
    std::fs::write(&cursor, "#!/bin/sh\necho 'Available models'\necho '- auto - Auto'\necho '* sonnet-4.5 (current)'\n").unwrap();
    std::fs::set_permissions(&cursor, std::fs::Permissions::from_mode(0o755)).unwrap();
    let codex = bin.join("codex");
    std::fs::write(&codex, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(0o755)).unwrap();

    let path = bin.to_string_lossy().into_owned();
    let found = backends::detect(&path, t.home.path(), Some("/Apps/DesktopGlass"));
    assert_eq!(found["cursor"], json!(cursor.to_string_lossy()));
    assert_eq!(found["codex"], json!(codex.to_string_lossy()));
    assert_eq!(found["apple"], json!("/Apps/DesktopGlass"));
    assert!(found.get("claude").is_none() && found.get("llamacpp").is_none(), "no 'server' placeholders");

    let r = t.ui("models/list", json!({"harness": "cursor", "binary": cursor.to_string_lossy()})).unwrap();
    assert_eq!(r, json!({"harness": "cursor", "models": ["auto", "sonnet-4.5"], "live": true}));
    let r = t.ui("models/list", json!({"harness": "claude"})).unwrap();
    assert_eq!(r, json!({"harness": "claude", "models": ["haiku", "sonnet", "opus"], "live": false}));
    assert!(t.ui("models/list", json!({"harness": "nope"})).is_err());

    // a local server's list (a one-shot fake Ollama)
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        let mut req = [0u8; 1024];
        let n = std::io::Read::read(&mut s, &mut req).unwrap();
        let body = r#"{"models":[{"name":"llama3.2:latest"},{"name":"qwen2.5:7b"}]}"#;
        write!(s, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        String::from_utf8_lossy(&req[..n]).into_owned()
    });
    t.set(&[("endpoint.ollama", json!(format!("http://127.0.0.1:{port}/")))]);
    let r = t.ui("models/list", json!({"harness": "ollama"})).unwrap();
    assert!(server.join().unwrap().starts_with("GET /api/tags"));
    assert_eq!(r["models"], json!(["llama3.2:latest", "qwen2.5:7b"]));
    // cached afterwards (the fake server is gone)
    assert_eq!(t.ui("models/list", json!({"harness": "ollama"})).unwrap()["live"], json!(true));

    // mlx: the Hugging Face cache under HOME
    let hub = t.home.path().join(".cache/huggingface/hub");
    std::fs::create_dir_all(hub.join("models--mlx-community--Qwen3-4B-4bit")).unwrap();
    std::fs::create_dir_all(hub.join("models--other--x")).unwrap();
    assert_eq!(backends::mlx_models(t.home.path()), vec!["mlx-community/Qwen3-4B-4bit"]);
    assert_eq!(t.ui("models/presets", json!({})).unwrap()["ollama"], json!(["llama3.2", "qwen2.5:7b"]));
}
