//! The backend catalogue (common.sh: harness_label, login_hint, is_local, unattended_permissions) and CLI
//! discovery (find_bin, find_mlx_python), including the macOS GUI-app PATH problem: apps launched from the Dock
//! get a minimal PATH, so the user's login-shell PATH is resolved once, cached, and put behind the usual install
//! folders.

use crate::process;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

/// Agent CLIs (run tools themselves and follow SKILL.md).
pub const AGENTS: &[&str] = &["claude", "codex", "gemini", "antigravity", "cursor", "copilot", "opencode"];
/// On-device backends (dg plays agent: scan → model → validated description.md → commit).
pub const LOCALS: &[&str] = &["ollama", "lmstudio", "llamacpp", "mlx", "apple"];

pub fn is_local(h: &str) -> bool {
    LOCALS.contains(&h)
}

pub fn label(h: &str) -> String {
    match h {
        "claude" => "Claude Code",
        "codex" => "Codex CLI",
        "gemini" => "Gemini CLI",
        "antigravity" => "Antigravity CLI",
        "cursor" => "Cursor CLI",
        "copilot" => "Copilot CLI",
        "opencode" => "OpenCode",
        "ollama" => "Ollama",
        "lmstudio" => "LM Studio",
        "llamacpp" => "llama.cpp server",
        "mlx" => "MLX",
        "apple" => "Apple Foundation Models",
        other => other,
    }
    .to_string()
}

pub fn login_hint(h: &str) -> &'static str {
    match h {
        "claude" => "Run `claude` in Terminal and type /login",
        "codex" => "Run `codex login` in Terminal",
        "gemini" => "Run `gemini` in Terminal and pick a sign-in method (or set GEMINI_API_KEY)",
        "antigravity" => "Run `agy` in Terminal — it opens your browser to sign in",
        "cursor" => "Run `cursor-agent login` in Terminal (or set CURSOR_API_KEY)",
        "copilot" => "Run `copilot` in Terminal and type /login",
        "opencode" => "Run `opencode auth login` in Terminal",
        _ => "",
    }
}

/// What each CLI may do in an unattended run (shown to the user before they opt in).
pub fn unattended_permissions(h: &str) -> &'static str {
    match h {
        "claude" => "Tool allow-list: Read, Write, Edit and dg only; edits auto-accepted.",
        "codex" => "Codex's own sandbox (--full-auto): writes only inside ~/Desktop, commands have no network.",
        "gemini" => "Auto-approves every action (--approval-mode yolo).",
        "antigravity" => "Auto-approves every tool call (--dangerously-skip-permissions).",
        "cursor" => "Auto-approves commands (--force).",
        "copilot" => "Allowed tools: read, write, dg.",
        "opencode" => "Auto-approves tool calls (opencode run).",
        _ => "",
    }
}

// ---------- PATH ----------

/// Where CLIs usually live (common.sh's list, plus LM Studio's `lms`, which Harness.swift also searches).
pub fn known_dirs(home: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = vec![
        home.join(".local/bin"),
        home.join(".claude/local"),
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        home.join(".npm-global/bin"),
        home.join(".bun/bin"),
        home.join(".opencode/bin"),
        home.join(".cargo/bin"),
    ];
    // $HOME/.nvm/versions/node/*/bin(N): zsh sorts glob matches by name
    let mut nvm: Vec<PathBuf> = std::fs::read_dir(home.join(".nvm/versions/node"))
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path().join("bin")).filter(|p| p.is_dir()).collect())
        .unwrap_or_default();
    nvm.sort();
    v.extend(nvm);
    v.push(home.join(".lmstudio/bin"));
    v
}

const LOGIN_PATH_TTL: Duration = Duration::from_secs(6 * 3600);

/// The login shell's PATH, cached in the app-support folder for six hours (starting an interactive login shell
/// costs up to a second or two). None if it can't be read.
pub fn login_shell_path(cache: &Path) -> Option<String> {
    if let Ok(meta) = std::fs::metadata(cache) {
        let fresh = meta
            .modified()
            .ok()
            .and_then(|m| SystemTime::now().duration_since(m).ok())
            .is_some_and(|age| age < LOGIN_PATH_TTL);
        if fresh {
            if let Ok(s) = std::fs::read_to_string(cache) {
                let s = s.trim().to_string();
                if !s.is_empty() {
                    return Some(s);
                }
            }
        }
    }
    let shell = std::env::var("SHELL")
        .ok()
        .filter(|s| {
            s.starts_with('/') && ["zsh", "bash", "sh", "ksh"].iter().any(|n| s.ends_with(&format!("/{n}")))
        })
        .unwrap_or_else(|| "/bin/zsh".into());
    let mut c = Command::new(shell);
    c.args(["-ilc", r#"printf '\n__DG_PATH__%s__DG_END__\n' "$PATH""#]);
    let (out, code) = process::capture(&mut c, Some(Duration::from_secs(10)));
    if code != 0 {
        return None;
    }
    let start = out.rfind("__DG_PATH__")? + "__DG_PATH__".len();
    let end = start + out[start..].find("__DG_END__")?;
    let path = out[start..end].trim().to_string();
    if path.is_empty() {
        return None;
    }
    if let Some(dir) = cache.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = cache.with_extension("tmp");
    if std::fs::write(&tmp, &path).is_ok() {
        let _ = std::fs::rename(&tmp, cache);
    }
    Some(path)
}

/// The PATH for finding and running CLIs: `DESKTOP_INDEX_PATH` if set (used as is), otherwise the known
/// install folders, then the login shell's PATH, then the inherited PATH (duplicates dropped).
pub fn search_path(home: &Path, override_path: Option<&str>, inherited: Option<&str>, login: impl FnOnce() -> Option<String>) -> String {
    if let Some(p) = override_path.filter(|p| !p.is_empty()) {
        return p.to_string();
    }
    let mut parts: Vec<String> = known_dirs(home).iter().map(|p| p.to_string_lossy().into_owned()).collect();
    if let Some(l) = login() {
        parts.extend(l.split(':').map(String::from));
    }
    parts.extend(inherited.unwrap_or("/usr/bin:/bin:/usr/sbin:/sbin").split(':').map(String::from));
    let mut seen = std::collections::HashSet::new();
    parts.retain(|p| !p.is_empty() && seen.insert(p.clone()));
    parts.join(":")
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// `command -v name` over `path`; absolute names are returned if executable.
pub fn which(name: &str, path: &str) -> Option<String> {
    if name.contains('/') {
        return is_executable(Path::new(name)).then(|| name.to_string());
    }
    path.split(':')
        .filter(|d| !d.is_empty())
        .map(|d| Path::new(d).join(name))
        .find(|p| is_executable(p))
        .map(|p| p.to_string_lossy().into_owned())
}

/// What discovery needs to know.
pub struct Discovery<'a> {
    pub path: &'a str,
    pub home: &'a Path,
    /// `DESKTOP_GLASS_BIN`: the app's main executable (Apple Foundation Models).
    pub glass_bin: Option<&'a str>,
    /// `DESKTOP_INDEX_PYTHON`: a Python to try first for MLX.
    pub python: Option<&'a str>,
    /// MLX: an `mlx_lm.server` already answering (checked by the caller), so no Python is needed.
    pub mlx_server_up: bool,
}

/// `find_bin`: the CLI's absolute path, or None if it isn't installed. On-device servers may run anywhere, so
/// for them "installed" just means we can try (`server`).
pub fn find_bin(h: &str, d: &Discovery) -> Option<String> {
    let w = |n: &str| which(n, d.path);
    match h {
        "claude" => w("claude"),
        "codex" => w("codex"),
        "gemini" => w("gemini"),
        "antigravity" => w("agy"),
        "cursor" => w("cursor-agent").or_else(|| {
            // generic name: only if it resolves into Cursor
            let b = w("agent")?;
            let real = std::fs::canonicalize(&b).unwrap_or_else(|_| PathBuf::from(&b));
            real.to_string_lossy().contains("cursor").then_some(b)
        }),
        "copilot" => w("copilot"),
        "opencode" => w("opencode"),
        "ollama" => w("ollama")
            .or_else(|| Path::new("/Applications/Ollama.app").is_dir().then(|| "/Applications/Ollama.app".into()))
            .or_else(|| Some("server".into())),
        "lmstudio" => w("lms")
            .or_else(|| Path::new("/Applications/LM Studio.app").is_dir().then(|| "/Applications/LM Studio.app".into()))
            .or_else(|| Some("server".into())),
        "llamacpp" => w("llama-server").or_else(|| Some("server".into())),
        "mlx" => {
            if d.mlx_server_up {
                Some("server".into())
            } else {
                find_mlx_python(d)
            }
        }
        "apple" => apple_bin(d.glass_bin),
        _ => None,
    }
}

/// A Python that can import mlx_lm (system, Homebrew, conda or pyenv).
pub fn find_mlx_python(d: &Discovery) -> Option<String> {
    let home = d.home.to_string_lossy();
    // The configured Python first, then EVERY python3/python on the search PATH (in order — mlx-lm is often in a
    // conda/pyenv Python that isn't the first python3 found), then the usual install spots.
    let mut candidates: Vec<String> = vec![d.python.unwrap_or("").to_string()];
    for dir in std::env::split_paths(d.path) {
        for name in ["python3", "python"] {
            candidates.push(dir.join(name).to_string_lossy().into_owned());
        }
    }
    candidates.extend([
        "/opt/homebrew/anaconda3/bin/python3".to_string(),
        format!("{home}/opt/anaconda3/bin/python3"),
        format!("{home}/miniconda3/bin/python"),
        format!("{home}/miniforge3/bin/python"),
        format!("{home}/anaconda3/bin/python"),
        format!("{home}/.pyenv/shims/python3"),
        "/opt/homebrew/bin/python3".into(),
        "/usr/local/bin/python3".into(),
    ]);
    let mut tried = std::collections::HashSet::new();
    for c in candidates.iter().filter(|c| !c.is_empty()) {
        let Some(p) = which(c, d.path) else { continue };
        // the same interpreter reached through different links only needs one (slow) import check
        let real = std::fs::canonicalize(&p).unwrap_or_else(|_| std::path::PathBuf::from(&p));
        if !tried.insert(real) {
            continue;
        }
        let mut cmd = Command::new(&p);
        cmd.args(["-c", "import mlx_lm"]).env("PATH", d.path);
        let (_, code) = process::capture(&mut cmd, Some(Duration::from_secs(60)));
        if code == 0 {
            return Some(p);
        }
    }
    None
}

/// The DesktopGlass app binary for `--foundation-models`: `DESKTOP_GLASS_BIN`, else the `DesktopGlass`
/// executable next to this `dg` (both live in DesktopGlass.app/Contents/MacOS). macOS only.
pub fn apple_bin(glass_bin: Option<&str>) -> Option<String> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    if let Some(b) = glass_bin.filter(|b| !b.is_empty()) {
        return is_executable(Path::new(b)).then(|| b.to_string());
    }
    let me = std::env::current_exe().ok()?;
    let me = std::fs::canonicalize(&me).unwrap_or(me);
    let app = me.parent()?.join("DesktopGlass");
    is_executable(&app).then(|| app.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_and_hints() {
        assert_eq!(label("copilot"), "Copilot CLI");
        assert_eq!(label("weird"), "weird");
        assert!(is_local("apple") && !is_local("claude"));
        assert_eq!(login_hint("codex"), "Run `codex login` in Terminal");
    }

    #[test]
    fn search_path_order_and_override() {
        let home = Path::new("/h");
        assert_eq!(search_path(home, Some("/x:/y"), Some("/z"), || panic!("no login shell")), "/x:/y");
        let p = search_path(home, None, Some("/usr/bin:/opt/homebrew/bin"), || Some("/l/bin:/usr/bin".into()));
        let parts: Vec<&str> = p.split(':').collect();
        assert_eq!(parts[0], "/h/.local/bin");
        let l = parts.iter().position(|x| *x == "/l/bin").unwrap();
        let u = parts.iter().position(|x| *x == "/usr/bin").unwrap();
        assert!(l < u);
        assert_eq!(parts.iter().filter(|x| **x == "/opt/homebrew/bin").count(), 1);
    }

    #[test]
    fn which_and_cursor_alias() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("dg-which-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("cursor-stuff")).unwrap();
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        let real = dir.join("cursor-stuff/agent");
        std::fs::write(&real, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink(&real, dir.join("bin/agent")).unwrap();
        let path = dir.join("bin").to_string_lossy().into_owned();
        let d = Discovery { path: &path, home: &dir, glass_bin: None, python: None, mlx_server_up: false };
        assert_eq!(find_bin("cursor", &d), Some(format!("{path}/agent")));
        assert_eq!(find_bin("codex", &d), None);
        assert_eq!(find_bin("llamacpp", &d).as_deref(), Some("server"));
        assert_eq!(find_bin("nope", &d), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
