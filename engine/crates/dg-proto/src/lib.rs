//! Engine <-> UI and control-channel message shapes, defined once.
//!
//! **UI link** (`dg engine`, spawned by the app): JSON-RPC 2.0, one JSON object per line on stdin/stdout.
//! - UI → engine requests: the `ui_methods` below.
//! - engine → UI notifications: `notify::*`.
//! - engine → UI requests: [`UI_CONTROL`] — a control-channel call the engine can't answer itself (browser, open
//!   editor buffers, windows, launching games). The UI answers with `result` or `error: {code, message}`.
//!
//! **Control channel** (unchanged on the wire, see plans/README.md §5): loopback TCP, `control.json` holds
//! `{port, token, pid}` (mode 600); each request is one line `{"token", "method", "params"}`, each reply one line
//! `{"result": …}` or `{"error": "message"}`.

use serde_json::{json, Map, Value};

/// Bumped whenever a message shape changes; the UI refuses an engine with a different major.
pub const PROTOCOL_VERSION: u32 = 1;

/// Requests the UI sends to the engine.
pub mod ui_methods {
    /// `{}` → `{version, protocol, pid, control: {port}}`
    pub const HELLO: &str = "engine/hello";
    /// `{keys?: [..]}` → `{key: value, ...}` (all shared settings when `keys` is absent)
    pub const SETTINGS_GET: &str = "settings/get";
    /// `{values: {key: value|null}}` → `{}`; `null` deletes the key
    pub const SETTINGS_SET: &str = "settings/set";
    /// `{}` → `{}`; the engine exits after replying
    pub const SHUTDOWN: &str = "engine/shutdown";
}

/// Every UI → engine request the engine answers, with its params → result in brief. Each family's module in
/// dg-engine/src/methods documents the shapes in full; this list is what `dg schema` publishes.
pub const UI_METHODS: &[(&str, &str)] = &[
    ("engine/hello", "{} → {version, protocol, pid, control:{port}}"),
    ("engine/shutdown", "{} → {}"),
    ("settings/get", "{keys?} → {key: value}"),
    ("settings/set", "{values:{key: value|null}} → {}"),
    // Desktop model (methods/desktop.rs)
    ("desktop/load", "{} → {root, items, indexed, questions, undescribed}"),
    ("desktop/list", "{path} → {path, items}"),
    ("desktop/search", "{query, limit?} → {query, paths}"),
    ("desktop/describe", "{path} → {path, info|null}"),
    ("prefs/get", "{} → Prefs"),
    ("prefs/set", "{data?, folder?, mode?} → Prefs"),
    // Re-indexing and backends (methods/index.rs)
    ("index/status", "{} → {running, checkingAuth, status, problem, scheduleBlocked, lastRun, lastSuccess}"),
    ("index/run", "{reason, full?, allowUnattended?} → {started, running, status, problem}"),
    ("index/check_auth", "{} → same as index/run"),
    ("index/cancel", "{} → {cancelled}"),
    ("backends/detect", "{appleAvailable?, glassBin?} → {installed:{backend: path}}"),
    ("models/list", "{harness, force?, binary?} → {harness, models, live}"),
    ("models/presets", "{} → {backend: [model]}"),
    // Tools (methods/tools/*.rs)
    ("html/parse", "{path, source} → {annotated, stylesheets, nodes}"),
    ("html/declarations", "{path, source, destination, selector} → declarations"),
    ("html/outline", "{path, source} → text"),
    ("html/edit", "{path, source, buffers, op, …} → {edits, message, structural}"),
    ("html/control", "{method, params, buffers} → result of the html.* control method"),
    ("policy/tables", "{} → allow-list tables for the UI's synchronous matcher"),
    ("policy/check", "{url} → {allowed, signIn, webPage, …}"),
    ("policy/resolve", "{raw, address?} → target|null"),
    ("games/scan", "{root} → [game]"),
    ("games/search", "{root} → [found]"),
    ("games/add_shortcuts", "{root, games} → {summary, added, failed}"),
    ("games/history", "{lastPlayed, sort} → {}"),
    ("servers/list", "{exclude} → [server]"),
];

/// Every engine → UI notification.
pub const NOTIFICATIONS: &[(&str, &str)] = &[
    ("desktop/changed", "{paths}"),
    ("settings/changed", "{keys}"),
    ("prefs/changed", "Prefs"),
    ("index/progress", "{running, checkingAuth, status}"),
    ("index/problem", "{problem: null | {kind, harness?, message}}"),
    ("index/finished", "{code, status, authOnly}"),
    ("engine/log", "{message}"),
];

/// Notifications the engine sends to the UI.
pub mod notify {
    /// `{paths: [..]}` — something under the Desktop changed (debounced)
    pub const DESKTOP_CHANGED: &str = "desktop/changed";
    /// `{keys: [..]}` — shared settings changed (by the engine, an MCP client or another process)
    pub const SETTINGS_CHANGED: &str = "settings/changed";
    /// `{message}` — engine log line worth surfacing in the UI's console
    pub const LOG: &str = "engine/log";
}

/// Engine → UI request carrying a control-channel call: `{method, params}` → whatever the control call returns.
pub const UI_CONTROL: &str = "ui/control";

/// Control-channel methods that only the UI can answer (they need windows, WebViews or open editor buffers).
/// Everything else is answered by the engine.
pub fn is_ui_bound(method: &str) -> bool {
    method.starts_with("browser.")
        || method.starts_with("html.")
        || matches!(method, "editor.open" | "overlay.show" | "games.play")
}

/// MCP permission group for a control method (Settings → MCP), or None if always allowed.
/// Mirrors `AppControl.gates`; the setting is `mcp.allow.<group>`.
pub fn gate(method: &str) -> Option<(&'static str, &'static str)> {
    match method {
        "html.style" | "html.split_style" | "html.group_style" | "html.set_text" | "html.set_html" | "html.duplicate"
        | "html.remove" => Some(("edit", "Edit files")),
        "schedule.set" | "backend.set" | "prefs.set" | "index.run" => {
            Some(("control", "Change settings and start re-indexing"))
        }
        "browser.eval" | "browser.click" | "browser.type" => Some(("script", "Click, type and run JavaScript")),
        "games.play" => Some(("play", "Launch games")),
        _ => None,
    }
}

/// Settings shared by the engine and the UI, stored in `settings.json`. Exact keys, or prefixes ending in `.`.
pub const SHARED_SETTINGS: &[&str] = &[
    "harness", "model.", "endpoint.", "schedule", "scheduleHour", "mcp.allow.", "unattended.", "sandboxUnattended",
    "signedOutHarness", "lastReindexRun", "lastReindexSuccess", "gamesFolder",
];

pub fn is_shared_setting(key: &str) -> bool {
    SHARED_SETTINGS.iter().any(|k| if k.ends_with('.') { key.starts_with(k) } else { key == *k })
}

// ---- JSON-RPC helpers ----

pub fn request(id: u64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

pub fn notification(method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "method": method, "params": params})
}

pub fn result(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

pub fn error(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL: i64 = -32603;
/// The engine's own failures (a handler returned an error message).
pub const APP_ERROR: i64 = 1;

/// JSON Schema (draft 2020-12) describing the link messages, printed by `dg schema`.
pub fn schema() -> Value {
    let mut methods = Map::new();
    for (m, doc) in UI_METHODS {
        methods.insert((*m).into(), Value::String((*doc).into()));
    }
    methods.insert(UI_CONTROL.into(), Value::String("engine→UI: {method, params} → any".into()));
    for (m, doc) in NOTIFICATIONS {
        methods.insert((*m).into(), Value::String(format!("notification: {doc}")));
    }
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "DesktopGlass engine protocol",
        "protocolVersion": PROTOCOL_VERSION,
        "framing": "newline-delimited JSON-RPC 2.0 over the engine's stdin/stdout",
        "methods": Value::Object(methods),
        "sharedSettings": SHARED_SETTINGS,
        "$defs": {
            "request": {"type": "object", "required": ["jsonrpc", "id", "method"],
                "properties": {"jsonrpc": {"const": "2.0"}, "id": {"type": ["integer", "string"]},
                               "method": {"type": "string"}, "params": {"type": "object"}}},
            "notification": {"type": "object", "required": ["jsonrpc", "method"],
                "properties": {"jsonrpc": {"const": "2.0"}, "method": {"type": "string"}, "params": {"type": "object"}}},
            "response": {"type": "object", "required": ["jsonrpc", "id"],
                "properties": {"jsonrpc": {"const": "2.0"}, "id": {}, "result": {},
                    "error": {"type": "object", "required": ["code", "message"],
                              "properties": {"code": {"type": "integer"}, "message": {"type": "string"}}}}},
            "controlRequest": {"type": "object", "required": ["token", "method"],
                "properties": {"token": {"type": "string"}, "method": {"type": "string"}, "params": {"type": "object"}}},
            "controlReply": {"oneOf": [{"type": "object", "required": ["result"]},
                {"type": "object", "required": ["error"], "properties": {"error": {"type": "string"}}}]}
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_bound_and_gates_match_appcontrol() {
        assert!(is_ui_bound("browser.open") && is_ui_bound("html.style") && is_ui_bound("games.play"));
        assert!(!is_ui_bound("schedule.get") && !is_ui_bound("games.list") && !is_ui_bound("ping"));
        assert_eq!(gate("html.remove").unwrap().0, "edit");
        assert_eq!(gate("index.run").unwrap().0, "control");
        assert_eq!(gate("browser.type").unwrap().0, "script");
        assert_eq!(gate("games.play").unwrap().0, "play");
        assert!(gate("browser.read").is_none() && gate("html.outline").is_none());
    }

    #[test]
    fn every_engine_method_is_listed() {
        // keep in step with dg-engine/src/methods: a name used there must appear here
        let listed: Vec<&str> = UI_METHODS.iter().chain(NOTIFICATIONS).map(|(m, _)| *m).collect();
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../dg-engine/src/methods");
        let mut stack = vec![std::path::PathBuf::from(dir)];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() { stack.push(p); continue; }
                let text = std::fs::read_to_string(&p).unwrap_or_default();
                for fam in ["desktop", "prefs", "index", "backends", "models", "html", "policy", "games", "servers"] {
                    let pat = format!("\"{fam}/");
                    let mut rest = text.as_str();
                    while let Some(i) = rest.find(&pat) {
                        let tail = &rest[i + 1..];
                        let end = tail.find('"').unwrap_or(0);
                        let name = &tail[..end];
                        if name.chars().all(|c| c.is_ascii_lowercase() || c == '/' || c == '_') && name.contains('/') {
                            assert!(listed.contains(&name), "{name} (used in {}) is missing from dg-proto", p.display());
                        }
                        rest = &tail[end..];
                    }
                }
            }
        }
    }

    #[test]
    fn shared_settings() {
        for k in ["harness", "model.claude", "endpoint.ollama", "mcp.allow.edit", "unattended.codex", "scheduleHour"] {
            assert!(is_shared_setting(k), "{k}");
        }
        for k in ["hotkey", "island.show", "editorFontSize", "harnessX", "models"] {
            assert!(!is_shared_setting(k), "{k}");
        }
    }
}
