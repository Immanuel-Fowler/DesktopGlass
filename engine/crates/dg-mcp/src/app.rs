//! The control channel to the running DesktopGlass app (`app_call` / `ensure_app` in mcp_server.py):
//! control.json `{port, token, pid}`, one JSON line with token/method/params over 127.0.0.1, one JSON line back.

use crate::pyjson::{self, type_name};
use crate::Server;
use dg_core::py::py_str;
use serde_json::{json, Map, Value};
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::process::{Command, Stdio};
use std::time::Duration;

/// Why an app call failed. `Unavailable` is Python's `AppUnavailable` (some tools fall back on it).
#[derive(Debug)]
pub enum AppError {
    Unavailable(String),
    Other(String),
}

impl AppError {
    pub fn message(self) -> String {
        match self {
            AppError::Unavailable(m) | AppError::Other(m) => m,
        }
    }
}

/// `int(x)` for the pid/port fields.
fn int_field(info: &Value, key: &str) -> Option<i64> {
    pyjson::py_int(info.get(key)?).ok()
}

/// Python's text for an OSError from a socket call.
fn os_error_text(e: &std::io::Error) -> String {
    if matches!(e.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) {
        return "timed out".into();
    }
    match e.raw_os_error() {
        Some(n) => {
            let msg = unsafe { std::ffi::CStr::from_ptr(libc::strerror(n)) }.to_string_lossy().into_owned();
            format!("[Errno {n}] {msg}")
        }
        None => e.to_string(),
    }
}

impl Server {
    /// `_control_info`: control.json when the app that wrote it is still running.
    pub fn control_info(&self) -> Option<Value> {
        let text = std::fs::read_to_string(&self.control).ok()?;
        let info: Value = serde_json::from_str(&text).ok()?;
        let pid = int_field(&info, "pid")?;
        let pid = libc::pid_t::try_from(pid).ok()?;
        if unsafe { libc::kill(pid, 0) } != 0 {
            return None;
        }
        Some(info)
    }

    /// `ensure_app`: the control info, launching the app in the background (`open -g`) when it isn't running.
    pub fn ensure_app(&self, launch: bool) -> Result<Option<Value>, AppError> {
        let info = self.control_info();
        if info.is_some() || !launch {
            return Ok(info);
        }
        let Some(app) = self.app_paths.iter().find(|a| a.exists()) else {
            return Err(AppError::Unavailable("DesktopGlass isn't installed — run install.sh".into()));
        };
        let _ = Command::new("/usr/bin/open")
            .arg("-g")
            .arg(app)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        for _ in 0..50 {
            std::thread::sleep(Duration::from_millis(200));
            if let Some(info) = self.control_info() {
                std::thread::sleep(Duration::from_millis(300));
                return Ok(Some(info));
            }
        }
        Err(AppError::Unavailable("DesktopGlass didn't start".into()))
    }

    /// `app_call`: asks the running app to do something; returns its `result`.
    pub fn app_call(&self, method: &str, params: Option<Value>, timeout: u64, launch: bool) -> Result<Value, AppError> {
        let info = self.ensure_app(launch)?.ok_or_else(|| AppError::Unavailable("DesktopGlass isn't running".into()))?;
        let port = int_field(&info, "port").ok_or_else(|| AppError::Other(pyjson::repr_str("port")))?;
        let port = u16::try_from(port).map_err(|_| AppError::Other("getsockaddrarg: port must be 0-65535.".into()))?;
        let token = info.get("token").cloned().ok_or_else(|| AppError::Other(pyjson::repr_str("token")))?;
        let params = params.filter(dg_core::py::truthy).unwrap_or_else(|| Value::Object(Map::new()));
        let line = pyjson::dumps(&json!({"token": token, "method": method, "params": params}), None, true) + "\n";
        let unreachable = |e: std::io::Error| AppError::Unavailable(format!("Couldn't reach DesktopGlass ({})", os_error_text(&e)));
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let mut sock = TcpStream::connect_timeout(&addr, Duration::from_secs(5)).map_err(unreachable)?;
        let t = Some(Duration::from_secs(timeout.max(1)));
        sock.set_read_timeout(t).map_err(unreachable)?;
        sock.set_write_timeout(t).map_err(unreachable)?;
        sock.write_all(line.as_bytes()).map_err(unreachable)?;
        let mut buf: Vec<u8> = Vec::new();
        let mut chunk = vec![0u8; 1 << 16];
        while !buf.ends_with(b"\n") {
            match sock.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return Err(unreachable(e)),
            }
        }
        let text = String::from_utf8_lossy(&buf);
        let text = if text.is_empty() { "{}" } else { text.as_ref() };
        let reply: Value = serde_json::from_str(text.trim()).map_err(|e| AppError::Other(e.to_string()))?;
        match &reply {
            Value::Object(o) => {
                if let Some(err) = o.get("error") {
                    return Err(AppError::Other(py_str(err)));
                }
                Ok(o.get("result").cloned().unwrap_or(Value::Null))
            }
            other => Err(AppError::Other(format!("'{}' object has no attribute 'get'", type_name(other)))),
        }
    }

    /// `via_app`: an app call whose result is shown as text.
    pub fn via_app(&self, method: &str, params: Option<Value>, timeout: u64) -> Result<String, String> {
        self.app_call(method, params, timeout, true).map(|v| pyjson::as_text(&v)).map_err(AppError::message)
    }
}
