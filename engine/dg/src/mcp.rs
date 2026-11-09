//! `dg mcp` (Phase 3). Thin wrapper over the dg-mcp crate: the MCP server over stdio (replaces mcp_server.py).
use std::process::ExitCode;

pub fn run(_args: &[String]) -> ExitCode {
    dg_mcp::run_stdio();
    ExitCode::SUCCESS
}
