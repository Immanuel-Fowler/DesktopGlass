//! Reads VERSION (and BUILD) from the repo's app.conf, the one place for the app's identity.
use std::{env, fs, path::PathBuf};

fn main() {
    // The repo's app.conf, found by walking up from this crate (dg/ and crates/* sit at different depths).
    let start = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let conf = start.ancestors().map(|d| d.join("app.conf")).find(|p| p.exists()).unwrap_or_else(|| start.join("app.conf"));
    println!("cargo:rerun-if-changed={}", conf.display());
    let text = fs::read_to_string(&conf).unwrap_or_default();
    let get = |key: &str| {
        text.lines()
            .filter_map(|l| l.trim().strip_prefix(key)?.strip_prefix('='))
            .map(|v| v.trim().trim_matches('"').to_string())
            .next()
    };
    let version = get("VERSION").unwrap_or_else(|| env::var("CARGO_PKG_VERSION").unwrap());
    println!("cargo:rustc-env=DG_VERSION={version}");
    println!("cargo:rustc-env=DG_BUILD={}", get("BUILD").unwrap_or_else(|| "0".into()));
    println!("cargo:rustc-env=DG_BUNDLE_ID={}", get("BUNDLE_ID").unwrap_or_else(|| "com.mannyfowler.desktopglass".into()));
}
