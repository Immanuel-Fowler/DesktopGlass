//! `dg mcp-install`, `dg hook` and `dg view` end to end. Every run gets a cleared environment with a temporary HOME,
//! CODEX_HOME and COPILOT_HOME, and DESKTOPGLASS_TOOL_PATH so only fake CLIs can be found. Nothing opens the viewer.

use dg_install::serde_json::{self, Value};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Home(PathBuf);

impl Home {
    fn new() -> Home {
        static N: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!("dg-cli-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("bin")).unwrap();
        Home(fs::canonicalize(&d).unwrap())
    }
    fn dg(&self, args: &[&str], env: &[(&str, &str)], stdin: &str) -> (i32, String) {
        let mut c = Command::new(env!("CARGO_BIN_EXE_dg"));
        c.args(args)
            .env_clear()
            .env("HOME", &self.0)
            .env("CODEX_HOME", self.0.join(".codex"))
            .env("COPILOT_HOME", self.0.join(".copilot"))
            .env("PATH", "/usr/bin:/bin")
            .env("DESKTOPGLASS_TOOL_PATH", self.0.join("bin"))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped());
        for (k, v) in env {
            c.env(k, v);
        }
        let mut child = c.spawn().unwrap();
        std::io::Write::write_all(child.stdin.as_mut().unwrap(), stdin.as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        (out.status.code().unwrap_or(-1), String::from_utf8(out.stdout).unwrap())
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn session_hook_speaks_only_inside_desktopglass() {
    let h = Home::new();
    let (code, out) = h.dg(&["hook", "session", "--format", "claude"], &[("TERM_PROGRAM", "DesktopGlass")], "{}");
    assert_eq!(code, 0);
    let v: Value = serde_json::from_str(&out).unwrap();
    let text = v["hookSpecificOutput"]["additionalContext"].as_str().unwrap();
    assert!(text.contains(&format!("`{}/.local/bin/dg view <absolute path>`", h.0.display())), "{text}");
    let (_, out) = h.dg(&["hook", "session", "--format", "cursor"], &[("TERM_PROGRAM", "DesktopGlass")], "");
    assert!(serde_json::from_str::<Value>(&out).unwrap()["additional_context"].is_string());
    for env in [&[("TERM_PROGRAM", "iTerm.app")][..], &[], &[("TERM_PROGRAM", "DesktopGlass"), ("DESKTOPGLASS_SESSION_CONTEXT", "0")]] {
        assert_eq!(h.dg(&["hook", "session"], env, "{}"), (0, String::new()), "{env:?}");
    }
}

#[test]
fn autoshow_does_nothing_outside_desktopglass() {
    let h = Home::new();
    fs::write(h.0.join("a.html"), "<p>").unwrap();
    let event = format!(r#"{{"tool_input": {{"file_path": "{}/a.html"}}}}"#, h.0.display());
    assert_eq!(h.dg(&["hook", "autoshow"], &[], &event), (0, String::new()));
    assert_eq!(h.dg(&["hook", "autoshow"], &[("TERM_PROGRAM", "DesktopGlass"), ("DESKTOPGLASS_AUTOSHOW", "0")], &event), (0, String::new()));
    // Bad input never fails the agent. (Only non-files here, so nothing would be opened.)
    assert_eq!(h.dg(&["hook", "autoshow"], &[("TERM_PROGRAM", "DesktopGlass")], "not json").0, 0);
    assert_eq!(h.dg(&["hook", "autoshow"], &[("TERM_PROGRAM", "DesktopGlass")], r#"{"tool_input": {"file_path": "/nope.html"}}"#).0, 0);
    assert_eq!(h.dg(&["hook", "nonsense"], &[], "").0, 0);
}

#[test]
fn view_builds_the_viewer_link() {
    let h = Home::new();
    let print = [("DESKTOPGLASS_VIEW_PRINT", "1")];
    assert_eq!(h.dg(&["view"], &print, "").1, "desktopglass://view\n");
    fs::create_dir_all(h.0.join("my dir")).unwrap();
    fs::write(h.0.join("my dir/r.html"), "x").unwrap();
    std::os::unix::fs::symlink(h.0.join("my dir"), h.0.join("link")).unwrap();
    let want = format!("desktopglass://open?url={}", h.0.join("my dir/r.html").to_string_lossy().replace('/', "%2F").replace(' ', "%20"));
    assert_eq!(h.dg(&["view", &format!("{}/link/r.html", h.0.display())], &print, "").1, format!("{want}\n"));
    assert_eq!(h.dg(&["view", &format!("{}/link/r.html", h.0.display()), "My report"], &print, "").1, format!("{want}&title=My%20report\n"));
    assert_eq!(h.dg(&["view", "http://localhost:5173/?a=1"], &print, "").1,
               "desktopglass://open?url=http%3A%2F%2Flocalhost%3A5173%2F%3Fa%3D1\n");
}

#[test]
fn mcp_install_round_trip_in_a_temp_home() {
    let h = Home::new();
    fs::create_dir_all(h.0.join(".cursor")).unwrap();
    fs::create_dir_all(h.0.join(".codex")).unwrap();
    let (code, out) = h.dg(&["mcp-install", "install"], &[], "");
    assert_eq!(code, 0);
    let v: Value = serde_json::from_str(&out).unwrap();
    let dg = format!("{}/.local/bin/dg", h.0.display());
    assert_eq!(v["server"], serde_json::json!({"command": dg, "args": ["mcp"]}));
    // Apps in /Applications are detected too; their configs still live under the temp HOME.
    let done = v["done"].as_array().unwrap();
    assert!(done.contains(&"Codex".into()) && done.contains(&"Cursor".into()), "{done:?}");
    assert!(!done.contains(&"Claude Code".into()), "no claude on the tool path");
    let toml = fs::read_to_string(h.0.join(".codex/config.toml")).unwrap();
    assert_eq!(toml, format!("\n\n[mcp_servers.desktopglass]\ncommand = \"{dg}\"\nargs = [\"mcp\"]\n"));
    let (_, out) = h.dg(&["mcp-install", "uninstall"], &[], "");
    let v: Value = serde_json::from_str(&out).unwrap();
    assert!(v["clients"].as_array().unwrap().iter().all(|c| c["installed"] == false));
    assert!(!h.0.join(".codex/hooks.json").exists());
    assert!(!h.0.join(".codex/AGENTS.md").exists());
}
