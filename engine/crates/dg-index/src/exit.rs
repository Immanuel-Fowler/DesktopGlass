//! Exit codes shared by `dg reindex` and `Reindexer.swift` (unchanged from reindex.sh / common.sh):
//! 0 indexed · 10 nothing changed · 75 already running · 76 usage/rate limit · 77 not signed in / missing token ·
//! 78 local model or server not ready · 79 unattended runs not allowed · 127 CLI not installed · other = failed.

pub const OK: i32 = 0;
/// A failure that isn't sign-in or a limit (also local_index.py's "every folder failed").
pub const FAILED: i32 = 1;
pub const USAGE: i32 = 2;
pub const UP_TO_DATE: i32 = 10;
pub const BUSY: i32 = 75;
pub const LIMITED: i32 = 76;
pub const SIGNED_OUT: i32 = 77;
pub const NOT_READY: i32 = 78;
pub const NEEDS_CONSENT: i32 = 79;
pub const NOT_INSTALLED: i32 = 127;

/// What preflight_auth reports when a CLI has no definite answer (looks signed out, unverified).
pub const PROBABLY_SIGNED_OUT: i32 = 2;
