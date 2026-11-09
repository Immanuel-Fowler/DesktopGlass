//! Running CLIs: output captured to a file (stdout and stderr together, like `> "$OUT" 2>&1`), stdin from
//! /dev/null, an optional timeout (`run_timeout`, which sends SIGTERM), and zsh-style exit codes (128 + signal).

use std::fs::File;
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

/// zsh's `$?` for a finished child: its exit code, or 128 + the signal that killed it.
pub fn shell_code(s: ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    s.code().unwrap_or_else(|| 128 + s.signal().unwrap_or(0))
}

/// Waits for `child`, sending SIGTERM after `timeout` (then waiting for it to exit).
pub fn wait_timeout(child: &mut std::process::Child, timeout: Option<Duration>) -> std::io::Result<ExitStatus> {
    let Some(limit) = timeout else { return child.wait() };
    let start = Instant::now();
    loop {
        if let Some(s) = child.try_wait()? {
            return Ok(s);
        }
        if start.elapsed() >= limit {
            unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) };
            return child.wait();
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Runs `cmd` with its output going to `out` (created/truncated). Returns the shell exit code; 127 if it
/// couldn't be started (what zsh reports for a missing command), 126 if it isn't executable.
pub fn run_to_file(cmd: &mut Command, out: &Path, timeout: Option<Duration>) -> i32 {
    let Ok(f) = File::create(out) else { return 1 };
    let Ok(f2) = f.try_clone() else { return 1 };
    cmd.stdin(Stdio::null()).stdout(f).stderr(f2);
    match cmd.spawn() {
        Ok(mut child) => wait_timeout(&mut child, timeout).map(shell_code).unwrap_or(1),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => 127,
        Err(_) => 126,
    }
}

/// `out=$(run_timeout N cmd 2>&1)`: combined output (trailing newlines trimmed) and exit code.
pub fn capture(cmd: &mut Command, timeout: Option<Duration>) -> (String, i32) {
    let tmp = temp_file("cap");
    let code = run_to_file(cmd, &tmp, timeout);
    let text = String::from_utf8_lossy(&std::fs::read(&tmp).unwrap_or_default()).trim_end_matches('\n').to_string();
    let _ = std::fs::remove_file(&tmp);
    (text, code)
}

/// A fresh path in the temp folder (`mktemp`), not yet created.
pub fn temp_file(tag: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    std::env::temp_dir().join(format!(
        "dg-{tag}-{}-{}-{nanos}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ))
}

/// `tail -n N` of a text.
pub fn tail_lines(text: &str, n: usize) -> String {
    let t = text.strip_suffix('\n').unwrap_or(text);
    let lines: Vec<&str> = t.split('\n').collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_kills_with_sigterm() {
        let mut c = Command::new("/bin/sleep");
        c.arg("5");
        let (_, code) = capture(&mut c, Some(Duration::from_millis(100)));
        assert_eq!(code, 128 + libc::SIGTERM);
    }

    #[test]
    fn tail() {
        assert_eq!(tail_lines("a\nb\nc\n", 2), "b\nc");
        assert_eq!(tail_lines("a", 5), "a");
    }
}
