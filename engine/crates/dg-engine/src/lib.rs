//! `dg engine`: the long-running engine, spawned and supervised by the UI (one per user session).
//!
//! - **UI link**: JSON-RPC 2.0 over stdin/stdout, one message per line (see `dg-proto`). EOF on stdin = the UI is
//!   gone, so the engine exits (and removes control.json).
//! - **Control channel**: owns the loopback listener, token and control.json that `ControlServer.swift` used to.
//!   Answers what it can itself; forwards UI-bound methods (browser, open editor buffers, windows, games.play) to
//!   the UI as `ui/control` requests. Same wire format as before, so MCP clients don't notice the move.
//! - **Settings**: owns settings.json for the settings both sides need (dg-proto `SHARED_SETTINGS`), migrated once
//!   from UserDefaults.
//! - **Watching**: FSEvents on the Desktop → debounced `desktop/changed` notifications.
//!
//! Engine-side control methods live in [`methods`]; adding one there is how logic moves out of Swift.

pub mod control;
pub mod link;
pub mod methods;
pub mod settings;
pub mod watch;

use serde_json::{json, Value};
use std::sync::Arc;

pub struct Engine {
    pub link: link::Link,
    pub settings: settings::Settings,
    pub control_port: std::sync::atomic::AtomicU16,
}

impl Engine {
    pub fn new(out: Box<dyn std::io::Write + Send>) -> Arc<Engine> {
        Self::with_settings(out, settings::Settings::open())
    }

    pub fn with_settings(out: Box<dyn std::io::Write + Send>, settings: settings::Settings) -> Arc<Engine> {
        Arc::new(Engine {
            link: link::Link::new(out),
            settings,
            control_port: std::sync::atomic::AtomicU16::new(0),
        })
    }

    /// A request from the UI over the link.
    pub fn handle_ui(self: &Arc<Self>, method: &str, params: &Value) -> Result<Value, String> {
        use dg_proto::ui_methods::*;
        match method {
            HELLO => Ok(json!({
                "version": dg_platform::VERSION,
                "protocol": dg_proto::PROTOCOL_VERSION,
                "pid": std::process::id(),
                "control": {"port": self.control_port.load(std::sync::atomic::Ordering::SeqCst)},
            })),
            SETTINGS_GET => {
                let keys: Option<Vec<String>> = params.get("keys").and_then(|k| k.as_array()).map(|a| {
                    a.iter().filter_map(|v| v.as_str().map(String::from)).collect()
                });
                Ok(self.settings.get(keys.as_deref()))
            }
            SETTINGS_SET => {
                let values = params.get("values").and_then(|v| v.as_object()).ok_or("values must be an object")?;
                let changed = self.settings.set(values)?;
                if !changed.is_empty() {
                    // The UI sent these itself; tell it anyway so other windows/observers stay in sync.
                    self.link.notify(dg_proto::notify::SETTINGS_CHANGED, json!({"keys": changed}));
                }
                Ok(json!({}))
            }
            SHUTDOWN => Ok(json!({})),
            // Control methods the UI wants answered by the engine too (e.g. games.list) use the same handlers.
            m => match methods::handle(self, m, params) {
                Some(r) => r,
                None => Err(format!("Unknown method {m}")),
            },
        }
    }

    /// A request from an MCP client over the control channel (already token-checked).
    pub fn handle_control(self: &Arc<Self>, method: &str, params: &Value) -> Result<Value, String> {
        // UI-link methods (`index/run`, `models/list`, …) are the app's own, never an MCP client's.
        if method.contains('/') {
            return Err(format!("Unknown method {method}"));
        }
        if let Some((group, label)) = dg_proto::gate(method) {
            if !self.settings.bool(&format!("mcp.allow.{group}")) {
                return Err(format!(
                    "“{label}” is turned off for AI apps — the user can allow it in DesktopGlass → Settings → MCP"
                ));
            }
        }
        if !dg_proto::is_ui_bound(method) {
            if let Some(r) = methods::handle(self, method, params) {
                return r;
            }
        }
        // Not ours (or not migrated yet): the UI answers.
        let timeout = std::time::Duration::from_secs(params.get("timeout").and_then(|t| t.as_u64()).unwrap_or(0).max(120) + 30);
        self.link.call(dg_proto::UI_CONTROL, json!({"method": method, "params": params}), timeout)
    }
}

/// Runs the engine until stdin closes. Returns the process exit code.
pub fn run(_args: &[String]) -> i32 {
    // Our stdout is the protocol channel; make sure nothing else in the process can write to it by accident.
    let out = link::take_stdout();
    let engine = Engine::new(out);
    engine.settings.migrate_from_user_defaults();

    let control = match control::Control::start(engine.clone()) {
        Ok(c) => {
            engine.control_port.store(c.port, std::sync::atomic::Ordering::SeqCst);
            Some(c)
        }
        Err(e) => {
            eprintln!("dg engine: control channel unavailable: {e}");
            None
        }
    };
    let _watch = watch::start(engine.clone(), dg_platform::desktop());
    settings::watch_external_changes(engine.clone());
    methods::start(&engine);

    link::serve(engine.clone(), std::io::stdin().lock());

    if let Some(c) = control {
        c.stop();
    }
    0
}
