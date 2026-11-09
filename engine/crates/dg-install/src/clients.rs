//! Each AI app's MCP registration and the extras agent CLIs get with it (instructions block, session-start hook,
//! Claude Code's autoshow hook), in each app's own config format.
//!
//! Registrations run the engine: `<home>/.local/bin/dg mcp`; hooks run `<dg> hook session --format <cli>` and
//! `<dg> hook autoshow`. Registrations and hooks written by the Python installer (`python3 …/mcp_server.py`,
//! `…/session_context.py`, `…/autoshow.py`) are recognised as ours, so install/refresh replace them in place and
//! uninstall removes both forms.

use crate::ctx::{run, split_program, Ctx};
use crate::guide::{read_text, Guide};
use crate::pyjson::{read_json, sibling, write_json, Obj};
use crate::Error;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const NAME: &str = "desktopglass";
const TIMEOUT: Duration = Duration::from_secs(30);

fn io(e: std::io::Error) -> Error {
    Error::Other(e.to_string())
}

// ---------- recognising our commands ----------

/// `<…/dg> <sub…>`, e.g. `dg_command(cmd, "hook autoshow")`.
fn is_dg_command(cmd: &str, sub: &str) -> bool {
    let (prog, rest) = split_program(cmd);
    let base = Path::new(&prog).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if base != "dg" && base != "dg.exe" {
        return false;
    }
    let rest: Vec<&str> = rest.split_whitespace().collect();
    let sub: Vec<&str> = sub.split_whitespace().collect();
    rest.len() >= sub.len() && rest[..sub.len()] == sub[..]
}

fn python_form(cmd: &str, script: &str) -> bool {
    cmd.contains(script) && cmd.contains("desktopglass")
}

/// A hook command that runs DesktopGlass's autoshow (the dg form or autoshow.py).
pub fn is_autoshow(cmd: &str) -> bool {
    python_form(cmd, "autoshow.py") || is_dg_command(cmd, "hook autoshow")
}

/// A hook command that runs DesktopGlass's session-start hook (the dg form or session_context.py).
pub fn is_session(cmd: &str) -> bool {
    python_form(cmd, "session_context.py") || is_dg_command(cmd, "hook session")
}

fn command_of(hook: &Value) -> String {
    match hook.get("command") {
        Some(Value::String(s)) => s.clone(),
        Some(v) => v.to_string(),
        None => String::new(),
    }
}

pub fn session_command(ctx: &Ctx, fmt: &str) -> String {
    format!("{} hook session --format {fmt}", ctx.dg_word())
}

pub fn autoshow_command(ctx: &Ctx) -> String {
    format!("{} hook autoshow", ctx.dg_word())
}

// ---------- JSON helpers ----------

fn obj_mut<'a>(map: &'a mut Obj, key: &str, default: Value) -> Result<&'a mut Value, Error> {
    let v = map.entry(key.to_string()).or_insert(default);
    Ok(v)
}

fn as_obj<'a>(v: &'a mut Value, what: &str) -> Result<&'a mut Obj, Error> {
    v.as_object_mut().ok_or_else(|| Error::Other(format!("'{what}' isn't an object")))
}

fn as_arr<'a>(v: &'a mut Value, what: &str) -> Result<&'a mut Vec<Value>, Error> {
    v.as_array_mut().ok_or_else(|| Error::Other(format!("'{what}' isn't a list")))
}

fn truthy(v: Option<&Value>) -> Option<&Value> {
    v.filter(|v| dg_core::py::truthy(v))
}

/// `(data.get("hooks") or {}).get(event) or []`
fn groups<'a>(data: &'a Obj, event: &str) -> Vec<&'a Value> {
    truthy(data.get("hooks"))
        .and_then(|h| truthy(h.get(event)))
        .and_then(Value::as_array)
        .map(|a| a.iter().collect())
        .unwrap_or_default()
}

fn group_hooks(g: &Value) -> Vec<Value> {
    truthy(g.get("hooks")).and_then(Value::as_array).cloned().unwrap_or_default()
}

/// Removes our hooks from `data["hooks"][event]` groups (Claude Code layout), tidying empty sections.
fn remove_grouped(data: &mut Obj, event: &str, ours: fn(&str) -> bool) -> bool {
    let mut kept = Vec::new();
    let mut changed = false;
    for g in groups(data, event) {
        let all = group_hooks(g);
        let hooks: Vec<Value> = all.iter().filter(|h| !ours(&command_of(h))).cloned().collect();
        changed |= hooks.len() != all.len();
        if !hooks.is_empty() {
            let mut g = g.as_object().cloned().unwrap_or_default();
            g.insert("hooks".into(), Value::Array(hooks));
            kept.push(Value::Object(g));
        }
    }
    if changed {
        let hooks = data.get_mut("hooks").and_then(Value::as_object_mut).expect("hooks seen above");
        if kept.is_empty() {
            hooks.shift_remove(event);
        } else {
            hooks.insert(event.into(), Value::Array(kept));
        }
        if hooks.is_empty() {
            data.shift_remove("hooks");
        }
    }
    changed
}

fn add_grouped(data: &mut Obj, event: &str, group: Value) -> Result<(), Error> {
    let hooks = as_obj(obj_mut(data, "hooks", json!({}))?, "hooks")?;
    let list = hooks.entry(event.to_string()).or_insert_with(|| json!([]));
    as_arr(list, event)?.push(group);
    Ok(())
}

// ---------- MCP clients ----------

#[derive(Clone, Debug)]
pub enum Kind {
    /// A JSON config with our entry under `key`.
    Json { path: PathBuf, detect: Vec<PathBuf>, key: &'static str, entry: Value },
    /// `claude mcp add|get|remove`.
    ClaudeCode,
    /// ~/.codex/config.toml (via `codex mcp add` when available).
    Codex,
}

#[derive(Clone, Debug)]
pub struct Client {
    pub id: &'static str,
    pub name: &'static str,
    pub kind: Kind,
}

pub fn server_args() -> Vec<String> {
    vec!["mcp".into()]
}

pub fn clients(ctx: &Ctx) -> Vec<Client> {
    let h = &ctx.home;
    let sup = &ctx.app_config;
    let app = |n: &str| ctx.applications.join(n);
    let cmd = ctx.dg.clone();
    let std_entry = json!({"command": cmd, "args": server_args()});
    let json_client = |id, name, path: PathBuf, detect: Vec<PathBuf>, key, entry: &Value| Client {
        id,
        name,
        kind: Kind::Json { path, detect, key, entry: entry.clone() },
    };
    vec![
        Client { id: "claude-code", name: "Claude Code", kind: Kind::ClaudeCode },
        json_client("claude-desktop", "Claude Desktop", sup.join("Claude/claude_desktop_config.json"),
                    vec![sup.join("Claude"), app("Claude.app")], "mcpServers", &std_entry),
        Client { id: "codex", name: "Codex", kind: Kind::Codex },
        json_client("cursor", "Cursor", h.join(".cursor/mcp.json"), vec![h.join(".cursor"), app("Cursor.app")],
                    "mcpServers", &std_entry),
        json_client("gemini", "Gemini CLI", h.join(".gemini/settings.json"), vec![h.join(".gemini")], "mcpServers",
                    &std_entry),
        json_client("antigravity", "Antigravity", h.join(".gemini/config/mcp_config.json"),
                    vec![h.join(".gemini/antigravity-cli"), h.join(".gemini/antigravity"), h.join(".local/bin/agy"),
                         app("Antigravity.app")], "mcpServers", &std_entry),
        json_client("copilot", "GitHub Copilot CLI", h.join(".copilot/mcp-config.json"), vec![h.join(".copilot")],
                    "mcpServers", &json!({"type": "local", "command": cmd, "args": server_args(), "tools": ["*"]})),
        json_client("opencode", "OpenCode", h.join(".config/opencode/opencode.json"),
                    vec![h.join(".config/opencode"), h.join(".opencode")], "mcp",
                    &json!({"type": "local", "command": [cmd, "mcp"], "enabled": true})),
        json_client("lmstudio", "LM Studio", h.join(".lmstudio/mcp.json"), vec![h.join(".lmstudio"), app("LM Studio.app")],
                    "mcpServers", &std_entry),
        json_client("windsurf", "Windsurf", h.join(".codeium/windsurf/mcp_config.json"),
                    vec![h.join(".codeium/windsurf"), app("Windsurf.app")], "mcpServers", &std_entry),
        json_client("vscode", "VS Code", sup.join("Code/User/mcp.json"),
                    vec![sup.join("Code/User"), app("Visual Studio Code.app")], "servers",
                    &json!({"type": "stdio", "command": cmd, "args": server_args()})),
    ]
}

impl Client {
    pub fn detected(&self, ctx: &Ctx) -> bool {
        match &self.kind {
            Kind::Json { detect, .. } => detect.iter().any(|p| p.exists()),
            Kind::ClaudeCode => ctx.which("claude").is_some(),
            Kind::Codex => ctx.codex_binary().is_some() || ctx.codex_home.exists(),
        }
    }

    pub fn installed(&self, ctx: &Ctx) -> bool {
        match &self.kind {
            Kind::Json { path, key, .. } => read_json(path)
                .map(|d| truthy(d.get(*key)).and_then(Value::as_object).is_some_and(|m| m.contains_key(NAME)))
                .unwrap_or(false),
            Kind::ClaudeCode => self.detected(ctx) && claude(ctx, &["get", NAME]).is_ok_and(|o| o.code == Some(0)),
            Kind::Codex => codex_text(ctx).is_some_and(|t| t.contains(&codex_header())),
        }
    }

    /// Registers the server, replacing a registration that points somewhere else (e.g. the Python server).
    pub fn install(&self, ctx: &Ctx) -> Result<(), Error> {
        match &self.kind {
            Kind::Json { path, key, entry, .. } => {
                let mut data = read_json(path)?;
                let servers = as_obj(obj_mut(&mut data, key, json!({}))?, key)?;
                servers.insert(NAME.into(), entry.clone());
                write_json(path, &data)
            }
            Kind::ClaudeCode => {
                if self.detected(ctx) {
                    if let Ok(o) = claude(ctx, &["get", NAME]) {
                        if o.code == Some(0) {
                            if o.stdout.contains(&ctx.dg) && !o.stdout.contains("mcp_server.py") {
                                return Ok(());
                            }
                            claude(ctx, &["remove", "--scope", "user", NAME])?;
                        }
                    }
                }
                let mut args = vec!["add", "--scope", "user", NAME, "--", &ctx.dg];
                args.push("mcp");
                let o = claude(ctx, &args)?;
                if o.code != Some(0) {
                    let msg = if o.stderr.is_empty() { o.stdout } else { o.stderr };
                    return Err(Error::Other(msg.trim().chars().take(200).collect()));
                }
                Ok(())
            }
            Kind::Codex => codex_install(ctx),
        }
    }

    pub fn uninstall(&self, ctx: &Ctx) -> Result<(), Error> {
        match &self.kind {
            Kind::Json { path, key, .. } => {
                let mut data = read_json(path)?;
                let removed = data.get_mut(*key).and_then(Value::as_object_mut).and_then(|m| m.shift_remove(NAME));
                if removed.is_some() {
                    write_json(path, &data)?;
                }
                Ok(())
            }
            Kind::ClaudeCode => {
                if self.installed(ctx) {
                    let _ = claude(ctx, &["remove", "--scope", "user", NAME]);
                }
                Ok(())
            }
            Kind::Codex => codex_uninstall(ctx),
        }
    }
}

fn claude(ctx: &Ctx, args: &[&str]) -> Result<crate::ctx::Output, Error> {
    let bin = ctx.which("claude").ok_or_else(|| Error::Other("claude not found".into()))?;
    let mut all = vec!["mcp"];
    all.extend_from_slice(args);
    run(&bin, &all, "", TIMEOUT).map_err(io)
}

// ---------- Codex (TOML) ----------

fn codex_header() -> String {
    format!("[mcp_servers.{NAME}]")
}

fn codex_path(ctx: &Ctx) -> PathBuf {
    ctx.codex_home.join("config.toml")
}

fn codex_text(ctx: &Ctx) -> Option<String> {
    let p = codex_path(ctx);
    if p.exists() { read_text(&p, true).ok() } else { None }
}

/// Our table's lines (header excluded), up to the next table.
fn codex_block(text: &str) -> Option<Vec<&str>> {
    let mut lines = text.lines();
    lines.find(|l| l.trim() == codex_header())?;
    Some(lines.take_while(|l| !l.trim_start().starts_with('[')).collect())
}

/// Whether our Codex table already runs this `dg`.
fn codex_current(ctx: &Ctx, text: &str) -> bool {
    codex_block(text).is_some_and(|b| {
        b.iter().any(|l| l.trim_start().starts_with("command") && l.contains(&ctx.dg)) && !b.iter().any(|l| l.contains("mcp_server.py"))
    })
}

fn codex_install(ctx: &Ctx) -> Result<(), Error> {
    if let Some(text) = codex_text(ctx) {
        if text.contains(&codex_header()) {
            if codex_current(ctx, &text) {
                return Ok(());
            }
            codex_uninstall(ctx)?; // an older registration (the Python server): replace it
        }
    }
    let installed = |ctx: &Ctx| codex_text(ctx).is_some_and(|t| t.contains(&codex_header()));
    // Codex's own command keeps its config format right (the Codex app rewrites config.toml itself).
    if let Some(bin) = ctx.codex_binary() {
        if let Ok(o) = run(&bin, &["mcp", "add", NAME, "--", &ctx.dg, "mcp"], "", TIMEOUT) {
            if o.code == Some(0) && installed(ctx) {
                return Ok(());
            }
        }
    }
    let path = codex_path(ctx);
    fs::create_dir_all(&ctx.codex_home).map_err(io)?;
    let text = if path.exists() { read_text(&path, false)? } else { String::new() };
    if !text.is_empty() {
        fs::copy(&path, sibling(&path, ".desktopglass-backup")).map_err(io)?;
    }
    let block = format!(
        "\n{}\ncommand = {}\nargs = {}\n",
        codex_header(),
        dg_core::py::dumps(&Value::String(ctx.dg.clone()), true),
        crate::pyjson::dumps(&json!(server_args()), None)
    );
    fs::write(&path, format!("{}\n{}", text.trim_end_matches('\n'), block)).map_err(io)
}

fn codex_uninstall(ctx: &Ctx) -> Result<(), Error> {
    let Some(text) = codex_text(ctx) else { return Ok(()) };
    if !text.contains(&codex_header()) {
        return Ok(());
    }
    let mut out = Vec::new();
    let mut skipping = false;
    for line in text.lines() {
        if line.trim() == codex_header() {
            skipping = true;
            continue;
        }
        if skipping && line.trim_start().starts_with('[') {
            skipping = false;
        }
        if !skipping {
            out.push(line);
        }
    }
    fs::write(codex_path(ctx), format!("{}\n", out.join("\n").trim_end_matches('\n'))).map_err(io)
}

// ---------- extras: instructions, session hooks, autoshow ----------

#[derive(Clone, Debug)]
pub enum Extra {
    Guide(Guide),
    /// Claude Code's PostToolUse hook that shows HTML/SVG files the agent writes.
    Autoshow { path: PathBuf, command: String },
    /// `{"hooks": {"SessionStart": [{"hooks": [...]}]}}`: Claude Code, Codex (hooks.json), Gemini CLI (ms timeout).
    SessionGrouped { path: PathBuf, command: String, timeout: u64 },
    /// Antigravity CLI: ~/.gemini/config/hooks.json, `{"desktopglass": {"SessionStart": [...]}}`.
    SessionNamed { path: PathBuf, command: String },
    /// Cursor CLI: ~/.cursor/hooks.json, `{"version": 1, "hooks": {"sessionStart": [{"command": …}]}}`.
    SessionCursor { path: PathBuf, command: String },
    /// A file of DesktopGlass's own: Copilot CLI's hooks/desktopglass.json, OpenCode's plugin.
    OwnFile { path: PathBuf, content: String },
}

impl Extra {
    pub fn is_session(&self) -> bool {
        !matches!(self, Extra::Guide(_) | Extra::Autoshow { .. })
    }

    pub fn path(&self) -> &Path {
        match self {
            Extra::Guide(g) => &g.path,
            Extra::Autoshow { path, .. }
            | Extra::SessionGrouped { path, .. }
            | Extra::SessionNamed { path, .. }
            | Extra::SessionCursor { path, .. }
            | Extra::OwnFile { path, .. } => path,
        }
    }

    pub fn installed(&self) -> bool {
        let any_grouped = |path: &Path, event: &str, ours: fn(&str) -> bool| {
            read_json(path)
                .map(|d| groups(&d, event).iter().any(|g| group_hooks(g).iter().any(|h| ours(&command_of(h)))))
                .unwrap_or(false)
        };
        match self {
            Extra::Guide(g) => g.installed(),
            Extra::Autoshow { path, .. } => any_grouped(path, "PostToolUse", is_autoshow),
            Extra::SessionGrouped { path, .. } => any_grouped(path, "SessionStart", is_session),
            Extra::SessionNamed { path, .. } => read_json(path)
                .map(|d| {
                    let entry = truthy(d.get(NAME));
                    let list = entry.and_then(|e| truthy(e.get("SessionStart"))).and_then(Value::as_array);
                    list.is_some_and(|l| l.iter().any(|h| is_session(&command_of(h))))
                })
                .unwrap_or(false),
            Extra::SessionCursor { path, .. } => {
                read_json(path).map(|d| cursor_entries(&d).iter().any(|h| is_session(&command_of(h)))).unwrap_or(false)
            }
            Extra::OwnFile { path, .. } => {
                path.exists()
                    && fs::read(path)
                        .map(|b| {
                            let t = String::from_utf8_lossy(&b);
                            t.contains("session_context") || t.contains("hook session")
                        })
                        .unwrap_or(false)
            }
        }
    }

    pub fn install(&self) -> Result<(), Error> {
        match self {
            Extra::Guide(g) => g.install(),
            Extra::Autoshow { path, command } => {
                let mut data = read_json(path)?;
                remove_grouped(&mut data, "PostToolUse", is_autoshow);
                add_grouped(&mut data, "PostToolUse", json!({"matcher": "Write|Edit|MultiEdit", "hooks": [
                    {"type": "command", "command": command, "timeout": 10}]}))?;
                write_json(path, &data)
            }
            Extra::SessionGrouped { path, command, timeout } => {
                let mut data = read_json(path)?;
                remove_grouped(&mut data, "SessionStart", is_session);
                add_grouped(&mut data, "SessionStart", json!({"hooks": [
                    {"type": "command", "command": command, "timeout": timeout}]}))?;
                write_json(path, &data)
            }
            Extra::SessionNamed { path, command } => {
                let mut data = read_json(path)?;
                data.insert(NAME.into(), json!({"SessionStart": [{"type": "command", "command": command, "timeout": 5}]}));
                write_json(path, &data)
            }
            Extra::SessionCursor { path, command } => {
                let mut data = read_json(path)?;
                if !data.contains_key("version") {
                    data.insert("version".into(), json!(1));
                }
                let mut entries: Vec<Value> =
                    cursor_entries(&data).into_iter().filter(|h| !is_session(&command_of(h))).collect();
                entries.push(json!({"command": command}));
                let hooks = as_obj(obj_mut(&mut data, "hooks", json!({}))?, "hooks")?;
                hooks.insert("sessionStart".into(), Value::Array(entries));
                write_json(path, &data)
            }
            Extra::OwnFile { path, content } => {
                if let Some(d) = path.parent() {
                    fs::create_dir_all(d).map_err(io)?;
                }
                let same = fs::read(path).map(|b| String::from_utf8_lossy(&b) == content.as_str()).unwrap_or(false);
                if !same {
                    fs::write(path, content).map_err(io)?;
                }
                Ok(())
            }
        }
    }

    pub fn uninstall(&self) -> Result<(), Error> {
        let path = self.path();
        if !path.exists() {
            return Ok(());
        }
        match self {
            Extra::Guide(g) => g.uninstall(),
            Extra::Autoshow { .. } => {
                let mut data = read_json(path)?;
                if remove_grouped(&mut data, "PostToolUse", is_autoshow) {
                    write_json(path, &data)?;
                }
                Ok(())
            }
            Extra::SessionGrouped { .. } => {
                let mut data = read_json(path)?;
                if remove_grouped(&mut data, "SessionStart", is_session) {
                    if data.is_empty() && path.file_name().is_some_and(|n| n == "hooks.json") {
                        fs::remove_file(path).map_err(io)?; // Codex's hooks.json only ever held our hook
                    } else {
                        write_json(path, &data)?;
                    }
                }
                Ok(())
            }
            Extra::SessionNamed { .. } => {
                let mut data = read_json(path)?;
                if data.shift_remove(NAME).is_some() {
                    if data.is_empty() {
                        fs::remove_file(path).map_err(io)?;
                    } else {
                        write_json(path, &data)?;
                    }
                }
                Ok(())
            }
            Extra::SessionCursor { .. } => {
                let mut data = read_json(path)?;
                let entries = cursor_entries(&data);
                let kept: Vec<Value> = entries.iter().filter(|h| !is_session(&command_of(h))).cloned().collect();
                if kept.len() == entries.len() {
                    return Ok(());
                }
                let hooks = data.get_mut("hooks").and_then(Value::as_object_mut).expect("entries came from here");
                if kept.is_empty() {
                    hooks.shift_remove("sessionStart");
                    if hooks.is_empty() {
                        data.shift_remove("hooks");
                    }
                } else {
                    hooks.insert("sessionStart".into(), Value::Array(kept));
                }
                if data.keys().all(|k| k == "version") {
                    fs::remove_file(path).map_err(io) // the file only ever held our hook
                } else {
                    write_json(path, &data)
                }
            }
            Extra::OwnFile { .. } => {
                if self.installed() {
                    fs::remove_file(path).map_err(io)?;
                }
                Ok(())
            }
        }
    }
}

fn cursor_entries(data: &Obj) -> Vec<Value> {
    truthy(data.get("hooks"))
        .and_then(|h| truthy(h.get("sessionStart")))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

pub fn copilot_hook(ctx: &Ctx) -> Extra {
    let content = crate::pyjson::dumps(
        &json!({"version": 1, "hooks": {"sessionStart": [
            {"type": "command", "bash": session_command(ctx, "copilot"), "timeoutSec": 5}]}}),
        Some(2),
    ) + "\n";
    Extra::OwnFile { path: ctx.copilot_home.join("hooks").join("desktopglass.json"), content }
}

pub fn opencode_plugin(ctx: &Ctx) -> Extra {
    let env = |k: &str| (k == "TERM_PROGRAM").then(|| "DesktopGlass".to_string());
    let text = crate::hooks::context(env, &ctx.dg).unwrap_or_default();
    let js = format!(
        r#"// DesktopGlass (dg hook session): tells OpenCode when it was started from DesktopGlass's terminal.
// Written by DesktopGlass's MCP installer; removed when you uninstall. Prints nothing anywhere else.
export const DesktopGlass = async () => {{
  if (process.env.TERM_PROGRAM !== "DesktopGlass" || process.env.DESKTOPGLASS_SESSION_CONTEXT === "0") return {{}}
  return {{
    "experimental.chat.system.transform": async (_input, output) => {{
      output.system.push({})
    }},
  }}
}}
"#,
        crate::pyjson::dumps(&Value::String(text), None)
    );
    Extra::OwnFile { path: ctx.home.join(".config/opencode/plugins/desktopglass.js"), content: js }
}

/// The session-start hook for an agent CLI, in that CLI's format (None for apps that aren't agent CLIs).
pub fn session_hook_for(ctx: &Ctx, id: &str) -> Option<Extra> {
    let h = &ctx.home;
    let grouped = |path: PathBuf, timeout| Extra::SessionGrouped { path, command: session_command(ctx, "claude"), timeout };
    Some(match id {
        "claude-code" => grouped(h.join(".claude/settings.json"), 5),
        "codex" => grouped(ctx.codex_home.join("hooks.json"), 5),
        "gemini" => grouped(h.join(".gemini/settings.json"), 5000),
        "antigravity" => Extra::SessionNamed { path: h.join(".gemini/config/hooks.json"), command: session_command(ctx, "antigravity") },
        "copilot" => copilot_hook(ctx),
        "cursor" => Extra::SessionCursor { path: h.join(".cursor/hooks.json"), command: session_command(ctx, "cursor") },
        "opencode" => opencode_plugin(ctx),
        _ => return None,
    })
}

pub fn guide_for(ctx: &Ctx, id: &str) -> Option<Guide> {
    let h = &ctx.home;
    let path = match id {
        "claude-code" => h.join(".claude/CLAUDE.md"),
        "codex" => ctx.codex_home.join("AGENTS.md"),
        "gemini" | "antigravity" => h.join(".gemini/GEMINI.md"), // Antigravity reads Gemini's global rules file
        "copilot" => h.join(".copilot/copilot-instructions.md"),
        "opencode" => h.join(".config/opencode/AGENTS.md"),
        _ => return None,
    };
    Some(Guide::new(path))
}

pub fn claude_autoshow(ctx: &Ctx) -> Extra {
    Extra::Autoshow { path: ctx.home.join(".claude/settings.json"), command: autoshow_command(ctx) }
}

/// What else an app gets with the MCP server: its instructions block, the session-start hook (every agent CLI),
/// and for Claude Code the autoshow hook.
pub fn extras_for(ctx: &Ctx, id: &str) -> Vec<Extra> {
    let mut out: Vec<Extra> = guide_for(ctx, id).map(Extra::Guide).into_iter().chain(session_hook_for(ctx, id)).collect();
    if id == "claude-code" {
        out.push(claude_autoshow(ctx));
    }
    out
}
