//! `_Organize Log/.reindex.lock`: a directory lock (atomic `mkdir`), stale after 30 minutes, removed on drop.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

pub const STALE_AFTER: Duration = Duration::from_secs(30 * 60);

pub struct Lock {
    dir: PathBuf,
}

impl Lock {
    /// `mkdir "$LOCK"`; if it exists and is older than 30 minutes, take it over (like reindex.sh, a failed
    /// takeover still proceeds). None means another run holds it.
    pub fn acquire(dir: &Path) -> Option<Lock> {
        if fs::create_dir(dir).is_err() {
            if !is_stale(dir) {
                return None;
            }
            let _ = fs::remove_dir(dir);
            let _ = fs::create_dir(dir);
        }
        Some(Lock { dir: dir.to_path_buf() })
    }
}

/// `find "$LOCK" -maxdepth 0 -mmin +30`
fn is_stale(dir: &Path) -> bool {
    let Ok(m) = fs::metadata(dir).and_then(|m| m.modified()) else { return false };
    SystemTime::now().duration_since(m).map(|age| age > STALE_AFTER).unwrap_or(false)
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_then_released() {
        let base = std::env::temp_dir().join(format!("dg-lock-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let dir = base.join(".reindex.lock");
        let a = Lock::acquire(&dir).expect("first");
        assert!(Lock::acquire(&dir).is_none(), "second is busy");
        drop(a);
        assert!(!dir.exists());
        assert!(Lock::acquire(&dir).is_some());
        let _ = fs::remove_dir_all(&base);
    }
}
