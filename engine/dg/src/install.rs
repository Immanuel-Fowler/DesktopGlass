//! `dg mcp-install` · `dg hook` · `dg view` (Phase 4). Thin wrappers over the dg-install crate.
use dg_install::{cli, hooks, Ctx};
use std::process::ExitCode;

/// `dg mcp-install status | install | uninstall | refresh | test`: prints the JSON the app reads. Always exits 0,
/// like mcp_install.py; per-app failures are in `errors`.
pub fn mcp_install(args: &[String]) -> ExitCode {
    println!("{}", cli::mcp_install(&Ctx::from_env(), args));
    ExitCode::SUCCESS
}

/// `dg hook session --format claude|copilot|cursor|antigravity` · `dg hook autoshow`. Never fails a session.
pub fn hook(args: &[String]) -> ExitCode {
    if let Some(line) = cli::hook(args) {
        println!("{line}");
    }
    ExitCode::SUCCESS
}

/// `dg view [file | folder | URL] [title]`: shows it in the DesktopGlass viewer (no argument: the server picker).
pub fn view(args: &[String]) -> ExitCode {
    if args.first().is_some_and(|a| a == "-h" || a == "--help") {
        println!("usage: dg view [file | folder | URL] [title]");
        return ExitCode::SUCCESS;
    }
    let link = hooks::view_link(args);
    if std::env::var_os("DESKTOPGLASS_VIEW_PRINT").is_some() {
        println!("{link}"); // for tests and scripts: print the link instead of opening it
        return ExitCode::SUCCESS;
    }
    if dg_platform::open_url(&link, false) { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}
