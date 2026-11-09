//! `_Organize Log/preferences.json`, written by DesktopGlass Settings (falls back to an old preferences.md).

use crate::py::{self, py_str, read_head, strip, strip_chars};
use crate::rules::LOG_DIR;
use serde_json::{json, Map, Value};
use std::path::Path;

#[derive(Clone, Debug, PartialEq)]
pub struct Prefs {
    pub version: Value,
    pub style: Value,
    pub private: Vec<String>,
    pub skip: Vec<String>,
    pub flag_status: Value,
    pub notes: Value,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            version: json!(1),
            style: json!("terse"),
            private: vec![],
            skip: vec![],
            flag_status: json!(true),
            notes: json!(""),
        }
    }
}

/// Iterating a JSON value the way Python iterates it (`for v in x`), for the `skip`/`private` lists.
fn py_iter(v: &Value) -> Vec<Value> {
    if !py::truthy(v) {
        return vec![];
    }
    match v {
        Value::Array(a) => a.clone(),
        Value::String(s) => s.chars().map(|c| Value::String(c.to_string())).collect(),
        Value::Object(o) => o.keys().map(|k| Value::String(k.clone())).collect(),
        _ => vec![], // Python raises here; an unusable list is treated as empty
    }
}

fn clean_list(v: &Value) -> Vec<String> {
    py_iter(v)
        .iter()
        .map(py_str)
        .filter(|s| !strip(s).is_empty())
        .map(|s| s.trim_matches('/').to_string())
        .collect()
}

/// Parses `- key: value` lines of the legacy preferences.md (`re.match(r"\s*[-*]?\s*(skip|private|style|notes)\s*:\s*(.*)", re.I)`).
fn legacy_line(line: &str) -> Option<(String, String)> {
    let mut s = line.trim_start_matches(py::is_space);
    if let Some(r) = s.strip_prefix(['-', '*']) {
        s = r.trim_start_matches(py::is_space);
    }
    let lower = s.to_ascii_lowercase();
    let key = ["skip", "private", "style", "notes"].into_iter().find(|k| lower.starts_with(k))?;
    let rest = s[key.len()..].trim_start_matches(py::is_space).strip_prefix(':')?;
    let val = rest.trim_start_matches(py::is_space);
    Some((key.to_string(), strip(val).to_string()))
}

pub fn load(root: &Path) -> Prefs {
    let log = root.join(LOG_DIR);
    let mut raw = Map::new();
    let mut lists: (Option<Value>, Option<Value>) = (None, None);
    let text = read_head(&log.join("preferences.json"), 200_000);
    if !strip(&text).is_empty() {
        if let Ok(Value::Object(o)) = serde_json::from_str::<Value>(&text) {
            raw = o;
        }
    } else {
        for line in py::splitlines(&read_head(&log.join("preferences.md"), 20_000)) {
            let Some((key, val)) = legacy_line(line) else { continue };
            if key == "skip" || key == "private" {
                let items: Vec<Value> = val
                    .split(',')
                    .filter(|v| !strip(v).is_empty())
                    .map(|v| Value::String(strip_chars(strip_chars(strip(v), "`"), "/").to_string()))
                    .collect();
                raw.insert(key, Value::Array(items));
            } else {
                raw.insert(key, Value::String(val));
            }
        }
    }
    let mut p = Prefs::default();
    for (k, v) in raw {
        match k.as_str() {
            "version" => p.version = v,
            "style" => p.style = v,
            "flagStatus" => p.flag_status = v,
            "notes" => p.notes = v,
            "private" => lists.0 = Some(v),
            "skip" => lists.1 = Some(v),
            _ => {}
        }
    }
    p.private = lists.0.map(|v| clean_list(&v)).unwrap_or_default();
    p.skip = lists.1.map(|v| clean_list(&v)).unwrap_or_default();
    p
}

impl Prefs {
    /// `str(prefs.get("notes") or "").strip()`
    pub fn notes_text(&self) -> String {
        if py::truthy(&self.notes) {
            strip(&py_str(&self.notes)).to_string()
        } else {
            String::new()
        }
    }
}
