//! Headless agent-CLI runs (reindex.sh), interactive sessions (ask.sh) and sign-in flows (login.sh):
//! SKILL.md → prompt, the per-CLI command lines and flags, and the optional Seatbelt sandbox (common.sh).

use std::path::Path;

/// One command line: the binary first. `sandboxed`: wrap in `sandbox-exec` when the sandbox is on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cmd {
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub sandboxed: bool,
}

fn cmd(bin: &str, args: Vec<String>) -> Cmd {
    let mut v = vec![bin.to_string()];
    v.extend(args);
    Cmd { args: v, env: vec![], sandboxed: false }
}

fn s(x: &str) -> String {
    x.to_string()
}

/// The note appended to headless prompts for Settings → Rewrite All.
pub const FULL_NOTE: &str = " This is a FULL REWRITE requested from DesktopGlass Settings: run the scan with --all and rewrite every description to match the current preferences.";

/// The one-word prompt `--auth-only` uses when there's no offline check.
pub const OK_PROMPT: &str = "Reply with the word OK.";

/// SKILL.md without its front matter (`awk 'f>=2{print; next} /^---[[:space:]]*$/{f++}'`), `<skill>` replaced.
pub fn skill_body(skill_md: &str, skill_dir: &str) -> String {
    let mut f = 0;
    let mut out = String::new();
    for line in skill_md.split_inclusive('\n') {
        let l = line.strip_suffix('\n').unwrap_or(line);
        if f >= 2 {
            out.push_str(l);
            out.push('\n');
            continue;
        }
        if l.strip_prefix("---").is_some_and(|r| r.chars().all(|c| c.is_ascii_whitespace())) {
            f += 1;
        }
    }
    // $(…) drops trailing newlines
    out.trim_end_matches('\n').replace("<skill>", skill_dir)
}

/// `skill_prompt <headless|interactive>`: SKILL.md as a prompt, for CLIs without Claude skills.
pub fn skill_prompt(mode: &str, skill_dir: &Path) -> String {
    let dir = skill_dir.to_string_lossy();
    let md = std::fs::read_to_string(skill_dir.join("SKILL.md")).unwrap_or_default();
    let body = skill_body(&md, &dir);
    let ask = if mode == "headless" {
        "This is a HEADLESS run: nobody is watching, so never ask or wait. Wherever the instructions mention asking the user or AskUserQuestion, append the question to _Organize Log/questions.md instead."
    } else {
        "Wherever the instructions mention AskUserQuestion, ask the questions in plain text and wait for the answers."
    };
    let text = format!(
        "Run the desktopglass-index task in {mode} mode. The working directory is ~/Desktop. The complete instructions are below; follow them exactly, using your own tools to read, write and edit files and to run shell commands. {ask}\n\n{body}"
    );
    text.trim_end_matches('\n').to_string()
}

/// Claude Code's tool allow-list for unattended runs: the skill, file tools and `dg inventory` only.
pub fn claude_allowed_tools(home: &str) -> String {
    format!("Skill,Read,Write,Edit,Bash(~/.local/bin/dg inventory:*),Bash({home}/.local/bin/dg inventory:*)")
}

/// Copilot CLI's allowed tools for unattended runs.
pub fn copilot_allowed_tools(home: &str) -> String {
    format!("read, write, shell(~/.local/bin/dg:*), shell({home}/.local/bin/dg:*)")
}

pub struct Run<'a> {
    pub harness: &'a str,
    pub bin: &'a str,
    pub model: &'a str,
    pub desk: &'a Path,
    pub skill_dir: &'a Path,
    pub home: &'a str,
    /// FULL_NOTE or "".
    pub full_note: &'a str,
}

/// The headless re-index command for an agent CLI (None: unknown CLI).
pub fn headless(r: &Run) -> Option<Cmd> {
    let m = |flag: &str| if r.model.is_empty() { vec![] } else { vec![s(flag), s(r.model)] };
    let prompt = || format!("{}{}", skill_prompt("headless", r.skill_dir), r.full_note);
    let skill = r.skill_dir.to_string_lossy().into_owned();
    let mut c = match r.harness {
        "claude" => {
            let mut a = vec![s("-p"), format!("Run the desktopglass-index skill in headless mode.{}", r.full_note)];
            a.extend(m("--model"));
            a.extend([s("--permission-mode"), s("acceptEdits"), s("--allowedTools"), claude_allowed_tools(r.home)]);
            a.extend([s("--max-turns"), s("40")]);
            cmd(r.bin, a)
        }
        "codex" => {
            // Codex sandboxes itself (Seatbelt can't be nested): workspace-write limited to ~/Desktop
            let mut a = vec![s("exec"), s("--full-auto"), s("--skip-git-repo-check"), s("-C")];
            a.push(r.desk.to_string_lossy().into_owned());
            a.extend(m("-m"));
            a.push(prompt());
            return Some(cmd(r.bin, a));
        }
        "gemini" => {
            let mut a = vec![s("-p"), prompt(), s("--approval-mode"), s("yolo")];
            a.extend(m("-m"));
            let mut c = cmd(r.bin, a);
            c.env.push((s("GEMINI_CLI_TRUST_WORKSPACE"), s("true")));
            c
        }
        "antigravity" => {
            // headless agy soft-denies tools that need approval, so an unattended run needs the blanket flag
            let mut a = vec![s("-p"), prompt(), s("--dangerously-skip-permissions"), s("--add-dir"), skill];
            a.extend(m("--model"));
            cmd(r.bin, a)
        }
        "cursor" => {
            let mut a = vec![s("-p"), s("--force"), s("--trust"), s("--output-format"), s("text")];
            a.extend(m("-m"));
            a.push(prompt());
            cmd(r.bin, a)
        }
        "copilot" => {
            let mut a = vec![s("-p"), prompt(), s("-s"), s("--no-ask-user"), s("--add-dir"), skill];
            a.extend(m("--model"));
            a.extend([s("--allow-tool"), copilot_allowed_tools(r.home)]);
            cmd(r.bin, a)
        }
        "opencode" => {
            let mut a = vec![s("run")];
            a.extend(m("-m"));
            a.push(prompt());
            cmd(r.bin, a)
        }
        _ => return None,
    };
    c.sandboxed = true;
    Some(c)
}

/// `--auth-only` without an offline answer: ask for a one-word reply (a few tokens). Never sandboxed.
pub fn auth_probe(harness: &str, bin: &str) -> Option<Cmd> {
    let p = s(OK_PROMPT);
    let mut c = match harness {
        "claude" => cmd(bin, vec![s("-p"), p, s("--max-turns"), s("1")]),
        "codex" => cmd(bin, vec![s("exec"), s("--skip-git-repo-check"), p]),
        "gemini" => cmd(bin, vec![s("-p"), p]),
        "antigravity" => cmd(bin, vec![s("-p"), p]),
        "cursor" => cmd(bin, vec![s("-p"), s("--trust"), s("--output-format"), s("text"), p]),
        "copilot" => cmd(bin, vec![s("-p"), p, s("-s"), s("--no-ask-user")]),
        "opencode" => cmd(bin, vec![s("run"), p]),
        _ => return None,
    };
    if harness == "gemini" {
        c.env.push((s("GEMINI_CLI_TRUST_WORKSPACE"), s("true")));
    }
    Some(c)
}

/// ask.sh: the interactive session for answering questions.md (None: unknown CLI).
pub fn interactive(harness: &str, bin: &str, model: &str, skill_dir: &Path) -> Option<Cmd> {
    let m = |flag: &str| if model.is_empty() { vec![] } else { vec![s(flag), s(model)] };
    let prompt = || skill_prompt("interactive", skill_dir);
    let skill = skill_dir.to_string_lossy().into_owned();
    let mut a = vec![];
    match harness {
        "claude" => {
            a.extend(m("--model"));
            a.push(s("/desktopglass-index"));
        }
        "codex" | "cursor" => {
            a.extend(m("-m"));
            a.push(prompt());
        }
        "gemini" => {
            a.extend(m("-m"));
            a.extend([s("-i"), prompt()]);
        }
        "antigravity" | "copilot" => {
            a.extend(m("--model"));
            a.extend([s("--add-dir"), skill, s("-i"), prompt()]);
        }
        "opencode" => {
            a.extend(m("-m"));
            a.extend([s("--prompt"), prompt()]);
        }
        _ => return None,
    }
    Some(cmd(bin, a))
}

/// login.sh for agent CLIs: (message lines printed first, command).
pub fn login(harness: &str, bin: &str) -> Option<(Option<&'static str>, Cmd)> {
    Some(match harness {
        "claude" => (Some("Type /login once Claude Code opens."), cmd(bin, vec![])),
        "codex" => (None, cmd(bin, vec![s("login")])),
        "gemini" => (Some("Choose a sign-in method when Gemini CLI opens."), cmd(bin, vec![])),
        "antigravity" => (Some("Antigravity opens your browser to sign in."), cmd(bin, vec![])),
        "cursor" => (None, cmd(bin, vec![s("login")])),
        "copilot" => (Some("Type /login once Copilot opens."), cmd(bin, vec![])),
        "opencode" => (None, cmd(bin, vec![s("auth"), s("login")])),
        _ => return None,
    })
}

// ---------- optional unattended-run sandbox ----------
// Off by default — each CLI's own permission controls apply. When turned on (Settings → Re-indexing → Sandbox),
// agent CLIs run inside a macOS sandbox (Seatbelt):
//   • writes allowed only to description.md files and _Organize Log on the Desktop, temp folders, and the
//     CLIs' own state/config folders (sessions, caches, token refresh)
//   • reads of SSH, cloud and GPG credentials are blocked
// Network stays open (the CLI has to reach its model). Enabled with DESKTOP_INDEX_SANDBOX=1.

pub const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// `_sb_re`: escapes a path for an SBPL regex.
pub fn sb_re(p: &str) -> String {
    let mut out = String::new();
    for c in p.chars() {
        if "[]\\.^$*+?(){}|".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The Seatbelt profile (common.sh's `sandbox_profile`). `desk_resolved` is `${DESK:A}`.
pub fn sandbox_profile(home: &str, desk: &str, desk_resolved: &str, skill_dir: &str) -> String {
    let h = home;
    let mut desks = vec![desk];
    if desk_resolved != desk {
        desks.push(desk_resolved);
    }
    let mut allow: Vec<String> = vec![];
    for x in desks {
        allow.push(format!("(regex #\"^{}/(.+/)?\\.?description\\.md\")", sb_re(x)));
        allow.push(format!("(subpath \"{x}/_Organize Log\")"));
    }
    // temp files and the CLIs' own session/cache folders
    allow.extend([
        s("(subpath \"/private/tmp\")"),
        s("(subpath \"/private/var/folders\")"),
        s("(subpath \"/dev\")"),
        format!("(subpath \"{h}/.claude\")"),
        format!("(regex #\"^{}/\\.claude\\.json\")", sb_re(h)),
    ]);
    for sub in [
        ".gemini", ".cursor", ".copilot", ".config/opencode", ".config/github-copilot", ".config/configstore",
        ".config/cursor", ".cache", ".local/share/opencode", ".local/state", ".npm", ".bun/install/cache",
        "Library/Caches", "Library/Logs",
    ] {
        allow.push(format!("(subpath \"{h}/{sub}\")"));
    }
    // …but never their settings, hooks, skills or MCP lists (that would let a run change what runs next time),
    // and never DesktopGlass's own scripts
    let mut protect = vec![format!("(subpath \"{skill_dir}\")")];
    for (kind, rel) in [
        ("literal", ".claude/settings.json"),
        ("literal", ".claude/settings.local.json"),
        ("literal", ".claude/CLAUDE.md"),
        ("subpath", ".claude/skills"),
        ("subpath", ".claude/commands"),
        ("subpath", ".claude/agents"),
        ("subpath", ".claude/hooks"),
        ("literal", ".gemini/settings.json"),
        ("literal", ".gemini/GEMINI.md"),
        ("literal", ".gemini/config/mcp_config.json"),
        ("literal", ".gemini/antigravity-cli/settings.json"),
        ("literal", ".cursor/mcp.json"),
        ("literal", ".cursor/cli.json"),
        ("subpath", ".cursor/rules"),
        ("literal", ".copilot/mcp-config.json"),
        ("literal", ".copilot/config.json"),
        ("literal", ".config/opencode/opencode.json"),
        ("literal", ".config/opencode/config.json"),
    ] {
        protect.push(format!("({kind} \"{h}/{rel}\")"));
    }
    format!(
        "(version 1)
(allow default)
(deny file-read* (subpath \"{h}/.ssh\") (subpath \"{h}/.aws\") (subpath \"{h}/.gnupg\") (subpath \"{h}/.kube\")
                 (subpath \"{h}/.docker\") (subpath \"{h}/.azure\") (subpath \"{h}/.config/gcloud\") (literal \"{h}/.netrc\"))
(deny file-write* (require-not (require-any {})))
(deny file-write* {})
",
        allow.join(" "),
        protect.join(" ")
    )
}

/// `run_sandboxed`: the argv to execute — wrapped in sandbox-exec when the sandbox is on and available.
pub fn wrap_sandbox(c: &Cmd, enabled: bool, profile: impl FnOnce() -> String) -> Vec<String> {
    if c.sandboxed && enabled && Path::new(SANDBOX_EXEC).exists() {
        let mut v = vec![s(SANDBOX_EXEC), s("-p"), profile()];
        v.extend(c.args.iter().cloned());
        v
    } else {
        c.args.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_strips_front_matter_and_substitutes() {
        let md = "---\nname: x\n---  \n# Title\nrun <skill>/a and <skill>/b\n\n\n";
        assert_eq!(skill_body(md, "/S"), "# Title\nrun /S/a and /S/b");
        assert_eq!(skill_body("no front matter\n", "/S"), "");
    }

    #[test]
    fn prompt_wording() {
        let dir = std::env::temp_dir().join(format!("dg-skill-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), "---\nname: t\n---\nBody at <skill>.\n").unwrap();
        let p = skill_prompt("headless", &dir);
        assert!(p.starts_with("Run the desktopglass-index task in headless mode. The working directory is ~/Desktop."));
        assert!(p.contains("This is a HEADLESS run: nobody is watching"));
        assert!(p.ends_with(&format!("instead.\n\nBody at {}.", dir.display())));
        let i = skill_prompt("interactive", &dir);
        assert!(i.contains("ask the questions in plain text and wait for the answers.\n\nBody"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn claude_and_copilot_allow_lists() {
        let r = Run {
            harness: "claude",
            bin: "/b/claude",
            model: "haiku",
            desk: Path::new("/h/Desktop"),
            skill_dir: Path::new("/nonexistent"),
            home: "/h",
            full_note: "",
        };
        let c = headless(&r).unwrap();
        assert_eq!(
            c.args,
            [
                "/b/claude", "-p", "Run the desktopglass-index skill in headless mode.", "--model", "haiku",
                "--permission-mode", "acceptEdits", "--allowedTools",
                "Skill,Read,Write,Edit,Bash(~/.local/bin/dg inventory:*),Bash(/h/.local/bin/dg inventory:*)",
                "--max-turns", "40"
            ]
        );
        assert!(c.sandboxed);
        let co = headless(&Run { harness: "copilot", model: "", ..r }).unwrap();
        assert_eq!(co.args.last().unwrap(), "read, write, shell(~/.local/bin/dg:*), shell(/h/.local/bin/dg:*)");
        let cx = headless(&Run { harness: "codex", ..r }).unwrap();
        assert!(!cx.sandboxed, "codex sandboxes itself");
        assert_eq!(&cx.args[1..7], ["exec", "--full-auto", "--skip-git-repo-check", "-C", "/h/Desktop", "-m"]);
        assert!(headless(&Run { harness: "nope", ..r }).is_none());
    }

    #[test]
    fn profile_shape() {
        let p = sandbox_profile("/Users/a", "/Users/a/Desktop", "/Volumes/D (1)/Desktop", "/Users/a/.claude/skills/desktopglass-index");
        assert!(p.starts_with("(version 1)\n(allow default)\n(deny file-read* (subpath \"/Users/a/.ssh\")"));
        assert!(p.contains(r#"(regex #"^/Users/a/Desktop/(.+/)?\.?description\.md")"#));
        assert!(p.contains(r#"(regex #"^/Volumes/D \(1\)/Desktop/(.+/)?\.?description\.md")"#));
        assert!(p.contains(r#"(regex #"^/Users/a/\.claude\.json")"#));
        assert!(p.contains("(deny file-write* (subpath \"/Users/a/.claude/skills/desktopglass-index\") (literal \"/Users/a/.claude/settings.json\")"));
        assert!(p.ends_with("(literal \"/Users/a/.config/opencode/config.json\"))\n"));
        let c = Cmd { args: vec![s("x")], env: vec![], sandboxed: true };
        assert_eq!(wrap_sandbox(&c, false, || unreachable!()), ["x"]);
        if Path::new(SANDBOX_EXEC).exists() {
            assert_eq!(wrap_sandbox(&c, true, || s("P")), [SANDBOX_EXEC, "-p", "P", "x"]);
        }
    }
}
