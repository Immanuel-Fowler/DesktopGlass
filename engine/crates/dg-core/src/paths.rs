//! Path handling: `Path(x).expanduser().resolve()` for the Desktop root, and confinement of Desktop-relative
//! paths to that root (symlinks resolved first, so a link can't lead outside).

use std::path::{Component, Path, PathBuf};

/// `~` and `~/…` expanded from `$HOME`, like `Path.expanduser()`.
pub fn expanduser(p: &str) -> PathBuf {
    if p == "~" || p.starts_with("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(p.get(2..).unwrap_or(""));
        }
    }
    PathBuf::from(p)
}

/// `os.path.realpath` (non-strict): resolves symlinks of the parts that exist and keeps the rest, folding `..`.
pub fn resolve(p: &Path) -> PathBuf {
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")).join(p)
    };
    let mut out = PathBuf::from("/");
    let mut hops = 0;
    let mut pending: Vec<std::ffi::OsString> =
        abs.components().rev().filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_os_string()),
            Component::ParentDir => Some("..".into()),
            _ => None,
        }).collect();
    while let Some(part) = pending.pop() {
        if part == ".." {
            out.pop();
            continue;
        }
        let next = out.join(&part);
        match std::fs::read_link(&next) {
            Ok(target) if hops < 40 => {
                hops += 1;
                if target.is_absolute() {
                    out = PathBuf::from("/");
                }
                for c in target.components().rev() {
                    match c {
                        Component::Normal(s) => pending.push(s.to_os_string()),
                        Component::ParentDir => pending.push("..".into()),
                        _ => {}
                    }
                }
            }
            _ => out = next,
        }
    }
    out
}

/// The Desktop root a command was given: expanded and resolved.
pub fn desktop_root(arg: &str) -> PathBuf {
    resolve(&expanduser(arg))
}

/// `safe_path`: a Desktop-relative path, resolved, that must stay inside `root` (which must already be resolved).
pub fn confine(root: &Path, rel: &str) -> Result<PathBuf, &'static str> {
    let rel = rel.trim().trim_matches('/');
    let p = resolve(&root.join(rel));
    if p == root || p.starts_with(root) {
        Ok(p)
    } else {
        Err("Path must be inside the Desktop")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confine_rejects_escapes() {
        let root = resolve(&std::env::temp_dir());
        assert!(confine(&root, "../etc").is_err());
        assert!(confine(&root, "/../../etc").is_err());
        assert_eq!(confine(&root, "a/../b").unwrap(), root.join("b"));
        assert_eq!(confine(&root, "").unwrap(), root);
    }

    #[test]
    fn resolve_folds_dots_and_links() {
        assert_eq!(resolve(Path::new("/a/b/../c")), PathBuf::from("/a/c"));
        assert_eq!(resolve(Path::new("/tmp/x-dg-nonexistent")), resolve(Path::new("/tmp")).join("x-dg-nonexistent"));
    }
}
