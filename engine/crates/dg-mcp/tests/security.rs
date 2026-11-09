//! Port of Tests/python/test_mcp_security.py, one test per Python test: every path an MCP client sends stays inside
//! the Desktop, writes can't break description.md's format, and gated tools are hidden and refused until the user
//! allows them. Plus the control channel against a fake app on loopback (never the real app).

use dg_core::description;
use dg_mcp::{Server, Settings, GATES};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// A temporary folder, resolved (macOS /var → /private/var), removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> TempDir {
        static N: AtomicUsize = AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!("dg-mcp-tests-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        TempDir(dg_core::paths::resolve(&p))
    }
    fn file(&self, rel: &str, text: &str) -> PathBuf {
        let p = self.0.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, text).unwrap();
        p
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct T {
    d: TempDir,
    outside: TempDir,
    s: Server,
}

fn setup() -> T {
    let d = TempDir::new();
    let outside = TempDir::new();
    outside.file("secret/description.md", "# Secret\n> do not touch\n");
    d.file("Projects/notes.txt", "x");
    d.file("Projects/web/package.json", "{}");
    d.file("Projects/description.md", "# Projects\n> Code\n\n## Contents\n- **notes.txt** — plans\n");
    let s = Server {
        root: d.0.clone(),
        control: d.0.join("no-control.json"),
        app_paths: vec![], // never launch a real app
        settings: Settings::Fixed(HashMap::new()),
        dg_exe: None,
    };
    T { d, outside, s }
}

impl T {
    fn allow(&mut self, group: &str) {
        if let Settings::Fixed(m) = &mut self.s.settings {
            m.insert(format!("mcp.allow.{group}"), "1".into());
        }
    }
    /// A tools/call through the JSON-RPC handler, as an MCP client would make it.
    fn call(&self, name: &str, args: Value) -> (bool, String) {
        let r = self.s.handle(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": args}})).unwrap();
        let result = &r["result"];
        (result["isError"].as_bool().unwrap(), result["content"][0]["text"].as_str().unwrap_or("").to_string())
    }
    fn desc(&self, rel: &str) -> String {
        fs::read_to_string(self.d.0.join(rel).join("description.md")).unwrap()
    }
    fn headings(&self, rel: &str) -> Vec<String> {
        self.desc(rel).lines().filter(|l| l.starts_with('#')).map(String::from).collect()
    }
    fn tools(&self) -> HashSet<String> {
        let r = self.s.handle(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})).unwrap();
        r["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect()
    }
}

// ---- SafePath ----

#[test]
fn safe_path_inside() {
    let t = setup();
    let r = &t.d.0;
    assert_eq!(t.s.safe_rel("").unwrap(), *r);
    assert_eq!(t.s.safe_rel("Projects").unwrap(), r.join("Projects"));
    assert_eq!(t.s.safe_rel("/Projects/").unwrap(), r.join("Projects"), "a leading slash still means Desktop-relative");
    assert_eq!(t.s.safe_rel("Projects/../Projects/web").unwrap(), r.join("Projects/web"));
}

#[test]
fn safe_path_escapes_are_refused() {
    let t = setup();
    for rel in ["..", "../", "../x", "Projects/../../x", "Projects/../..", "../../../etc"] {
        assert!(t.s.safe_rel(rel).is_err(), "{rel}");
    }
}

#[test]
fn safe_path_symlink_out_of_the_desktop_is_refused() {
    let t = setup();
    std::os::unix::fs::symlink(t.outside.0.join("secret"), t.d.0.join("Projects/link")).unwrap();
    assert!(t.s.safe_rel("Projects/link").is_err());
    let (is_error, text) = t.call("list_folder", json!({"path": "Projects/link"}));
    assert!(is_error && text.contains("inside the Desktop"));
}

#[test]
fn safe_path_tools_refuse_traversal() {
    let mut t = setup();
    t.allow("edit");
    for (tool, args) in [
        ("list_folder", json!({"path": "../"})),
        ("get_description", json!({"path": "../../etc"})),
        ("update_description", json!({"path": "..", "summary": "pwned"})),
    ] {
        let (is_error, text) = t.call(tool, args);
        assert!(is_error, "{tool}");
        assert!(text.contains("inside the Desktop"), "{tool}: {text}");
    }
    assert!(fs::read_to_string(t.outside.0.join("secret/description.md")).unwrap().contains("do not touch"));
}

// ---- UpdateDescription ----

#[test]
fn update_newlines_cant_add_sections_or_bullets() {
    let mut t = setup();
    t.allow("edit");
    let (is_error, _) = t.call(
        "update_description",
        json!({
            "path": "Projects",
            "summary": "ok\n## Contents\n- **evil.txt** — injected",
            "tags": ["a,b", "two\nlines", ""],
            "blurbs": {"notes.txt": "fine\n# Title takeover"},
            "notes": "keep this\n## Contents\n- **x** — y",
        }),
    );
    assert!(!is_error);
    let d = description::parse(&t.desc("Projects"));
    assert_eq!(d.title.as_deref(), Some("Projects"));
    assert_eq!(d.summary.as_deref(), Some("ok ## Contents - **evil.txt** — injected"));
    assert_eq!(d.tags, vec!["a b", "two lines"]);
    assert_eq!(d.blurbs["notes.txt"], "fine # Title takeover");
    assert_eq!(d.blurbs.keys().cloned().collect::<HashSet<_>>(), HashSet::from(["notes.txt".to_string(), "web".to_string()]), "only real items get bullets");
    assert_eq!(d.notes, vec!["keep this", "- **x** — y"]);
    assert_eq!(t.headings("Projects"), vec!["# Projects", "## Contents", "## Notes"]);
}

#[test]
fn update_only_described_folders() {
    let mut t = setup();
    t.allow("edit");
    for path in ["", "Projects/web", "Projects/notes.txt", "Nope"] {
        let (is_error, _) = t.call("update_description", json!({"path": path, "summary": "x"}));
        assert!(is_error, "{path:?}");
    }
}

#[test]
fn update_unknown_items_are_reported_not_written() {
    let mut t = setup();
    t.allow("edit");
    let (is_error, text) = t.call("update_description", json!({"path": "Projects", "blurbs": {"ghost.txt": "boo", "web/": "site"}}));
    assert!(!is_error);
    assert!(text.contains("no such items: ghost.txt"), "{text}");
    assert!(t.desc("Projects").contains("- **web/** — site"));
    assert!(!t.desc("Projects").contains("ghost"));
}

#[test]
fn update_limits() {
    let mut t = setup();
    t.allow("edit");
    let tags: Vec<String> = (0..10).map(|i| format!("t{i}")).collect();
    t.call("update_description", json!({"path": "Projects", "summary": "s".repeat(500), "tags": tags}));
    let d = description::parse(&t.desc("Projects"));
    assert_eq!(d.summary.unwrap().chars().count(), 160);
    assert_eq!(d.tags.len(), 6);
}

// ---- ResolveQuestion ----

fn questions(t: &T, lines: &[&str]) {
    t.d.file("_Organize Log/questions.md", &(lines.join("\n") + "\n"));
}

#[test]
fn resolve_answer_goes_into_the_folder_notes_on_one_line() {
    let mut t = setup();
    t.allow("edit");
    questions(&t, &["- [ ] 2026-11-04 · Projects — What is web?"]);
    let (is_error, _) = t.call("resolve_question", json!({"question": "what is web", "answer": "My site\n## Contents\n- **x**"}));
    assert!(!is_error);
    assert!(t.desc("Projects").contains("Q: What is web? A: My site ## Contents - **x**"));
    assert_eq!(t.headings("Projects"), vec!["# Projects", "## Contents", "## Notes"]);
    let qs = fs::read_to_string(t.d.0.join("_Organize Log/questions.md")).unwrap();
    assert!(qs.contains("- [x] 2026-11-04 · Projects — What is web? → My site ## Contents - **x**"), "{qs}");
}

#[test]
fn resolve_question_pointing_outside_the_desktop_writes_nothing_there() {
    let mut t = setup();
    t.allow("edit");
    // os.path.relpath(outside/secret, desktop)
    let rel = relpath(&t.outside.0.join("secret"), &t.d.0);
    questions(&t, &[&format!("- [ ] 2026-11-04 · {rel} — Whose secret?")]);
    let (is_error, _) = t.call("resolve_question", json!({"question": "whose secret", "answer": "mine"}));
    assert!(!is_error, "the question is still checked off");
    assert!(!fs::read_to_string(t.outside.0.join("secret/description.md")).unwrap().contains("mine"));
}

#[test]
fn resolve_question_about_the_desktop_itself_writes_nothing_above_it() {
    // Stricter than mcp_server.py, which would write description.md into the Desktop's parent here.
    let mut t = setup();
    t.allow("edit");
    questions(&t, &["- [ ] 2026-11-04 · Projects/.. — What is all this?"]);
    let (is_error, _) = t.call("resolve_question", json!({"question": "all this", "answer": "stuff"}));
    assert!(!is_error);
    assert!(!t.d.0.parent().unwrap().join("description.md").exists());
}

#[test]
fn resolve_unknown_question() {
    let mut t = setup();
    t.allow("edit");
    questions(&t, &["- [x] done · Projects — old"]);
    let (is_error, text) = t.call("resolve_question", json!({"question": "old", "answer": "x"}));
    assert!(is_error);
    assert!(text.contains("No open question"));
}

fn relpath(target: &Path, base: &Path) -> String {
    let t: Vec<_> = target.components().collect();
    let b: Vec<_> = base.components().collect();
    let common = t.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let mut parts: Vec<String> = vec!["..".into(); b.len() - common];
    parts.extend(t[common..].iter().map(|c| c.as_os_str().to_string_lossy().into_owned()));
    parts.join("/")
}

// ---- Gates ----

#[test]
fn gated_tools_are_hidden_until_allowed() {
    let mut t = setup();
    let gated: HashSet<String> = GATES.iter().flat_map(|g| g.1.iter().map(|s| s.to_string())).collect();
    assert!(gated.is_disjoint(&t.tools()));
    assert!(t.tools().contains("desktop_overview"));
    for g in GATES {
        t.allow(g.0);
    }
    assert!(gated.is_subset(&t.tools()));
    assert_eq!(t.tools().len(), 35);
}

#[test]
fn gated_tools_are_refused_when_called_anyway() {
    let t = setup();
    for tool in ["update_description", "resolve_question", "reindex_desktop", "browser_eval", "play_game"] {
        let (is_error, text) = t.call(tool, json!({"path": "Projects", "summary": "x", "question": "q", "answer": "a", "game": "g"}));
        assert!(is_error, "{tool}");
        assert!(text.contains("turned off for AI apps"), "{tool}: {text}");
    }
    assert!(t.desc("Projects").contains("> Code"), "nothing was written");
}

#[test]
fn every_tool_has_a_handler_and_a_schema() {
    let tools = dg_mcp::tools();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names.len(), names.iter().collect::<HashSet<_>>().len());
    assert_eq!(names.len(), 35);
    let t = setup();
    for tool in tools {
        assert_eq!(tool["inputSchema"]["type"], "object", "{}", tool["name"]);
        // a handler exists: an unknown tool would be a JSON-RPC error, not a tool result
        let r = t.s.handle(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": tool["name"], "arguments": {}}})).unwrap();
        assert!(r.get("result").is_some(), "{}", tool["name"]);
    }
    let gated: Vec<&str> = GATES.iter().flat_map(|g| g.1.iter().copied()).collect();
    assert!(gated.iter().all(|g| names.contains(g)));
}

// ---- protocol ----

#[test]
fn protocol_negotiation_and_errors() {
    let t = setup();
    for (asked, got) in [("2025-06-18", "2025-06-18"), ("2025-03-26", "2025-03-26"), ("2024-11-05", "2024-11-05"), ("2099-01-01", "2025-06-18")] {
        let r = t.s.handle(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": asked}})).unwrap();
        assert_eq!(r["result"]["protocolVersion"], got);
    }
    assert!(t.s.handle(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).is_none());
    assert_eq!(t.s.handle_line("not json"), vec![r#"{"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": "Parse error"}}"#]);
    assert_eq!(
        t.s.handle_line(r#"{"jsonrpc":"2.0","id":2,"method":"nope"}"#),
        vec![r#"{"jsonrpc": "2.0", "id": 2, "error": {"code": -32601, "message": "Method not found: nope"}}"#]
    );
    let r = t.s.handle(&json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "rm_rf"}})).unwrap();
    assert_eq!(r["error"]["code"], -32602);
    assert_eq!(t.s.handle_line(r#"[{"jsonrpc":"2.0","id":1,"method":"ping"},{"jsonrpc":"2.0","method":"x"}]"#).len(), 1);
}

// ---- control channel (a fake app on loopback) ----

fn fake_app(t: &T, reply: &'static str) -> std::thread::JoinHandle<String> {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    fs::write(&t.s.control, json!({"port": port, "token": "tok", "pid": std::process::id()}).to_string()).unwrap();
    std::thread::spawn(move || {
        let (sock, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(&sock).read_line(&mut line).unwrap();
        (&sock).write_all(reply.as_bytes()).unwrap();
        line
    })
}

#[test]
fn app_tools_go_through_the_control_channel() {
    let t = setup();
    let h = fake_app(&t, "{\"result\": {\"tabs\": [\"t1\"]}}\n");
    let (is_error, text) = t.call("browser_tabs", json!({}));
    assert!(!is_error);
    assert_eq!(text, "{\n  \"tabs\": [\n    \"t1\"\n  ]\n}");
    assert_eq!(h.join().unwrap(), "{\"token\": \"tok\", \"method\": \"browser.tabs\", \"params\": {}}\n");
}

#[test]
fn app_errors_and_screenshots() {
    let t = setup();
    let h = fake_app(&t, "{\"error\": \"No such tab\"}\n");
    assert_eq!(t.call("browser_read", json!({"tab": "x"})), (true, "Error: No such tab".into()));
    assert!(h.join().unwrap().contains("\"max_chars\": 20000"));
    let h = fake_app(&t, "{\"result\": {\"png\": \"AAAA\", \"title\": \"T\", \"url\": \"u\"}}\n");
    let r = t.s.handle(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "browser_screenshot", "arguments": {}}})).unwrap();
    assert_eq!(r["result"]["content"], json!([{"type": "image", "data": "AAAA", "mimeType": "image/png"}, {"type": "text", "text": "T — u"}]));
    h.join().unwrap();
}

#[test]
fn app_not_running_and_not_installed() {
    let t = setup();
    assert_eq!(t.call("show_overlay", json!({})), (true, "Error: DesktopGlass isn't installed — run install.sh".into()));
    // a stale control.json (dead pid) counts as not running
    fs::write(&t.s.control, r#"{"port": 1, "token": "x", "pid": 999999}"#).unwrap();
    assert_eq!(t.call("browser_tabs", json!({})), (true, "Error: DesktopGlass isn't installed — run install.sh".into()));
    // index_status never launches the app; it just leaves the App line out
    let (is_error, text) = t.call("index_status", json!({}));
    assert!(!is_error && text.starts_with("Indexed 1 folders") && !text.contains("App:"), "{text}");
}

#[test]
fn reindex_fallback_gets_the_same_environment_as_reindex_sh() {
    let mut t = setup();
    if let Settings::Fixed(m) = &mut t.s.settings {
        m.insert("harness".into(), "ollama".into());
        m.insert("endpoint.ollama".into(), "http://127.0.0.1:11434".into());
        m.insert("unattended.ollama".into(), "1".into());
    }
    let args = json!({"full": true}).as_object().unwrap().clone();
    let env: HashMap<String, String> = t.s.reindex_env(&args).into_iter().collect();
    assert_eq!(env["DESKTOP_INDEX_HARNESS"], "ollama");
    assert_eq!(env["DESKTOP_INDEX_MODEL"], "");
    assert_eq!(env["DESKTOP_INDEX_ENDPOINT"], "http://127.0.0.1:11434");
    assert_eq!(env["DESKTOP_INDEX_REASON"], "mcp");
    assert_eq!(env["DESKTOP_INDEX_UNATTENDED"], "1");
    assert_eq!(env["DESKTOP_INDEX_SANDBOX"], "0");
    assert_eq!(env["DESKTOP_INDEX_FULL"], "1");
    let t2 = setup();
    let env: HashMap<String, String> = t2.s.reindex_env(&Default::default()).into_iter().collect();
    assert_eq!((env["DESKTOP_INDEX_HARNESS"].as_str(), env["DESKTOP_INDEX_MODEL"].as_str()), ("claude", "haiku"));
    assert!(!env.contains_key("DESKTOP_INDEX_FULL") && !env.contains_key("DESKTOP_INDEX_ENDPOINT"));
}

#[test]
fn reindex_fallback_runs_dg_reindex_and_maps_exit_codes() {
    // A stand-in for `dg` that records its arguments and exits 10; the real reindex is never run.
    let mut t = setup();
    let fake = t.d.file("fake-dg", "#!/bin/sh\necho \"$@ $DESKTOP_INDEX_REASON\" > \"$(dirname \"$0\")/ran\"\necho working\necho 'Nothing changed'\nexit 10\n");
    fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    t.s.dg_exe = Some(fake);
    t.allow("control");
    assert_eq!(t.call("reindex_desktop", json!({})), (false, "Up to date: Nothing changed".into()));
    assert_eq!(fs::read_to_string(t.d.0.join("ran")).unwrap(), "reindex mcp\n");
}
