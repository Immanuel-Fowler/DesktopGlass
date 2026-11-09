//! HTML source ops, artifact policy, games and local servers: the engine side of the logic that used to live in
//! `HTMLSource.swift`/`HTMLOps.swift`, `ArtifactPolicy.swift`, `GameFinder.swift`/`GameCatalog.swift`/the scan in
//! `GameLibrary.swift`, and `LocalServers.swift`. The rules themselves are in `dg-core`; this module turns them into
//! UI-link methods (`html/*`, `policy/*`, `games/*`, `servers/*`) and answers the control methods that don't need the
//! UI (`servers.list`, `games.list`).
//!
//! - [`html`]: the visual HTML editor and the `html.*` control calls. Unsaved editor buffers live in the UI, so the
//!   UI sends their text along (`buffers`) and applies the returned edits to its own documents (that keeps ⌘Z).
//! - [`policy`]: `policy/tables` (the allow-lists the UI's WebView decisions evaluate synchronously),
//!   `policy/check`, `policy/resolve`.
//! - [`games`]: Games-folder scan, Find Games, shortcuts, play history pushed by the UI, `games.list`.
//! - [`servers`]: local web servers.

pub mod games;
pub mod html;
pub mod policy;
pub mod servers;

use crate::Engine;
use serde_json::Value;
use std::sync::Arc;

pub fn handle(engine: &Arc<Engine>, method: &str, params: &Value) -> Option<Result<Value, String>> {
    html::handle(engine, method, params)
        .or_else(|| policy::handle(engine, method, params))
        .or_else(|| games::handle(engine, method, params))
        .or_else(|| servers::handle(engine, method, params))
}

pub fn start(_engine: &Arc<Engine>) {}

// ---- small param helpers shared by the submodules ----

pub(crate) fn str_param<'a>(p: &'a Value, key: &str) -> Option<&'a str> {
    p.get(key).and_then(|v| v.as_str())
}

pub(crate) fn bool_param(p: &Value, key: &str, default: bool) -> bool {
    p.get(key).and_then(|v| v.as_bool()).unwrap_or(default)
}

pub(crate) fn required<'a>(p: &'a Value, key: &str) -> Result<&'a str, String> {
    str_param(p, key).ok_or_else(|| format!("{key} is required"))
}

#[cfg(test)]
pub(crate) mod test_support {
    use crate::{settings::Settings, Engine};
    use std::path::PathBuf;
    use std::sync::Arc;

    /// A unique, empty temp directory (removed by the caller when it cares).
    pub fn temp_dir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!(
            "dg-tools-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// An engine whose settings live in a temp dir and whose link output goes nowhere.
    pub fn engine(dir: &std::path::Path) -> Arc<Engine> {
        Engine::with_settings(Box::new(std::io::sink()), Settings::open_at(dir.join("settings.json")))
    }
}
