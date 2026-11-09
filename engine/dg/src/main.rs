//! `dg`: the DesktopGlass engine CLI. Each subcommand takes its raw arguments and parses them itself, so every
//! module keeps the exact arguments, output and exit codes of the script it replaces.
//!
//! | subcommand                     | module          | replaces                                  |
//! |--------------------------------|-----------------|-------------------------------------------|
//! | `inventory`                    | inventory.rs    | inventory.py                              |
//! | `reindex` · `ask` · `login`    | reindex.rs      | reindex.sh, common.sh, local_index.py, ask.sh, login.sh |
//! | `mcp`                          | mcp.rs          | mcp_server.py                             |
//! | `mcp-install` · `hook` · `view`| install.rs      | mcp_install.py, autoshow.py, session_context.py, dgview |
//! | `engine` · `schema`            | engine.rs       | logic inside the Swift app (Phase 5)      |

use clap::{Arg, ArgAction, Command};
use std::process::ExitCode;

mod engine;
mod install;
mod inventory;
mod mcp;
mod reindex;

/// A subcommand whose arguments are passed through untouched to its module.
fn passthrough(name: &'static str, about: &'static str) -> Command {
    Command::new(name).about(about).disable_help_flag(true).arg(
        Arg::new("args").num_args(0..).trailing_var_arg(true).allow_hyphen_values(true).action(ArgAction::Append),
    )
}

fn cli() -> Command {
    Command::new("dg")
        .version(env!("DG_VERSION"))
        .about("DesktopGlass engine: Desktop inventory, indexing and integrations")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(passthrough("inventory", "Compact Desktop inventory: DESKTOP scan [--all] | check | commit | status"))
        .subcommand(passthrough("reindex", "Re-index the Desktop with the backend chosen in Settings"))
        .subcommand(passthrough("ask", "Interactive indexing session in a terminal"))
        .subcommand(passthrough("login", "Sign in to an agent CLI"))
        .subcommand(passthrough("mcp", "MCP server over stdio"))
        .subcommand(passthrough("mcp-install", "Register the MCP server with AI apps: status | install | uninstall | refresh | test"))
        .subcommand(passthrough("hook", "Agent CLI hooks: autoshow | session --format <cli>"))
        .subcommand(passthrough("view", "Show a file or URL in the DesktopGlass viewer"))
        .subcommand(passthrough("engine", "Long-running engine for the app (JSON-RPC over stdio)"))
        .subcommand(passthrough("schema", "Print the engine protocol's JSON Schema"))
}

fn main() -> ExitCode {
    let m = cli().get_matches();
    let (name, sub) = match m.subcommand() {
        Some(x) => x,
        None => return ExitCode::from(2),
    };
    let args: Vec<String> = sub.get_many::<String>("args").map(|v| v.cloned().collect()).unwrap_or_default();
    match name {
        "inventory" => inventory::run(&args),
        "reindex" => reindex::reindex(&args),
        "ask" => reindex::ask(&args),
        "login" => reindex::login(&args),
        "mcp" => mcp::run(&args),
        "mcp-install" => install::mcp_install(&args),
        "hook" => install::hook(&args),
        "view" => install::view(&args),
        "engine" => engine::run(&args),
        "schema" => engine::schema(&args),
        _ => ExitCode::from(2),
    }
}
