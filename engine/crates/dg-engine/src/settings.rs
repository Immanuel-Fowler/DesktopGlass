//! settings.json: the settings both the engine and the UI need (dg-proto `SHARED_SETTINGS`). The engine owns the
//! file; the UI mirrors its own copies (UserDefaults, for SwiftUI bindings) through `settings/get|set` and the
//! `settings/changed` notification. One-shot CLIs (`dg reindex`, `dg mcp`) read it via `dg_platform::read_setting`.
//!
//! First run: if settings.json doesn't exist, every shared key is copied from UserDefaults once, with its type.

use crate::Engine;
use serde_json::{json, Map, Value};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

/// Every backend id (Harness.swift raw values) and MCP group, to expand the prefix keys during migration.
pub const HARNESSES: &[&str] = &[
    "claude", "codex", "gemini", "antigravity", "cursor", "copilot", "opencode", "ollama", "lmstudio", "llamacpp", "mlx",
    "apple",
];
pub const MCP_GROUPS: &[&str] = &["edit", "control", "script", "play"];

pub struct Settings {
    path: PathBuf,
    map: Mutex<Map<String, Value>>,
    stamp: Mutex<Option<SystemTime>>,
}

fn load(path: &PathBuf) -> Map<String, Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

fn mtime(path: &PathBuf) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

impl Settings {
    pub fn open() -> Settings {
        Self::open_at(dg_platform::settings_file())
    }

    pub fn open_at(path: PathBuf) -> Settings {
        let map = load(&path);
        Settings { stamp: Mutex::new(mtime(&path)), path, map: Mutex::new(map) }
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    pub fn get(&self, keys: Option<&[String]>) -> Value {
        let map = self.map.lock().unwrap();
        match keys {
            None => Value::Object(map.clone()),
            Some(keys) => Value::Object(keys.iter().filter_map(|k| map.get(k).map(|v| (k.clone(), v.clone()))).collect()),
        }
    }

    pub fn value(&self, key: &str) -> Option<Value> {
        self.map.lock().unwrap().get(key).cloned()
    }

    pub fn string(&self, key: &str) -> Option<String> {
        match self.value(key)? {
            Value::String(s) => Some(s),
            Value::Null => None,
            v => Some(v.to_string()),
        }
    }

    /// Bools, 0/1 numbers and "1"/"true"/"yes" strings; missing = false.
    pub fn bool(&self, key: &str) -> bool {
        match self.value(key) {
            Some(Value::Bool(b)) => b,
            Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0) != 0.0,
            Some(Value::String(s)) => matches!(s.to_ascii_lowercase().as_str(), "1" | "true" | "yes"),
            _ => false,
        }
    }

    /// Applies `values` (null deletes) to shared keys only; returns the keys that actually changed.
    pub fn set(&self, values: &Map<String, Value>) -> Result<Vec<String>, String> {
        if let Some(bad) = values.keys().find(|k| !dg_proto::is_shared_setting(k)) {
            return Err(format!("{bad} isn't a shared setting"));
        }
        let mut changed = Vec::new();
        let snapshot = {
            let mut map = self.map.lock().unwrap();
            for (k, v) in values {
                let old = map.get(k).cloned();
                if v.is_null() {
                    if old.is_some() {
                        map.remove(k);
                        changed.push(k.clone());
                    }
                } else if old.as_ref() != Some(v) {
                    map.insert(k.clone(), v.clone());
                    changed.push(k.clone());
                }
            }
            map.clone()
        };
        if !changed.is_empty() {
            self.save(&snapshot).map_err(|e| format!("Couldn't save settings: {e}"))?;
        }
        Ok(changed)
    }

    fn save(&self, map: &Map<String, Value>) -> std::io::Result<()> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::create_dir_all(self.path.parent().unwrap())?;
        let tmp = self.path.with_extension(format!("json.{}.tmp", std::process::id()));
        {
            let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
            f.write_all(serde_json::to_string_pretty(&Value::Object(map.clone()))?.as_bytes())?;
            f.write_all(b"\n")?;
        }
        std::fs::rename(&tmp, &self.path)?;
        *self.stamp.lock().unwrap() = mtime(&self.path);
        Ok(())
    }

    /// Re-reads the file if someone else changed it; returns the keys whose values differ.
    pub fn reload_if_changed(&self) -> Vec<String> {
        let now = mtime(&self.path);
        {
            let mut stamp = self.stamp.lock().unwrap();
            if *stamp == now {
                return vec![];
            }
            *stamp = now;
        }
        let fresh = load(&self.path);
        let mut map = self.map.lock().unwrap();
        let mut keys: Vec<String> =
            fresh.iter().filter(|(k, v)| map.get(*k) != Some(*v)).map(|(k, _)| k.clone()).collect();
        keys.extend(map.keys().filter(|k| !fresh.contains_key(*k)).cloned());
        *map = fresh;
        keys
    }

    /// Copies every shared key from UserDefaults once (when settings.json doesn't exist yet).
    pub fn migrate_from_user_defaults(&self) {
        if self.path.exists() {
            return;
        }
        let migrated = read_user_defaults(&all_shared_keys());
        let _ = self.set(&migrated);
        if migrated.is_empty() {
            let _ = self.save(&Map::new()); // still mark the migration as done
        }
    }
}

/// Exact keys to look for in UserDefaults (prefix keys expanded over backends / MCP groups).
pub fn all_shared_keys() -> Vec<String> {
    let mut keys = Vec::new();
    for k in dg_proto::SHARED_SETTINGS {
        match *k {
            "model." | "endpoint." | "unattended." => keys.extend(HARNESSES.iter().map(|h| format!("{k}{h}"))),
            "mcp.allow." => keys.extend(MCP_GROUPS.iter().map(|g| format!("{k}{g}"))),
            k => keys.push(k.to_string()),
        }
    }
    keys
}

/// `defaults read-type` + `defaults read` for each key, in one shell, typed into JSON.
fn read_user_defaults(keys: &[String]) -> Map<String, Value> {
    let script = r#"id="$1"; shift
for k in "$@"; do
  t=$(/usr/bin/defaults read-type "$id" "$k" 2>/dev/null) || continue
  v=$(/usr/bin/defaults read "$id" "$k" 2>/dev/null) || continue
  printf '%s\t%s\t%s\0' "$k" "${t#Type is }" "$v"
done"#;
    let out = Command::new("/bin/sh").arg("-c").arg(script).arg("sh").arg(dg_platform::BUNDLE_ID).args(keys).output();
    let mut map = Map::new();
    let Ok(out) = out else { return map };
    for rec in out.stdout.split(|b| *b == 0) {
        let rec = String::from_utf8_lossy(rec);
        let mut parts = rec.splitn(3, '\t');
        let (Some(k), Some(t), Some(v)) = (parts.next(), parts.next(), parts.next()) else { continue };
        if let Some(value) = typed(t.trim(), v.trim()) {
            map.insert(k.to_string(), value);
        }
    }
    map
}

/// A `defaults` value of the given type as JSON (dates stay ISO-like strings; arrays/dicts are skipped).
pub fn typed(kind: &str, v: &str) -> Option<Value> {
    match kind {
        "boolean" => Some(Value::Bool(v == "1")),
        "integer" => v.parse::<i64>().ok().map(Value::from),
        "float" => v.parse::<f64>().ok().map(|f| json!(f)),
        "string" | "date" => Some(Value::String(v.to_string())),
        _ => None,
    }
}

/// Picks up edits made by other processes (e.g. a future `dg settings set`) and tells the UI.
pub fn watch_external_changes(engine: Arc<Engine>) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(1));
        let keys = engine.settings.reload_if_changed();
        if !keys.is_empty() {
            engine.link.notify(dg_proto::notify::SETTINGS_CHANGED, json!({"keys": keys}));
        }
    });
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub struct TempHome(PathBuf);
    impl TempHome {
        pub fn path(&self) -> &PathBuf {
            &self.0
        }
    }
    impl Drop for TempHome {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    pub fn temp_home(tag: &str) -> TempHome {
        let d = std::env::temp_dir().join(format!("dg-engine-{tag}-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        TempHome(d)
    }

    #[test]
    fn set_only_shared_keys_and_report_changes() {
        let h = temp_home("settings");
        let s = Settings::open_at(h.path().join("settings.json"));
        let mut v = Map::new();
        v.insert("harness".into(), json!("codex"));
        v.insert("mcp.allow.edit".into(), json!(true));
        assert_eq!(s.set(&v).unwrap().len(), 2);
        assert!(s.set(&v).unwrap().is_empty(), "same values change nothing");
        assert!(s.bool("mcp.allow.edit") && !s.bool("mcp.allow.play"));
        assert_eq!(s.string("harness").as_deref(), Some("codex"));
        let mut bad = Map::new();
        bad.insert("hotkey".into(), json!("x"));
        assert!(s.set(&bad).is_err());
        let mut del = Map::new();
        del.insert("harness".into(), Value::Null);
        assert_eq!(s.set(&del).unwrap(), vec!["harness".to_string()]);

        // persisted, mode 600, and readable by a fresh instance
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(s.path()).unwrap().permissions().mode() & 0o777, 0o600);
        let again = Settings::open_at(h.path().join("settings.json"));
        assert!(again.bool("mcp.allow.edit") && again.string("harness").is_none());
    }

    #[test]
    fn external_edits_are_detected() {
        let h = temp_home("reload");
        let p = h.path().join("settings.json");
        let s = Settings::open_at(p.clone());
        let mut v = Map::new();
        v.insert("schedule".into(), json!("daily"));
        s.set(&v).unwrap();
        assert!(s.reload_if_changed().is_empty());
        std::thread::sleep(Duration::from_millis(1100)); // mtime granularity
        std::fs::write(&p, r#"{"schedule":"hourly","scheduleHour":4}"#).unwrap();
        let mut keys = s.reload_if_changed();
        keys.sort();
        assert_eq!(keys, vec!["schedule".to_string(), "scheduleHour".to_string()]);
        assert_eq!(s.string("schedule").as_deref(), Some("hourly"));
    }

    #[test]
    fn typed_values_and_key_expansion() {
        assert_eq!(typed("boolean", "1"), Some(json!(true)));
        assert_eq!(typed("integer", "3"), Some(json!(3)));
        assert_eq!(typed("string", "daily"), Some(json!("daily")));
        assert_eq!(typed("dictionary", "{}"), None);
        let keys = all_shared_keys();
        assert!(keys.contains(&"model.claude".to_string()) && keys.contains(&"mcp.allow.play".to_string()));
        assert!(keys.contains(&"scheduleHour".to_string()) && !keys.iter().any(|k| k.ends_with('.')));
    }
}
