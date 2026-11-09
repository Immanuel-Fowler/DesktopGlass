//! The control channel, moved here from ControlServer.swift with the same wire format: loopback TCP on a random
//! port; control.json `{port, token, pid}` (mode 600); one request per line `{"token", "method", "params"}`;
//! one reply per line `{"result": …}` or `{"error": "message"}`.

use crate::Engine;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

const MAX_LINE: u64 = 4 << 20;

pub struct Control {
    pub port: u16,
    token: String,
    file: PathBuf,
    stopped: Arc<AtomicBool>,
}

/// 64 hex characters from the OS's random source.
pub fn new_token() -> String {
    let mut bytes = [0u8; 32];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = f.read_exact(&mut bytes);
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

impl Control {
    pub fn start(engine: Arc<Engine>) -> std::io::Result<Control> {
        Self::start_at(engine, dg_platform::control_file())
    }

    pub fn start_at(engine: Arc<Engine>, file: PathBuf) -> std::io::Result<Control> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let token = new_token();
        write_info(&file, port, &token)?;
        let stopped = Arc::new(AtomicBool::new(false));
        let (tok, stop) = (token.clone(), stopped.clone());
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(conn) = conn else { continue };
                let (engine, tok) = (engine.clone(), tok.clone());
                std::thread::spawn(move || serve_conn(engine, conn, &tok));
            }
        });
        Ok(Control { port, token, file, stopped })
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    /// Stops accepting and removes control.json (only if it's still ours).
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        if let Ok(text) = std::fs::read_to_string(&self.file) {
            if text.contains(&self.token) {
                let _ = std::fs::remove_file(&self.file);
            }
        }
        let _ = TcpStream::connect(("127.0.0.1", self.port)); // wake the accept loop
    }
}

fn write_info(file: &PathBuf, port: u16, token: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(file.parent().unwrap())?;
    let tmp = file.with_extension(format!("json.{}.tmp", std::process::id()));
    {
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
        f.write_all(json!({"port": port, "token": token, "pid": std::process::id()}).to_string().as_bytes())?;
    }
    std::fs::rename(&tmp, file)
}

fn serve_conn(engine: Arc<Engine>, conn: TcpStream, token: &str) {
    let Ok(mut writer) = conn.try_clone() else { return };
    let mut reader = BufReader::new(conn.take(MAX_LINE));
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        if !line.ends_with('\n') {
            return; // over the size cap or cut off
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(Value::Object(obj)) => {
                if obj.get("token").and_then(|t| t.as_str()) != Some(token) {
                    json!({"error": "Unauthorized"})
                } else {
                    let method = obj.get("method").and_then(|m| m.as_str()).unwrap_or("");
                    let params = match obj.get("params") {
                        Some(Value::Object(p)) => Value::Object(p.clone()),
                        _ => json!({}),
                    };
                    match engine.handle_control(method, &params) {
                        Ok(v) => json!({"result": v}),
                        Err(e) => json!({"error": e}),
                    }
                }
            }
            _ => json!({"error": "Bad request"}),
        };
        let mut out = serde_json::to_vec(&reply).unwrap_or_else(|_| b"{\"error\":\"Unencodable result\"}".to_vec());
        out.push(b'\n');
        if writer.write_all(&out).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;

    fn ask(port: u16, line: &str) -> Value {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.write_all(line.as_bytes()).unwrap();
        s.write_all(b"\n").unwrap();
        let mut r = BufReader::new(s);
        let mut out = String::new();
        r.read_line(&mut out).unwrap();
        serde_json::from_str(&out).unwrap()
    }

    #[test]
    fn token_gates_and_ping() {
        let home = crate::settings::tests::temp_home("control");
        let engine = Engine::with_settings(Box::new(std::io::sink()), crate::settings::Settings::open_at(home.path().join("s.json")));
        let file = home.path().join("control.json");
        let c = Control::start_at(engine, file.clone()).unwrap();
        let info: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(info["port"], json!(c.port));
        assert_eq!(info["pid"], json!(std::process::id()));
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);

        assert_eq!(ask(c.port, r#"{"token":"wrong","method":"ping"}"#), json!({"error": "Unauthorized"}));
        assert_eq!(ask(c.port, "nonsense"), json!({"error": "Bad request"}));
        let r = ask(c.port, &format!(r#"{{"token":"{}","method":"ping"}}"#, c.token()));
        assert_eq!(r["result"]["ok"], json!(true));

        // gated methods are refused while the group is off (default)
        let r = ask(c.port, &format!(r#"{{"token":"{}","method":"games.play","params":{{"game":"x"}}}}"#, c.token()));
        assert!(r["error"].as_str().unwrap().contains("Launch games"));

        c.stop();
        assert!(!file.exists());
    }
}
