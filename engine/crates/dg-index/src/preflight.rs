//! Sign-in handling (common.sh): `preflight_auth` (offline checks, no tokens spent) and `classify_failure`
//! (scans a failed run's output for sign-in and rate-limit messages). The pattern tables are data.

use crate::exit;
use crate::pattern::Pattern;
use crate::process;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

/// Output that means "sign in first" (`grep -Ei`, per line).
pub const AUTH_PATTERN: &str = concat!(
    "not logged in|not signed in|not authenticated|authentication (is )?(required|failed|error)|unauthori[sz]ed|",
    "(^|[^0-9])401([^0-9]|$)|invalid (api[ _-]?key|x-api-key|bearer|token|credentials)|",
    "api[ _-]?key (is )?(missing|required|not (set|found|valid))|please (run )?/login|",
    r#"run [`"']?[a-z-]+ (auth )?login|login required|please log ?in|no authentication|no credentials|"#,
    "credentials (not found|expired|missing)|(token|session) (has )?(expired|is invalid|missing)|",
    "set an auth method|providerautherror|no providers? (configured|found)"
);

/// Output that means "usage or rate limit".
pub const LIMIT_PATTERN: &str = concat!(
    "rate limit|usage limit|quota (exceeded|exhausted)|(^|[^0-9])429([^0-9]|$)|too many requests|limit reached|",
    "out of credits|insufficient (credits|quota)"
);

/// Offline status commands and the output that means "definitely signed out" (zsh globs in common.sh, as
/// case-sensitive patterns over the whole output). `unless_env`: any of these set means the CLI has a key anyway.
pub struct StatusCheck {
    pub harness: &'static str,
    pub args: &'static [&'static str],
    pub signed_out: &'static str,
    pub unless_env: &'static [&'static str],
}

pub const STATUS_CHECKS: &[StatusCheck] = &[
    StatusCheck {
        harness: "codex",
        args: &["login", "status"],
        signed_out: "[Nn]ot logged in|[Nn]ot signed in",
        unless_env: &[],
    },
    StatusCheck {
        harness: "cursor",
        args: &["status"],
        signed_out: "[Nn]ot logged in|[Nn]ot authenticated|login.*required",
        unless_env: &[],
    },
    // `*"0 credentials"*` in common.sh also matched "10 credentials"; the digit guard fixes that.
    StatusCheck {
        harness: "opencode",
        args: &["auth", "list"],
        signed_out: "(^|[^0-9])0 credentials",
        unless_env: &["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "GEMINI_API_KEY", "OPENROUTER_API_KEY"],
    },
];

/// Environment variables that mean "signed in" (any non-empty one).
pub fn ok_env(h: &str) -> &'static [&'static str] {
    match h {
        "claude" => &[
            "ANTHROPIC_API_KEY",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "ANTHROPIC_AUTH_TOKEN",
            "CLAUDE_CODE_USE_BEDROCK",
            "CLAUDE_CODE_USE_VERTEX",
        ],
        "codex" => &["OPENAI_API_KEY", "CODEX_API_KEY"],
        "cursor" => &["CURSOR_API_KEY"],
        "gemini" => &["GEMINI_API_KEY", "GOOGLE_API_KEY", "GOOGLE_APPLICATION_CREDENTIALS"],
        "copilot" => &["COPILOT_GITHUB_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"],
        _ => &[],
    }
}

/// Credential files (relative to $HOME) whose non-empty presence means "signed in".
pub fn ok_files(h: &str) -> &'static [&'static str] {
    match h {
        "claude" => &[".claude/.credentials.json"],
        "gemini" => &[".gemini/oauth_creds.json"],
        "antigravity" => &[".gemini/antigravity-cli/antigravity-oauth-token"],
        _ => &[],
    }
}

/// Settings files (relative to $HOME) and the pattern that means "an auth method is configured".
pub fn ok_settings(h: &str) -> &'static [(&'static str, &'static str)] {
    match h {
        "claude" => &[(".claude/settings.json", r#""apiKeyHelper""#)],
        "gemini" => &[(".gemini/settings.json", "selectedType|selectedAuthType")],
        _ => &[],
    }
}

/// What preflight_auth needs from the outside world (swappable in tests).
pub struct Env<'a> {
    pub home: &'a Path,
    pub var: &'a dyn Fn(&str) -> Option<String>,
    /// Runs the CLI with the given arguments (15 s timeout) and returns its combined output.
    pub run: &'a dyn Fn(&[&str]) -> String,
    /// `security find-generic-password -s <service>` succeeded.
    pub keychain: &'a dyn Fn(&str) -> bool,
}

fn set(env: &Env, k: &str) -> bool {
    (env.var)(k).is_some_and(|v| !v.is_empty())
}

fn non_empty(p: &Path) -> bool {
    std::fs::metadata(p).is_ok_and(|m| m.len() > 0)
}

/// `preflight_auth`: 0 looks signed in, 2 probably signed out (unverified), 77 definitely signed out.
/// Only command-based checks (codex, cursor, opencode) ever return 77.
pub fn preflight_auth(h: &str, env: &Env) -> i32 {
    if ok_env(h).iter().any(|k| set(env, k)) {
        return exit::OK;
    }
    if h == "claude" && (env.keychain)("Claude Code-credentials") {
        return exit::OK;
    }
    if ok_files(h).iter().any(|f| non_empty(&env.home.join(f))) {
        return exit::OK;
    }
    for (file, pat) in ok_settings(h) {
        let text = std::fs::read_to_string(env.home.join(file)).unwrap_or_default();
        if Pattern::new(pat, false).any_line(&text) {
            return exit::OK;
        }
    }
    if h == "antigravity" && set(env, "GEMINI_API_KEY") {
        let text = std::fs::read_to_string(env.home.join(".gemini/antigravity-cli/settings.json")).unwrap_or_default();
        if Pattern::new(r#""modelProvider"\s*:\s*"gemini""#, false).any_line(&text) {
            return exit::OK;
        }
    }
    if let Some(c) = STATUS_CHECKS.iter().find(|c| c.harness == h) {
        let out = (env.run)(c.args);
        if signed_out_output(c, &out) && !c.unless_env.iter().any(|k| set(env, k)) {
            return exit::SIGNED_OUT;
        }
        return exit::OK;
    }
    match h {
        // Claude and Gemini keep tokens in files/keychain we checked; Antigravity may use the keychain instead.
        "claude" | "gemini" | "antigravity" => exit::PROBABLY_SIGNED_OUT,
        _ => exit::OK, // copilot keeps credentials in the keychain: rely on output detection
    }
}

pub fn signed_out_output(c: &StatusCheck, out: &str) -> bool {
    Pattern::new(c.signed_out, false).is_match(out)
}

#[allow(clippy::type_complexity)]
/// The real environment: process env, `$bin <args>` with a 15 s timeout, the `security` tool.
pub fn system_env<'a>(
    home: &'a Path,
    bin: &'a str,
    path: &'a str,
) -> (impl Fn(&str) -> Option<String>, impl Fn(&[&str]) -> String + 'a, impl Fn(&str) -> bool) {
    let var = |k: &str| std::env::var(k).ok();
    let run = move |args: &[&str]| {
        let mut c = Command::new(bin);
        c.args(args).env("PATH", path).current_dir(home);
        process::capture(&mut c, Some(Duration::from_secs(15))).0
    };
    let keychain = |service: &str| {
        Command::new("/usr/bin/security")
            .args(["find-generic-password", "-s", service])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    };
    (var, run, keychain)
}

/// `classify_failure`: 77 sign-in, 76 limit, 1 other — from the last 80 lines of a run's output.
pub fn classify_failure(output: &str) -> i32 {
    let tail = process::tail_lines(output, 80);
    if Pattern::new(AUTH_PATTERN, true).any_line(&tail) {
        return exit::SIGNED_OUT;
    }
    if Pattern::new(LIMIT_PATTERN, true).any_line(&tail) {
        return exit::LIMITED;
    }
    exit::FAILED
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_captured_output() {
        let auth = [
            "Invalid API key · Please run /login",
            "Error: 401 Unauthorized",
            "OAuth token has expired. Please obtain a new token or refresh your existing token.",
            "Please set an Auth method in your /Users/x/.gemini/settings.json or specify GEMINI_API_KEY",
            "Error: Authentication required. Run `cursor-agent login` first.",
            "ProviderAuthError: No API key found for provider anthropic",
            "You are not logged in. Run `codex login`.",
            "Error: No authentication information found.",
            "stream error: unexpected status 401 Unauthorized: {\"detail\":\"Unauthorized\"}",
            "Error: No credentials found. Please run copilot and /login",
            "api key not set",
            "Authentication is required to use this model",
            "Error: session expired",
            "no providers configured",
        ];
        for s in auth {
            assert_eq!(classify_failure(&format!("starting…\n{s}\n")), 77, "{s}");
        }
        let limit = [
            "Claude AI usage limit reached|1730000000",
            "You've hit your usage limit. Upgrade to Pro",
            "Error: 429 Too Many Requests",
            "RESOURCE_EXHAUSTED: Quota exceeded for quota metric",
            "rate limit exceeded, retry later",
            "Insufficient credits",
        ];
        for s in limit {
            assert_eq!(classify_failure(s), 76, "{s}");
        }
        let other = ["Error: wrote 4010 bytes", "panic at line 14012", "Operation not permitted", "ok", ""];
        for s in other {
            assert_eq!(classify_failure(s), 1, "{s}");
        }
    }

    #[test]
    fn classify_looks_only_at_last_80_lines() {
        let mut s = String::from("Error: 401 Unauthorized\n");
        for i in 0..80 {
            s.push_str(&format!("line {i}\n"));
        }
        assert_eq!(classify_failure(&s), 1);
    }

    #[test]
    fn status_output_tables() {
        let check = |h: &str, out: &str| signed_out_output(STATUS_CHECKS.iter().find(|c| c.harness == h).unwrap(), out);
        assert!(check("codex", "Not logged in"));
        assert!(!check("codex", "Logged in using ChatGPT"));
        assert!(check("cursor", "Status: not authenticated"));
        assert!(check("cursor", "login is required"));
        assert!(!check("cursor", "✓ Logged in as a@b.c"));
        assert!(check("opencode", "┌  Credentials ~/.local/share/opencode/auth.json\n│\n└  0 credentials"));
        assert!(!check("opencode", "└  10 credentials"));
        assert!(!check("opencode", "└  2 credentials"));
    }

    #[test]
    fn preflight_rules() {
        let home = std::env::temp_dir().join(format!("dg-preflight-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        let no_keychain = |_: &str| false;
        let case = |h: &str, vars: &[(&str, &str)], out: &str| {
            let var = |k: &str| vars.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string());
            let run = |_: &[&str]| out.to_string();
            preflight_auth(h, &Env { home: &home, var: &var, run: &run, keychain: &no_keychain })
        };
        assert_eq!(case("claude", &[], ""), 2);
        assert_eq!(case("claude", &[("ANTHROPIC_API_KEY", "x")], ""), 0);
        assert_eq!(case("claude", &[("ANTHROPIC_API_KEY", "")], ""), 2);
        std::fs::write(home.join(".claude/settings.json"), "{\"apiKeyHelper\": \"x\"}").unwrap();
        assert_eq!(case("claude", &[], ""), 0);
        assert_eq!(case("codex", &[], "Not logged in"), 77);
        assert_eq!(case("codex", &[("OPENAI_API_KEY", "k")], "Not logged in"), 0);
        assert_eq!(case("codex", &[], "Logged in using an API key"), 0);
        assert_eq!(case("cursor", &[], "Not logged in"), 77);
        assert_eq!(case("opencode", &[], "0 credentials"), 77);
        assert_eq!(case("opencode", &[("OPENROUTER_API_KEY", "k")], "0 credentials"), 0);
        assert_eq!(case("gemini", &[], ""), 2);
        assert_eq!(case("antigravity", &[], ""), 2);
        std::fs::create_dir_all(home.join(".gemini/antigravity-cli")).unwrap();
        std::fs::write(home.join(".gemini/antigravity-cli/settings.json"), "{\"modelProvider\": \"gemini\"}").unwrap();
        assert_eq!(case("antigravity", &[], ""), 2);
        assert_eq!(case("antigravity", &[("GEMINI_API_KEY", "k")], ""), 0);
        assert_eq!(case("copilot", &[], ""), 0);
        let _ = std::fs::remove_dir_all(&home);
    }
}
