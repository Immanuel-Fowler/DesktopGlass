//! Port of `Artifacts/ArtifactPolicy.swift`: what the artifact viewer is allowed to show, what may be handed to other
//! apps, and how paths/URLs from AIs and the address bar are resolved. Only the rules move here; applying them in the
//! WebView (navigation policy, user agent) stays in Swift.
//!
//! Inputs are strings. A string starting with `/` is treated as `URL(fileURLWithPath:)` (a local path, which counts
//! as a directory when it ends in `/` or is one on disk); anything else is parsed like `URL(string:)` with a small
//! RFC 3986 splitter (scheme, host, percent-decoded path), which is all the rules look at.

use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// `ArtifactPolicy.fileExtensions`.
pub const FILE_EXTENSIONS: &[&str] = &["html", "htm", "svg", "pdf", "png", "jpg", "jpeg", "gif", "webp", "md", "txt", "json"];

/// `ArtifactPolicy.hosts`: hosts that publish AI artifacts, canvases or instant previews.
pub const HOSTS: &[&str] = &[
    "claude.ai", "claude.site", "claudeusercontent.com", // Claude artifacts (claude.ai checked by path below)
    "chatgpt.com", "chat.openai.com", // ChatGPT canvas / shared links
    "gemini.google.com", "aistudio.google.com", "g.co", // Gemini canvas / AI Studio apps
    "v0.app", "v0.dev", "vusercontent.net", // v0
    "lovable.app", "lovable.dev", "bolt.new", "bolt.host", // Lovable, Bolt
    "stackblitz.com", "webcontainer.io", "codesandbox.io", "csb.app", "replit.app", "repl.co", "vercel.app",
    "netlify.app", "pages.dev", "github.io", "huggingface.co", "hf.space",
];

/// `ArtifactPolicy.signInHosts`: sign-in pages an artifact may send you to (only inside a tab already showing one).
pub const SIGN_IN_HOSTS: &[&str] = &[
    "claude.ai", "anthropic.com", "auth.openai.com", "openai.com", "chatgpt.com", "accounts.google.com",
    "appleid.apple.com", "github.com", "login.microsoftonline.com", "login.live.com", "vercel.com", "netlify.com",
    "auth0.com", "clerk.accounts.dev", "accounts.dev", "huggingface.co", "replit.com", "lovable.dev", "stackblitz.com",
    "codesandbox.io", "bolt.new",
];

/// `ArtifactPolicy.safariUserAgent`.
pub const SAFARI_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15";

/// The parts of a URL the policy looks at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UrlParts {
    /// As written (not lowercased), like `URL.scheme`.
    pub scheme: Option<String>,
    /// Without brackets for IPv6, not lowercased; None when there's no authority or it's empty.
    pub host: Option<String>,
    /// Percent-decoded, like `URL.path`.
    pub path: String,
    /// `URL.hasDirectoryPath`.
    pub has_directory_path: bool,
}

impl UrlParts {
    fn is_file(&self) -> bool {
        self.scheme.as_deref().map(|s| s.eq_ignore_ascii_case("file")).unwrap_or(false)
    }
    fn scheme_lower(&self) -> Option<String> {
        self.scheme.as_ref().map(|s| s.to_lowercase())
    }
    fn host_lower(&self) -> Option<String> {
        self.host.as_ref().map(|h| h.to_lowercase())
    }
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = |c: u8| (c as char).to_digit(16);
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn scheme_of(s: &str) -> Option<(&str, &str)> {
    let (scheme, rest) = s.split_once(':')?;
    let mut chars = scheme.chars();
    let first = chars.next()?;
    if !first.is_ascii_alphabetic() || !chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')) {
        return None;
    }
    Some((scheme, rest))
}

/// Splits `s` like `URL(string:)` (or `URL(fileURLWithPath:)` for a string starting with `/`).
pub fn parse(s: &str) -> UrlParts {
    if s.starts_with('/') {
        return UrlParts {
            scheme: Some("file".into()),
            host: None,
            path: s.to_string(),
            has_directory_path: s.ends_with('/') || Path::new(s).is_dir(),
        };
    }
    let (scheme, rest) = match scheme_of(s) {
        Some((sc, r)) => (Some(sc.to_string()), r),
        None => (None, s),
    };
    let rest = rest.split(['?', '#']).next().unwrap_or("");
    let (host, path) = if let Some(after) = rest.strip_prefix("//") {
        let end = after.find('/').unwrap_or(after.len());
        let authority = &after[..end];
        let hostport = authority.rsplit_once('@').map(|(_, h)| h).unwrap_or(authority);
        let host = if let Some(v6) = hostport.strip_prefix('[') {
            v6.split(']').next().unwrap_or("").to_string()
        } else {
            match hostport.rsplit_once(':') {
                Some((h, port)) if port.bytes().all(|b| b.is_ascii_digit()) => h.to_string(),
                _ => hostport.to_string(),
            }
        };
        (if host.is_empty() { None } else { Some(host) }, &after[end..])
    } else {
        (None, rest)
    };
    UrlParts { scheme, host, path: percent_decode(path), has_directory_path: path.ends_with('/') }
}

fn path_extension(path: &str) -> String {
    let last = path.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    match last.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => ext.to_string(),
        _ => String::new(),
    }
}

fn host_matches(host: &str, list: &[&str]) -> bool {
    list.iter().any(|h| host == *h || host.ends_with(&format!(".{h}")))
}

/// `ArtifactPolicy.isAllowed(_:)`: what the viewer shows on its own.
pub fn is_allowed(url: &str) -> bool {
    let u = parse(url);
    if u.is_file() {
        return FILE_EXTENSIONS.contains(&path_extension(&u.path).to_lowercase().as_str()) || u.has_directory_path;
    }
    let scheme = u.scheme_lower();
    let host = u.host_lower();
    let (Some(_), Some(host)) = (scheme.filter(|s| s == "http" || s == "https"), host) else {
        return matches!(u.scheme.as_deref(), Some("about") | Some("blob") | Some("data"));
    };
    if ["localhost", "127.0.0.1", "0.0.0.0", "::1", "[::1]"].contains(&host.as_str()) || host.ends_with(".localhost") {
        return true;
    }
    let p = &u.path;
    if host == "claude.ai" {
        return p.starts_with("/artifact") || p.starts_with("/public") || p.starts_with("/share");
    }
    if host == "chatgpt.com" || host == "chat.openai.com" {
        return p.starts_with("/canvas") || p.starts_with("/share");
    }
    host_matches(&host, HOSTS)
}

/// `ArtifactPolicy.isSignIn(_:)`: https sign-in pages allowed as navigations inside an artifact tab.
pub fn is_sign_in(url: &str) -> bool {
    let u = parse(url);
    match (u.scheme_lower().as_deref(), u.host_lower()) {
        (Some("https"), Some(host)) => host_matches(&host, SIGN_IN_HOSTS),
        _ => false,
    }
}

/// `ArtifactPolicy.isWebPage(_:)`: http/https with a host.
pub fn is_web_page(url: &str) -> bool {
    let u = parse(url);
    matches!(u.scheme_lower().as_deref(), Some("http") | Some("https")) && u.host.map(|h| !h.is_empty()).unwrap_or(false)
}

/// `ArtifactPolicy.canOpenOnRequest(_:)`: what opens when asked for by name (dgview, show_artifact, address bar).
pub fn can_open_on_request(url: &str) -> bool {
    is_allowed(url) || is_web_page(url)
}

/// `ArtifactPolicy.canHandOff(_:)`: what may be passed to another app: web pages and mail links only.
pub fn can_hand_off(url: &str) -> bool {
    let u = parse(url);
    match u.scheme_lower().as_deref() {
        Some("mailto") => true,
        Some("http") | Some("https") => u.host.map(|h| !h.is_empty()).unwrap_or(false),
        _ => false,
    }
}

/// What `resolve` / `address` produce: a URL string or a local file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Url(String),
    File(PathBuf),
}

impl Target {
    /// The URL string, or the path for a file (pass either back to the predicates above).
    pub fn as_str(&self) -> String {
        match self {
            Target::Url(u) => u.clone(),
            Target::File(p) => p.to_string_lossy().into_owned(),
        }
    }
    pub fn to_json(&self) -> Value {
        match self {
            Target::Url(u) => json!({"kind": "url", "url": u}),
            Target::File(p) => json!({"kind": "file", "path": p.to_string_lossy()}),
        }
    }
}

fn trim_ws_nl(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace())
}

/// `ArtifactPolicy.address(_:)`: address-bar text → URL ("localhost:5173" → http://…, "example.com/x" → https://…,
/// paths as `resolve`).
pub fn address(raw: &str, desktop: &Path) -> Option<Target> {
    let s = trim_ws_nl(raw);
    if s.is_empty() {
        return None;
    }
    let lower = s.to_lowercase();
    if ["localhost", "127.0.0.1", "0.0.0.0", "[::1]"]
        .iter()
        .any(|h| lower == *h || lower.starts_with(&format!("{h}:")) || lower.starts_with(&format!("{h}/")))
    {
        return Some(Target::Url(format!("http://{s}")));
    }
    resolve(s, desktop)
}

/// `ArtifactPolicy.resolve(_:)`: a path, `~/path`, Desktop-relative path (against `desktop`, Swift's
/// `Layout.desktop`) or http/https/file URL → a target. Paths must exist; a bare host like "myapp.vercel.app" means
/// the web.
pub fn resolve(raw: &str, desktop: &Path) -> Option<Target> {
    let s = trim_ws_nl(raw);
    if s.is_empty() {
        return None;
    }
    if let Some((scheme, _)) = scheme_of(s) {
        if ["http", "https", "file"].contains(&scheme.to_lowercase().as_str()) && !s.contains(char::is_whitespace) {
            return Some(Target::Url(s.to_string()));
        }
    }
    let expanded = crate::paths::expanduser(s);
    let candidates: Vec<PathBuf> =
        if expanded.is_absolute() { vec![expanded.clone()] } else { vec![desktop.join(s), expanded.clone()] };
    for c in candidates {
        if c.exists() {
            return Some(Target::File(c));
        }
    }
    let host = s.split('/').next().unwrap_or("");
    if !s.starts_with('/') && !s.starts_with('~') && !s.starts_with('.') && !s.contains(' ') && host.contains('.') {
        return Some(Target::Url(format!("https://{s}")));
    }
    None
}

/// Every predicate for one URL, for the engine's JSON-RPC.
pub fn check_json(url: &str) -> Value {
    json!({
        "allowed": is_allowed(url), "signIn": is_sign_in(url), "webPage": is_web_page(url),
        "openOnRequest": can_open_on_request(url), "handOff": can_hand_off(url),
    })
}

#[cfg(test)]
mod tests {
    //! Port of the ArtifactPolicy cases in PathSecurityTests.swift and AccountsAndVisualizationsTests.swift.
    use super::*;

    fn desktop() -> PathBuf {
        PathBuf::from("/nonexistent-dg-desktop")
    }

    #[test]
    fn allowed_artifacts() {
        for s in [
            "http://localhost:5173/", "http://127.0.0.1:3000/x", "http://app.localhost/", "https://myapp.vercel.app/",
            "https://a.b.github.io/page", "https://claude.ai/artifact/abc", "https://claude.ai/public/artifacts/x",
            "https://chatgpt.com/canvas/shared/1", "https://claude.site/x", "about:blank",
        ] {
            assert!(is_allowed(s), "{s}");
        }
        for f in ["/tmp/a.html", "/tmp/a.SVG", "/tmp/report.pdf", "/tmp/notes.md"] {
            assert!(is_allowed(f), "{f}");
        }
    }

    #[test]
    fn lookalike_hosts_and_other_pages_are_not_artifacts() {
        for s in [
            "https://claude.ai/chat/123", "https://claude.ai/settings", "https://evilclaude.ai/artifact/x",
            "https://claude.ai.evil.com/artifact/x", "https://vercel.app.evil.com/", "https://notvercel.app/",
            "https://chatgpt.com/c/123", "https://example.com/", "ftp://localhost/x", "javascript:alert(1)",
            "ssh://localhost", "x-apple.systempreferences:com.apple.preference.security",
        ] {
            assert!(!is_allowed(s), "{s}");
        }
    }

    #[test]
    fn only_viewable_file_types() {
        for f in ["/tmp/dg-none/Calculator.app", "/tmp/run.command", "/tmp/install.sh", "/tmp/x.exe", "/etc/passwd"] {
            assert!(!is_allowed(f), "{f}");
        }
    }

    #[test]
    fn sign_in_pages_stay_in_the_viewer_but_arent_artifacts() {
        for s in [
            "https://claude.ai/login?returnTo=/artifact/x", "https://accounts.google.com/o/oauth2/auth",
            "https://auth.openai.com/authorize", "https://github.com/login/oauth/authorize",
        ] {
            assert!(is_sign_in(s), "{s}");
        }
        assert!(!is_allowed("https://claude.ai/login"), "only reachable from inside a tab");
        for s in [
            "http://claude.ai/login", "https://evil-claude.ai/login", "https://claude.ai.evil.com/login",
            "https://example.com/login", "file:///tmp/login.html",
        ] {
            assert!(!is_sign_in(s), "{s}");
        }
    }

    #[test]
    fn any_web_page_opens_when_asked_for_by_name() {
        for s in ["https://example.com/", "http://news.ycombinator.com/item?id=1", "https://claude.ai/artifact/x"] {
            assert!(can_open_on_request(s), "{s}");
        }
        assert!(can_open_on_request("/tmp/report.html"));
        for s in [
            "file:///tmp/run.command", "file:///Applications/Calculator.app", "javascript:alert(1)", "ssh://host",
            "https:///no-host", "vscode://file/x", "x-apple.systempreferences:com.apple.preference.security",
        ] {
            assert!(!can_open_on_request(s), "{s}");
        }
        assert!(!is_allowed("https://example.com/"), "but not on its own");
    }

    #[test]
    fn address_bar_text() {
        let d = desktop();
        let a = |s: &str| address(s, &d).map(|t| t.as_str());
        assert_eq!(a("localhost:5173").as_deref(), Some("http://localhost:5173"));
        assert_eq!(a("127.0.0.1:8000/docs").as_deref(), Some("http://127.0.0.1:8000/docs"));
        assert_eq!(a("example.com/x").as_deref(), Some("https://example.com/x"));
        assert_eq!(a(" https://a.dev ").as_deref(), Some("https://a.dev"));
        assert_eq!(a(""), None);
        assert_eq!(a("localhostile"), None, "only the real localhost");
        assert!(!a("javascript:alert(1)").map(|u| can_open_on_request(&u)).unwrap_or(false));
    }

    #[test]
    fn only_web_and_mail_are_handed_to_other_apps() {
        assert!(can_hand_off("https://example.com/"));
        assert!(can_hand_off("http://example.com/x"));
        assert!(can_hand_off("mailto:someone@example.com"));
        for s in [
            "file:///Applications/Calculator.app", "file:///tmp/run.command", "javascript:alert(1)", "ssh://host",
            "x-apple.systempreferences:com.apple.preference.security", "vscode://file/tmp/x", "https:///no-host",
            "smb://server/share", "tel:123",
        ] {
            assert!(!can_hand_off(s), "{s}");
        }
    }

    #[test]
    fn resolve_targets() {
        let d = desktop();
        assert_eq!(resolve("", &d), None);
        assert_eq!(resolve("   ", &d), None);
        assert_eq!(resolve("https://x.dev/a", &d), Some(Target::Url("https://x.dev/a".into())));
        assert_eq!(resolve("myapp.vercel.app/path", &d), Some(Target::Url("https://myapp.vercel.app/path".into())));
        let dir = std::env::temp_dir().join(format!("dg-policy-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("report.html");
        std::fs::write(&file, "<p>x</p>").unwrap();
        let fs = file.to_string_lossy().into_owned();
        assert_eq!(resolve(&fs, &d), Some(Target::File(file.clone())));
        assert_eq!(resolve(&format!("  {fs}\n"), &d), Some(Target::File(file.clone())));
        // Desktop-relative (Swift resolves against Layout.desktop).
        assert_eq!(resolve("report.html", &dir), Some(Target::File(dir.join("report.html"))));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_paths_dont_turn_into_web_addresses() {
        let d = desktop();
        assert_eq!(resolve("/no/such/file.html", &d), None);
        assert_eq!(resolve(&format!("~/no-such-folder-{}/x.html", std::process::id()), &d), None);
        assert_eq!(resolve("./x.html", &d), None);
        assert_eq!(resolve("notes/plan", &d), None);
        assert_eq!(resolve("javascript:alert(1)", &d), None);
    }

    /// AccountsAndVisualizationsTests.testEveryLoginPageIsASignInPage (ArtifactAccount.all's login URLs).
    #[test]
    fn every_login_page_is_a_sign_in_page() {
        for u in [
            "https://claude.ai/login", "https://chatgpt.com/auth/login",
            "https://accounts.google.com/ServiceLogin?continue=https%3A%2F%2Fgemini.google.com%2Fapp",
            "https://vercel.com/login", "https://github.com/login",
        ] {
            assert!(is_sign_in(u), "{u}");
        }
    }

    /// From AccountsAndVisualizationsTests (Codex visualization pages open in the viewer).
    #[test]
    fn local_html_pages_open_in_the_viewer() {
        assert!(is_allowed("/tmp/pages/thread-a/chart.html"));
        assert!(is_allowed("file:///tmp/pages/thread-a/chart.html"));
    }
}
