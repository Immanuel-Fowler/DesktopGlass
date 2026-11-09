//! Control/UI methods the engine answers itself. Logic moves out of Swift by adding a case to one of the
//! submodules; a method every submodule returns None for is forwarded to the UI unchanged.
//!
//! - `desktop`: the Desktop model (folders, description.md, search) and indexing preferences
//! - `index`: re-indexing (runs, schedule, status, problems) and backend detection
//! - `tools`: HTML source ops, artifact policy, games, local servers

pub mod desktop;
pub mod index;
pub mod tools;

use crate::Engine;
use serde_json::{json, Value};
use std::sync::Arc;

pub fn handle(engine: &Arc<Engine>, method: &str, params: &Value) -> Option<Result<Value, String>> {
    if method == "ping" {
        return Some(Ok(json!({"ok": true, "version": dg_platform::VERSION})));
    }
    desktop::handle(engine, method, params)
        .or_else(|| index::handle(engine, method, params))
        .or_else(|| tools::handle(engine, method, params))
}

/// Called once when the engine starts (after settings and the control channel are up), so a module can start
/// background work (e.g. the re-index scheduler).
pub fn start(engine: &Arc<Engine>) {
    desktop::start(engine);
    index::start(engine);
    tools::start(engine);
}
