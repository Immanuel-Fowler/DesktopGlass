//! DesktopGlass core: Desktop rules, description.md, scan, manifest, index assembly, preferences and paths.
//! No async. Side effects are the file system, plus two clearly isolated spots: `local_servers` (lsof, localhost
//! HTTP) and `games::platform` (CoreFoundation aliases, plutil).

pub mod artifact_policy;
pub mod assemble;
pub mod description;
pub mod desktop_model;
pub mod games;
pub mod html;
pub mod local_servers;
pub mod manifest;
pub mod paths;
pub mod prefs;
pub mod py;
pub mod rules;
pub mod scan;
