//! Desktop file watching (FSEvents via `notify`) → debounced `desktop/changed` notifications, replacing polling.
//! Our own bookkeeping files (`_Organize Log/`, `.DS_Store`) don't count as changes.

use crate::Engine;
use notify::{RecursiveMode, Watcher};
use serde_json::json;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

const QUIET: Duration = Duration::from_millis(400);
const MAX_WAIT: Duration = Duration::from_secs(2);

/// True for paths whose changes the UI should hear about.
pub fn interesting(root: &Path, path: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(root) else { return false };
    let mut parts = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned());
    match parts.next() {
        None => true,
        Some(first) if first == "_Organize Log" => false,
        Some(_) => !rel.file_name().map(|n| n == ".DS_Store").unwrap_or(false),
    }
}

/// Starts watching `root`; the returned watcher must be kept alive.
pub fn start(engine: Arc<Engine>, root: PathBuf) -> Option<notify::RecommendedWatcher> {
    let (tx, rx) = channel::<PathBuf>();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res {
            for p in ev.paths {
                let _ = tx.send(p);
            }
        }
    })
    .ok()?;
    watcher.watch(&root, RecursiveMode::Recursive).ok()?;
    std::thread::spawn(move || {
        let mut batch = BTreeSet::new();
        let mut first: Option<Instant> = None;
        loop {
            match rx.recv_timeout(QUIET) {
                Ok(p) => {
                    if interesting(&root, &p) {
                        batch.insert(p.to_string_lossy().into_owned());
                        first.get_or_insert_with(Instant::now);
                    }
                    if first.map(|t| t.elapsed() < MAX_WAIT).unwrap_or(true) {
                        continue;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            if !batch.is_empty() {
                let paths: Vec<String> = std::mem::take(&mut batch).into_iter().collect();
                engine.link.notify(dg_proto::notify::DESKTOP_CHANGED, json!({"paths": paths}));
            }
            first = None;
        }
    });
    Some(watcher)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_bookkeeping() {
        let root = Path::new("/u/Desktop");
        assert!(interesting(root, Path::new("/u/Desktop/Projects/a.txt")));
        assert!(interesting(root, Path::new("/u/Desktop/Projects/description.md")));
        assert!(!interesting(root, Path::new("/u/Desktop/_Organize Log/desktop-index.md")));
        assert!(!interesting(root, Path::new("/u/Desktop/Docs/.DS_Store")));
        assert!(!interesting(root, Path::new("/elsewhere/x")));
    }

    #[test]
    fn reports_changes_debounced() {
        let home = crate::settings::tests::temp_home("watch");
        let root = home.path().join("Desktop");
        std::fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        #[derive(Clone, Default)]
        struct Buf(Arc<std::sync::Mutex<Vec<u8>>>);
        impl std::io::Write for Buf {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let buf = Buf::default();
        let engine = Engine::with_settings(Box::new(buf.clone()), crate::settings::Settings::open_at(home.path().join("s.json")));
        let _w = start(engine, root.clone()).expect("watcher");
        std::thread::sleep(Duration::from_millis(300));
        std::fs::write(root.join("a.txt"), "x").unwrap();
        std::fs::write(root.join("b.txt"), "y").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && !String::from_utf8_lossy(&buf.0.lock().unwrap()).contains("b.txt") {
            std::thread::sleep(Duration::from_millis(100));
        }
        let text = String::from_utf8_lossy(&buf.0.lock().unwrap()).into_owned();
        assert!(text.contains("desktop/changed") && text.contains("a.txt") && text.contains("b.txt"), "{text}");
    }
}
