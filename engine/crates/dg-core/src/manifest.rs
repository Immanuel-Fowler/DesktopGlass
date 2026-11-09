//! `_Organize Log/.manifest.json`: per-folder entry signatures from the last commit, used for change detection.

use crate::py::{self, read_head};
use crate::rules::{self, Container, LOG_DIR};
use crate::scan::entry_sig;
use serde_json::{json, Map, Value};
use std::path::Path;

pub const FILE: &str = ".manifest.json";

/// The saved manifest, or `{}` when it is missing or unreadable.
pub fn load(root: &Path) -> Value {
    let text = read_head(&root.join(LOG_DIR).join(FILE), 5_000_000);
    let text = if text.is_empty() { "{}" } else { text.as_str() };
    serde_json::from_str(text).unwrap_or_else(|_| Value::Object(Map::new()))
}

/// `{"version":1,"updated":…,"containers":{rel:{"entries":{name:sig}}}}`
pub fn build(containers: &[Container]) -> Value {
    let mut conts = Map::new();
    for c in containers {
        let entries: Map<String, Value> =
            rules::listdir(&c.path).into_iter().map(|n| { let s = entry_sig(&c.path.join(&n)); (n, json!(s)) }).collect();
        conts.insert(c.rel.clone(), json!({ "entries": entries }));
    }
    json!({
        "version": 1,
        "updated": py::strftime_local("%Y-%m-%dT%H:%M:%S", None),
        "containers": conts,
    })
}

/// Serialized exactly like `json.dumps(manifest, separators=(",", ":"))` (ASCII-escaped).
pub fn serialize(manifest: &Value) -> String {
    py::dumps(manifest, true)
}
