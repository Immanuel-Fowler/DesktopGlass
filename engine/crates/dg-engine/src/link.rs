//! The UI link: newline-delimited JSON-RPC 2.0 over stdin/stdout.
//!
//! Requests from the UI are handled on their own thread each (a slow one never blocks the next); engine → UI
//! requests (`ui/control`) wait for the matching response id with a timeout.

use crate::Engine;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::os::unix::io::FromRawFd;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub struct Link {
    out: Mutex<Box<dyn Write + Send>>,
    next_id: AtomicU64,
    pending: Mutex<HashMap<u64, Sender<Result<Value, String>>>>,
}

/// Moves the real stdout to a private fd and points fd 1 at stderr, so a stray `println!` (or a child process
/// inheriting stdout) can never corrupt the protocol stream.
pub fn take_stdout() -> Box<dyn Write + Send> {
    unsafe {
        let fd = libc::dup(1);
        if fd < 0 {
            return Box::new(std::io::stdout());
        }
        libc::dup2(2, 1);
        Box::new(std::fs::File::from_raw_fd(fd))
    }
}

impl Link {
    pub fn new(out: Box<dyn Write + Send>) -> Link {
        Link { out: Mutex::new(out), next_id: AtomicU64::new(1), pending: Mutex::new(HashMap::new()) }
    }

    pub fn send(&self, msg: &Value) {
        let mut line = serde_json::to_vec(msg).unwrap_or_default();
        line.push(b'\n');
        let mut out = self.out.lock().unwrap();
        let _ = out.write_all(&line);
        let _ = out.flush();
    }

    pub fn notify(&self, method: &str, params: Value) {
        self.send(&dg_proto::notification(method, params));
    }

    /// Engine → UI request; waits for the response (or the timeout).
    pub fn call(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = channel();
        self.pending.lock().unwrap().insert(id, tx);
        self.send(&dg_proto::request(id, method, params));
        let r = rx.recv_timeout(timeout).unwrap_or_else(|_| Err("DesktopGlass didn't answer in time".into()));
        self.pending.lock().unwrap().remove(&id);
        r
    }

    /// A response from the UI to one of our requests.
    fn resolve(&self, msg: &Value) {
        let Some(id) = msg.get("id").and_then(|i| i.as_u64()) else { return };
        let Some(tx) = self.pending.lock().unwrap().remove(&id) else { return };
        let r = match msg.get("error") {
            Some(e) => Err(e.get("message").and_then(|m| m.as_str()).unwrap_or("Error").to_string()),
            None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
        };
        let _ = tx.send(r);
    }

    /// Fails every waiting call (the UI went away).
    fn fail_all(&self) {
        for (_, tx) in self.pending.lock().unwrap().drain() {
            let _ = tx.send(Err("DesktopGlass closed".into()));
        }
    }
}

/// Reads the UI's messages until EOF.
pub fn serve(engine: Arc<Engine>, input: impl BufRead) {
    for line in input.lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => {
                engine.link.send(&dg_proto::error(&Value::Null, -32700, "Parse error"));
                continue;
            }
        };
        let method = msg.get("method").and_then(|m| m.as_str()).map(String::from);
        match method {
            None => engine.link.resolve(&msg), // a response to ui/control
            Some(method) => {
                let id = msg.get("id").cloned();
                let params = msg.get("params").cloned().unwrap_or_else(|| json!({}));
                let shutdown = method == dg_proto::ui_methods::SHUTDOWN;
                let e = engine.clone();
                let work = move || {
                    let r = e.handle_ui(&method, &params);
                    if let Some(id) = id {
                        e.link.send(&match r {
                            Ok(v) => dg_proto::result(&id, v),
                            Err(m) => dg_proto::error(&id, dg_proto::APP_ERROR, &m),
                        });
                    }
                };
                if shutdown {
                    work();
                    break;
                }
                std::thread::spawn(work);
            }
        }
    }
    engine.link.fail_all();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Default)]
    struct Buf(Arc<Mutex<Vec<u8>>>);
    impl Write for Buf {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn lines(b: &Buf) -> Vec<Value> {
        String::from_utf8(b.0.lock().unwrap().clone())
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn hello_and_unknown_method_and_shutdown() {
        let tmp = crate::settings::tests::temp_home("link");
        let buf = Buf::default();
        let engine = Engine::with_settings(Box::new(buf.clone()), crate::settings::Settings::open_at(tmp.path().join("s.json")));
        let input = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"engine/hello\",\"params\":{}}\n\
                     {\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"nope\"}\n\
                     not json\n\
                     {\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"engine/shutdown\"}\n\
                     {\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"engine/hello\"}\n";
        serve(engine, std::io::Cursor::new(input));
        std::thread::sleep(Duration::from_millis(100));
        let out = lines(&buf);
        let by_id = |id: u64| out.iter().find(|m| m["id"] == json!(id)).cloned();
        assert_eq!(by_id(1).unwrap()["result"]["protocol"], json!(dg_proto::PROTOCOL_VERSION));
        assert_eq!(by_id(2).unwrap()["error"]["message"], json!("Unknown method nope"));
        assert!(out.iter().any(|m| m["error"]["code"] == json!(-32700)));
        assert!(by_id(3).unwrap().get("result").is_some());
        assert!(by_id(4).is_none(), "nothing is handled after shutdown");
        drop(tmp);
    }

    #[test]
    fn call_waits_for_the_matching_response() {
        let buf = Buf::default();
        let link = Arc::new(Link::new(Box::new(buf.clone())));
        let l = link.clone();
        let t = std::thread::spawn(move || l.call("ui/control", json!({"method": "browser.tabs"}), Duration::from_secs(5)));
        std::thread::sleep(Duration::from_millis(50));
        let sent = lines(&buf);
        let id = sent[0]["id"].as_u64().unwrap();
        link.resolve(&json!({"jsonrpc": "2.0", "id": id + 100, "result": "wrong"}));
        link.resolve(&json!({"jsonrpc": "2.0", "id": id, "result": ["tab"]}));
        assert_eq!(t.join().unwrap().unwrap(), json!(["tab"]));

        let l = link.clone();
        let t = std::thread::spawn(move || l.call("ui/control", json!({}), Duration::from_secs(5)));
        std::thread::sleep(Duration::from_millis(50));
        let id = lines(&buf)[1]["id"].as_u64().unwrap();
        link.resolve(&json!({"jsonrpc": "2.0", "id": id, "error": {"code": 1, "message": "No tab"}}));
        assert_eq!(t.join().unwrap(), Err("No tab".into()));
    }
}
