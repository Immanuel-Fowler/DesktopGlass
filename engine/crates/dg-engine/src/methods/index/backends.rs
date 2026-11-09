//! Backends as the app shows them (Harness.swift ids, labels, defaults) plus what moved out of Swift: detection
//! of installed CLIs (HarnessDetector) and model lists (ModelCatalog). CLI discovery itself is dg-index's
//! (`find_bin`, the login-shell PATH), so `dg reindex` and Settings agree on what's installed.

use dg_index::harness;
use serde_json::{json, Map, Value};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

/// Every backend, in the app's order (`Harness.allCases`).
pub const ALL: &[&str] =
    &["claude", "codex", "gemini", "antigravity", "cursor", "copilot", "opencode", "apple", "mlx", "ollama", "lmstudio", "llamacpp"];

pub fn is_known(h: &str) -> bool {
    ALL.contains(&h)
}

/// The app's labels (these appear in problem messages, so they match Harness.swift exactly).
pub fn label(h: &str) -> &'static str {
    match h {
        "claude" => "Claude Code",
        "codex" => "Codex CLI",
        "gemini" => "Gemini CLI",
        "antigravity" => "Antigravity CLI",
        "cursor" => "Cursor CLI",
        "copilot" => "GitHub Copilot CLI",
        "opencode" => "OpenCode",
        "apple" => "Apple Foundation Models",
        "mlx" => "MLX",
        "ollama" => "Ollama",
        "lmstudio" => "LM Studio",
        "llamacpp" => "llama.cpp server",
        _ => "",
    }
}

pub fn is_local(h: &str) -> bool {
    matches!(h, "apple" | "mlx" | "ollama" | "lmstudio" | "llamacpp")
}

pub fn default_endpoint(h: &str) -> Option<&'static str> {
    match h {
        "ollama" => Some("http://localhost:11434"),
        "lmstudio" => Some("http://localhost:1234"),
        "llamacpp" => Some("http://localhost:8080"),
        _ => None,
    }
}

pub fn uses_server(h: &str) -> bool {
    default_endpoint(h).is_some()
}

pub fn default_model(h: &str) -> &'static str {
    if h == "claude" {
        "haiku"
    } else {
        ""
    }
}

/// Only letters, digits and `. _ / : -` (the model is passed through a shell).
pub fn sanitize_model(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric() || "._/:-".contains(*c)).collect()
}

/// An http(s) URL with a host, trimmed; "" otherwise.
pub fn sanitize_endpoint(s: &str) -> String {
    let t = s.trim_matches([' ', '\t']);
    let rest = t.strip_prefix("http://").or_else(|| t.strip_prefix("https://"));
    let Some(rest) = rest else { return String::new() };
    if t.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return String::new();
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit('@').next().unwrap_or("");
    let host = if host.starts_with('[') { host } else { host.split(':').next().unwrap_or("") };
    if host.is_empty() {
        return String::new();
    }
    t.to_string()
}

/// Fallback list when the backend can't list its own models (or isn't reachable yet).
pub fn presets(h: &str) -> &'static [&'static str] {
    match h {
        "claude" => &["haiku", "sonnet", "opus"],
        "codex" => &["gpt-5.5", "gpt-5.4", "gpt-5.4-mini", "gpt-5.3-codex"],
        "gemini" => &["auto", "pro", "flash", "flash-lite", "gemini-3.1-pro-preview", "gemini-3-flash-preview", "gemini-3.1-flash-lite"],
        "cursor" => &["auto"],
        "copilot" => &["claude-sonnet-4.6", "claude-haiku-4.5", "gpt-5.4", "gpt-5.4-mini", "gpt-5.3-codex"],
        "mlx" => &["mlx-community/Qwen2.5-7B-Instruct-4bit", "mlx-community/Llama-3.2-3B-Instruct-4bit"],
        "ollama" => &["llama3.2", "qwen2.5:7b"],
        _ => &[],
    }
}

/// CLI arguments that print the models, for agent CLIs that support it.
pub fn list_args(h: &str) -> Option<&'static [&'static str]> {
    match h {
        "cursor" | "antigravity" | "opencode" => Some(&["models"]),
        "copilot" => Some(&["--help"]),
        _ => None,
    }
}

// ---------- detection ----------

/// The PATH CLIs are looked up on: `DESKTOP_INDEX_PATH` if set, else known install folders + the login shell's
/// PATH (cached) + our own.
pub fn search_path(home: &Path) -> String {
    let override_path = std::env::var("DESKTOP_INDEX_PATH").ok();
    let inherited = std::env::var("PATH").ok();
    let cache = dg_platform::app_support().join("login-path");
    harness::search_path(home, override_path.as_deref(), inherited.as_deref(), || harness::login_shell_path(&cache))
}

/// `{backend: path}` for every backend that's installed. On-device servers count only when their CLI or app is
/// there (dg-index's "server" fallback means "may run anywhere", not "installed"); Apple Foundation Models only
/// when the app says the model is available (`apple`: the app binary that runs it).
pub fn detect(path: &str, home: &Path, apple: Option<&str>) -> Map<String, Value> {
    let d = harness::Discovery { path, home, glass_bin: None, python: None, mlx_server_up: false };
    let mut found = Map::new();
    for h in ALL {
        let bin = match *h {
            "apple" => apple.map(String::from),
            h => harness::find_bin(h, &d).filter(|b| b != "server"),
        };
        if let Some(b) = bin {
            found.insert(h.to_string(), Value::String(b));
        }
    }
    found
}

// ---------- models ----------

/// Removes ANSI escape sequences (`ESC [ … letter`).
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\u{1b}' && it.peek() == Some(&'[') {
            it.next();
            while let Some(&n) = it.peek() {
                it.next();
                if n.is_ascii_alphabetic() {
                    break;
                }
                if !(n.is_ascii_digit() || n == ';' || n == '?') {
                    break;
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

fn model_id_like(s: &str) -> bool {
    let mut cs = s.chars();
    matches!(cs.next(), Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit())
        && cs.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "._:/-".contains(c))
}

/// Model ids from a CLI's listing output.
pub fn parse_models(h: &str, raw: &str) -> Vec<String> {
    let text = strip_ansi(raw);
    let ws = |c: char| c == ' ' || c == '\t' || c == '\r';
    let mut out: Vec<String> = Vec::new();
    match h {
        "opencode" => {
            out = text.split('\n').map(|l| l.trim_matches(ws)).filter(|l| l.contains('/') && !l.contains(' ')).map(String::from).collect();
        }
        "cursor" | "antigravity" => {
            for line in text.split('\n') {
                let t = line.trim_matches(|c: char| ws(c) || "-*•>".contains(c));
                let Some(first) = t.split(' ').find(|s| !s.is_empty()) else { continue };
                if first.chars().count() > 1 && model_id_like(first) {
                    out.push(first.to_string());
                }
            }
        }
        "copilot" => {
            if let Some(i) = text.find("--model") {
                let tail = &text[i + "--model".len()..];
                let segment = &tail[..tail.find("\n  -").unwrap_or(tail.len())];
                if let Some(c) = segment.find("choices:") {
                    let mut choices = &segment[c + "choices:".len()..];
                    while let Some(a) = choices.find('"') {
                        let after = &choices[a + 1..];
                        let Some(b) = after.find('"') else { break };
                        if b > 0 {
                            out.push(after[..b].to_string());
                        }
                        choices = &after[b + 1..];
                    }
                }
            }
        }
        _ => {}
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|m| seen.insert(m.clone()));
    out
}

/// Names a local server reports (`/api/tags` for Ollama, `/v1/models` otherwise).
pub fn server_models(h: &str, endpoint: &str) -> Vec<String> {
    let base = sanitize_endpoint(endpoint);
    let base = base.trim_matches('/');
    if base.is_empty() {
        return vec![];
    }
    let url = format!("{base}{}", if h == "ollama" { "/api/tags" } else { "/v1/models" });
    let Ok(v) = dg_index::http::json(&url, None, Duration::from_secs(4)) else { return vec![] };
    let list = v.get("models").or_else(|| v.get("data")).and_then(|l| l.as_array()).cloned().unwrap_or_default();
    list.iter().filter_map(|m| m.get("name").or_else(|| m.get("id")).and_then(|n| n.as_str()).map(String::from)).collect()
}

/// MLX models downloaded to the Hugging Face cache.
pub fn mlx_models(home: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(home.join(".cache/huggingface/hub"))
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter_map(|e| e.file_name().to_str().map(String::from))
                .filter_map(|n| n.strip_prefix("models--mlx-community--").map(|m| format!("mlx-community/{m}")))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// Runs the CLI's listing command (20 s at most) and parses it.
pub fn cli_models(h: &str, bin: &str, home: &Path) -> Vec<String> {
    let Some(args) = list_args(h) else { return vec![] };
    let dir = Path::new(bin).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let home = home.to_string_lossy();
    let inherited = std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into());
    // node-based CLIs need node next to them
    let path = [dir, format!("{home}/.local/bin"), "/opt/homebrew/bin".into(), "/usr/local/bin".into(), inherited].join(":");
    let mut c = Command::new(bin);
    c.args(args).env("PATH", path);
    let (text, _) = dg_index::process::capture(&mut c, Some(Duration::from_secs(20)));
    parse_models(h, &text)
}

/// The live model list for a backend (empty if it can't be read). `binary`: the CLI, for backends listed by one.
pub fn live_models(h: &str, binary: Option<&str>, endpoint: &str, home: &Path) -> Vec<String> {
    match h {
        "ollama" | "lmstudio" | "llamacpp" => server_models(h, endpoint),
        "mlx" => mlx_models(home),
        _ => match binary {
            Some(b) if list_args(h).is_some() => cli_models(h, b, home),
            _ => vec![],
        },
    }
}

pub fn presets_json() -> Value {
    Value::Object(ALL.iter().map(|h| (h.to_string(), json!(presets(h)))).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizers() {
        assert_eq!(sanitize_model("gpt-5; rm -rf ~"), "gpt-5rm-rf");
        assert_eq!(sanitize_model("qwen2.5:7b"), "qwen2.5:7b");
        assert_eq!(sanitize_endpoint(" http://localhost:11434 "), "http://localhost:11434");
        assert_eq!(sanitize_endpoint("https://box.local/v1"), "https://box.local/v1");
        assert_eq!(sanitize_endpoint("ftp://x"), "");
        assert_eq!(sanitize_endpoint("http://"), "");
        assert_eq!(sanitize_endpoint("localhost:1234"), "");
    }

    #[test]
    fn parses_cli_listings() {
        let opencode = "\u{1b}[1manthropic/claude-sonnet-4\u{1b}[0m\nopenai/gpt-5\nnot a model\nanthropic/claude-sonnet-4\n";
        assert_eq!(parse_models("opencode", opencode), vec!["anthropic/claude-sonnet-4", "openai/gpt-5"]);
        let cursor = "Available models\n\n- auto - Auto\n* sonnet-4.5 (current)\n  gpt-5\nTip: run with -m\n";
        assert_eq!(parse_models("cursor", cursor), vec!["auto", "sonnet-4.5", "gpt-5"]);
        let copilot = "Options:\n  --model <model>  Set the AI model (choices: \"claude-sonnet-4.6\", \"gpt-5.4\",\n    \"gpt-5.4\")\n  --other  x \"nope\"\n";
        assert_eq!(parse_models("copilot", copilot), vec!["claude-sonnet-4.6", "gpt-5.4"]);
        assert!(parse_models("claude", "haiku").is_empty());
    }

    #[test]
    fn catalogue() {
        assert_eq!(label("copilot"), "GitHub Copilot CLI");
        assert!(is_local("mlx") && !is_local("codex"));
        assert!(uses_server("llamacpp") && !uses_server("mlx"));
        assert_eq!(default_model("claude"), "haiku");
        assert_eq!(ALL.len(), crate::settings::HARNESSES.len());
        assert!(presets_json()["antigravity"].as_array().unwrap().is_empty());
    }
}
