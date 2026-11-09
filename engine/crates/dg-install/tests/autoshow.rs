//! Port of Tests/python/test_autoshow.py: which written files the Claude Code autoshow hook shows, and installing
//! it without disturbing the user's other settings and hooks.

mod common;
use common::Home;
use dg_install::clients::{claude_autoshow, is_autoshow, Extra};
use dg_install::hooks::{autoshow_enabled, open_link, target};
use dg_install::Error;
use serde_json::{json, Value};
use std::path::Path;

fn event(h: &Home, path: &str) -> Value {
    json!({"tool_name": "Write", "cwd": h.root.to_string_lossy(), "tool_input": {"file_path": path}})
}

fn t(h: &Home, p: &Path) -> Option<std::path::PathBuf> {
    target(&event(h, &p.to_string_lossy()), Path::new("/"))
}

#[test]
fn html_and_svg_are_shown() {
    let h = Home::new();
    for name in ["report.html", "chart.SVG", "page.htm"] {
        let p = h.file(&format!("out/{name}"), "<p>x</p>");
        assert_eq!(t(&h, &p), Some(p.clone()), "{name}");
    }
}

#[test]
fn relative_paths_use_the_session_folder() {
    let h = Home::new();
    let p = h.file("chart.svg", "<svg/>");
    assert_eq!(target(&event(&h, "chart.svg"), Path::new("/")), Some(p));
    assert_eq!(target(&event(&h, "./chart.svg"), Path::new("/")), Some(h.root.join("chart.svg")));
}

#[test]
fn other_files_and_build_output_are_ignored() {
    let h = Home::new();
    for p in [
        h.file("main.py", "x"),
        h.file("node_modules/pkg/index.html", "x"),
        h.file("site/dist/index.html", "x"),
        h.file(".git/x.html", "x"),
    ] {
        assert_eq!(t(&h, &p), None, "{}", p.display());
    }
    assert_eq!(t(&h, &h.root.join("missing.html")), None);
    assert_eq!(target(&json!({"tool_input": {}}), Path::new("/")), None);
    assert_eq!(target(&json!({}), Path::new("/")), None);
}

#[test]
fn pages_inside_a_js_project_need_its_dev_server() {
    let h = Home::new();
    h.file("app/package.json", "{}");
    let page = h.file("app/src/index.html", "x");
    let icon = h.file("app/src/logo.svg", "<svg/>");
    assert_eq!(t(&h, &page), None);
    assert_eq!(t(&h, &icon), Some(icon.clone()), "an SVG is still worth showing");
}

#[test]
fn only_inside_desktopglass_and_can_be_turned_off() {
    let env = |pairs: &'static [(&'static str, &'static str)]| {
        move |k: &str| pairs.iter().find(|(a, _)| *a == k).map(|(_, v)| v.to_string())
    };
    assert!(autoshow_enabled(env(&[("TERM_PROGRAM", "DesktopGlass")])));
    assert!(!autoshow_enabled(env(&[("TERM_PROGRAM", "iTerm.app")])));
    assert!(!autoshow_enabled(env(&[])));
    assert!(!autoshow_enabled(env(&[("TERM_PROGRAM", "DesktopGlass"), ("DESKTOPGLASS_AUTOSHOW", "0")])));
}

#[test]
fn the_link_quotes_the_whole_path() {
    assert_eq!(open_link("/Users/a b/é.html", None), "desktopglass://open?url=%2FUsers%2Fa%20b%2F%C3%A9.html");
}

fn hook(h: &Home) -> Extra {
    claude_autoshow(&h.ctx())
}

fn ours(d: &Value) -> Vec<Value> {
    d["hooks"]["PostToolUse"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|g| g["hooks"].as_array().unwrap().clone())
        .filter(|x| is_autoshow(x["command"].as_str().unwrap_or("")))
        .collect()
}

#[test]
fn install_keeps_other_settings_and_hooks() {
    let h = Home::new();
    let theirs = json!({"type": "command", "command": "say done"});
    h.file(".claude/settings.json", &json!({"model": "opus", "hooks": {
        "PostToolUse": [{"matcher": "Bash", "hooks": [theirs]}], "Stop": [{"hooks": [theirs]}]}}).to_string());
    let x = hook(&h);
    x.install().unwrap();
    x.install().unwrap(); // twice: still one entry
    let d = h.json(".claude/settings.json");
    assert_eq!(d["model"], "opus");
    assert_eq!(d["hooks"]["Stop"], json!([{"hooks": [theirs]}]));
    let o = ours(&d);
    assert_eq!(o.len(), 1);
    assert_eq!(o[0]["command"], format!("{}/.local/bin/dg hook autoshow", h.root.display()));
    assert!(x.installed());

    x.uninstall().unwrap();
    let d = h.json(".claude/settings.json");
    assert_eq!(d["hooks"]["PostToolUse"], json!([{"matcher": "Bash", "hooks": [theirs]}]));
    assert!(!x.installed());
}

#[test]
fn uninstall_tidies_empty_sections() {
    let h = Home::new();
    let x = hook(&h);
    x.install().unwrap();
    x.uninstall().unwrap();
    assert_eq!(h.json(".claude/settings.json"), json!({}));
}

#[test]
fn unreadable_settings_are_not_overwritten() {
    let h = Home::new();
    h.file(".claude/settings.json", "{ // comments aren't JSON\n}");
    assert!(matches!(hook(&h).install(), Err(Error::Parse(_))));
    assert!(h.read(".claude/settings.json").contains("comments"));
}

#[test]
fn recognises_both_forms_and_nothing_else() {
    assert!(is_autoshow("/usr/bin/python3 \"/Users/a/.claude/skills/desktopglass-index/scripts/autoshow.py\""));
    assert!(is_autoshow("/Users/a/.local/bin/dg hook autoshow"));
    assert!(is_autoshow("'/Users/a b/.local/bin/dg' hook autoshow"));
    assert!(!is_autoshow("/usr/bin/python3 /tmp/autoshow.py"));
    assert!(!is_autoshow("/Users/a/.local/bin/dg hook session --format claude"));
    assert!(!is_autoshow("say done"));
}
