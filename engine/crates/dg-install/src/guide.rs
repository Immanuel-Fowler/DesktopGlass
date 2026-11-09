//! The marked DesktopGlass block in an agent CLI's global instructions file (CLAUDE.md, AGENTS.md, GEMINI.md, …).
//! Everything outside the markers is left exactly as it was; uninstall removes only the block.

use crate::Error;
use std::fs;
use std::path::{Path, PathBuf};

pub const BEGIN: &str = "<!-- desktopglass:start -->";
pub const END: &str = "<!-- desktopglass:end -->";
pub const GUIDE: &str = "<!-- desktopglass:start -->
## DesktopGlass

This Mac runs DesktopGlass, which keeps an organized index of ~/Desktop available through the `desktopglass` MCP
server. To find something on the Desktop, use `desktop_overview` and `search_desktop` before listing folders.
<!-- desktopglass:end -->";

/// (start, end) byte offsets of the marked block, or None (also for an unterminated block).
pub fn guide_block(text: &str) -> Option<(usize, usize)> {
    let a = text.find(BEGIN)?;
    let b = text[a..].find(END)? + a;
    Some((a, b + END.len()))
}

pub fn with_guide(text: &str) -> String {
    if let Some((a, b)) = guide_block(text) {
        return format!("{}{}{}", &text[..a], GUIDE, &text[b..]);
    }
    let head = if text.trim().is_empty() { String::new() } else { format!("{}\n\n", text.trim_end_matches('\n')) };
    format!("{head}{GUIDE}\n")
}

pub fn without_guide(text: &str) -> String {
    let Some((a, b)) = guide_block(text) else { return text.to_string() };
    let before = text[..a].trim_end_matches('\n');
    let after = text[b..].trim_start_matches('\n');
    if before.is_empty() {
        return after.to_string();
    }
    if after.is_empty() { format!("{before}\n") } else { format!("{before}\n\n{after}") }
}

/// Python's text-mode read: universal newlines.
pub(crate) fn read_text(path: &Path, lossy: bool) -> Result<String, Error> {
    let bytes = fs::read(path).map_err(|e| Error::Other(e.to_string()))?;
    let text = if lossy {
        String::from_utf8_lossy(&bytes).into_owned()
    } else {
        String::from_utf8(bytes).map_err(|e| Error::Parse(e.to_string()))?
    };
    Ok(text.replace("\r\n", "\n").replace('\r', "\n"))
}

/// The DesktopGlass block in one agent's global instructions file.
#[derive(Clone, Debug)]
pub struct Guide {
    pub path: PathBuf,
}

impl Guide {
    pub fn new(path: PathBuf) -> Guide {
        Guide { path }
    }

    pub fn installed(&self) -> bool {
        self.path.exists() && read_text(&self.path, true).map(|t| guide_block(&t).is_some()).unwrap_or(false)
    }

    pub fn install(&self) -> Result<(), Error> {
        let text = if self.path.exists() { read_text(&self.path, false)? } else { String::new() };
        let new = with_guide(&text);
        if new != text {
            if let Some(d) = self.path.parent() {
                fs::create_dir_all(d).map_err(|e| Error::Other(e.to_string()))?;
            }
            fs::write(&self.path, new).map_err(|e| Error::Other(e.to_string()))?;
        }
        Ok(())
    }

    pub fn uninstall(&self) -> Result<(), Error> {
        if !self.path.exists() {
            return Ok(());
        }
        let text = read_text(&self.path, false)?;
        let new = without_guide(&text);
        if new == text {
            return Ok(());
        }
        let r = if new.trim().is_empty() { fs::remove_file(&self.path) } else { fs::write(&self.path, new) };
        r.map_err(|e| Error::Other(e.to_string()))
    }
}
