//! Installer and hooks (Phase 4): replaces mcp_install.py, autoshow.py, session_context.py and dgview.
//!
//! - `clients`: each AI app's MCP registration (JSON `mcpServers`/`servers`/`mcp`, Codex TOML, `claude mcp add`) and
//!   the extras agent CLIs get with it (instructions block, session-start hook, autoshow hook). Registrations point
//!   at `~/.local/bin/dg mcp`; Python-era registrations are recognised and migrated.
//! - `guide`: the marked block in CLAUDE.md / AGENTS.md / GEMINI.md / ….
//! - `hooks`: what `dg hook session`, `dg hook autoshow` and `dg view` do.
//! - `ctx`: paths (per OS) and CLI lookup, injectable for tests.
//! - `pyjson`: JSON files written byte-identically to the Python installer.

pub mod cli;
pub mod clients;
pub mod ctx;
pub mod guide;
pub mod hooks;
pub mod pyjson;

pub use ctx::Ctx;
/// Re-exported so the `dg` binary's tests can read the JSON it prints.
pub use serde_json;

/// Why a client couldn't be changed. `Parse`: its config file isn't plain JSON (or UTF-8), so it was left alone.
#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    Parse(String),
    Other(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Parse(m) | Error::Other(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for Error {}
