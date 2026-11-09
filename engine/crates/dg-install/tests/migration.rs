//! Python → dg migration: registrations and hooks written by mcp_install.py (`python3 …/mcp_server.py`,
//! `…/session_context.py`, `…/autoshow.py`) are moved to the dg form by `refresh` and `install`, the user's other
//! config is untouched, and `uninstall` removes both forms. Everything runs in a temporary home with a fake `claude`.

mod common;
use common::Home;
use dg_install::cli::mcp_install;
use serde_json::{json, Value};

const PY: &str = "/usr/bin/python3";
const SCRIPTS: &str = "/Users/old/.claude/skills/desktopglass-index/scripts";

fn server() -> String {
    format!("{SCRIPTS}/mcp_server.py")
}
fn py_session(fmt: Option<&str>) -> String {
    let base = format!("{PY} \"{SCRIPTS}/session_context.py\"");
    fmt.map(|f| format!("{base} --format {f}")).unwrap_or(base)
}
fn py_autoshow() -> String {
    format!("{PY} \"{SCRIPTS}/autoshow.py\"")
}

fn run(h: &Home, cmd: &str) -> Value {
    serde_json::from_str(&mcp_install(&h.ctx(), &[cmd.to_string()])).unwrap()
}

fn dg(h: &Home) -> String {
    format!("{}/.local/bin/dg", h.root.display())
}

/// A fake `claude` that keeps its one registration in a file next to it.
fn fake_claude(h: &Home) {
    let state = h.root.join("bin/claude-state");
    h.tool(
        "claude",
        &format!(
            r#"state="{}"
case "$2" in
  get) [ -f "$state" ] || exit 1; echo "desktopglass:"; cat "$state"; exit 0;;
  add) shift 6; echo "  Command: $*" > "$state";;
  remove) rm -f "$state";;
esac"#,
            state.display()
        ),
    );
}

/// A home where the Python installer registered DesktopGlass everywhere, next to the user's own config.
fn python_era_home() -> Home {
    let h = Home::new();
    fake_claude(&h);
    h.file("bin/claude-state", &format!("  Command: {PY} {}\n", server()));
    let py_entry = json!({"command": PY, "args": [server()]});
    let mine = json!({"command": "npx", "args": ["other-server"]});
    h.file(".claude/settings.json", &json!({"model": "opus", "hooks": {
        "SessionStart": [{"hooks": [{"type": "command", "command": "echo hi"}, {"type": "command", "command": py_session(None), "timeout": 5}]}],
        "PostToolUse": [{"matcher": "Write|Edit|MultiEdit", "hooks": [{"type": "command", "command": py_autoshow(), "timeout": 10}]}]
    }}).to_string());
    h.file(".cursor/mcp.json", &json!({"mcpServers": {"other": mine, "desktopglass": py_entry}}).to_string());
    h.file(".cursor/hooks.json", &json!({"version": 1, "hooks": {"sessionStart": [{"command": "./audit.sh"}, {"command": py_session(Some("cursor"))}]}}).to_string());
    h.file(".gemini/settings.json", &json!({"theme": "Dracula", "mcpServers": {"desktopglass": py_entry},
        "hooks": {"SessionStart": [{"hooks": [{"type": "command", "command": py_session(None), "timeout": 5000}]}]}}).to_string());
    h.dir(".gemini/antigravity");
    h.file(".gemini/config/mcp_config.json", &json!({"mcpServers": {"desktopglass": py_entry}}).to_string());
    h.file(".gemini/config/hooks.json", &json!({"mine": {"Stop": []}, "desktopglass": {"SessionStart": [
        {"type": "command", "command": py_session(Some("antigravity")), "timeout": 5}]}}).to_string());
    h.file(".copilot/mcp-config.json", &json!({"mcpServers": {"desktopglass": {"type": "local", "command": PY, "args": [server()], "tools": ["*"]}}}).to_string());
    h.file(".copilot/hooks/desktopglass.json", &json!({"version": 1, "hooks": {"sessionStart": [
        {"type": "command", "bash": py_session(Some("copilot")), "timeoutSec": 5}]}}).to_string());
    h.file(".config/opencode/opencode.json", &json!({"theme": "x", "mcp": {"desktopglass": {"type": "local", "command": [PY, server()], "enabled": true}}}).to_string());
    h.file(".config/opencode/plugins/desktopglass.js", "// DesktopGlass (session_context): old plugin\n");
    h.file("Library/Application Support/Code/User/mcp.json", &json!({"servers": {"desktopglass": {"type": "stdio", "command": PY, "args": [server()]}}}).to_string());
    h.file(".codex/config.toml", &format!(
        "model = \"o3\"\n\n[mcp_servers.desktopglass]\ncommand = \"{PY}\"\nargs = [\"{}\"]\n\n[profiles.fast]\nmodel = \"o4-mini\"\n", server()));
    h.file(".codex/hooks.json", &json!({"hooks": {"SessionStart": [{"hooks": [{"type": "command", "command": py_session(None), "timeout": 5}]}]}}).to_string());
    h
}

fn no_python_left(h: &Home) {
    for rel in [
        ".claude/settings.json", ".cursor/mcp.json", ".cursor/hooks.json", ".gemini/settings.json",
        ".gemini/config/mcp_config.json", ".gemini/config/hooks.json", ".copilot/mcp-config.json",
        ".copilot/hooks/desktopglass.json", ".config/opencode/opencode.json", ".config/opencode/plugins/desktopglass.js",
        "Library/Application Support/Code/User/mcp.json", ".codex/config.toml", ".codex/hooks.json", "bin/claude-state",
    ] {
        let t = h.read(rel);
        for old in ["mcp_server.py", "session_context.py", "autoshow.py", "python3"] {
            assert!(!t.contains(old), "{rel} still has {old}:\n{t}");
        }
    }
}

fn check_migrated(h: &Home) {
    let dg = dg(h);
    let entry = json!({"command": dg, "args": ["mcp"]});
    no_python_left(h);
    assert_eq!(h.read("bin/claude-state"), format!("  Command: {dg} mcp\n"));

    let s = h.json(".claude/settings.json");
    assert_eq!(s["model"], "opus");
    assert_eq!(s["hooks"]["SessionStart"], json!([
        {"hooks": [{"type": "command", "command": "echo hi"}]},
        {"hooks": [{"type": "command", "command": format!("{dg} hook session --format claude"), "timeout": 5}]}]));
    assert_eq!(s["hooks"]["PostToolUse"], json!([{"matcher": "Write|Edit|MultiEdit", "hooks": [
        {"type": "command", "command": format!("{dg} hook autoshow"), "timeout": 10}]}]));

    let c = h.json(".cursor/mcp.json");
    assert_eq!(c["mcpServers"]["other"], json!({"command": "npx", "args": ["other-server"]}));
    assert_eq!(c["mcpServers"]["desktopglass"], entry);
    let keys: Vec<&String> = c["mcpServers"].as_object().unwrap().keys().collect();
    assert_eq!(keys, ["other", "desktopglass"], "order kept");
    assert_eq!(h.json(".cursor/hooks.json"), json!({"version": 1, "hooks": {"sessionStart": [
        {"command": "./audit.sh"}, {"command": format!("{dg} hook session --format cursor")}]}}));

    let g = h.json(".gemini/settings.json");
    assert_eq!(g["theme"], "Dracula");
    assert_eq!(g["mcpServers"]["desktopglass"], entry);
    assert_eq!(g["hooks"]["SessionStart"], json!([{"hooks": [
        {"type": "command", "command": format!("{dg} hook session --format claude"), "timeout": 5000}]}]));
    assert_eq!(h.json(".gemini/config/mcp_config.json")["mcpServers"]["desktopglass"], entry);
    let a = h.json(".gemini/config/hooks.json");
    assert_eq!(a["mine"], json!({"Stop": []}));
    assert_eq!(a["desktopglass"]["SessionStart"][0]["command"], format!("{dg} hook session --format antigravity"));

    assert_eq!(h.json(".copilot/mcp-config.json")["mcpServers"]["desktopglass"],
               json!({"type": "local", "command": dg, "args": ["mcp"], "tools": ["*"]}));
    assert_eq!(h.json(".copilot/hooks/desktopglass.json")["hooks"]["sessionStart"][0]["bash"],
               format!("{dg} hook session --format copilot"));
    let o = h.json(".config/opencode/opencode.json");
    assert_eq!(o["theme"], "x");
    assert_eq!(o["mcp"]["desktopglass"], json!({"type": "local", "command": [dg, "mcp"], "enabled": true}));
    assert!(h.read(".config/opencode/plugins/desktopglass.js").contains("experimental.chat.system.transform"));
    assert_eq!(h.json("Library/Application Support/Code/User/mcp.json")["servers"]["desktopglass"],
               json!({"type": "stdio", "command": dg, "args": ["mcp"]}));

    let toml = h.read(".codex/config.toml");
    assert!(toml.starts_with("model = \"o3\"\n"), "{toml}");
    assert!(toml.contains("[profiles.fast]\nmodel = \"o4-mini\"\n"), "{toml}");
    assert!(toml.contains(&format!("[mcp_servers.desktopglass]\ncommand = \"{dg}\"\nargs = [\"mcp\"]\n")), "{toml}");
    assert_eq!(toml.matches("[mcp_servers.desktopglass]").count(), 1);
    assert_eq!(h.json(".codex/hooks.json")["hooks"]["SessionStart"][0]["hooks"][0]["command"],
               format!("{dg} hook session --format claude"));
}

#[test]
fn refresh_moves_every_python_registration_to_dg() {
    let h = python_era_home();
    let out = run(&h, "refresh");
    assert_eq!(out["errors"], json!([]));
    assert_eq!(out["server"], json!({"command": dg(&h), "args": ["mcp"]}));
    for name in ["Claude Code", "Codex", "Cursor", "Gemini CLI", "Antigravity", "GitHub Copilot CLI", "OpenCode", "VS Code"] {
        assert!(out["done"].as_array().unwrap().contains(&json!(name)), "{name}: {out}");
    }
    check_migrated(&h);
    // Running it again changes nothing.
    let before = h.read(".claude/settings.json") + &h.read(".codex/config.toml") + &h.read(".cursor/hooks.json");
    run(&h, "refresh");
    assert_eq!(before, h.read(".claude/settings.json") + &h.read(".codex/config.toml") + &h.read(".cursor/hooks.json"));
}

#[test]
fn install_also_migrates() {
    let h = python_era_home();
    let out = run(&h, "install");
    assert_eq!(out["errors"], json!([]));
    check_migrated(&h);
    for row in out["clients"].as_array().unwrap().iter().filter(|r| r["detected"] == true) {
        assert_eq!(row["installed"], true, "{row}");
        for k in ["instructions", "sessionHook", "autoshow"] {
            assert!(row.get(k).is_none_or(|v| v == true), "{row}");
        }
    }
}

#[test]
fn uninstall_removes_python_and_dg_forms() {
    // Python form only.
    let h = python_era_home();
    run(&h, "uninstall");
    assert!(!h.exists("bin/claude-state"));
    assert_eq!(h.json(".claude/settings.json"), json!({"model": "opus", "hooks": {"SessionStart": [{"hooks": [{"type": "command", "command": "echo hi"}]}]}}));
    assert_eq!(h.json(".cursor/hooks.json"), json!({"version": 1, "hooks": {"sessionStart": [{"command": "./audit.sh"}]}}));
    assert_eq!(h.json(".gemini/settings.json"), json!({"theme": "Dracula", "mcpServers": {}}));
    assert_eq!(h.json(".gemini/config/hooks.json"), json!({"mine": {"Stop": []}}));
    assert!(!h.exists(".copilot/hooks/desktopglass.json"));
    assert!(!h.exists(".config/opencode/plugins/desktopglass.js"));
    assert!(!h.exists(".codex/hooks.json"));
    assert_eq!(h.read(".codex/config.toml"), "model = \"o3\"\n\n[profiles.fast]\nmodel = \"o4-mini\"\n");
    let out = run(&h, "status");
    for row in out["clients"].as_array().unwrap() {
        for k in ["installed", "instructions", "sessionHook", "autoshow"] {
            assert!(row.get(k).is_none_or(|v| v == false), "{row}");
        }
    }

    // A mix: some hooks in the dg form, some still Python (e.g. a half-finished upgrade).
    let h = python_era_home();
    run(&h, "install");
    let s = h.read(".claude/settings.json");
    let mixed: Value = serde_json::from_str(&s).unwrap();
    let mut mixed = mixed;
    mixed["hooks"]["SessionStart"].as_array_mut().unwrap().push(json!({"hooks": [{"type": "command", "command": py_session(None)}]}));
    h.file(".claude/settings.json", &mixed.to_string());
    run(&h, "uninstall");
    assert_eq!(h.json(".claude/settings.json"), json!({"model": "opus", "hooks": {"SessionStart": [{"hooks": [{"type": "command", "command": "echo hi"}]}]}}));
}

#[test]
fn a_config_that_isnt_json_is_skipped_not_overwritten() {
    let h = Home::new();
    h.file(".cursor/mcp.json", "{ // mine\n}");
    let out = run(&h, "install");
    assert_eq!(out["errors"], json!(["Cursor: skipped (its config file isn't plain JSON)"]));
    assert_eq!(h.read(".cursor/mcp.json"), "{ // mine\n}");
}

#[test]
fn backups_are_made_once() {
    let h = Home::new();
    h.file(".lmstudio/mcp.json", r#"{"mcpServers": {"x": {"command": "y"}}}"#);
    run(&h, "install");
    run(&h, "uninstall");
    run(&h, "install");
    assert_eq!(h.read(".lmstudio/mcp.json.desktopglass-backup"), r#"{"mcpServers": {"x": {"command": "y"}}}"#);
}

#[test]
fn status_writes_nothing() {
    let h = python_era_home();
    let snapshot = |h: &Home| {
        let mut v = Vec::new();
        for e in walk(&h.root) {
            v.push((e.clone(), std::fs::read(&e).unwrap()));
        }
        v
    };
    let before = snapshot(&h);
    let out = run(&h, "status");
    assert_eq!(out["clients"][0]["installed"], true, "Python registrations still count as installed");
    assert_eq!(before, snapshot(&h));
}

fn walk(d: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(d).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() { out.extend(walk(&p)) } else { out.push(p) }
    }
    out.sort();
    out
}
