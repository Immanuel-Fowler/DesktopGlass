//! MCP server (Phase 3): replaces `mcp_server.py`, byte for byte on the wire.
//!
//! JSON-RPC 2.0 over stdio, one message per line; nothing but protocol messages on stdout (logs go to stderr).
//! File tools read and write the Desktop directly and work without the app; app tools go through the control
//! channel (see `app.rs`), launching the app with `open -g` when it isn't running.
//!
//! Permission groups (`edit`, `control`, `script`, `play`; reading is always allowed) come from the
//! `mcp.allow.<group>` settings via `dg_platform::read_setting`. Tests point that at a JSON file with
//! `DG_SETTINGS_FILE=<path>` (see dg-platform) instead of UserDefaults; unit tests use `Settings::Fixed`.
//!
//! Environment: `DESKTOP_GLASS_ROOT` overrides the Desktop folder (as in mcp_server.py); `HOME` locates
//! `~/Desktop`, control.json and the app.

pub mod app;
pub mod files;
pub mod pyjson;

use files::Args;
use pyjson::{as_text, disp, or};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::Duration;

/// Protocol versions we speak, newest first (an unknown request gets the newest).
pub const SUPPORTED: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

pub const INSTRUCTIONS: &str = "The user's organized Mac Desktop, maintained by DesktopGlass. Call desktop_overview first, then \
list_folder / search_desktop / recent_changes to find things. Fix descriptions you learn are wrong with update_description. \
Whenever you produce a visual artifact (HTML, SVG, a chart) or start a web server, call show_artifact so the user sees it; \
use the browser_* tools to check it.";

/// Permission groups (DesktopGlass → Settings → MCP): the tools each one unlocks and its label.
pub const GATES: [(&str, &[&str], &str); 4] = [
    (
        "edit",
        &[
            "update_description", "resolve_question", "html_set_style", "html_split_style", "html_group_style",
            "html_set_text", "html_set_html", "html_duplicate_element", "html_remove_element",
        ],
        "Edit files",
    ),
    ("control", &["reindex_desktop", "set_schedule", "set_index_backend", "set_preferences"], "Change settings and start re-indexing"),
    ("script", &["browser_click", "browser_type", "browser_eval"], "Click, type and run JavaScript in the artifact viewer"),
    ("play", &["play_game"], "Launch games from the Games folder"),
];

/// The 35 tools with their exact names, descriptions and input schemas (exported from mcp_server.py's TOOLS).
pub fn tools() -> &'static Vec<Value> {
    static TOOLS: OnceLock<Vec<Value>> = OnceLock::new();
    TOOLS.get_or_init(|| serde_json::from_str(include_str!("tools.json")).expect("tools.json"))
}

/// Where settings come from.
#[derive(Clone, Debug)]
pub enum Settings {
    /// `dg_platform::read_setting` (DG_SETTINGS_FILE → settings.json → UserDefaults).
    Platform,
    /// A fixed map, for tests.
    Fixed(HashMap<String, String>),
}

pub struct Server {
    /// The Desktop root, resolved.
    pub root: PathBuf,
    /// control.json `{port, token, pid}`.
    pub control: PathBuf,
    /// Where to look for DesktopGlass.app when it must be launched.
    pub app_paths: Vec<PathBuf>,
    pub settings: Settings,
    /// The `dg` binary whose `reindex` subcommand runs when the app can't (default: this executable).
    pub dg_exe: Option<PathBuf>,
}

fn truthy_text(v: &str) -> bool {
    matches!(v.trim().to_lowercase().as_str(), "1" | "true" | "yes")
}

impl Server {
    /// The server `dg mcp` runs: the real Desktop (or `DESKTOP_GLASS_ROOT`), control.json and app locations.
    pub fn from_env() -> Server {
        let home = dg_platform::home();
        let root = match std::env::var("DESKTOP_GLASS_ROOT") {
            Ok(r) => dg_core::paths::desktop_root(&r),
            Err(_) => dg_core::paths::resolve(&home.join("Desktop")),
        };
        Server {
            root,
            control: dg_platform::control_file(),
            app_paths: vec![home.join("Applications/DesktopGlass.app"), PathBuf::from("/Applications/DesktopGlass.app")],
            settings: Settings::Platform,
            dg_exe: std::env::current_exe().ok(),
        }
    }

    /// `app_default(key)`: the setting's text, `""` when unset.
    pub fn setting(&self, key: &str) -> String {
        match &self.settings {
            Settings::Platform => dg_platform::read_setting(key).unwrap_or_default(),
            Settings::Fixed(m) => m.get(key).cloned().unwrap_or_default(),
        }
    }

    /// The permission groups the user has allowed (read once per request).
    pub fn allowed_groups(&self) -> Vec<&'static str> {
        GATES.iter().filter(|(k, _, _)| truthy_text(&self.setting(&format!("mcp.allow.{k}")))).map(|g| g.0).collect()
    }

    /// `blocked_by`: the label of the permission a tool needs that isn't granted.
    pub fn blocked_by(tool: &str, allowed: &[&str]) -> Option<&'static str> {
        GATES.iter().find(|(k, tools, _)| tools.contains(&tool) && !allowed.contains(k)).map(|g| g.2)
    }

    pub fn has_handler(name: &str) -> bool {
        tools().iter().any(|t| t["name"] == name)
    }

    /// The environment `tool_reindex` gives the reindex run (same variables reindex.sh gets from Python).
    pub fn reindex_env(&self, args: &Args) -> Vec<(String, String)> {
        let harness = Some(self.setting("harness")).filter(|h| !h.is_empty()).unwrap_or_else(|| "claude".into());
        let mut env = vec![("DESKTOP_INDEX_HARNESS".to_string(), harness.clone())];
        let model = self.setting(&format!("model.{harness}"));
        let model = if !model.is_empty() { model } else if harness == "claude" { "haiku".into() } else { String::new() };
        env.push(("DESKTOP_INDEX_MODEL".into(), model));
        let endpoint = self.setting(&format!("endpoint.{harness}"));
        if !endpoint.is_empty() {
            env.push(("DESKTOP_INDEX_ENDPOINT".into(), endpoint));
        }
        let home = dg_platform::home();
        for candidate in [
            home.join("Applications/DesktopGlass.app/Contents/MacOS/DesktopGlass"),
            PathBuf::from("/Applications/DesktopGlass.app/Contents/MacOS/DesktopGlass"),
        ] {
            if candidate.exists() {
                env.push(("DESKTOP_GLASS_BIN".into(), candidate.to_string_lossy().into_owned()));
                break;
            }
        }
        env.push(("DESKTOP_INDEX_REASON".into(), "mcp".into()));
        let flag = |b: bool| if b { "1" } else { "0" }.to_string();
        env.push(("DESKTOP_INDEX_UNATTENDED".into(), flag(truthy_text(&self.setting(&format!("unattended.{harness}"))))));
        env.push(("DESKTOP_INDEX_SANDBOX".into(), flag(truthy_text(&self.setting("sandboxUnattended")))));
        if or(args.get("full")).is_some() {
            env.push(("DESKTOP_INDEX_FULL".into(), "1".into()));
        }
        env
    }

    /// `tool_reindex`: runs `dg reindex` directly (used when the app isn't available).
    pub fn tool_reindex(&self, args: &Args) -> Result<String, String> {
        let exe = self.dg_exe.clone().ok_or("can't find the dg executable")?;
        let mut cmd = Command::new(&exe);
        cmd.arg("reindex").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        for (k, v) in self.reindex_env(args) {
            cmd.env(k, v);
        }
        let child = cmd.spawn().map_err(|e| files::os_err(&e, &exe))?;
        let pid = child.id();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(child.wait_with_output());
        });
        let out = match rx.recv_timeout(Duration::from_secs(1800)) {
            Ok(r) => r.map_err(|e| e.to_string())?,
            Err(_) => {
                unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
                return Err(format!("Command '['{}', 'reindex']' timed out after 1800 seconds", exe.display()));
            }
        };
        let code = {
            use std::os::unix::process::ExitStatusExt;
            out.status.code().unwrap_or_else(|| -out.status.signal().unwrap_or(0))
        };
        let stdout = String::from_utf8_lossy(&out.stdout);
        let last = dg_core::py::splitlines(dg_core::py::strip(&stdout)).last().copied().unwrap_or("").to_string();
        let label = match code {
            0 => "Indexed".to_string(),
            10 => "Up to date".into(),
            75 => "Already indexing".into(),
            76 => "Usage limit".into(),
            77 => "Not signed in".into(),
            78 => "Model not ready".into(),
            79 => "Unattended runs not allowed".into(),
            127 => "Backend not installed".into(),
            n => format!("Exit {n}"),
        };
        Ok(format!("{label}: {last}"))
    }

    fn tool_reindex_app(&self, args: &Args) -> Result<String, String> {
        let wait = or(args.get("wait")).is_some();
        let params = json!({"full": or(args.get("full")).is_some(), "wait": wait, "timeout": 1500});
        match self.app_call("index.run", Some(params), if wait { 1600 } else { 30 }, true) {
            Ok(v) => Ok(as_text(&v)),
            Err(app::AppError::Unavailable(_)) => self.tool_reindex(args), // app not available: run it directly
            Err(e) => Err(e.message()),
        }
    }

    fn tool_status_full(&self, args: &Args) -> Result<String, String> {
        let text = self.tool_status(args)?;
        match self.app_call("app.status", None, 10, false) {
            Ok(v) => Ok(format!("{text}\n\nApp: {}", as_text(&v))),
            Err(_) => Ok(text),
        }
    }

    fn tool_screenshot(&self, args: &Args) -> Result<Vec<Value>, String> {
        let res = self.app_call("browser.screenshot", Some(json!({"tab": args.get("tab")})), 60, true).map_err(app::AppError::message)?;
        let png = pyjson::index(&res, "png")?.clone();
        let text = format!("{} — {}", pyjson::get_text(&res, "title")?, pyjson::get_text(&res, "url")?);
        Ok(vec![json!({"type": "image", "data": png, "mimeType": "image/png"}), json!({"type": "text", "text": text})])
    }

    fn tool_read_page(&self, args: &Args) -> Result<String, String> {
        let max = or(args.get("max_chars")).cloned().unwrap_or(json!(20000));
        let res = self
            .app_call("browser.read", Some(json!({"tab": args.get("tab"), "max_chars": max})), 60, true)
            .map_err(app::AppError::message)?;
        let g = |k: &str| pyjson::get_text(&res, k);
        let mut out = vec![format!("# {}", g("title")?), format!("URL: {}", g("url")?), format!("Tab: {}", g("tab")?), String::new(), g("text")?];
        let s = |v: &Value, k: &str| pyjson::index(v, k).map(dg_core::py::py_str);
        let t = |v: &Value, k: &str| pyjson::index(v, k).map(dg_core::py::truthy);
        if let Some(fields) = or(pyjson::get(&res, "fields")?) {
            out.push(String::new());
            out.push("Form fields:".into());
            for f in pyjson::py_iter(fields)? {
                let mut line = format!("- <{}", s(&f, "tag")?);
                if t(&f, "type")? {
                    line += &format!(" type={}", s(&f, "type")?);
                }
                line.push('>');
                if t(&f, "id")? {
                    line += &format!(" #{}", s(&f, "id")?);
                }
                if t(&f, "name")? {
                    line += &format!(" name={}", s(&f, "name")?);
                }
                if t(&f, "label")? {
                    line += &format!(" “{}”", s(&f, "label")?);
                }
                out.push(line);
            }
        }
        if let Some(links) = or(pyjson::get(&res, "links")?) {
            out.push(String::new());
            out.push("Links:".into());
            for l in pyjson::py_iter(links)?.iter().take(50) {
                let text = if t(l, "text")? { s(l, "text")? } else { "(no text)".into() };
                out.push(format!("- {text} → {}", s(l, "href")?));
            }
        }
        Ok(out.join("\n"))
    }

    /// Runs one tool. `Ok` holds the result's content list.
    pub fn run_tool(&self, name: &str, args: &Args) -> Result<Vec<Value>, String> {
        let a = || Some(Value::Object(args.clone()));
        let text = match name {
            "desktop_overview" => self.tool_overview(args),
            "list_folder" => self.tool_list_folder(args),
            "search_desktop" => self.tool_search(args),
            "get_description" => self.tool_get_description(args),
            "recent_changes" => self.tool_recent(args),
            "update_description" => self.tool_update_description(args),
            "list_questions" => self.tool_list_questions(args),
            "resolve_question" => self.tool_resolve_question(args),
            "index_status" => self.tool_status_full(args),
            "reindex_desktop" => self.tool_reindex_app(args),
            "get_schedule" => self.via_app("schedule.get", None, 120),
            "set_schedule" => {
                let mut p = Map::new();
                p.insert("schedule".into(), files::req(args, "schedule")?.clone());
                if let Some(h) = args.get("hour") {
                    p.insert("hour".into(), h.clone());
                }
                self.via_app("schedule.set", Some(Value::Object(p)), 120)
            }
            "set_index_backend" => self.via_app("backend.set", a(), 120),
            "set_preferences" => self.via_app("prefs.set", a(), 120),
            "open_in_editor" => self.via_app("editor.open", Some(json!({"path": files::req(args, "path")?})), 120),
            "show_overlay" => self.via_app("overlay.show", None, 120),
            "show_artifact" => self.via_app(
                "browser.open",
                Some(json!({"target": files::req(args, "target")?, "title": args.get("title")})),
                60,
            ),
            "list_local_servers" => self.via_app("servers.list", None, 30),
            "browser_tabs" => self.via_app("browser.tabs", None, 120),
            "browser_read" => self.tool_read_page(args),
            "browser_screenshot" => return self.tool_screenshot(args),
            "browser_click" => self.via_app("browser.click", a(), 60),
            "browser_type" => self.via_app("browser.type", a(), 60),
            "browser_eval" => self.via_app("browser.eval", a(), 60),
            "browser_navigate" => {
                let action = args.get("action");
                let action = if action.and_then(Value::as_str) == Some("go") { json!("") } else { action.cloned().unwrap_or(Value::Null) };
                self.via_app("browser.navigate", Some(json!({"tab": args.get("tab"), "action": action, "url": args.get("url")})), 60)
            }
            "html_outline" => self.via_app("html.outline", a(), 120),
            "html_set_style" => self.via_app("html.style", a(), 120),
            "html_split_style" => self.via_app("html.split_style", a(), 120),
            "html_group_style" => self.via_app("html.group_style", a(), 120),
            "html_set_text" => self.via_app("html.set_text", a(), 120),
            "html_set_html" => self.via_app("html.set_html", a(), 120),
            "html_duplicate_element" => self.via_app("html.duplicate", a(), 120),
            "html_remove_element" => self.via_app("html.remove", a(), 120),
            "list_games" => self.via_app("games.list", a(), 60),
            "play_game" => self.via_app("games.play", a(), 60),
            other => Err(format!("Unknown tool: {other}")),
        }?;
        Ok(vec![json!({"type": "text", "text": text})])
    }

    fn tool_result(text: String, is_error: bool) -> Value {
        json!({"content": [{"type": "text", "text": text}], "isError": is_error})
    }

    /// One JSON-RPC message in; the response (None for notifications).
    pub fn handle(&self, msg: &Value) -> Option<Value> {
        let msg = msg.as_object()?;
        let id = msg.get("id").filter(|v| !v.is_null())?.clone(); // notifications (initialized, cancelled, …)
        let empty = Map::new();
        let params = match or(msg.get("params")) {
            Some(Value::Object(p)) => p,
            _ => &empty,
        };
        let method = msg.get("method");
        let ok = |result: Value| json!({"jsonrpc": "2.0", "id": id, "result": result});
        let err = |code: i64, message: String| json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}});
        Some(match method.and_then(Value::as_str) {
            Some("initialize") => {
                let requested = params.get("protocolVersion").and_then(Value::as_str);
                let version = requested.filter(|r| SUPPORTED.contains(r)).unwrap_or(SUPPORTED[0]);
                ok(json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": "desktopglass", "version": dg_platform::VERSION},
                    "instructions": INSTRUCTIONS,
                }))
            }
            Some("ping") => ok(json!({})),
            Some("tools/list") => {
                let allowed = self.allowed_groups();
                let list: Vec<Value> = tools()
                    .iter()
                    .filter(|t| Self::blocked_by(t["name"].as_str().unwrap_or(""), &allowed).is_none())
                    .cloned()
                    .collect();
                ok(json!({"tools": list}))
            }
            Some("tools/call") => {
                let name = params.get("name");
                let name_str = name.and_then(Value::as_str).filter(|n| Self::has_handler(n));
                let Some(name_str) = name_str else {
                    return Some(err(-32602, format!("Unknown tool: {}", disp(name))));
                };
                if let Some(needs) = Self::blocked_by(name_str, &self.allowed_groups()) {
                    return Some(ok(Self::tool_result(
                        format!("“{needs}” is turned off for AI apps. The user can allow it in DesktopGlass → Settings → MCP."),
                        true,
                    )));
                }
                let args = match or(params.get("arguments")) {
                    None => Ok(Map::new()),
                    Some(Value::Object(a)) => Ok(a.clone()),
                    Some(other) => Err(format!("'{}' object has no attribute 'get'", pyjson::type_name(other))),
                };
                match args.and_then(|a| self.run_tool(name_str, &a)) {
                    Ok(content) => ok(json!({"content": content, "isError": false})),
                    Err(e) => ok(Self::tool_result(format!("Error: {e}"), true)),
                }
            }
            Some("resources/list") => ok(json!({"resources": []})),
            Some("prompts/list") => ok(json!({"prompts": []})),
            _ => err(-32601, format!("Method not found: {}", disp(method))),
        })
    }

    /// Handles one input line (a message or a batch); returns the response lines.
    pub fn handle_line(&self, line: &str) -> Vec<String> {
        let line = dg_core::py::strip(line);
        if line.is_empty() {
            return vec![];
        }
        let msgs = match serde_json::from_str::<Value>(line) {
            Ok(Value::Array(a)) => a,
            Ok(v) => vec![v],
            Err(_) => {
                let e = json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": "Parse error"}});
                return vec![pyjson::dumps(&e, None, false)];
            }
        };
        msgs.iter().filter_map(|m| self.handle(m)).map(|r| pyjson::dumps(&r, None, false)).collect()
    }
}

/// `dg mcp`: serves stdin/stdout until stdin closes.
pub fn run_stdio() {
    let server = Server::from_env();
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let mut buf: Vec<u8> = Vec::new();
    loop {
        buf.clear();
        match input.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let line = String::from_utf8_lossy(&buf);
        for out in server.handle_line(&line) {
            let mut o = std::io::stdout().lock();
            let _ = o.write_all(out.as_bytes());
            let _ = o.write_all(b"\n");
            let _ = o.flush();
        }
    }
}
