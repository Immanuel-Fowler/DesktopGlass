//! The three subcommands: `dg mcp-install`, `dg hook`, `dg view`.

use crate::clients::{clients, extras_for, guide_for, session_hook_for, claude_autoshow, server_args, Client};
use crate::ctx::{run, Ctx};
use crate::pyjson::dumps;
use crate::Error;
use serde_json::{json, Map, Value};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn short(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn describe(c: &Client, e: &Error) -> String {
    match e {
        Error::Parse(_) => format!("{}: skipped (its config file isn't plain JSON)", c.name),
        Error::Other(m) => format!("{}: {}", c.name, short(m, 160)),
    }
}

pub fn status_of(ctx: &Ctx, c: &Client) -> Value {
    let mut row = Map::new();
    let detected = c.detected(ctx);
    row.insert("id".into(), json!(c.id));
    row.insert("name".into(), json!(c.name));
    row.insert("detected".into(), json!(detected));
    row.insert("installed".into(), json!(detected && c.installed(ctx)));
    if let Some(g) = guide_for(ctx, c.id) {
        row.insert("instructions".into(), json!(detected && g.installed()));
    }
    if let Some(s) = session_hook_for(ctx, c.id) {
        row.insert("sessionHook".into(), json!(detected && s.installed()));
    }
    if c.id == "claude-code" {
        row.insert("autoshow".into(), json!(detected && claude_autoshow(ctx).installed()));
    }
    Value::Object(row)
}

fn try_client(ctx: &Ctx, c: &Client, install: bool) -> Result<(), Error> {
    if install { c.install(ctx)? } else { c.uninstall(ctx)? }
    for x in extras_for(ctx, c.id) {
        if install { x.install()? } else { x.uninstall()? }
    }
    Ok(())
}

/// Re-registers where the server is already registered (now pointing at `dg`), brings installed extras up to date,
/// and adds the session-start hook wherever the server is registered.
fn try_refresh(ctx: &Ctx, c: &Client, done: &mut Vec<String>) -> Result<(), Error> {
    let registered = c.detected(ctx) && c.installed(ctx);
    if registered {
        c.install(ctx)?; // replaces an older registration in place (Python → dg)
        done.push(c.name.to_string());
    }
    for x in extras_for(ctx, c.id) {
        if x.installed() || (registered && x.is_session()) {
            x.install()?;
        }
    }
    Ok(())
}

/// Calls the server the way an AI app would: initialize, tools/list, index_status.
pub fn test_server(ctx: &Ctx) -> Value {
    let started = Instant::now();
    let msgs = [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18",
               "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}}}),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "index_status", "arguments": {}}}),
    ];
    let input: String = msgs.iter().map(|m| dumps(m, None) + "\n").collect();
    let mut program = PathBuf::from(&ctx.dg);
    if !program.exists() {
        if let Ok(me) = std::env::current_exe() {
            program = me; // not linked into ~/.local/bin yet: test this dg
        }
    }
    let out = run(&program, &["mcp"], &input, Duration::from_secs(30));
    let (stdout, stderr) = match out {
        Ok(o) => (o.stdout, o.stderr),
        Err(e) => (String::new(), e.to_string()),
    };
    let mut replies = std::collections::HashMap::new();
    for line in stdout.lines() {
        if let Ok(m) = serde_json::from_str::<Value>(line) {
            replies.insert(m.get("id").map(Value::to_string).unwrap_or_default(), m);
        }
    }
    let reply = |id: &str| replies.get(id).cloned().unwrap_or(Value::Null);
    let tools: Vec<Value> = reply("2")["result"]["tools"]
        .as_array()
        .map(|a| a.iter().filter_map(|t| t.get("name").cloned()).collect())
        .unwrap_or_default();
    let status = reply("3")["result"]["content"][0]["text"].as_str().unwrap_or("").lines().next().unwrap_or("").to_string();
    let init = &reply("1")["result"];
    let ok = dg_core::py::truthy(init) && !tools.is_empty();
    let error = if ok {
        String::new()
    } else if stderr.trim().is_empty() {
        "no reply".into()
    } else {
        short(stderr.trim(), 300)
    };
    json!({"ok": ok, "tools": tools, "status": status, "ms": started.elapsed().as_millis() as u64, "error": error})
}

/// `dg mcp-install status | install | uninstall | refresh | test`: the JSON the DesktopGlass app reads.
pub fn mcp_install(ctx: &Ctx, args: &[String]) -> String {
    let cmd = args.first().map(String::as_str).unwrap_or("status");
    let mut result = Map::new();
    result.insert("server".into(), json!({"command": ctx.dg, "args": server_args()}));
    let all = clients(ctx);
    match cmd {
        "refresh" => {
            let (mut done, mut errors) = (Vec::new(), Vec::new());
            for c in &all {
                if let Err(e) = try_refresh(ctx, c, &mut done) {
                    errors.push(describe(c, &e));
                }
            }
            result.insert("done".into(), json!(done));
            result.insert("errors".into(), json!(errors));
        }
        "install" | "uninstall" => {
            let (mut done, mut errors) = (Vec::new(), Vec::new());
            for c in all.iter().filter(|c| c.detected(ctx)) {
                match try_client(ctx, c, cmd == "install") {
                    Ok(()) => done.push(c.name),
                    Err(e) => errors.push(describe(c, &e)),
                }
            }
            result.insert("done".into(), json!(done));
            result.insert("errors".into(), json!(errors));
        }
        "test" => {
            result.insert("test".into(), test_server(ctx));
        }
        _ => {}
    }
    result.insert("clients".into(), Value::Array(all.iter().map(|c| status_of(ctx, c)).collect()));
    dumps(&Value::Object(result), None)
}

/// `dg hook session --format F` (prints the session context) or `dg hook autoshow` (reads the PostToolUse event on
/// stdin and shows the file). Both act only inside DesktopGlass's terminal and never fail.
pub fn hook(args: &[String]) -> Option<String> {
    let env = |k: &str| std::env::var(k).ok();
    let dg = crate::ctx::dg_command(&dg_platform::home());
    match args.first().map(String::as_str) {
        Some("session") => crate::hooks::session_output(&args[1..], env, &dg),
        Some("autoshow") => {
            if !crate::hooks::autoshow_enabled(env) {
                return None;
            }
            let mut input = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut input).ok()?;
            let event: Value = serde_json::from_str(&input).ok()?;
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
            if let Some(path) = crate::hooks::target(&event, &cwd) {
                let link = crate::hooks::open_link(&path.to_string_lossy(), None);
                dg_platform::open_url(&link, true);
            }
            None
        }
        _ => None,
    }
}
