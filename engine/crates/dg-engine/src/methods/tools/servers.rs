//! Local web servers (dg-core `local_servers`, ported from LocalServers.swift).
//!
//! - `servers/list {exclude?: [port]}` (UI link) and `servers.list {}` (control channel) →
//!   `[{port, process, pid, title, status, url}]`. The engine's own control port is always left out (the Swift app
//!   excluded it too: `artifacts.excludedPorts`).

use crate::Engine;
use dg_core::local_servers;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::sync::Arc;

pub fn handle(engine: &Arc<Engine>, method: &str, p: &Value) -> Option<Result<Value, String>> {
    match method {
        "servers/list" | "servers.list" => Some(Ok(local_servers::scan_json(&excluded(engine, p)))),
        _ => None,
    }
}

fn excluded(engine: &Arc<Engine>, p: &Value) -> HashSet<u16> {
    let mut ports: HashSet<u16> = p
        .get("exclude")
        .and_then(|e| e.as_array())
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_u64().and_then(|n| u16::try_from(n).ok()))
        .collect();
    let own = engine.control_port.load(Ordering::SeqCst);
    if own > 0 {
        ports.insert(own);
    }
    ports
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{engine, temp_dir};
    use super::*;
    use serde_json::json;

    #[test]
    fn exclusions_include_the_control_port() {
        let dir = temp_dir("servers");
        let e = engine(&dir);
        e.control_port.store(4321, Ordering::SeqCst);
        let ports = excluded(&e, &json!({"exclude": [80, 70000, "x"]}));
        assert_eq!(ports, HashSet::from([80, 4321]));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
