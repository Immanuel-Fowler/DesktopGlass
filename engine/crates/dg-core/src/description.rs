//! description.md: parse and render, byte-compatible with the files the app, the MCP server and the indexer
//! write today.
//!
//! Format: `# Title`, `> summary`, `Tags: a, b`, `## Contents` with bullets `- **name/** — blurb`, `## Notes`.

use crate::py::{self, is_space, strip};
use std::collections::HashMap;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Description {
    pub title: Option<String>,
    pub summary: Option<String>,
    pub tags: Vec<String>,
    pub blurbs: HashMap<String, String>,
    pub notes: Vec<String>,
}

/// Matches `^[-*]\s+\*\*(.+?)\*\*\s*(?:[—–:-]\s*)?(.*)$` on an already-stripped line.
fn parse_bullet(line: &str) -> Option<(String, String)> {
    let mut chars = line.chars();
    if !matches!(chars.next(), Some('-' | '*')) {
        return None;
    }
    let after_marker = chars.as_str();
    let rest = after_marker.trim_start_matches(is_space);
    if rest.len() == after_marker.len() {
        return None; // needs at least one space
    }
    let body = rest.strip_prefix("**")?;
    // (.+?) then \*\* : the first "**" that leaves a non-empty name.
    let first = body.chars().next()?;
    let end = body[first.len_utf8()..].find("**")? + first.len_utf8();
    let name = &body[..end];
    let mut tail = body[end + 2..].trim_start_matches(is_space);
    if let Some(c) = tail.chars().next() {
        if matches!(c, '—' | '–' | ':' | '-') {
            tail = tail[c.len_utf8()..].trim_start_matches(is_space);
        }
    }
    Some((strip(name).trim_end_matches('/').to_string(), strip(tail).to_string()))
}

/// `inventory.parse_desc`
pub fn parse(text: &str) -> Description {
    let mut d = Description::default();
    let mut section = String::new();
    for raw in py::splitlines(text) {
        let line = strip(raw);
        if let Some(h) = line.strip_prefix("## ") {
            section = strip(h).to_lowercase();
            continue;
        }
        if d.title.is_none() {
            if let Some(t) = line.strip_prefix("# ") {
                d.title = Some(strip(t).to_string());
                continue;
            }
        }
        if d.summary.is_none() {
            if let Some(s) = line.strip_prefix("> ") {
                d.summary = Some(strip(s).to_string());
                continue;
            }
        }
        if line.to_lowercase().starts_with("tags:") {
            // "tags:" is ASCII, and lower-casing never turns non-ASCII into it, so the prefix is 5 bytes.
            d.tags = line[5..].split(',').map(|t| strip(t).to_lowercase()).filter(|t| !t.is_empty()).collect();
            continue;
        }
        if section == "contents" {
            if let Some((name, blurb)) = parse_bullet(line) {
                d.blurbs.insert(name, blurb);
                continue;
            }
        }
        if section == "notes" && !line.is_empty() {
            d.notes.push(line.to_string());
        }
    }
    d
}

/// `one_line`: collapses whitespace (newlines included) so a value can't break the line-based format.
pub fn one_line(text: &str, limit: Option<usize>) -> String {
    let s = text.split(is_space).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ");
    match limit {
        Some(n) if n > 0 => py::take_chars(&s, n).to_string(),
        _ => s,
    }
}

/// One line of `## Contents`: a visible folder entry.
pub struct Entry<'a> {
    pub name: &'a str,
    pub is_dir: bool,
}

/// `- **name/** — blurb` (no dash when the blurb is empty).
pub fn bullet(name: &str, is_dir: bool, blurb: Option<&str>) -> String {
    let slash = if is_dir { "/" } else { "" };
    match blurb {
        Some(b) if !b.is_empty() => format!("- **{name}{slash}** — {b}"),
        _ => format!("- **{name}{slash}**"),
    }
}

impl Description {
    /// The file `mcp_server.write_desc` writes: every visible entry of the folder in listing order.
    pub fn render(&self, folder_name: &str, entries: &[Entry]) -> String {
        let title = self.title.as_deref().filter(|t| !t.is_empty()).unwrap_or(folder_name);
        let summary = self.summary.as_deref().filter(|s| !s.is_empty()).unwrap_or("(?)");
        let mut lines = vec![format!("# {title}"), format!("> {summary}")];
        if !self.tags.is_empty() {
            lines.push(format!("Tags: {}", self.tags.join(", ")));
        }
        lines.push(String::new());
        lines.push("## Contents".into());
        for e in entries {
            lines.push(bullet(e.name, e.is_dir, self.blurbs.get(e.name).map(String::as_str)));
        }
        if !self.notes.is_empty() {
            lines.push(String::new());
            lines.push("## Notes".into());
            lines.extend(self.notes.iter().cloned());
        }
        lines.join("\n") + "\n"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bullets() {
        assert_eq!(parse_bullet("- **a/** — b"), Some(("a".into(), "b".into())));
        assert_eq!(parse_bullet("* **notes.txt**: scratch"), Some(("notes.txt".into(), "scratch".into())));
        assert_eq!(parse_bullet("- **a-b.md** - hyphen"), Some(("a-b.md".into(), "hyphen".into())));
        assert_eq!(parse_bullet("- **empty**"), Some(("empty".into(), "".into())));
        assert_eq!(parse_bullet("-**x** y"), None);
        assert_eq!(parse_bullet("- missing bold — x"), None);
        assert_eq!(parse_bullet("- *****x"), Some(("*".into(), "x".into())));
        assert_eq!(parse_bullet("- **a** —— b"), Some(("a".into(), "— b".into())));
    }

    #[test]
    fn one_line_collapses_newlines() {
        assert_eq!(one_line("a\n## Notes\n- **x**  y", None), "a ## Notes - **x** y");
        assert_eq!(one_line("  long text  ", Some(4)), "long");
        assert_eq!(one_line("", None), "");
    }

    #[test]
    fn render_round_trips_a_canonical_file() {
        let text = "# Projects\n> Code\nTags: swift, web\n\n## Contents\n- **App/** — the app\n- **notes.txt**\n\n## Notes\nArchived.\n";
        let d = parse(text);
        let entries = [Entry { name: "App", is_dir: true }, Entry { name: "notes.txt", is_dir: false }];
        assert_eq!(d.render("Projects", &entries), text);
    }
}
