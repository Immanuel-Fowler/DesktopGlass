//! The artifact viewer's rules (dg-core `artifact_policy`, ported from ArtifactPolicy.swift).
//!
//! WebKit asks its navigation/popup questions synchronously, so the UI can't wait for a round trip each time.
//! Instead it fetches `policy/tables` once (at engine start-up and after every restart) and evaluates them with a
//! tiny matcher whose every branch is described here: the tables are the rules, and they come from Rust. A test
//! below runs that same matcher (`matcher` in this file, mirrored line for line by `ArtifactPolicy.swift`) against
//! dg-core's predicates over a corpus, so the tables can't drift from the rules.
//!
//! - `policy/tables {}` → the tables (see [`tables`]).
//! - `policy/check {url}` → `{allowed, signIn, webPage, openOnRequest, handOff}` (dg-core's own predicates).
//! - `policy/resolve {raw, address?}` → `{kind: "url", url} | {kind: "file", path} | null`: `ArtifactPolicy.resolve`
//!   (or `.address` for address-bar text), against the Desktop.

use super::{bool_param, str_param};
use crate::Engine;
use dg_core::artifact_policy as ap;
use serde_json::{json, Value};
use std::sync::Arc;

pub fn handle(_engine: &Arc<Engine>, method: &str, p: &Value) -> Option<Result<Value, String>> {
    Some(match method {
        "policy/tables" => Ok(tables()),
        "policy/check" => str_param(p, "url").map(ap::check_json).ok_or_else(|| "url is required".into()),
        "policy/resolve" => {
            let raw = str_param(p, "raw").unwrap_or("");
            let desktop = dg_platform::desktop();
            let t = if bool_param(p, "address", false) { ap::address(raw, &desktop) } else { ap::resolve(raw, &desktop) };
            Ok(t.map(|t| t.to_json()).unwrap_or(Value::Null))
        }
        _ => return None,
    })
}

/// Hosts that always count as local (http/https).
const LOCAL_HOSTS: &[&str] = &["localhost", "127.0.0.1", "0.0.0.0", "::1", "[::1]"];
/// Host suffixes that count as local.
const LOCAL_SUFFIXES: &[&str] = &[".localhost"];
/// Hosts whose artifacts live only under some paths (the rest of the site isn't an artifact).
const PATH_RULES: &[(&str, &[&str])] = &[
    ("claude.ai", &["/artifact", "/public", "/share"]),
    ("chatgpt.com", &["/canvas", "/share"]),
    ("chat.openai.com", &["/canvas", "/share"]),
];
/// Non-web schemes the viewer may show (compared as written, like `url.scheme == "about"`).
const INERT_SCHEMES: &[&str] = &["about", "blob", "data"];
/// Schemes (besides http/https pages with a host) that may be handed to another app.
const HAND_OFF_SCHEMES: &[&str] = &["mailto"];

/// The rules as data. Matcher (what `ArtifactPolicy.swift` does with them):
/// - `isAllowed(u)`: file URL → its lowercased extension is in `fileExtensions`, or it's a directory. http/https with
///   a host (lowercased) → `localHosts` or a `localSuffixes` suffix → allowed; host in `pathRules` → the path starts
///   with one of its prefixes; else the host is, or is a subdomain of, one of `artifactHosts`. Otherwise → the
///   scheme (as written) is in `inertSchemes`.
/// - `isSignIn(u)`: https with a host that is, or is a subdomain of, one of `signInHosts`.
/// - `isWebPage(u)`: http/https with a non-empty host.
/// - `canOpenOnRequest(u)`: `isAllowed || isWebPage`.
/// - `canHandOff(u)`: scheme in `handOffSchemes`, or `isWebPage`.
pub fn tables() -> Value {
    json!({
        "version": 1,
        "fileExtensions": ap::FILE_EXTENSIONS,
        "artifactHosts": ap::HOSTS,
        "signInHosts": ap::SIGN_IN_HOSTS,
        "localHosts": LOCAL_HOSTS,
        "localSuffixes": LOCAL_SUFFIXES,
        "pathRules": PATH_RULES.iter().map(|(h, p)| (h.to_string(), json!(p))).collect::<serde_json::Map<_, _>>(),
        "inertSchemes": INERT_SCHEMES,
        "handOffSchemes": HAND_OFF_SCHEMES,
        "safariUserAgent": ap::SAFARI_USER_AGENT,
    })
}

#[cfg(test)]
mod tests {
    //! The UI's matcher, reproduced over the tables, must agree with dg-core everywhere.
    use super::*;

    struct Matcher(Value);

    impl Matcher {
        fn list(&self, k: &str) -> Vec<String> {
            self.0[k].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect()
        }
        fn host_in(host: &str, list: &[String]) -> bool {
            list.iter().any(|h| host == h || host.ends_with(&format!(".{h}")))
        }
        fn allowed(&self, url: &str) -> bool {
            let u = ap::parse(url);
            if u.scheme.as_deref().map(|s| s.eq_ignore_ascii_case("file")) == Some(true) {
                let last = u.path.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string();
                let ext = match last.rsplit_once('.') {
                    Some((stem, e)) if !stem.is_empty() => e.to_lowercase(),
                    _ => String::new(),
                };
                return self.list("fileExtensions").contains(&ext) || u.has_directory_path;
            }
            let scheme = u.scheme.clone().map(|s| s.to_lowercase());
            match (scheme.as_deref(), u.host.clone().map(|h| h.to_lowercase())) {
                (Some("http") | Some("https"), Some(host)) => {
                    if self.list("localHosts").contains(&host)
                        || self.list("localSuffixes").iter().any(|s| host.ends_with(s.as_str()))
                    {
                        return true;
                    }
                    if let Some(prefixes) = self.0["pathRules"].get(&host) {
                        return prefixes.as_array().unwrap().iter().any(|p| u.path.starts_with(p.as_str().unwrap()));
                    }
                    Self::host_in(&host, &self.list("artifactHosts"))
                }
                _ => u.scheme.map(|s| self.list("inertSchemes").contains(&s)).unwrap_or(false),
            }
        }
        fn sign_in(&self, url: &str) -> bool {
            let u = ap::parse(url);
            u.scheme.map(|s| s.to_lowercase()).as_deref() == Some("https")
                && u.host.map(|h| Self::host_in(&h.to_lowercase(), &self.list("signInHosts"))).unwrap_or(false)
        }
        fn web_page(&self, url: &str) -> bool {
            let u = ap::parse(url);
            matches!(u.scheme.map(|s| s.to_lowercase()).as_deref(), Some("http") | Some("https"))
                && u.host.map(|h| !h.is_empty()).unwrap_or(false)
        }
        fn hand_off(&self, url: &str) -> bool {
            let u = ap::parse(url);
            u.scheme.map(|s| self.list("handOffSchemes").contains(&s.to_lowercase())).unwrap_or(false) || self.web_page(url)
        }
    }

    const CORPUS: &[&str] = &[
        "http://localhost:5173/", "http://127.0.0.1:3000/x", "http://app.localhost/", "https://myapp.vercel.app/",
        "https://a.b.github.io/page", "https://claude.ai/artifact/abc", "https://claude.ai/public/artifacts/x",
        "https://chatgpt.com/canvas/shared/1", "https://claude.site/x", "about:blank", "/tmp/a.html", "/tmp/a.SVG",
        "/tmp/report.pdf", "/tmp/notes.md", "https://claude.ai/chat/123", "https://claude.ai/settings",
        "https://evilclaude.ai/artifact/x", "https://claude.ai.evil.com/artifact/x", "https://vercel.app.evil.com/",
        "https://notvercel.app/", "https://chatgpt.com/c/123", "https://example.com/", "ftp://localhost/x",
        "javascript:alert(1)", "ssh://localhost", "x-apple.systempreferences:com.apple.preference.security",
        "/tmp/dg-none/Calculator.app", "/tmp/run.command", "/tmp/install.sh", "/tmp/x.exe", "/etc/passwd",
        "https://claude.ai/login?returnTo=/artifact/x", "https://accounts.google.com/o/oauth2/auth",
        "https://auth.openai.com/authorize", "https://github.com/login/oauth/authorize", "http://claude.ai/login",
        "https://evil-claude.ai/login", "https://claude.ai.evil.com/login", "https://example.com/login",
        "file:///tmp/login.html", "http://news.ycombinator.com/item?id=1", "file:///tmp/run.command",
        "file:///Applications/Calculator.app", "https:///no-host", "vscode://file/x", "mailto:someone@example.com",
        "smb://server/share", "tel:123", "https://chat.openai.com/share/x", "https://chat.openai.com/c/x",
        "HTTPS://CLAUDE.AI/artifact/x", "http://[::1]:8080/", "http://0.0.0.0:9000", "https://Sub.HF.space/x",
        "data:text/html,hi", "blob:https://x/y", "ABOUT:blank", "https://user@localhost:99/x", "file:///tmp/dir/",
        "https://login.live.com/", "https://x.clerk.accounts.dev/sign-in", "MAILTO:x@y.z",
    ];

    #[test]
    fn swift_matcher_over_the_tables_agrees_with_dg_core() {
        let m = Matcher(tables());
        for u in CORPUS {
            assert_eq!(m.allowed(u), ap::is_allowed(u), "allowed {u}");
            assert_eq!(m.sign_in(u), ap::is_sign_in(u), "signIn {u}");
            assert_eq!(m.web_page(u), ap::is_web_page(u), "webPage {u}");
            assert_eq!(m.allowed(u) || m.web_page(u), ap::can_open_on_request(u), "openOnRequest {u}");
            assert_eq!(m.hand_off(u), ap::can_hand_off(u), "handOff {u}");
        }
    }

    #[test]
    fn resolve_and_check_methods() {
        let dir = super::super::test_support::temp_dir("policy");
        let e = super::super::test_support::engine(&dir);
        let file = dir.join("chart.html");
        std::fs::write(&file, "<p>x</p>").unwrap();
        let r = handle(&e, "policy/resolve", &json!({"raw": file})).unwrap().unwrap();
        assert_eq!(r, json!({"kind": "file", "path": file}));
        let r = handle(&e, "policy/resolve", &json!({"raw": "localhost:5173", "address": true})).unwrap().unwrap();
        assert_eq!(r, json!({"kind": "url", "url": "http://localhost:5173"}));
        let r = handle(&e, "policy/resolve", &json!({"raw": "/no/such/file.html"})).unwrap().unwrap();
        assert_eq!(r, Value::Null);
        let r = handle(&e, "policy/check", &json!({"url": "https://claude.ai/artifact/x"})).unwrap().unwrap();
        assert_eq!(r["allowed"], true);
        assert_eq!(r["handOff"], true);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
