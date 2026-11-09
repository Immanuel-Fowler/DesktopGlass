//! Port of Tests/python/test_session_hook.py: the session-start hook speaks only inside DesktopGlass's terminal,
//! and installing it into each CLI's config leaves everything else alone.

mod common;
use common::Home;
use dg_install::clients::{clients, copilot_hook, extras_for, is_session, opencode_plugin, Extra};
use dg_install::hooks::{context, session_output};
use dg_install::Error;
use serde_json::{json, Value};
use std::path::PathBuf;

const DG: &str = "/Users/someone/.local/bin/dg";

fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let v: Vec<(String, String)> = pairs.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
    move |k: &str| v.iter().find(|(a, _)| a == k).map(|(_, b)| b.clone())
}

fn out(args: &[&str], e: &[(&str, &str)]) -> Option<Value> {
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    session_output(&args, env(e), DG).map(|s| serde_json::from_str(&s).unwrap())
}

const INSIDE: &[(&str, &str)] = &[("TERM_PROGRAM", "DesktopGlass")];

// ---------- Context ----------

#[test]
fn inside_desktopglass_it_adds_context() {
    let d = out(&[], INSIDE).unwrap();
    let d = &d["hookSpecificOutput"];
    assert_eq!(d["hookEventName"], "SessionStart");
    let text = d["additionalContext"].as_str().unwrap();
    assert!(text.contains("inside DesktopGlass"));
    assert!(text.contains("show_artifact"));
    assert!(text.contains(&format!("`{DG} view <absolute path>`")));
}

#[test]
fn elsewhere_it_prints_nothing() {
    for e in [&[("TERM_PROGRAM", "iTerm.app")][..], &[("TERM_PROGRAM", "Apple_Terminal")], &[]] {
        assert_eq!(out(&[], e), None, "{e:?}");
        assert_eq!(out(&["--format", "copilot"], e), None, "{e:?}");
    }
}

#[test]
fn it_can_be_turned_off_for_a_session() {
    assert_eq!(context(env(&[("TERM_PROGRAM", "DesktopGlass"), ("DESKTOPGLASS_SESSION_CONTEXT", "0")]), DG), None);
}

// ---------- Formats ----------

#[test]
fn outputs() {
    assert!(out(&[], INSIDE).unwrap()["hookSpecificOutput"]["additionalContext"].is_string());
    assert!(out(&["--format", "claude"], INSIDE).unwrap()["hookSpecificOutput"]["additionalContext"].is_string());
    let has = |v: &Value| v.as_str().unwrap().contains("inside DesktopGlass");
    assert!(has(&out(&["--format", "copilot"], INSIDE).unwrap()["additionalContext"]));
    assert!(has(&out(&["--format", "cursor"], INSIDE).unwrap()["additional_context"]));
    assert!(has(&out(&["--format", "antigravity"], INSIDE).unwrap()["injectSteps"][0]["systemMessage"]["systemMessage"]));
    assert!(out(&["--format", "nonsense"], INSIDE).unwrap().get("hookSpecificOutput").is_some(), "unknown formats fall back");
    assert!(out(&["--format"], INSIDE).unwrap().get("hookSpecificOutput").is_some());
}

// ---------- Install ----------

fn grouped(path: PathBuf, timeout: u64) -> Extra {
    Extra::SessionGrouped { path, command: format!("{DG} hook session --format claude"), timeout }
}

fn ours(h: &Home, rel: &str) -> Vec<Value> {
    h.json(rel)["hooks"]["SessionStart"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|g| g["hooks"].as_array().unwrap().clone())
        .filter(|x| is_session(x["command"].as_str().unwrap_or("")))
        .collect()
}

#[test]
fn claude_settings_keep_other_settings_and_hooks() {
    let h = Home::new();
    let theirs = json!({"type": "command", "command": "echo hi"});
    let before = json!({"model": "opus", "hooks": {
        "SessionStart": [{"hooks": [theirs]}], "PostToolUse": [{"matcher": "Bash", "hooks": [theirs]}]}});
    let path = h.file(".claude/settings.json", &before.to_string());
    let x = grouped(path, 5);
    x.install().unwrap();
    x.install().unwrap(); // twice: still one entry
    let o = ours(&h, ".claude/settings.json");
    assert_eq!(o.len(), 1);
    assert_eq!(o[0]["command"], format!("{DG} hook session --format claude"));
    assert!(x.installed());
    x.uninstall().unwrap();
    assert_eq!(h.json(".claude/settings.json"), before);
    assert!(!x.installed());
}

#[test]
fn codex_hooks_file_is_created_and_removed() {
    let h = Home::new();
    let x = grouped(h.root.join(".codex/hooks.json"), 5);
    x.install().unwrap();
    assert_eq!(ours(&h, ".codex/hooks.json").len(), 1);
    x.uninstall().unwrap();
    assert!(!h.exists(".codex/hooks.json"), "a hooks.json that only held our hook is removed");
}

#[test]
fn codex_hooks_file_keeps_their_hooks() {
    let h = Home::new();
    let before = json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "echo hi"}]}]}});
    let path = h.file(".codex/hooks.json", &before.to_string());
    let x = grouped(path, 5);
    x.install().unwrap();
    x.uninstall().unwrap();
    assert_eq!(h.json(".codex/hooks.json"), before);
}

#[test]
fn unreadable_file_is_not_overwritten() {
    let h = Home::new();
    let path = h.file(".claude/settings.json", "{ // comments aren't JSON\n}");
    let x = grouped(path, 5);
    assert!(!x.installed());
    assert!(matches!(x.install(), Err(Error::Parse(_))));
    assert_eq!(h.read(".claude/settings.json"), "{ // comments aren't JSON\n}");
}

#[test]
fn every_agent_cli_gets_one() {
    let h = Home::new();
    let ctx = h.ctx();
    let ids: Vec<&str> = clients(&ctx).iter().map(|c| c.id).collect();
    for id in ["claude-code", "codex", "gemini", "antigravity", "copilot", "cursor", "opencode"] {
        assert!(ids.contains(&id));
        assert!(extras_for(&ctx, id).iter().any(Extra::is_session), "{id}");
    }
    for id in ["claude-desktop", "lmstudio", "vscode"] {
        assert!(ids.contains(&id));
        assert!(!extras_for(&ctx, id).iter().any(Extra::is_session), "{id}");
    }
}

// ---------- Other CLIs ----------

#[test]
fn gemini_uses_claude_layout_with_millisecond_timeout() {
    let h = Home::new();
    let path = h.file(".gemini/settings.json", r#"{"theme": "Dracula"}"#);
    let x = grouped(path, 5000);
    x.install().unwrap();
    assert_eq!(h.json(".gemini/settings.json")["hooks"]["SessionStart"][0]["hooks"][0]["timeout"], 5000);
    x.uninstall().unwrap();
    assert_eq!(h.json(".gemini/settings.json"), json!({"theme": "Dracula"}));
}

#[test]
fn antigravity_named_hook() {
    let h = Home::new();
    let theirs = json!({"Stop": [{"type": "command", "command": "say done"}]});
    let path = h.file(".gemini/config/hooks.json", &json!({"mine": theirs}).to_string());
    let cmd = format!("{DG} hook session --format antigravity");
    let x = Extra::SessionNamed { path, command: cmd.clone() };
    x.install().unwrap();
    x.install().unwrap();
    let d = h.json(".gemini/config/hooks.json");
    assert_eq!(d["mine"], theirs);
    assert!(d["desktopglass"]["SessionStart"][0]["command"].as_str().unwrap().contains("--format antigravity"));
    assert!(x.installed());
    x.uninstall().unwrap();
    assert_eq!(h.json(".gemini/config/hooks.json"), json!({"mine": theirs}));
    let alone = Extra::SessionNamed { path: h.root.join("fresh/hooks.json"), command: cmd };
    alone.install().unwrap();
    alone.uninstall().unwrap();
    assert!(!h.exists("fresh/hooks.json"));
}

#[test]
fn cursor_keeps_its_other_hooks() {
    let h = Home::new();
    let theirs = json!({"command": "./audit.sh"});
    let before = json!({"version": 1, "hooks": {"sessionStart": [theirs], "afterFileEdit": [theirs]}});
    let path = h.file(".cursor/hooks.json", &before.to_string());
    let cmd = format!("{DG} hook session --format cursor");
    let x = Extra::SessionCursor { path, command: cmd.clone() };
    x.install().unwrap();
    x.install().unwrap();
    let starts = h.json(".cursor/hooks.json")["hooks"]["sessionStart"].clone();
    assert_eq!(starts.as_array().unwrap().len(), 2);
    assert!(starts[1]["command"].as_str().unwrap().contains("--format cursor"));
    x.uninstall().unwrap();
    assert_eq!(h.json(".cursor/hooks.json"), before);
    let fresh = Extra::SessionCursor { path: h.root.join("c2/hooks.json"), command: cmd };
    fresh.install().unwrap();
    fresh.uninstall().unwrap();
    assert!(!h.exists("c2/hooks.json"), "a hooks.json that only held our hook is removed");
}

#[test]
fn copilot_and_opencode_get_files_of_their_own() {
    let h = Home::new();
    let ctx = h.ctx();
    for (x, needle) in [(copilot_hook(&ctx), "\"sessionStart\""), (opencode_plugin(&ctx), "experimental.chat.system.transform")] {
        assert!(x.path().starts_with(&h.root), "never the real home");
        x.install().unwrap();
        assert!(std::fs::read_to_string(x.path()).unwrap().contains(needle));
        assert!(x.installed());
        x.uninstall().unwrap();
        assert!(!x.path().exists());
    }
}

#[test]
fn copilot_file_format() {
    let h = Home::new();
    let Extra::OwnFile { content, .. } = copilot_hook(&h.ctx()) else { panic!() };
    let d: Value = serde_json::from_str(&content).unwrap();
    assert_eq!(d["version"], 1);
    let entry = &d["hooks"]["sessionStart"][0];
    assert_eq!(entry["bash"], format!("{}/.local/bin/dg hook session --format copilot", h.root.display()));
    assert_eq!(entry["timeoutSec"], 5);
}

#[test]
fn opencode_plugin_only_speaks_inside_desktopglass() {
    let h = Home::new();
    let Extra::OwnFile { content: js, .. } = opencode_plugin(&h.ctx()) else { panic!() };
    assert!(js.contains(r#"process.env.TERM_PROGRAM !== "DesktopGlass""#));
    assert!(js.contains("inside DesktopGlass"));
    assert!(js.contains("export const DesktopGlass"));
    assert!(js.contains(&format!("{}/.local/bin/dg view", h.root.display())));
}

#[test]
fn an_unrelated_file_with_our_name_is_left_alone() {
    let h = Home::new();
    let path = h.file("desktopglass.json", r#"{"mine": true}"#);
    let x = Extra::OwnFile { path: path.clone(), content: "x".into() };
    assert!(!x.installed());
    x.uninstall().unwrap();
    assert!(path.exists());
}
