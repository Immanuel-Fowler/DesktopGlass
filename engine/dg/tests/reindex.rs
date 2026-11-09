//! End-to-end checks of `dg reindex` / `dg ask` / `dg login` with fake CLIs and a fake model server: every exit
//! code, the status lines Reindexer.swift shows, the lock, reindex.log and the per-CLI command lines.
//! Nothing real runs: HOME is a temp folder, PATH is pinned with DESKTOP_INDEX_PATH to fake scripts plus
//! /usr/bin:/bin, and the Desktop is a copy of a golden fixture.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const DG: &str = env!("CARGO_BIN_EXE_dg");

/// Records its arguments (separated by \x1e) and working directory, then prints $FAKE_OUT and exits $FAKE_CODE.
/// `login`/`status`/`auth` (the offline sign-in checks) print $FAKE_STATUS_OUT instead.
const FAKE: &str = r#"#!/bin/sh
n=$(basename "$0")
case "$1" in login|status|auth) [ -n "$FAKE_STATUS_OUT" ] && { printf '%s\n' "$FAKE_STATUS_OUT"; exit 0; } ;; esac
: > "$HOME/args-$n"; for a in "$@"; do printf '%s\036' "$a" >> "$HOME/args-$n"; done
pwd > "$HOME/pwd-$n"
[ -n "$FAKE_CAT" ] && cat "$FAKE_CAT"
[ -n "$FAKE_OUT" ] && printf '%s\n' "$FAKE_OUT"
exit ${FAKE_CODE:-0}
"#;

struct Env {
    root: PathBuf,
    home: PathBuf,
    vars: Vec<(String, String)>,
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let t = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &t);
        } else {
            std::fs::copy(e.path(), &t).unwrap();
        }
    }
}

fn exe(p: &Path, text: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(p, text).unwrap();
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
}

impl Env {
    /// `fixture`: a golden Desktop to copy, or None for an empty one. `fakes`: binary names to create.
    fn new(tag: &str, fixture: Option<&str>, fakes: &[&str]) -> Env {
        let tmp = std::fs::canonicalize(std::env::temp_dir()).unwrap();
        let root = tmp.join(format!("dg-reindex-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        let desk = home.join("Desktop");
        match fixture {
            Some(f) => copy_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/golden/desktops").join(f), &desk),
            None => std::fs::create_dir_all(&desk).unwrap(),
        }
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        for f in fakes {
            exe(&bin.join(f), FAKE);
        }
        let skill = home.join(".claude/skills/desktopglass-index");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(skill.join("SKILL.md"), "---\nname: desktopglass-index\n---\n\n# Desktop Index\nRun `<skill>/x`.\n").unwrap();
        let vars = vec![
            ("HOME".into(), home.to_string_lossy().into_owned()),
            ("PATH".into(), "/usr/bin:/bin".into()),
            ("TMPDIR".into(), std::env::temp_dir().to_string_lossy().into_owned()),
            ("DESKTOP_INDEX_PATH".into(), format!("{}:/usr/bin:/bin", bin.display())),
        ];
        Env { root, home, vars }
    }

    fn desk(&self) -> PathBuf {
        self.home.join("Desktop")
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.desk().join("_Organize Log/reindex.log")).unwrap_or_default()
    }

    fn args(&self, bin: &str) -> Vec<String> {
        let s = std::fs::read_to_string(self.home.join(format!("args-{bin}"))).unwrap_or_default();
        s.split('\u{1e}').filter(|a| !a.is_empty()).map(String::from).collect()
    }

    fn run(&self, sub: &str, args: &[&str], extra: &[(&str, &str)]) -> Output {
        let mut c = Command::new(DG);
        c.env_clear().arg(sub).args(args);
        for (k, v) in &self.vars {
            c.env(k, v);
        }
        for (k, v) in extra {
            c.env(k, v);
        }
        c.output().unwrap()
    }

    fn reindex(&self, extra: &[(&str, &str)]) -> (i32, String) {
        self.reindex_args(&[], extra)
    }

    fn reindex_args(&self, args: &[&str], extra: &[(&str, &str)]) -> (i32, String) {
        let o = self.run("reindex", args, extra);
        let text = String::from_utf8_lossy(&o.stdout).into_owned() + &String::from_utf8_lossy(&o.stderr);
        // Reindexer.swift shows the last line
        let last = text.lines().last().unwrap_or("").to_string();
        (o.status.code().unwrap_or(-1), last)
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

const UNATTENDED: (&str, &str) = ("DESKTOP_INDEX_UNATTENDED", "1");

// ---------- agent CLIs ----------

#[test]
fn up_to_date_exits_10_without_calling_the_cli() {
    let e = Env::new("uptodate", None, &["claude"]);
    std::fs::create_dir_all(e.desk().join("Docs")).unwrap();
    std::fs::write(e.desk().join("Docs/a.txt"), "x").unwrap();
    std::fs::write(e.desk().join("Docs/description.md"), "# Docs\n> d\n\n## Contents\n- **a.txt** — a\n").unwrap();
    let o = Command::new(DG).args(["inventory", e.desk().to_str().unwrap(), "commit"]).output().unwrap();
    assert!(o.status.success());
    assert_eq!(e.reindex(&[UNATTENDED]), (10, "Up to date — nothing changed".into()));
    assert!(e.args("claude").is_empty(), "no AI call");
    assert!(!e.desk().join("_Organize Log/.reindex.lock").exists(), "lock released");
}

#[test]
fn claude_success_exits_0_commits_and_logs() {
    let e = Env::new("claude-ok", Some("basic"), &["claude"]);
    let (code, last) = e.reindex(&[UNATTENDED, ("DESKTOP_INDEX_MODEL", "haiku"), ("DESKTOP_INDEX_REASON", "button"), ("FAKE_OUT", "done")]);
    assert_eq!(code, 0, "{last}");
    assert!(e.desk().join("_Organize Log/desktop-index.md").exists(), "safety-net commit");
    assert!(e.desk().join("_Organize Log/.manifest.json").exists());
    assert!(!last.is_empty() && !last.contains("exited"), "status line from commit: {last}");
    let home = e.home.to_string_lossy();
    assert_eq!(
        e.args("claude"),
        [
            "-p", "Run the desktopglass-index skill in headless mode.", "--model", "haiku", "--permission-mode", "acceptEdits",
            "--allowedTools", &format!("Skill,Read,Write,Edit,Bash(~/.local/bin/dg inventory:*),Bash({home}/.local/bin/dg inventory:*)"),
            "--max-turns", "40"
        ]
    );
    let pwd = std::fs::read_to_string(e.home.join("pwd-claude")).unwrap();
    assert_eq!(Path::new(pwd.trim()), e.desk(), "runs in ~/Desktop");
    let log = e.log();
    let first = log.lines().next().unwrap();
    assert!(first.starts_with("=== ") && first.ends_with(" · button · Claude Code · haiku · exit 0"), "{first}");
    assert!(log.contains("\ndone\n"));
    assert!(!e.desk().join("_Organize Log/.reindex.lock").exists());
}

#[test]
fn full_rewrite_and_skill_prompt_for_codex() {
    let e = Env::new("codex-full", Some("basic"), &["codex"]);
    let (code, _) = e.reindex_args(&["--full", "--harness", "codex", "--model", "gpt-5.5"], &[UNATTENDED]);
    assert_eq!(code, 0);
    let a = e.args("codex");
    let desk = e.desk().to_string_lossy().into_owned();
    assert_eq!(a[..7], ["exec", "--full-auto", "--skip-git-repo-check", "-C", desk.as_str(), "-m", "gpt-5.5"]);
    let prompt = &a[7];
    assert!(prompt.starts_with("Run the desktopglass-index task in headless mode. The working directory is ~/Desktop."));
    assert!(prompt.contains("This is a HEADLESS run: nobody is watching, so never ask or wait."));
    let skill = e.home.join(".claude/skills/desktopglass-index");
    assert!(prompt.contains(&format!("# Desktop Index\nRun `{}/x`.", skill.display())), "{prompt}");
    assert!(prompt.ends_with("`. This is a FULL REWRITE requested from DesktopGlass Settings: run the scan with --all and rewrite every description to match the current preferences."));
}

#[test]
fn copilot_allow_list_and_gemini_flags() {
    let e = Env::new("copilot", Some("basic"), &["copilot", "gemini"]);
    assert_eq!(e.reindex(&[UNATTENDED, ("DESKTOP_INDEX_HARNESS", "copilot")]).0, 0);
    let a = e.args("copilot");
    let home = e.home.to_string_lossy();
    assert_eq!(a.last().unwrap(), &format!("read, write, shell(~/.local/bin/dg:*), shell({home}/.local/bin/dg:*)"));
    assert!(!a.iter().any(|x| x.contains("python3")));
    assert_eq!(e.reindex(&[UNATTENDED, ("DESKTOP_INDEX_HARNESS", "gemini"), ("DESKTOP_INDEX_FULL", "1")]).0, 0);
    let g = e.args("gemini");
    assert_eq!(g[0], "-p");
    assert_eq!(g[2..], ["--approval-mode", "yolo"]);
}

#[test]
fn consent_missing_exits_79() {
    let e = Env::new("consent", Some("basic"), &["claude"]);
    let (code, last) = e.reindex(&[]);
    assert_eq!(code, 79);
    assert_eq!(last, "Unattended runs with Claude Code are off — review and allow them in Settings → Re-indexing");
    assert!(e.log().contains(" · skipped: unattended runs not allowed for Claude Code\n"));
    assert!(e.args("claude").is_empty());
}

#[test]
fn preflight_signed_out_exits_77_without_running() {
    let e = Env::new("preflight", Some("basic"), &["codex"]);
    let (code, last) = e.reindex(&[UNATTENDED, ("DESKTOP_INDEX_HARNESS", "codex"), ("FAKE_STATUS_OUT", "Not logged in")]);
    assert_eq!(code, 77);
    assert_eq!(last, "Not signed in to Codex CLI — Run `codex login` in Terminal");
    assert!(e.log().contains(" · skipped: not signed in to Codex CLI\n"));
    assert!(e.args("codex").is_empty(), "no headless run");
}

#[test]
fn failures_are_classified() {
    let e = Env::new("classify", Some("basic"), &["claude", "gemini", "opencode"]);
    let (code, last) = e.reindex(&[UNATTENDED, ("FAKE_OUT", "Invalid API key · Please run /login"), ("FAKE_CODE", "1")]);
    assert_eq!((code, last.as_str()), (77, "Not signed in to Claude Code — Run `claude` in Terminal and type /login"));
    let (code, last) = e.reindex(&[UNATTENDED, ("DESKTOP_INDEX_HARNESS", "gemini"), ("FAKE_OUT", "Error: 429 Too Many Requests"), ("FAKE_CODE", "1")]);
    assert_eq!((code, last.as_str()), (76, "Gemini CLI hit a usage or rate limit — try again later or switch CLI"));
    let (code, last) = e.reindex(&[UNATTENDED, ("DESKTOP_INDEX_HARNESS", "opencode"), ("FAKE_OUT", "boom"), ("FAKE_CODE", "3")]);
    assert_eq!((code, last.as_str()), (3, "OpenCode exited with 3 — see _Organize Log/reindex.log"));
    assert!(e.log().contains(" · manual · OpenCode · exit 3\nboom\n"));
    let (code, last) = e.reindex(&[UNATTENDED, ("FAKE_OUT", "EPERM: Operation not permitted, open '/x'"), ("FAKE_CODE", "1")]);
    assert_eq!((code, last.as_str()), (1, "Claude Code tried to do something outside DesktopGlass's sandbox — see _Organize Log/reindex.log"));
}

#[test]
fn lock_busy_exits_75_and_stale_lock_is_taken_over() {
    let e = Env::new("lock", Some("basic"), &["claude"]);
    let lock = e.desk().join("_Organize Log/.reindex.lock");
    std::fs::create_dir_all(&lock).unwrap();
    assert_eq!(e.reindex(&[UNATTENDED]), (75, "Already indexing".into()));
    assert!(lock.exists(), "someone else's lock stays");
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(31 * 60);
    std::fs::File::open(&lock).unwrap().set_modified(old).unwrap();
    assert_eq!(e.reindex(&[UNATTENDED]).0, 0);
    assert!(!lock.exists());
}

#[test]
fn missing_cli_exits_127() {
    let e = Env::new("missing", Some("basic"), &[]);
    let (code, last) = e.reindex(&[UNATTENDED, ("DESKTOP_INDEX_HARNESS", "codex")]);
    assert_eq!((code, last.as_str()), (127, "Codex CLI isn't installed — install it or pick another CLI in Settings"));
    let (code, last) = e.reindex(&[("DESKTOP_INDEX_HARNESS", "apple")]);
    assert_eq!((code, last.as_str()), (127, "Apple Foundation Models run through the DesktopGlass app (macOS 26+)"));
    let (code, last) = e.reindex(&[("DESKTOP_INDEX_HARNESS", "nope")]);
    assert_eq!((code, last.as_str()), (127, "nope isn't installed — install it or pick another CLI in Settings"));
    let (code, _) = e.reindex(&[("DESKTOP_INDEX_HARNESS", "cursor")]);
    assert_eq!(code, 127);
}

#[test]
fn permissions_and_usage() {
    let e = Env::new("perms", None, &["claude"]);
    assert_eq!(e.reindex_args(&["--permissions"], &[]), (0, "Tool allow-list: Read, Write, Edit and dg only; edits auto-accepted.".into()));
    assert_eq!(e.reindex_args(&["--bogus"], &[]).0, 2);
}

#[test]
fn auth_only_for_agent_clis() {
    let e = Env::new("auth", None, &["codex", "claude"]);
    let codex = ("DESKTOP_INDEX_HARNESS", "codex");
    assert_eq!(e.reindex_args(&["--auth-only"], &[codex, ("FAKE_STATUS_OUT", "Logged in using ChatGPT")]), (0, "Signed in to Codex CLI".into()));
    assert_eq!(e.args("codex"), ["exec", "--skip-git-repo-check", "Reply with the word OK."]);
    assert_eq!(e.reindex_args(&["--auth-only"], &[codex, ("FAKE_STATUS_OUT", "Not logged in")]).0, 77);
    let (code, last) = e.reindex_args(&["--auth-only"], &[("FAKE_OUT", "API Error: 401 {\"type\":\"authentication_error\"}"), ("FAKE_CODE", "1")]);
    assert_eq!((code, last.as_str()), (77, "Not signed in to Claude Code — Run `claude` in Terminal and type /login"));
    assert_eq!(e.args("claude"), ["-p", "Reply with the word OK.", "--max-turns", "1"]);
    let (code, last) = e.reindex_args(&["--auth-only"], &[("FAKE_OUT", "usage limit reached"), ("FAKE_CODE", "1")]);
    assert_eq!((code, last.as_str()), (76, "Claude Code is signed in but hit a usage limit"));
    let (code, last) = e.reindex_args(&["--auth-only"], &[("FAKE_OUT", "?"), ("FAKE_CODE", "5")]);
    assert_eq!((code, last.as_str()), (1, "Claude Code couldn't be reached (exit 5)"));
}

#[test]
fn sandbox_blocks_credential_reads() {
    if !Path::new("/usr/bin/sandbox-exec").exists() {
        return;
    }
    let e = Env::new("sandbox", Some("basic"), &["claude"]);
    std::fs::create_dir_all(e.home.join(".ssh")).unwrap();
    std::fs::write(e.home.join(".ssh/id_test"), "SECRET").unwrap();
    let cat = e.home.join(".ssh/id_test");
    let cat = cat.to_str().unwrap();
    // without the sandbox the fake CLI can read it
    assert_eq!(e.reindex(&[UNATTENDED, ("FAKE_CAT", cat)]).0, 0);
    assert!(e.log().contains("SECRET"));
    std::fs::remove_file(e.desk().join("_Organize Log/reindex.log")).unwrap();
    let (code, _) = e.reindex_args(&["--full", "--sandbox"], &[UNATTENDED, ("FAKE_CAT", cat)]);
    assert_eq!(code, 0, "{}", e.log());
    let log = e.log();
    assert!(!log.contains("SECRET") && log.contains("Operation not permitted"), "{log}");
}

// ---------- on-device backends ----------

/// A fake model server: `routes(path, body) -> (status, json)`. Returns its base URL.
fn serve(routes: fn(&str, &str) -> (u16, String)) -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    std::thread::spawn(move || {
        for c in l.incoming() {
            let Ok(mut c) = c else { continue };
            let mut buf = Vec::new();
            let mut chunk = [0u8; 8192];
            let (head_end, len) = loop {
                let n = c.read(&mut chunk).unwrap_or(0);
                if n == 0 {
                    break (buf.len(), 0);
                }
                buf.extend_from_slice(&chunk[..n]);
                if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&buf[..i]).to_lowercase();
                    let len = head
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length:").map(|v| v.trim().parse().unwrap_or(0)))
                        .unwrap_or(0);
                    break (i + 4, len);
                }
            };
            while buf.len() < head_end + len {
                let n = c.read(&mut chunk).unwrap_or(0);
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            let text = String::from_utf8_lossy(&buf).into_owned();
            let path = text.split_whitespace().nth(1).unwrap_or("/").to_string();
            let body = text.get(head_end..).unwrap_or("").to_string();
            let (status, reply) = routes(&path, &body);
            let _ = write!(c, "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}", reply.len());
        }
    });
    format!("http://{addr}")
}

const DESC_REPLY: &str = "<think>planning</think>\n```markdown\n# Whatever\n> Described by the model.\nTags: Docs, Test\n\n## Contents\n- **Taxes/** — tax papers\n- **data.csv** — a table\n```";

fn ollama_ok(path: &str, body: &str) -> (u16, String) {
    match path {
        "/api/tags" => (200, r#"{"models":[{"name":"llama3.2:latest"}]}"#.into()),
        "/api/chat" => {
            assert!(body.contains("\"num_ctx\":8192") && body.contains("Entries as JSON"), "{body}");
            (200, serde_json_reply("message", DESC_REPLY))
        }
        _ => (404, "{}".into()),
    }
}

fn serde_json_reply(key: &str, content: &str) -> String {
    let esc = content.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n");
    if key == "message" {
        format!(r#"{{"message":{{"role":"assistant","content":"{esc}"}}}}"#)
    } else {
        format!(r#"{{"choices":[{{"message":{{"role":"assistant","content":"{esc}"}}}}]}}"#)
    }
}

#[test]
fn ollama_indexes_and_writes_descriptions() {
    let e = Env::new("ollama", Some("basic"), &[]);
    let url = serve(ollama_ok);
    let ollama = [("DESKTOP_INDEX_HARNESS", "ollama"), ("DESKTOP_INDEX_ENDPOINT", url.as_str())];
    assert_eq!(e.reindex_args(&["--auth-only"], &ollama), (0, "Ollama is ready · llama3.2:latest".into()));
    let (code, last) = e.reindex(&[ollama[0], ollama[1], ("DESKTOP_INDEX_MODEL", "llama3.2"), ("DESKTOP_INDEX_REASON", "schedule")]);
    assert_eq!(code, 0, "{last}\n{}", e.log());
    assert!(last.starts_with("described "), "{last}");
    let docs = std::fs::read_to_string(e.desk().join("Docs/description.md")).unwrap();
    assert!(docs.starts_with("# Docs\n> Described by the model.\nTags: docs, test\n\n## Contents\n- **data.csv** — a table\n"), "{docs}");
    assert!(docs.contains("- **Taxes/** — tax papers\n"));
    // an unchanged blurb is kept verbatim (Projects has a description with App/ → "the Mac app")
    let projects = std::fs::read_to_string(e.desk().join("Projects/description.md")).unwrap();
    assert!(projects.contains("- **App/** — the Mac app"), "{projects}");
    assert!(e.desk().join("_Organize Log/desktop-index.md").exists());
    assert!(e.log().starts_with("=== ") && e.log().lines().next().unwrap().ends_with(" · schedule · Ollama · llama3.2 · exit 0"));
    // nothing changed now
    assert_eq!(e.reindex(&ollama).0, 10);
}

#[test]
fn ollama_not_running_or_model_missing_exits_78() {
    let e = Env::new("ollama78", Some("basic"), &[]);
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let dead = format!("http://127.0.0.1:{port}");
    let (code, last) = e.reindex(&[("DESKTOP_INDEX_HARNESS", "ollama"), ("DESKTOP_INDEX_ENDPOINT", &dead)]);
    assert_eq!((code, last), (78, format!("Ollama isn't running at {dead} — open the Ollama app or run `ollama serve`")));
    let url = serve(ollama_ok);
    let (code, last) = e.reindex(&[("DESKTOP_INDEX_HARNESS", "ollama"), ("DESKTOP_INDEX_ENDPOINT", &url), ("DESKTOP_INDEX_MODEL", "qwen")]);
    assert_eq!((code, last.as_str()), (78, "Ollama model 'qwen' isn't pulled — run `ollama pull qwen`"));
}

#[test]
fn server_errors_stop_with_78_and_bad_replies_skip() {
    let e = Env::new("errors", Some("basic"), &[]);
    let url = serve(|p, _| if p == "/api/tags" { (200, r#"{"models":[{"name":"m"}]}"#.into()) } else { (500, "{}".into()) });
    let (code, last) = e.reindex(&[("DESKTOP_INDEX_HARNESS", "ollama"), ("DESKTOP_INDEX_ENDPOINT", &url)]);
    assert_eq!(code, 78);
    assert!(last.starts_with("Ollama stopped responding (") && last.ends_with(") — is it still running?"), "{last}");
    let url = serve(|p, _| if p == "/api/tags" { (200, r#"{"models":[{"name":"m"}]}"#.into()) } else { (200, "not json".into()) });
    let (code, last) = e.reindex(&[("DESKTOP_INDEX_HARNESS", "ollama"), ("DESKTOP_INDEX_ENDPOINT", &url)]);
    assert_eq!((code, last.as_str()), (1, "Ollama exited with 1"));
    assert!(e.log().contains("\nskipped Docs: "));
}

#[test]
fn lmstudio_and_mlx_over_openai_api() {
    let e = Env::new("openai", Some("basic"), &[]);
    let empty = serve(|_, _| (200, r#"{"data":[]}"#.into()));
    let (code, last) = e.reindex(&[("DESKTOP_INDEX_HARNESS", "lmstudio"), ("DESKTOP_INDEX_ENDPOINT", &empty)]);
    assert_eq!((code, last.as_str()), (78, "LM Studio has no model loaded"));
    let url = serve(|p, body| match p {
        "/v1/models" => (200, r#"{"data":[{"id":"mlx-community/Qwen"}]}"#.into()),
        "/v1/chat/completions" => {
            assert!(body.contains("\"temperature\":0.2"), "{body}");
            (200, serde_json_reply("choices", DESC_REPLY))
        }
        _ => (404, "{}".into()),
    });
    let (code, last) = e.reindex(&[("DESKTOP_INDEX_HARNESS", "mlx"), ("DESKTOP_INDEX_ENDPOINT", &url), ("DESKTOP_INDEX_MODEL", "mlx-community/Qwen")]);
    assert_eq!(code, 0, "{last}\n{}", e.log());
    assert!(std::fs::read_to_string(e.desk().join("Docs/description.md")).unwrap().contains("> Described by the model."));
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let (code, last) = e.reindex(&[("DESKTOP_INDEX_HARNESS", "llamacpp"), ("DESKTOP_INDEX_ENDPOINT", &format!("http://127.0.0.1:{port}")), ("DESKTOP_INDEX_FULL", "1")]);
    assert_eq!(code, 78);
    assert!(last.ends_with("— start llama-server"), "{last}");
}

#[test]
fn apple_foundation_models_through_the_app_binary() {
    let e = Env::new("apple", Some("basic"), &[]);
    let app = e.root.join("DesktopGlass");
    exe(
        &app,
        &format!(
            "#!/bin/sh\ncase \"$1\" in --foundation-models-check) echo ready; exit 0;; --foundation-models) cat > \"$HOME/fm-in\"; printf '%s\\n' '{}'; exit 0;; esac\nexit 2\n",
            DESC_REPLY.replace('\'', "")
        ),
    );
    let apple = [("DESKTOP_INDEX_HARNESS", "apple"), ("DESKTOP_GLASS_BIN", app.to_str().unwrap())];
    assert_eq!(e.reindex_args(&["--auth-only"], &apple), (0, "Apple Foundation Models is ready · on-device".into()));
    let (code, last) = e.reindex(&apple);
    assert_eq!(code, 0, "{last}\n{}", e.log());
    let input = std::fs::read_to_string(e.home.join("fm-in")).unwrap();
    assert!(input.starts_with("{\"system\":\"You write short description.md files"), "{input}");
    assert!(std::fs::read_to_string(e.desk().join("Docs/description.md")).unwrap().contains("> Described by the model."));
    exe(&app, "#!/bin/sh\necho \"Apple's on-device model isn't available (deviceNotEligible).\"\nexit 78\n");
    let (code, last) = e.reindex_args(&["--full"], &apple);
    assert_eq!((code, last.as_str()), (78, "Apple's on-device model isn't available (deviceNotEligible)."));
}

// ---------- ask / login ----------

#[test]
fn ask_and_login() {
    let e = Env::new("ask", None, &["claude", "codex", "gemini"]);
    let o = e.run("ask", &["ollama"], &[]);
    assert_eq!(o.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&o.stdout).starts_with("On-device models can't hold a conversation here."));
    assert_eq!(e.run("ask", &["opencode"], &[]).status.code(), Some(127));
    assert_eq!(e.run("ask", &["claude", "sonnet"], &[]).status.code(), Some(0));
    assert_eq!(e.args("claude"), ["--model", "sonnet", "/desktopglass-index"]);
    let pwd = std::fs::read_to_string(e.home.join("pwd-claude")).unwrap();
    assert_eq!(Path::new(pwd.trim()), e.desk());
    assert_eq!(e.run("ask", &["gemini"], &[]).status.code(), Some(0));
    let g = e.args("gemini");
    assert_eq!(g[0], "-i");
    assert!(g[1].starts_with("Run the desktopglass-index task in interactive mode."));
    // login: exec's the CLI's sign-in flow (the fake's status branch answers `codex login`)
    let o = e.run("login", &["codex"], &[("FAKE_STATUS_OUT", "Successfully logged in")]);
    assert_eq!(o.status.code(), Some(0));
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(out.starts_with("Signing in to Codex CLI. When you're done, go back to DesktopGlass → Settings → Check sign-in.\n\n"));
    assert!(out.ends_with("Successfully logged in\n"));
    let o = e.run("login", &["claude"], &[]);
    assert!(String::from_utf8_lossy(&o.stdout).contains("Type /login once Claude Code opens."));
    assert_eq!(e.args("claude"), Vec::<String>::new());
    assert_eq!(e.run("login", &["copilot"], &[]).status.code(), Some(127));
    let o = e.run("login", &["llamacpp"], &[]);
    assert_eq!(String::from_utf8_lossy(&o.stdout), "Start a server, e.g.: llama-server -m model.gguf --port 8080\n");
}
