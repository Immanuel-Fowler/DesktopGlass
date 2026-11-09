//! A minimal blocking HTTP/1.1 JSON client over std (local model servers only: plain `http://`, one request per
//! connection, Content-Length or chunked replies). Errors split like Python's urllib: `Transport` for
//! URLError (refused connection, HTTP error status) and `Other` for everything else (timeouts, bad JSON).

use serde_json::Value;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

#[derive(Debug)]
pub enum Error {
    Transport(String),
    Other(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Error::Transport(s) | Error::Other(s) => f.write_str(s),
        }
    }
}

struct Url {
    host: String,
    port: u16,
    path: String,
}

fn parse_url(url: &str) -> Result<Url, Error> {
    let rest = url.strip_prefix("http://").ok_or_else(|| {
        Error::Transport(format!("only http:// endpoints are supported (got {url})"))
    })?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if !h.ends_with(']') || h.starts_with('[') => {
            (h.to_string(), p.parse().map_err(|_| Error::Transport(format!("bad port in {url}")))?)
        }
        _ => (authority.to_string(), 80),
    };
    let host = host.trim_start_matches('[').trim_end_matches(']').to_string();
    if host.is_empty() {
        return Err(Error::Transport(format!("bad URL {url}")));
    }
    Ok(Url { host, port, path: path.to_string() })
}

/// GET (payload None) or POST JSON; returns the parsed JSON reply.
pub fn json(url: &str, payload: Option<&Value>, timeout: Duration) -> Result<Value, Error> {
    let u = parse_url(url)?;
    let addrs: Vec<_> = (u.host.as_str(), u.port)
        .to_socket_addrs()
        .map_err(|e| Error::Transport(format!("{}: {e}", u.host)))?
        .collect();
    let mut stream = None;
    let mut last = String::from("no address");
    for a in &addrs {
        match TcpStream::connect_timeout(a, timeout.min(Duration::from_secs(10))) {
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(e) => last = e.to_string(),
        }
    }
    let mut s = stream.ok_or(Error::Transport(last))?;
    let _ = s.set_read_timeout(Some(timeout));
    let _ = s.set_write_timeout(Some(timeout));
    let body = payload.map(|p| serde_json::to_vec(p).unwrap_or_default()).unwrap_or_default();
    let host_hdr = if u.host.contains(':') { format!("[{}]:{}", u.host, u.port) } else { format!("{}:{}", u.host, u.port) };
    let head = format!(
        "{} {} HTTP/1.1\r\nHost: {host_hdr}\r\nAccept: application/json\r\nConnection: close\r\n{}\r\n",
        if payload.is_some() { "POST" } else { "GET" },
        u.path,
        if payload.is_some() {
            format!("Content-Type: application/json\r\nContent-Length: {}\r\n", body.len())
        } else {
            String::new()
        }
    );
    s.write_all(head.as_bytes()).and_then(|_| s.write_all(&body)).map_err(|e| Error::Transport(e.to_string()))?;
    let mut raw = Vec::new();
    if let Err(e) = s.read_to_end(&mut raw) {
        if raw.is_empty() || !matches!(e.kind(), std::io::ErrorKind::ConnectionReset) {
            return Err(match e.kind() {
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => Error::Other("timed out".into()),
                _ => Error::Transport(e.to_string()),
            });
        }
    }
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| Error::Transport("malformed HTTP reply".into()))?;
    let headers = String::from_utf8_lossy(&raw[..split]).into_owned();
    let mut payload_bytes = raw[split + 4..].to_vec();
    let status_line = headers.lines().next().unwrap_or("");
    let status: u16 = status_line.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    let chunked = headers.lines().any(|l| {
        let l = l.to_ascii_lowercase();
        l.starts_with("transfer-encoding:") && l.contains("chunked")
    });
    if chunked {
        payload_bytes = dechunk(&payload_bytes);
    }
    if !(200..300).contains(&status) {
        let msg = String::from_utf8_lossy(&payload_bytes);
        return Err(Error::Transport(format!("HTTP Error {status}: {}", msg.trim().chars().take(200).collect::<String>())));
    }
    let text = String::from_utf8_lossy(&payload_bytes);
    serde_json::from_str(&text).map_err(|e| Error::Other(format!("bad JSON from {url}: {e}")))
}

fn dechunk(mut b: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(nl) = b.windows(2).position(|w| w == b"\r\n") {
        let size_str = String::from_utf8_lossy(&b[..nl]);
        let size = usize::from_str_radix(size_str.split(';').next().unwrap_or("").trim(), 16).unwrap_or(0);
        b = &b[nl + 2..];
        if size == 0 || b.len() < size {
            out.extend_from_slice(&b[..size.min(b.len())]);
            break;
        }
        out.extend_from_slice(&b[..size]);
        b = b.get(size + 2..).unwrap_or(&[]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn serve_once(reply: &'static str) -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut c, _) = l.accept().unwrap();
            let mut buf = [0u8; 4096];
            let _ = c.read(&mut buf);
            c.write_all(reply.as_bytes()).unwrap();
        });
        format!("http://{addr}")
    }

    #[test]
    fn content_length_and_chunked() {
        let u = serve_once("HTTP/1.1 200 OK\r\nContent-Length: 8\r\n\r\n{\"a\":1}\n");
        assert_eq!(json(&format!("{u}/x"), None, Duration::from_secs(5)).unwrap()["a"], 1);
        let u = serve_once("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\n{\"b\"\r\n3\r\n:2}\r\n0\r\n\r\n");
        assert_eq!(json(&u, Some(&serde_json::json!({})), Duration::from_secs(5)).unwrap()["b"], 2);
        let u = serve_once("HTTP/1.1 404 Not Found\r\nContent-Length: 2\r\n\r\nno");
        assert!(matches!(json(&u, None, Duration::from_secs(5)), Err(Error::Transport(_))));
    }

    #[test]
    fn refused_and_https() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        assert!(matches!(json(&format!("http://127.0.0.1:{port}/"), None, Duration::from_secs(2)), Err(Error::Transport(_))));
        assert!(matches!(json("https://x/", None, Duration::from_secs(2)), Err(Error::Transport(_))));
        let u = parse_url("http://localhost:11434").unwrap();
        assert_eq!((u.host.as_str(), u.port, u.path.as_str()), ("localhost", 11434, "/"));
    }
}
