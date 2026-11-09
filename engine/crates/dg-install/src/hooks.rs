//! What the agent hooks and `dg view` do (replaces session_context.py, autoshow.py and dgview).
//!
//! Both hooks act only in sessions started from DesktopGlass's own terminal (`TERM_PROGRAM=DesktopGlass`); anywhere
//! else they print and do nothing, and they always exit 0 so they never block an agent.

use crate::pyjson::dumps;
use dg_core::py;
use serde_json::{json, Value};
use std::path::{Component, Path, PathBuf};

// ---------- session-start context ----------

pub const FORMATS: [&str; 4] = ["claude", "copilot", "cursor", "antigravity"];

/// The text to add to a session, or None when it wasn't started from DesktopGlass (or the context is turned off
/// with DESKTOPGLASS_SESSION_CONTEXT=0). `dg` is the command the agent can run to show a file.
pub fn context(env: impl Fn(&str) -> Option<String>, dg: &str) -> Option<String> {
    if env("TERM_PROGRAM").as_deref() != Some("DesktopGlass") || env("DESKTOPGLASS_SESSION_CONTEXT").as_deref() == Some("0") {
        return None;
    }
    Some(format!(
        "This session is running inside DesktopGlass's terminal (TERM_PROGRAM=DesktopGlass), so the user is \
         looking at DesktopGlass right now. Show any visual output there: call the desktopglass `show_artifact` \
         tool with the file's absolute path, or run `{dg} view <absolute path>` if that tool isn't available."
    ))
}

/// The session-start output each CLI reads; unknown formats get Claude Code's.
pub fn payload(text: &str, fmt: &str) -> Value {
    match fmt {
        "copilot" => json!({"additionalContext": text}),
        "cursor" => json!({"additional_context": text}),
        "antigravity" => json!({"injectSteps": [{"systemMessage": {"systemMessage": text}}]}),
        _ => json!({"hookSpecificOutput": {"hookEventName": "SessionStart", "additionalContext": text}}),
    }
}

/// `dg hook session [--format F]`: the line to print, if any.
pub fn session_output(args: &[String], env: impl Fn(&str) -> Option<String>, dg: &str) -> Option<String> {
    let fmt = args
        .iter()
        .position(|a| a == "--format")
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
        .unwrap_or("claude");
    let fmt = if FORMATS.contains(&fmt) { fmt } else { "claude" };
    context(env, dg).map(|t| dumps(&payload(&t, fmt), None))
}

// ---------- autoshow (Claude Code PostToolUse) ----------

const VIEWABLE: [&str; 3] = [".html", ".htm", ".svg"];
const SKIP_PARTS: [&str; 9] = ["node_modules", ".git", "dist", "build", ".next", "vendor", "coverage", ".venv", "venv"];

/// Whether autoshow may act in this environment.
pub fn autoshow_enabled(env: impl Fn(&str) -> Option<String>) -> bool {
    env("TERM_PROGRAM").as_deref() == Some("DesktopGlass") && env("DESKTOPGLASS_AUTOSHOW").as_deref() != Some("0")
}

/// `Path(raw)` as pathlib normalizes it: repeated slashes and `.` parts dropped (`..` kept).
fn normalize(p: &Path) -> PathBuf {
    p.components().filter(|c| !matches!(c, Component::CurDir)).collect()
}

/// The file to show for a hook event, or None.
pub fn target(event: &Value, cwd: &Path) -> Option<PathBuf> {
    let input = event.get("tool_input").filter(|v| py::truthy(v));
    let pick = |k: &str| input.and_then(|i| i.get(k)).filter(|v| py::truthy(v)).map(py::py_str);
    let raw = pick("file_path").or_else(|| pick("path"))?;
    let mut path = normalize(Path::new(&raw));
    if !path.is_absolute() {
        let base = event.get("cwd").filter(|v| py::truthy(v)).map(|v| PathBuf::from(py::py_str(v))).unwrap_or_else(|| cwd.to_path_buf());
        path = normalize(&base.join(path));
    }
    let name = path.file_name()?.to_string_lossy().into_owned();
    let suffix = py::suffix(&name).to_lowercase();
    if !VIEWABLE.contains(&suffix.as_str()) {
        return None;
    }
    if path.components().any(|c| matches!(c, Component::Normal(s) if SKIP_PARTS.iter().any(|p| s == *p))) {
        return None;
    }
    if suffix != ".svg" && in_js_project(&path, 3) {
        return None;
    }
    path.is_file().then_some(path)
}

/// An index.html next to a package.json is a framework entry point, not something to open directly.
pub fn in_js_project(path: &Path, levels: usize) -> bool {
    let mut d = match path.parent() {
        Some(d) => d.to_path_buf(),
        None => return false,
    };
    for _ in 0..levels {
        if d.join("package.json").exists() {
            return true;
        }
        match d.parent() {
            Some(p) if p != d => d = p.to_path_buf(),
            _ => break,
        }
    }
    false
}

// ---------- the viewer ----------

/// `urllib.parse.quote(s, safe="")`.
pub fn quote(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"_.-~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The link that shows a file (absolute path) or URL in the viewer.
pub fn open_link(target: &str, title: Option<&str>) -> String {
    let mut link = format!("desktopglass://open?url={}", quote(target));
    if let Some(t) = title.filter(|t| !t.is_empty()) {
        link.push_str("&title=");
        link.push_str(&quote(t));
    }
    link
}

/// `dg view [file | folder | URL] [title]`: the link to open. With no argument, the viewer's local-server picker.
/// An existing path is made absolute with symlinks resolved; anything else (a URL) is passed through.
pub fn view_link(args: &[String]) -> String {
    let Some(first) = args.first().filter(|a| !a.is_empty()) else { return "desktopglass://view".into() };
    let p = Path::new(first);
    let target = if p.exists() { dg_core::paths::resolve(p).to_string_lossy().into_owned() } else { first.clone() };
    open_link(&target, args.get(1).map(String::as_str))
}
