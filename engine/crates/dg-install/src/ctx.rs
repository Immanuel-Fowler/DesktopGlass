//! Where things are: the user's home and the agent CLIs' homes, the per-OS app folders, the `dg` command that
//! registrations point at, and how agent CLIs are found. Everything is in one `Ctx` so tests can point it at a
//! temporary home and a fake PATH.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct Ctx {
    pub home: PathBuf,
    /// `$CODEX_HOME` or ~/.codex
    pub codex_home: PathBuf,
    /// `$COPILOT_HOME` or ~/.copilot
    pub copilot_home: PathBuf,
    /// Absolute path of the `dg` that registrations and hooks run: ~/.local/bin/dg (install.sh links it to the app's).
    pub dg: String,
    /// Folders searched for agent CLIs (`claude`, `codex`), in order.
    pub search: Vec<PathBuf>,
    /// Codex binaries inside apps, used when `codex` isn't on the search path.
    pub codex_bundles: Vec<PathBuf>,
    /// The OS's per-user app config folder (macOS: ~/Library/Application Support).
    pub app_config: PathBuf,
    /// Where GUI apps are installed (macOS: /Applications), for detection only.
    pub applications: PathBuf,
}

impl Ctx {
    /// From the environment: `HOME`, `CODEX_HOME`, `COPILOT_HOME`, `PATH`. `DESKTOPGLASS_TOOL_PATH`, when set, is the
    /// whole CLI search path (no well-known folders, no app bundles): tests use it to keep real CLIs out of reach.
    pub fn from_env() -> Ctx {
        let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
        let home = dg_platform::home();
        let mut ctx = Ctx::for_home(&home);
        if let Some(p) = env("CODEX_HOME") {
            ctx.codex_home = p;
        }
        if let Some(p) = env("COPILOT_HOME") {
            ctx.copilot_home = p;
        }
        match std::env::var_os("DESKTOPGLASS_TOOL_PATH") {
            Some(p) => {
                ctx.search = std::env::split_paths(&p).collect();
                ctx.codex_bundles.clear();
            }
            None => {
                if let Some(p) = std::env::var_os("PATH") {
                    ctx.search.extend(std::env::split_paths(&p));
                }
            }
        }
        ctx
    }

    /// Defaults for a home folder, ignoring the environment (the CLI search path is just the well-known folders).
    pub fn for_home(home: &Path) -> Ctx {
        let os = os_paths(home);
        let search = [".local/bin", ".claude/local"]
            .iter()
            .map(|p| home.join(p))
            .chain(os.cli_dirs.iter().cloned())
            .chain([".npm-global/bin", ".bun/bin", ".opencode/bin"].iter().map(|p| home.join(p)))
            .collect();
        Ctx {
            home: home.to_path_buf(),
            codex_home: home.join(".codex"),
            copilot_home: home.join(".copilot"),
            dg: dg_command(home),
            search,
            codex_bundles: os.codex_bundles,
            app_config: os.app_config,
            applications: os.applications,
        }
    }

    /// `shutil.which(cmd)` over the search path.
    pub fn which(&self, cmd: &str) -> Option<PathBuf> {
        self.search.iter().map(|d| d.join(exe(cmd))).find(|p| is_executable(p))
    }

    pub fn codex_binary(&self) -> Option<PathBuf> {
        self.which("codex").or_else(|| self.codex_bundles.iter().find(|p| is_executable(p)).cloned())
    }

    /// The `dg` command as one shell word, for hook command lines.
    pub fn dg_word(&self) -> String {
        shell_word(&self.dg)
    }
}

/// ~/.local/bin/dg: a stable path (install.sh points it at the current app bundle's `dg`).
pub fn dg_command(home: &Path) -> String {
    home.join(".local").join("bin").join(exe("dg")).to_string_lossy().into_owned()
}

pub struct OsPaths {
    pub app_config: PathBuf,
    pub applications: PathBuf,
    pub cli_dirs: Vec<PathBuf>,
    pub codex_bundles: Vec<PathBuf>,
}

/// The folders that differ by OS. macOS now; the Windows arm is a first sketch for Phase 6.
#[cfg(not(windows))]
pub fn os_paths(home: &Path) -> OsPaths {
    let apps = PathBuf::from("/Applications");
    OsPaths {
        app_config: home.join("Library").join("Application Support"),
        cli_dirs: vec![PathBuf::from("/opt/homebrew/bin"), PathBuf::from("/usr/local/bin")],
        codex_bundles: vec![
            apps.join("Codex.app/Contents/Resources/codex"),
            apps.join("ChatGPT.app/Contents/Resources/codex"),
        ],
        applications: apps,
    }
}

#[cfg(windows)]
pub fn os_paths(home: &Path) -> OsPaths {
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    OsPaths {
        app_config: env("APPDATA").unwrap_or_else(|| home.join("AppData").join("Roaming")),
        applications: env("ProgramFiles").unwrap_or_else(|| PathBuf::from(r"C:\Program Files")),
        cli_dirs: vec![env("APPDATA").unwrap_or_default().join("npm")],
        codex_bundles: vec![],
    }
}

fn exe(cmd: &str) -> String {
    if cfg!(windows) { format!("{cmd}.exe") } else { cmd.to_string() }
}

fn is_executable(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let Ok(c) = std::ffi::CString::new(p.as_os_str().as_bytes()) else { return false };
        p.is_file() && unsafe { libc::access(c.as_ptr(), libc::X_OK) } == 0
    }
    #[cfg(not(unix))]
    {
        p.is_file()
    }
}

/// A path as one POSIX shell word: bare when it has nothing special, single-quoted otherwise.
pub fn shell_word(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+@%:,=".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// The first shell word of a command line (quotes removed) and the rest.
pub fn split_program(cmd: &str) -> (String, &str) {
    let s = cmd.trim_start();
    let mut word = String::new();
    let mut quote: Option<char> = None;
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        match (quote, c) {
            (None, c) if c.is_whitespace() => return (word, &s[i..]),
            (None, '\'' | '"') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (None | Some('"'), '\\') => {
                if let Some((_, n)) = chars.next() {
                    word.push(n);
                }
            }
            (_, c) => word.push(c),
        }
    }
    (word, "")
}

pub struct Output {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// Runs a program with `input` on stdin, killing it after `timeout`.
pub fn run(program: &Path, args: &[&str], input: &str, timeout: Duration) -> std::io::Result<Output> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdin = child.stdin.take();
    let input = input.to_string();
    let writer = std::thread::spawn(move || {
        if let Some(s) = stdin.as_mut() {
            let _ = s.write_all(input.as_bytes());
        }
    });
    let mut out_pipe = child.stdout.take().unwrap();
    let mut err_pipe = child.stderr.take().unwrap();
    let out_t = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = out_pipe.read_to_end(&mut v);
        v
    });
    let err_t = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = err_pipe.read_to_end(&mut v);
        v
    });
    let start = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break Some(s);
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let _ = writer.join();
    let stdout = String::from_utf8_lossy(&out_t.join().unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&err_t.join().unwrap_or_default()).into_owned();
    if status.is_none() {
        return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, format!("timed out after {}s", timeout.as_secs())));
    }
    Ok(Output { code: status.and_then(|s| s.code()), stdout, stderr })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words() {
        assert_eq!(shell_word("/Users/a/.local/bin/dg"), "/Users/a/.local/bin/dg");
        assert_eq!(shell_word("/Users/a b/dg"), "'/Users/a b/dg'");
        assert_eq!(split_program("'/x y/dg' hook autoshow"), ("/x y/dg".to_string(), " hook autoshow"));
        assert_eq!(split_program("/usr/bin/python3 \"/a/b.py\""), ("/usr/bin/python3".to_string(), " \"/a/b.py\""));
        assert_eq!(split_program("dg"), ("dg".to_string(), ""));
    }
}
