//! Everything that differs by OS. macOS only for now; Phase 6 adds the Windows side behind the same functions.
//! Identity (bundle id, version) comes from the repo's app.conf at build time.

use std::path::PathBuf;
use std::process::{Command, Stdio};

pub const VERSION: &str = env!("DG_VERSION");
pub const BUILD: &str = env!("DG_BUILD");
pub const BUNDLE_ID: &str = env!("DG_BUNDLE_ID");

pub fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

/// The user's Desktop folder.
pub fn desktop() -> PathBuf {
    home().join("Desktop")
}

/// ~/Library/Application Support/DesktopGlass (created on demand by callers).
pub fn app_support() -> PathBuf {
    home().join("Library/Application Support/DesktopGlass")
}

/// The control channel's `{port, token, pid}` file.
pub fn control_file() -> PathBuf {
    app_support().join("control.json")
}

/// Shared settings (backend, models, schedule, `mcp.allow.*`, …) live here, owned by `dg engine`.
pub fn settings_file() -> PathBuf {
    app_support().join("settings.json")
}

/// All of settings.json (empty if missing or unreadable).
pub fn load_settings() -> serde_json::Map<String, serde_json::Value> {
    std::fs::read_to_string(settings_file())
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

/// Writes settings.json atomically (temp file + rename), mode 600.
pub fn save_settings(map: &serde_json::Map<String, serde_json::Value>) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let path = settings_file();
    std::fs::create_dir_all(path.parent().unwrap())?;
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    let text = serde_json::to_string_pretty(&serde_json::Value::Object(map.clone())).unwrap_or_else(|_| "{}".into());
    {
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.write_all(b"\n")?;
    }
    std::fs::rename(&tmp, &path)
}

/// A settings.json value rendered the way `defaults read` prints it (bools as 1/0), so callers see one format.
fn setting_text(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::Null => None,
        serde_json::Value::Bool(b) => Some(if *b { "1" } else { "0" }.into()),
        serde_json::Value::String(s) => Some(s.trim().to_string()),
        other => Some(other.to_string()),
    }
}

/// One app setting, trimmed; None if unset. Reads settings.json first (the engine's store for shared settings),
/// then UserDefaults (`defaults read <bundle id> <key>`) for anything not migrated yet.
/// `DG_SETTINGS_FILE=<path>` points at a test settings file and skips UserDefaults entirely.
pub fn read_setting(key: &str) -> Option<String> {
    if let Some(path) = std::env::var_os("DG_SETTINGS_FILE") {
        let map = std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default();
        return map.get(key).and_then(setting_text);
    }
    if let Some(v) = load_settings().get(key) {
        return setting_text(v);
    }
    read_user_default(key)
}

/// `defaults read <bundle id> <key>`, trimmed; None if unset.
pub fn read_user_default(key: &str) -> Option<String> {
    let out = Command::new("/usr/bin/defaults")
        .args(["read", BUNDLE_ID, key])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Some(s)
}

/// A boolean setting: `1`/`true`/`yes` → true; unset → `default`.
pub fn read_bool_setting(key: &str, default: bool) -> bool {
    match read_setting(key) {
        Some(v) => matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes"),
        None => default,
    }
}

/// Where DesktopGlass.app is installed, if found.
pub fn app_path() -> Option<PathBuf> {
    [home().join("Applications/DesktopGlass.app"), PathBuf::from("/Applications/DesktopGlass.app")]
        .into_iter()
        .find(|p| p.exists())
}

/// Launches the app in the background without stealing focus (`open -g`). Returns false if it isn't installed.
pub fn launch_app_background() -> bool {
    let target = match app_path() {
        Some(p) => p.to_string_lossy().into_owned(),
        None => return false,
    };
    Command::new("/usr/bin/open")
        .args(["-g", &target])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Opens a URL (http(s)://, desktopglass://, file path) with the system handler; `background` keeps focus.
pub fn open_url(url: &str, background: bool) -> bool {
    let mut c = Command::new("/usr/bin/open");
    if background {
        c.arg("-g");
    }
    c.arg(url).stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
}
