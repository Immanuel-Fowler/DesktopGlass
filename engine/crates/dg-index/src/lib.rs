//! Reindex pipeline (Phase 2): replaces reindex.sh, common.sh, local_index.py, ask.sh and login.sh.
//!
//! `dg reindex` reads the same environment as reindex.sh (flags override it) and keeps its exit codes
//! ([`exit`]), stdout status lines, lock, `reindex.log` format, prompts and per-CLI command lines.

pub mod agent;
pub mod exit;
pub mod harness;
pub mod http;
pub mod local;
pub mod lock;
pub mod pattern;
pub mod preflight;
pub mod process;

use dg_core::paths;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Everything a run needs, from `DESKTOP_INDEX_*` (and friends) and flags.
#[derive(Debug, Clone)]
pub struct Options {
    pub harness: String,
    pub model: String,
    pub endpoint: String,
    pub reason: String,
    pub full: bool,
    pub unattended: bool,
    pub sandbox: bool,
    pub home: PathBuf,
    pub desk: PathBuf,
    pub skill_dir: PathBuf,
    pub glass_bin: Option<String>,
    pub python: Option<String>,
    /// DESKTOP_INDEX_PATH: use exactly this PATH (no login shell, no known folders).
    pub path_override: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Run,
    AuthOnly,
    Permissions,
}

pub const USAGE: &str = "Usage: dg reindex [--auth-only | --permissions] [--harness NAME] [--model ID] [--endpoint URL]
                  [--reason WHY] [--full] [--unattended] [--sandbox] [--desktop DIR] [--skill-dir DIR]
Env (same as reindex.sh): DESKTOP_INDEX_HARNESS, DESKTOP_INDEX_MODEL, DESKTOP_INDEX_ENDPOINT, DESKTOP_INDEX_REASON,
  DESKTOP_INDEX_FULL, DESKTOP_INDEX_UNATTENDED=1, DESKTOP_INDEX_SANDBOX=1, DESKTOP_GLASS_BIN, DESKTOP_INDEX_PYTHON,
  DESKTOP_INDEX_SKILL_DIR, DESKTOP_INDEX_DESKTOP, DESKTOP_INDEX_PATH
Exit: 0 indexed · 10 nothing changed · 75 already running · 76 usage limit · 77 not signed in ·
  78 local model not ready · 79 unattended runs not allowed · 127 not installed · other = failed";

impl Options {
    /// Reads the environment through `var` (so tests can supply their own).
    pub fn from_env(var: &dyn Fn(&str) -> Option<String>) -> Options {
        let get = |k: &str| var(k).unwrap_or_default();
        let home = var("HOME").filter(|h| !h.is_empty()).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"));
        let nonempty = |k: &str| var(k).filter(|v| !v.is_empty());
        Options {
            harness: nonempty("DESKTOP_INDEX_HARNESS").unwrap_or_else(|| "claude".into()),
            model: get("DESKTOP_INDEX_MODEL"),
            endpoint: get("DESKTOP_INDEX_ENDPOINT"),
            reason: nonempty("DESKTOP_INDEX_REASON").unwrap_or_else(|| "manual".into()),
            full: nonempty("DESKTOP_INDEX_FULL").is_some(),
            unattended: get("DESKTOP_INDEX_UNATTENDED") == "1",
            sandbox: get("DESKTOP_INDEX_SANDBOX") == "1",
            desk: nonempty("DESKTOP_INDEX_DESKTOP").map(PathBuf::from).unwrap_or_else(|| home.join("Desktop")),
            skill_dir: nonempty("DESKTOP_INDEX_SKILL_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".claude/skills/desktopglass-index")),
            glass_bin: nonempty("DESKTOP_GLASS_BIN"),
            python: nonempty("DESKTOP_INDEX_PYTHON"),
            path_override: nonempty("DESKTOP_INDEX_PATH"),
            home,
        }
    }

    /// Applies flags; returns the mode. `--flag value` and `--flag=value` both work.
    pub fn apply_args(&mut self, args: &[String]) -> Result<Mode, String> {
        let mut mode = Mode::Run;
        let mut i = 0;
        while i < args.len() {
            let (flag, inline) = match args[i].split_once('=') {
                Some((f, v)) if f.starts_with("--") => (f.to_string(), Some(v.to_string())),
                _ => (args[i].clone(), None),
            };
            let mut value = || -> Result<String, String> {
                if let Some(v) = inline.clone() {
                    return Ok(v);
                }
                i += 1;
                args.get(i).cloned().ok_or_else(|| format!("{flag} needs a value"))
            };
            match flag.as_str() {
                "--auth-only" => mode = Mode::AuthOnly,
                "--permissions" => mode = Mode::Permissions,
                "--harness" => self.harness = value()?,
                "--model" => self.model = value()?,
                "--endpoint" => self.endpoint = value()?,
                "--reason" => self.reason = value()?,
                "--desktop" => self.desk = PathBuf::from(value()?),
                "--skill-dir" => self.skill_dir = PathBuf::from(value()?),
                "--full" => self.full = true,
                "--unattended" => self.unattended = true,
                "--sandbox" => self.sandbox = true,
                "-h" | "--help" => return Err(String::new()),
                other => return Err(format!("unknown argument: {other}")),
            }
            i += 1;
        }
        Ok(mode)
    }

    fn log_dir(&self) -> PathBuf {
        self.desk.join(dg_core::rules::LOG_DIR)
    }

    fn label(&self) -> String {
        harness::label(&self.harness)
    }
}

fn say(line: &str) {
    let mut o = std::io::stdout().lock();
    let _ = writeln!(o, "{line}");
    let _ = o.flush();
}

/// The PATH for CLIs (also exported, so node-based CLIs find `node` next to them).
fn setup_path(o: &Options) -> String {
    let inherited = std::env::var("PATH").ok();
    let cache = dg_platform::app_support().join("login-path");
    let p = harness::search_path(&o.home, o.path_override.as_deref(), inherited.as_deref(), || {
        harness::login_shell_path(&cache)
    });
    std::env::set_var("PATH", &p);
    p
}

fn mlx_endpoint(o: &Options) -> String {
    let e = if o.endpoint.is_empty() { local::default_endpoint("mlx") } else { &o.endpoint };
    e.trim_end_matches('/').to_string()
}

fn find(o: &Options, path: &str) -> Option<String> {
    let mlx_up = o.harness == "mlx"
        && (!o.endpoint.is_empty()
            || http::json(&format!("{}/v1/models", mlx_endpoint(o)), None, std::time::Duration::from_secs(2)).is_ok());
    let d = harness::Discovery {
        path,
        home: &o.home,
        glass_bin: o.glass_bin.as_deref(),
        python: o.python.as_deref(),
        mlx_server_up: mlx_up,
    };
    harness::find_bin(&o.harness, &d)
}

fn now() -> String {
    dg_core::py::strftime_local("%F %T", None)
}

fn append_log(o: &Options, text: &str) {
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(o.log_dir().join("reindex.log")) {
        let _ = f.write_all(text.as_bytes());
    }
}

/// `=== 2026-11-06 17:00:00 · button · Claude Code · haiku · exit 0`
fn run_header(o: &Options, code: i32) -> String {
    let model = if o.model.is_empty() { String::new() } else { format!(" · {}", o.model) };
    format!("=== {} · {} · {}{model} · exit {code}\n", now(), o.reason, o.label())
}

fn local_options(o: &Options, bin: &str, ping: bool) -> local::Options {
    local::Options {
        backend: o.harness.clone(),
        model: o.model.clone(),
        endpoint: o.endpoint.clone(),
        root: paths::desktop_root(&o.desk.to_string_lossy()),
        all: o.full,
        ping,
        home: o.home.clone(),
        bin: bin.to_string(),
    }
}

/// `dg reindex`: returns the exit code.
pub fn reindex(o: &Options, mode: Mode) -> i32 {
    let h = o.harness.as_str();
    let label = o.label();
    let _ = std::fs::create_dir_all(o.log_dir());
    let path = setup_path(o);

    let Some(bin) = find(o, &path) else {
        match h {
            "mlx" => say("MLX needs the mlx-lm package — run: pip install mlx-lm"),
            "apple" => say("Apple Foundation Models run through the DesktopGlass app (macOS 26+)"),
            _ => say(&format!("{label} isn't installed — install it or pick another CLI in Settings")),
        }
        return exit::NOT_INSTALLED;
    };

    // ---- what an unattended run is allowed to do (shown before the user opts in) ----
    if mode == Mode::Permissions {
        say(harness::unattended_permissions(h));
        return exit::OK;
    }

    // ---- sign-in check only (Settings → "Check sign-in") ----
    if mode == Mode::AuthOnly && harness::is_local(h) {
        let mut last = String::new();
        let code = local::run(&local_options(o, &bin, true), &mut |l| last = l.to_string());
        say(&last);
        return code;
    }
    if mode == Mode::AuthOnly {
        return auth_only(o, &bin, &path);
    }

    // ---- normal re-index ----
    let Some(_lock) = lock::Lock::acquire(&o.log_dir().join(".reindex.lock")) else {
        say("Already indexing");
        return exit::BUSY;
    };

    let full_note = if o.full { agent::FULL_NOTE } else { "" };
    let root = paths::desktop_root(&o.desk.to_string_lossy());
    if !o.full && !dg_core::scan::needs_work(&dg_core::scan::scan(&root, false)) {
        say("Up to date — nothing changed");
        return exit::UP_TO_DATE;
    }

    if harness::is_local(h) {
        let mut lines: Vec<String> = vec![];
        let code = local::run(&local_options(o, &bin, false), &mut |l| lines.push(l.to_string()));
        let mut body = lines.join("\n");
        if !body.is_empty() {
            body.push('\n');
        }
        append_log(o, &format!("{}{body}", run_header(o, code)));
        let last = lines.iter().rev().find(|l| !l.starts_with("skipped ")).cloned();
        say(&last.unwrap_or_else(|| format!("{label} exited with {code}")));
        return code;
    }

    // Agent CLIs approve their own actions in a headless run, so the user has to opt in first.
    if !o.unattended {
        say(&format!("Unattended runs with {label} are off — review and allow them in Settings → Re-indexing"));
        append_log(o, &format!("=== {} · skipped: unattended runs not allowed for {label}\n", now()));
        return exit::NEEDS_CONSENT;
    }

    if preflight(o, &bin, &path) == exit::SIGNED_OUT {
        say(&format!("Not signed in to {label} — {}", harness::login_hint(h)));
        append_log(o, &format!("=== {} · skipped: not signed in to {label}\n", now()));
        return exit::SIGNED_OUT;
    }

    if !o.desk.is_dir() {
        return exit::FAILED; // `cd "$DESK" || exit 1`
    }
    let home = o.home.to_string_lossy().into_owned();
    let run = agent::Run {
        harness: h,
        bin: &bin,
        model: &o.model,
        desk: &o.desk,
        skill_dir: &o.skill_dir,
        home: &home,
        full_note,
    };
    let Some(c) = agent::headless(&run) else {
        say(&format!("Unknown CLI: {h}"));
        return exit::USAGE;
    };
    let argv = agent::wrap_sandbox(&c, o.sandbox, || {
        let desk = o.desk.to_string_lossy().into_owned();
        let resolved = paths::resolve(&o.desk).to_string_lossy().into_owned();
        agent::sandbox_profile(&home, &desk, &resolved, &o.skill_dir.to_string_lossy())
    });
    let out_file = process::temp_file("run");
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..]).current_dir(&o.desk).env("PATH", &path).envs(c.env.iter().cloned());
    let code = process::run_to_file(&mut cmd, &out_file, None);
    let output = String::from_utf8_lossy(&std::fs::read(&out_file).unwrap_or_default()).into_owned();
    let _ = std::fs::remove_file(&out_file);
    append_log(o, &format!("{}{output}", run_header(o, code)));

    if code == 0 {
        // safety net: rebuild the index even if the CLI skipped that step
        match dg_core::assemble::commit(&root) {
            Ok(status) => say(&status),
            Err(e) => eprintln!("dg inventory commit: {}: {e}", root.display()),
        }
        return exit::OK;
    }

    match preflight::classify_failure(&output) {
        exit::SIGNED_OUT => {
            say(&format!("Not signed in to {label} — {}", harness::login_hint(h)));
            exit::SIGNED_OUT
        }
        exit::LIMITED => {
            say(&format!("{label} hit a usage or rate limit — try again later or switch CLI"));
            exit::LIMITED
        }
        _ => {
            let log = std::fs::read_to_string(o.log_dir().join("reindex.log")).unwrap_or_default();
            if process::tail_lines(&log, 40).contains("Operation not permitted") {
                say(&format!("{label} tried to do something outside DesktopGlass's sandbox — see _Organize Log/reindex.log"));
            } else {
                say(&format!("{label} exited with {code} — see _Organize Log/reindex.log"));
            }
            code
        }
    }
}

fn preflight(o: &Options, bin: &str, path: &str) -> i32 {
    let (var, run, keychain) = preflight::system_env(&o.home, bin, path);
    preflight::preflight_auth(&o.harness, &preflight::Env { home: &o.home, var: &var, run: &run, keychain: &keychain })
}

/// `--auth-only` for agent CLIs: offline check, else a one-word prompt (90 s timeout).
fn auth_only(o: &Options, bin: &str, path: &str) -> i32 {
    let h = o.harness.as_str();
    let label = o.label();
    if preflight(o, bin, path) == exit::SIGNED_OUT {
        say(&format!("Not signed in to {label} — {}", harness::login_hint(h)));
        return exit::SIGNED_OUT;
    }
    if !o.desk.is_dir() {
        return exit::FAILED;
    }
    let Some(c) = agent::auth_probe(h, bin) else {
        say(&format!("Signed in to {label}"));
        return exit::OK;
    };
    let mut cmd = Command::new(&c.args[0]);
    cmd.args(&c.args[1..]).current_dir(&o.desk).env("PATH", path).envs(c.env.iter().cloned());
    let (out, code) = process::capture(&mut cmd, Some(std::time::Duration::from_secs(90)));
    if code == 0 {
        say(&format!("Signed in to {label}"));
        return exit::OK;
    }
    match preflight::classify_failure(&out) {
        exit::SIGNED_OUT => {
            say(&format!("Not signed in to {label} — {}", harness::login_hint(h)));
            exit::SIGNED_OUT
        }
        exit::LIMITED => {
            say(&format!("{label} is signed in but hit a usage limit"));
            exit::LIMITED
        }
        _ => {
            say(&format!("{label} couldn't be reached (exit {code})"));
            exit::FAILED
        }
    }
}

/// Replaces this process with `argv` in `dir` (like zsh's `exec`). Returns only on failure.
fn exec_in(argv: &[String], dir: &Path, path: &str) -> i32 {
    use std::os::unix::process::CommandExt;
    let err = Command::new(&argv[0]).args(&argv[1..]).current_dir(dir).env("PATH", path).exec();
    eprintln!("{}: {err}", argv[0]);
    if err.kind() == std::io::ErrorKind::NotFound {
        exit::NOT_INSTALLED
    } else {
        126
    }
}

/// `dg ask [harness] [model]` (ask.sh): an interactive session so the task can ask its questions.
pub fn ask(o: &Options) -> i32 {
    let h = o.harness.as_str();
    if harness::is_local(h) {
        say("On-device models can't hold a conversation here. Open _Organize Log/questions.md, answer inline,");
        say("or switch to an agent CLI (Claude Code, Codex, …) in DesktopGlass Settings to answer interactively.");
        return exit::OK;
    }
    let path = setup_path(o);
    let Some(bin) = find(o, &path) else {
        say(&format!("{} isn't installed.", o.label()));
        return exit::NOT_INSTALLED;
    };
    if !o.desk.is_dir() {
        return exit::FAILED;
    }
    match agent::interactive(h, &bin, &o.model, &o.skill_dir) {
        Some(c) => exec_in(&c.args, &o.desk, &path),
        None => exit::OK,
    }
}

/// `dg login <harness>` (login.sh): the CLI's sign-in flow, or how to start a local backend.
pub fn login(o: &Options) -> i32 {
    let h = o.harness.as_str();
    let open = |args: &[&str]| {
        Command::new("/usr/bin/open")
            .args(args)
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };
    match h {
        "ollama" => {
            if open(&["-a", "Ollama"]) {
                return exit::OK;
            }
            let path = setup_path(o);
            return match harness::which("ollama", &path) {
                Some(b) => exec_in(&[b, "serve".into()], &o.home, &path),
                None => exit::NOT_INSTALLED,
            };
        }
        "lmstudio" => {
            open(&["-a", "LM Studio"]);
            say("Load a model and start the local server (Developer tab).");
            return exit::OK;
        }
        "llamacpp" => {
            say("Start a server, e.g.: llama-server -m model.gguf --port 8080");
            return exit::OK;
        }
        "mlx" => {
            say("Install MLX:  pip install mlx-lm");
            say("DesktopGlass starts mlx_lm.server for you, or start one yourself: mlx_lm.server --model <model>");
            return exit::OK;
        }
        "apple" => {
            open(&["x-apple.systempreferences:com.apple.Siri-Settings.extension"]);
            say("Turn on Apple Intelligence in System Settings.");
            return exit::OK;
        }
        _ => {}
    }
    let label = o.label();
    let path = setup_path(o);
    let Some(bin) = find(o, &path) else {
        say(&format!("{label} isn't installed."));
        return exit::NOT_INSTALLED;
    };
    let Some((msg, c)) = agent::login(h, &bin) else { return exit::OK };
    say(&format!("Signing in to {label}. When you're done, go back to DesktopGlass → Settings → Check sign-in."));
    say("");
    if let Some(m) = msg {
        say(m);
    }
    exec_in(&c.args, &std::env::current_dir().unwrap_or_else(|_| o.home.clone()), &path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_and_flags() {
        let env = |k: &str| match k {
            "HOME" => Some("/h".to_string()),
            "DESKTOP_INDEX_HARNESS" => Some("codex".into()),
            "DESKTOP_INDEX_FULL" => Some("1".into()),
            "DESKTOP_INDEX_UNATTENDED" => Some("0".into()),
            _ => None,
        };
        let mut o = Options::from_env(&env);
        assert_eq!((o.harness.as_str(), o.full, o.unattended, o.reason.as_str()), ("codex", true, false, "manual"));
        assert_eq!(o.desk, PathBuf::from("/h/Desktop"));
        assert_eq!(o.skill_dir, PathBuf::from("/h/.claude/skills/desktopglass-index"));
        let args: Vec<String> = ["--harness=gemini", "--model", "flash", "--unattended", "--auth-only"].map(String::from).to_vec();
        assert_eq!(o.apply_args(&args), Ok(Mode::AuthOnly));
        assert_eq!((o.harness.as_str(), o.model.as_str(), o.unattended), ("gemini", "flash", true));
        assert!(o.apply_args(&["--bogus".into()]).is_err());
        assert!(o.apply_args(&["--model".into()]).is_err());
        let o2 = Options::from_env(&|k| (k == "HOME").then(|| "/h".to_string()));
        assert_eq!(o2.harness, "claude");
    }
}
