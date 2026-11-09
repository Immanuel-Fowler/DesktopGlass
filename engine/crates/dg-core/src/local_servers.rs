//! Port of `Control/LocalServers.swift`: web servers running on this machine right now (dev servers, previews,
//! notebooks…), found with `lsof` and confirmed by asking each port for a page.
//!
//! Pure parts: [`parse_lsof`] (the `lsof -Fpcn` output) and [`page_title`] (title / fallback from a response).
//! Side effects are isolated in [`listening_ports`] (runs `/usr/sbin/lsof`) and [`probe`] (one HTTP GET to
//! localhost with std's TCP, no HTTP crate).
//!
//! Differences from URLSession: redirects are followed only when they stay on this port of localhost (URLSession
//! follows any); the request is a plain HTTP/1.1 GET with `Connection: close` (chunked bodies are decoded); no TLS.

use serde_json::{json, Value};
use std::collections::HashSet;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

/// `LocalServer`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LocalServer {
    pub port: u16,
    pub process: String,
    pub pid: i64,
    pub title: String,
    pub status: u16,
}

impl LocalServer {
    /// `url`: `http://localhost:PORT/`.
    pub fn url(&self) -> String {
        format!("http://localhost:{}/", self.port)
    }
    /// `dictionary`: the same keys the control channel / MCP `list_local_servers` return.
    pub fn to_json(&self) -> Value {
        json!({"port": self.port, "process": self.process, "pid": self.pid, "title": self.title, "status": self.status,
               "url": self.url()})
    }
}

/// System services that listen on ports but aren't anything you'd want to browse (lsof truncates names to 9 chars).
pub const IGNORED_PROCESSES: &[&str] = &[
    "rapportd", "ControlCe", "ControlCenter", "sharingd", "identitys", "mDNSRespo", "launchd", "AirPlayXPC", "remoted",
    "WiFiAgent",
];

/// The listening-port part of `LocalServerScanner.listeningPorts()`: parses `lsof -nP -iTCP -sTCP:LISTEN -Fpcn`
/// output into one candidate per port that's reachable from localhost (first process wins).
pub fn parse_lsof(output: &str) -> Vec<LocalServer> {
    let (mut pid, mut command) = (0i64, String::new());
    let mut seen = HashSet::new();
    let mut result = vec![];
    for line in output.split('\n').filter(|l| !l.is_empty()) {
        let mut chars = line.chars();
        let Some(tag) = chars.next() else { continue };
        let value = chars.as_str();
        match tag {
            'p' => pid = value.parse().unwrap_or(0),
            'c' => command = value.to_string(),
            'n' => {
                // e.g. 127.0.0.1:3000, *:5173, [::1]:8080
                let Some((host, port)) = value.rsplit_once(':') else { continue };
                let Ok(port) = port.parse::<u16>() else { continue };
                let reachable = host == "*"
                    || host.starts_with("127.")
                    || host == "[::1]"
                    || host == "localhost"
                    || host == "[::]"
                    || host == "0.0.0.0";
                if reachable && seen.insert(port) {
                    result.push(LocalServer { port, process: command.clone(), pid, title: String::new(), status: 0 });
                }
            }
            _ => {}
        }
    }
    result
}

/// Runs `/usr/sbin/lsof` and parses it (empty when lsof isn't there or fails).
pub fn listening_ports() -> Vec<LocalServer> {
    let out = std::process::Command::new("/usr/sbin/lsof")
        .args(["-nP", "-iTCP", "-sTCP:LISTEN", "-Fpcn"])
        .stderr(std::process::Stdio::null())
        .output();
    match out {
        Ok(o) => parse_lsof(&String::from_utf8_lossy(&o.stdout)),
        Err(_) => vec![],
    }
}

fn is_ws_nl(c: char) -> bool {
    c == '\t' || matches!(c as u32, 0x0A..=0x0D | 0x85 | 0x2028 | 0x2029 | 0x20 | 0xA0 | 0x1680 | 0x2000..=0x200A | 0x202F | 0x205F | 0x3000)
}

/// Regex `<title[^>]*>([^<]{1,120})</title>` (case-insensitive): the whole first match.
fn title_match(html: &str) -> Option<String> {
    let t: Vec<char> = html.chars().collect();
    let ci = |at: usize, pat: &str| -> bool {
        let p: Vec<char> = pat.chars().collect();
        at + p.len() <= t.len() && t[at..at + p.len()].iter().zip(&p).all(|(a, b)| a.to_ascii_lowercase() == *b)
    };
    for i in 0..t.len() {
        if !ci(i, "<title") {
            continue;
        }
        let Some(off) = t[i + 6..].iter().position(|&c| c == '>') else { continue };
        let start = i + 6 + off + 1;
        let mut k = start;
        while k < t.len() && t[k] != '<' {
            k += 1;
        }
        let n = k - start;
        if (1..=120).contains(&n) && ci(k, "</title>") {
            return Some(t[i..k + 8].iter().collect());
        }
    }
    None
}

/// `<[^>]+>` → "".
fn strip_tags(s: &str) -> String {
    let t: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < t.len() {
        if t[i] == '<' && i + 1 < t.len() && t[i + 1] != '>' {
            if let Some(off) = t[i + 1..].iter().position(|&c| c == '>') {
                i += off + 2;
                continue;
            }
        }
        out.push(t[i]);
        i += 1;
    }
    out
}

/// The title part of `LocalServerScanner.probe`: the page `<title>` when the response is HTML (from the first
/// 64,000 bytes, which must be valid UTF-8), else "JSON API" for JSON, else the process name.
pub fn page_title(body: &[u8], content_type: &str, process: &str) -> String {
    let ty = content_type.to_lowercase();
    let mut title = String::new();
    if ty.contains("html") {
        if let Ok(html) = std::str::from_utf8(&body[..body.len().min(64_000)]) {
            if let Some(m) = title_match(html) {
                title = strip_tags(&m).trim_matches(is_ws_nl).to_string();
            }
        }
    }
    if title.is_empty() {
        title = if ty.contains("json") { "JSON API".into() } else { process.to_string() };
    }
    title
}

/// A parsed HTTP response.
#[derive(Clone, Debug, Default)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

/// Parses raw HTTP/1.x response bytes (headers + body, chunked or not). Pure.
pub fn parse_http_response(raw: &[u8]) -> Option<HttpResponse> {
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = String::from_utf8_lossy(&raw[..split]);
    let mut lines = head.split("\r\n");
    let status_line = lines.next()?;
    let mut parts = status_line.split_whitespace();
    if !parts.next()?.starts_with("HTTP/") {
        return None;
    }
    let status: u16 = parts.next()?.parse().ok()?;
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':').map(|(k, v)| (k.trim().to_string(), v.trim().to_string())))
        .collect();
    let mut body = raw[split + 4..].to_vec();
    let mut r = HttpResponse { status, headers, body: vec![] };
    if r.header("transfer-encoding").map(|v| v.to_lowercase().contains("chunked")).unwrap_or(false) {
        body = dechunk(&body);
    } else if let Some(n) = r.header("content-length").and_then(|v| v.parse::<usize>().ok()) {
        body.truncate(n);
    }
    r.body = body;
    Some(r)
}

fn dechunk(b: &[u8]) -> Vec<u8> {
    let mut out = vec![];
    let mut i = 0;
    while i < b.len() {
        let Some(eol) = b[i..].windows(2).position(|w| w == b"\r\n") else { break };
        let size_s = String::from_utf8_lossy(&b[i..i + eol]);
        let Ok(size) = usize::from_str_radix(size_s.split(';').next().unwrap_or("").trim(), 16) else { break };
        if size == 0 {
            break;
        }
        let start = i + eol + 2;
        let end = (start + size).min(b.len());
        out.extend_from_slice(&b[start..end]);
        i = end + 2;
    }
    out
}

fn get(addrs: &[SocketAddr], port: u16, path: &str, deadline: Instant) -> Option<HttpResponse> {
    let per_request = Duration::from_millis(1200);
    for addr in addrs {
        let left = deadline.checked_duration_since(Instant::now())?;
        let Ok(mut s) = TcpStream::connect_timeout(addr, per_request.min(left)) else { continue };
        let _ = s.set_read_timeout(Some(per_request.min(left)));
        let _ = s.set_write_timeout(Some(per_request.min(left)));
        let req = format!(
            "GET {path} HTTP/1.1\r\nHost: localhost:{port}\r\nAccept: text/html,application/json;q=0.9,*/*;q=0.8\r\nUser-Agent: DesktopGlass\r\nConnection: close\r\n\r\n"
        );
        if s.write_all(req.as_bytes()).is_err() {
            continue;
        }
        let mut raw = vec![];
        let mut buf = [0u8; 16384];
        loop {
            if Instant::now() > deadline || raw.len() > 80_000 {
                break;
            }
            match s.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    raw.extend_from_slice(&buf[..n]);
                    if let Some(r) = parse_http_response(&raw) {
                        if let Some(len) = r.header("content-length").and_then(|v| v.parse::<usize>().ok()) {
                            if r.body.len() >= len {
                                break;
                            }
                        }
                    }
                }
                Err(_) => break,
            }
        }
        if let Some(r) = parse_http_response(&raw) {
            return Some(r);
        }
    }
    None
}

/// `LocalServerScanner.probe`: asks `http://localhost:PORT/` for a page (1.2 s per request, 2 s in all) and fills in
/// the status and title; None when nothing answers HTTP.
pub fn probe(candidate: &LocalServer) -> Option<LocalServer> {
    let deadline = Instant::now() + Duration::from_secs(2);
    let addrs: Vec<SocketAddr> = ("localhost", candidate.port).to_socket_addrs().ok()?.collect();
    let mut path = "/".to_string();
    let mut resp = get(&addrs, candidate.port, &path, deadline)?;
    for _ in 0..5 {
        if !matches!(resp.status, 301 | 302 | 303 | 307 | 308) {
            break;
        }
        let Some(loc) = resp.header("location").map(str::to_string) else { break };
        let next = if loc.starts_with('/') {
            loc
        } else {
            let mut found = None;
            for h in ["localhost", "127.0.0.1", "[::1]"] {
                let prefix = format!("http://{h}:{}", candidate.port);
                if let Some(rest) = loc.strip_prefix(&prefix) {
                    found = Some(if rest.is_empty() { "/".to_string() } else { rest.to_string() });
                }
            }
            match found {
                Some(p) if p.starts_with('/') => p,
                _ => break,
            }
        };
        if next == path {
            break;
        }
        path = next;
        match get(&addrs, candidate.port, &path, deadline) {
            Some(r) => resp = r,
            None => break,
        }
    }
    let mut server = candidate.clone();
    server.status = resp.status;
    server.title = page_title(&resp.body, resp.header("content-type").unwrap_or(""), &candidate.process);
    Some(server)
}

/// `LocalServerScanner.scan(excludingPorts:completion:)`, synchronously: listening ports minus `excluding_ports` and
/// system services, each probed in parallel; the ones that answer HTTP, sorted by port.
pub fn scan(excluding_ports: &HashSet<u16>) -> Vec<LocalServer> {
    let candidates: Vec<LocalServer> = listening_ports()
        .into_iter()
        .filter(|c| !excluding_ports.contains(&c.port) && !IGNORED_PROCESSES.contains(&c.process.as_str()))
        .collect();
    probe_all(&candidates)
}

/// Probes candidates in parallel; the ones that answer, sorted by port.
pub fn probe_all(candidates: &[LocalServer]) -> Vec<LocalServer> {
    let mut found: Vec<LocalServer> = std::thread::scope(|s| {
        let handles: Vec<_> = candidates.iter().map(|c| s.spawn(move || probe(c))).collect();
        handles.into_iter().filter_map(|h| h.join().ok().flatten()).collect()
    });
    found.sort_by_key(|s| s.port);
    found
}

/// `scan` as a JSON array of `LocalServer.dictionary`s.
pub fn scan_json(excluding_ports: &HashSet<u16>) -> Value {
    Value::Array(scan(excluding_ports).iter().map(LocalServer::to_json).collect())
}

#[cfg(test)]
mod tests {
    //! No Swift tests exist for LocalServers.swift; these cover the pure parsing and a probe against a test server.
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn parses_lsof_output() {
        let out = "p501\ncnode\nf23\nn127.0.0.1:5173\nf24\nn[::1]:5173\np77\ncrapportd\nf5\nn*:49152\np900\ncpython3.1\nf3\nn*:8000\np901\ncpostgres\nf6\nn192.168.1.4:5432\nf7\nn*:*\np902\ncjupyter\nf3\nn[::]:8888\nn0.0.0.0:9000\nnlocalhost:3000\n";
        let got = parse_lsof(out);
        let ports: Vec<(u16, &str, i64)> = got.iter().map(|s| (s.port, s.process.as_str(), s.pid)).collect();
        assert_eq!(
            ports,
            [(5173, "node", 501), (49152, "rapportd", 77), (8000, "python3.1", 900), (8888, "jupyter", 902), (9000, "jupyter", 902), (3000, "jupyter", 902)]
        );
        assert_eq!(got[0].to_json()["url"], "http://localhost:5173/");
        assert!(parse_lsof("").is_empty());
    }

    #[test]
    fn titles() {
        assert_eq!(page_title(b"<html><head><TITLE lang=en>\n  Vite App \n</title>", "text/html; charset=utf-8", "node"), "Vite App");
        assert_eq!(page_title(b"<title></title>", "text/html", "node"), "node");
        assert_eq!(page_title(&[b"<title>".as_slice(), &[b'x'; 121], b"</title>"].concat(), "text/html", "node"), "node");
        assert_eq!(page_title(b"<title>A</title>", "text/plain", "ruby"), "ruby");
        assert_eq!(page_title(b"{}", "application/json", "uvicorn"), "JSON API");
        assert_eq!(page_title(b"<title>skip</title>", "", "deno"), "deno");
        assert_eq!(page_title(b"<titlex>T</title>", "text/html", "p"), "T");
    }

    #[test]
    fn parses_http_responses() {
        let r = parse_http_response(b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nTransfer-Encoding: chunked\r\n\r\n5\r\n<titl\r\n8\r\ne>Hi</ti\r\n5\r\ntle>!\r\n0\r\n\r\n").unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(r.body, b"<title>Hi</title>!");
        assert!(parse_http_response(b"garbage").is_none());
    }

    fn serve(responses: Vec<&'static str>) -> u16 {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for r in responses {
                let Ok((mut s, _)) = l.accept() else { return };
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf);
                let _ = s.write_all(r.as_bytes());
            }
        });
        port
    }

    #[test]
    fn probe_follows_local_redirects_and_reads_the_title() {
        let port = serve(vec![
            "HTTP/1.1 302 Found\r\nLocation: /app/\r\nContent-Length: 0\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 30\r\n\r\n<title>Dashboard</title><p>x</",
        ]);
        let c = LocalServer { port, process: "node".into(), pid: 1, title: String::new(), status: 0 };
        let s = probe_all(&[c]);
        assert_eq!(s.len(), 1);
        assert_eq!((s[0].status, s[0].title.as_str()), (200, "Dashboard"));
    }

    #[test]
    fn probe_skips_ports_that_dont_speak_http() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        let c = LocalServer { port, process: "x".into(), pid: 1, title: String::new(), status: 0 };
        assert!(probe(&c).is_none());
    }
}
