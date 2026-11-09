//! The file tools: they read and write the Desktop directly and work without the app.
//! Every path is confined to the Desktop (symlinks resolved first), every value written into description.md is
//! forced onto one line, and nothing is moved, renamed or deleted.

use crate::pyjson::{self, disp, or, py_float, py_int, py_iter, slice_end};
use crate::Server;
use dg_core::description::{self, Description, Entry};
use dg_core::py::{self, py_str, read_head, strip, truthy};
use dg_core::rules::{self, Kind, DESC, LOG_DIR};
use dg_core::{assemble, paths, prefs};
use serde_json::{Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

pub type Args = Map<String, Value>;

/// `args[key]` (a KeyError shows as `'key'`).
pub fn req<'a>(args: &'a Args, key: &str) -> Result<&'a Value, String> {
    args.get(key).ok_or_else(|| pyjson::repr_str(key))
}

/// Python's OSError text: `[Errno 13] Permission denied: '/path'`.
pub fn os_err(e: &std::io::Error, path: &Path) -> String {
    match e.raw_os_error() {
        Some(n) => {
            let msg = unsafe { std::ffi::CStr::from_ptr(libc::strerror(n)) }.to_string_lossy().into_owned();
            format!("[Errno {n}] {msg}: {}", pyjson::repr_str(&path.to_string_lossy()))
        }
        None => e.to_string(),
    }
}

/// `Path.read_text(encoding="utf-8", errors="replace")`: invalid bytes become U+FFFD, newlines are universal.
fn read_text(p: &Path) -> Result<String, String> {
    let bytes = fs::read(p).map_err(|e| os_err(&e, p))?;
    let s = String::from_utf8_lossy(&bytes);
    Ok(if s.contains('\r') { s.replace("\r\n", "\n").replace('\r', "\n") } else { s.into_owned() })
}

/// `one_line(text, limit)` on a JSON value: `str(text or "")`, whitespace collapsed.
fn one_line(v: &Value, limit: Option<usize>) -> String {
    let s = if truthy(v) { py_str(v) } else { String::new() };
    description::one_line(&s, limit)
}

/// Desktop-relative, slash-separated (`""` for the root).
fn rel_of(root: &Path, p: &Path) -> String {
    p.strip_prefix(root).map(|r| r.to_string_lossy().into_owned()).unwrap_or_default()
}

fn depth_of(rel: &str) -> usize {
    if rel.is_empty() { 0 } else { rel.split('/').count() }
}

fn parse_at(folder: &Path, limit: usize) -> Description {
    description::parse(&read_head(&folder.join(DESC), limit))
}

fn nonempty(s: &Option<String>) -> Option<&str> {
    s.as_deref().filter(|s| !s.is_empty())
}

/// Python's `str.lower()` (close enough: Rust's full Unicode lower-casing).
fn lower(s: &str) -> String {
    s.to_lowercase()
}

/// `re.search(r"·\s*(.+?)\s+—\s+(.*)$", line)`: (folder, question), both still to be stripped.
pub fn question_parts(line: &str) -> Option<(String, String)> {
    let c: Vec<char> = line.chars().collect();
    let ws = |i: usize| i < c.len() && py::is_space(c[i]);
    let run = |mut i: usize| {
        while ws(i) {
            i += 1;
        }
        i
    };
    for start in (0..c.len()).filter(|&i| c[i] == '·') {
        let after = start + 1;
        let full = run(after);
        // \s* is greedy: try the longest run first, then give characters back.
        for s0 in (after..=full).rev() {
            // (.+?) is lazy: the first end k (at least one character) where \s+—\s+ follows.
            for k in s0 + 1..c.len() {
                if !ws(k) {
                    continue;
                }
                let dash = run(k);
                if dash < c.len() && c[dash] == '—' && ws(dash + 1) {
                    let rest = run(dash + 1);
                    let g1: String = c[s0..k].iter().collect();
                    let g2: String = c[rest..].iter().collect();
                    return Some((g1, g2));
                }
            }
        }
    }
    None
}

impl Server {
    fn log(&self) -> PathBuf {
        self.root.join(LOG_DIR)
    }

    fn questions_file(&self) -> PathBuf {
        self.log().join("questions.md")
    }

    /// `safe_path`: a Desktop-relative path (from a tool argument), resolved and confined to the Desktop.
    pub fn safe_path(&self, rel: Option<&Value>) -> Result<PathBuf, String> {
        let rel = match or(rel) {
            None => String::new(),
            Some(Value::String(s)) => s.clone(),
            Some(other) => return Err(format!("'{}' object has no attribute 'strip'", pyjson::type_name(other))),
        };
        self.safe_rel(&rel)
    }

    pub fn safe_rel(&self, rel: &str) -> Result<PathBuf, String> {
        paths::confine(&self.root, strip(rel)).map_err(str::to_string)
    }

    /// `inventory.commit` with its status line sent to stderr (stdout belongs to the protocol).
    fn commit_quiet(&self) -> Result<(), String> {
        match assemble::commit(&self.root) {
            Ok(status) => {
                eprintln!("{status}");
                Ok(())
            }
            Err(e) => Err(os_err(&e, &self.log())),
        }
    }

    fn write_desc(&self, folder: &Path, d: &Description) -> Result<(), String> {
        let names = rules::listdir(folder);
        let entries: Vec<Entry> = names.iter().map(|n| Entry { name: n, is_dir: rules::is_dir(&folder.join(n)) }).collect();
        let name = folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let path = folder.join(DESC);
        fs::write(&path, d.render(&name, &entries)).map_err(|e| os_err(&e, &path))
    }

    pub fn tool_overview(&self, _: &Args) -> Result<String, String> {
        let index = self.log().join("desktop-index.md");
        if index.exists() {
            return read_text(&index);
        }
        Ok("The Desktop hasn't been indexed yet. Run reindex_desktop, or open DesktopGlass and click Re-index.".into())
    }

    pub fn tool_list_folder(&self, args: &Args) -> Result<String, String> {
        let folder = self.safe_path(args.get("path"))?;
        if !folder.is_dir() {
            return Err(format!("Not a folder: {}", disp(args.get("path"))));
        }
        let d = parse_at(&folder, 50_000);
        let rel = if folder == self.root { String::new() } else { rel_of(&self.root, &folder) };
        let name = folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let mut lines = vec![
            format!("# {}", nonempty(&d.title).unwrap_or(&name)),
            format!("Path: Desktop/{rel}").trim_end_matches('/').to_string(),
        ];
        if let Some(s) = nonempty(&d.summary) {
            lines.push(format!("Summary: {s}"));
        }
        if !d.tags.is_empty() {
            lines.push(format!("Tags: {}", d.tags.join(", ")));
        }
        lines.push(String::new());
        let depth = if rel.is_empty() { 1 } else { depth_of(&rel) + 1 };
        for n in rules::listdir(&folder) {
            let p = folder.join(&n);
            let kind = rules::kind(&p, depth);
            let slash = if rules::is_dir(&p) { "/" } else { "" };
            let blurb = d.blurbs.get(&n).filter(|b| !b.is_empty()).map(|b| format!(" — {b}")).unwrap_or_default();
            lines.push(format!("- {n}{slash} [{}]{blurb}", kind.as_str()));
        }
        if !d.notes.is_empty() {
            lines.push(String::new());
            lines.push(format!("Notes: {}", d.notes.join(" ")));
        }
        Ok(lines.join("\n"))
    }

    pub fn tool_search(&self, args: &Args) -> Result<String, String> {
        let query = args.get("query").map(py_str).unwrap_or_default();
        let lowered = lower(&query);
        let terms: Vec<&str> = lowered.split(py::is_space).filter(|t| !t.is_empty()).collect();
        if terms.is_empty() {
            return Err("Empty query".into());
        }
        let limit = py_int(or(args.get("limit")).unwrap_or(&Value::from(30)))?;
        let skip = prefs::load(&self.root).skip;
        let mut hits: Vec<(u8, String)> = Vec::new();
        for c in rules::containers(&self.root, &skip) {
            let d = parse_at(&c.path, 50_000);
            let summary = nonempty(&d.summary).unwrap_or("");
            let folder_hay = lower(&[c.rel.as_str(), summary, &d.tags.join(" ")].join(" "));
            if terms.iter().all(|t| folder_hay.contains(t)) {
                hits.push((3, format!("{}/ — {summary}", c.rel)));
            }
            for n in rules::listdir(&c.path) {
                let b = d.blurbs.get(&n).map(String::as_str).unwrap_or("");
                let hay = lower(&format!("{n} {b}"));
                if terms.iter().all(|t| hay.contains(t)) {
                    let score = if lower(&n).contains(terms[0]) { 2 } else { 1 };
                    let slash = if rules::is_dir(&c.path.join(&n)) { "/" } else { "" };
                    let tail = if b.is_empty() { String::new() } else { format!(" — {b}") };
                    hits.push((score, format!("{}/{n}{slash}{tail}", c.rel)));
                }
            }
        }
        hits.sort_by_key(|h| std::cmp::Reverse(h.0)); // stable, like Python's sort
        if hits.is_empty() {
            return Ok(format!("Nothing on the Desktop matches “{}”.", disp(args.get("query"))));
        }
        let end = slice_end(hits.len(), limit);
        Ok(hits[..end].iter().map(|h| format!("- {}", h.1)).collect::<Vec<_>>().join("\n"))
    }

    pub fn tool_get_description(&self, args: &Args) -> Result<String, String> {
        let path = req(args, "path")?;
        let folder = self.safe_path(Some(path))?;
        let f = folder.join(DESC);
        if !f.exists() {
            return Ok(format!("{} has no description.md yet.", py_str(path)));
        }
        read_text(&f)
    }

    /// `tool_status`: the file side of index_status.
    pub fn tool_status(&self, _: &Args) -> Result<String, String> {
        let status = assemble::status_line(&self.root);
        let index = self.log().join("desktop-index.md");
        let when = match fs::metadata(&index) {
            Ok(m) => py::strftime_local("%b %d, %Y %I:%M %p", Some(py::mtime_float(&m))),
            Err(_) => String::new(),
        };
        let p = prefs::load(&self.root);
        let missing: Vec<String> = rules::containers(&self.root, &p.skip)
            .into_iter()
            .filter(|c| nonempty(&parse_at(&c.path, 20_000).summary).is_none())
            .map(|c| c.rel)
            .collect();
        let qtext = read_head(&self.questions_file(), 100_000);
        let questions: Vec<&str> = py::splitlines(&qtext).into_iter().map(strip).filter(|l| l.starts_with("- [ ]")).collect();
        let mut prefs_json = Map::new();
        prefs_json.insert("version".into(), p.version.clone());
        prefs_json.insert("style".into(), p.style.clone());
        prefs_json.insert("private".into(), Value::from(p.private.clone()));
        prefs_json.insert("skip".into(), Value::from(p.skip.clone()));
        prefs_json.insert("flagStatus".into(), p.flag_status.clone());
        prefs_json.insert("notes".into(), p.notes.clone());
        let mut out = vec![status, format!("Index rebuilt: {}", if when.is_empty() { "never" } else { &when })];
        if !missing.is_empty() {
            out.push(format!("Undescribed: {}", missing[..missing.len().min(30)].join(", ")));
        }
        if !questions.is_empty() {
            out.push("Open questions:".into());
            out.extend(questions.iter().take(20).map(|s| s.to_string()));
        }
        out.push(format!("Preferences: {}", pyjson::dumps(&Value::Object(prefs_json), None, true)));
        Ok(out.join("\n"))
    }

    pub fn tool_recent(&self, args: &Args) -> Result<String, String> {
        let days = py_float(or(args.get("days")).unwrap_or(&Value::from(7)))?;
        let limit = py_int(or(args.get("limit")).unwrap_or(&Value::from(40)))?;
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
        let cutoff = now - days * 86400.0;
        let skip = prefs::load(&self.root).skip;
        let mut folders: Vec<PathBuf> = vec![self.root.clone()];
        folders.extend(rules::containers(&self.root, &skip).into_iter().map(|c| c.path));
        let mut rows: Vec<(f64, String, String)> = Vec::new();
        for c in &folders {
            let at_root = *c == self.root;
            let d = if at_root { Description::default() } else { parse_at(c, 50_000) };
            for n in rules::listdir(c) {
                let p = c.join(&n);
                let Ok(st) = fs::metadata(&p) else { continue };
                let m = py::mtime_float(&st);
                if m >= cutoff && !(at_root && rules::kind(&p, 1) == Kind::Group) {
                    let slash = if rules::is_dir(&p) { "/" } else { "" };
                    let rel = format!("{}{slash}", rel_of(&self.root, &p));
                    rows.push((m, rel, d.blurbs.get(&n).cloned().unwrap_or_default()));
                }
            }
        }
        rows.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
        if rows.is_empty() {
            return Ok(format!("Nothing on the Desktop changed in the last {} days.", pyjson::format_g(days)));
        }
        let end = slice_end(rows.len(), limit);
        Ok(rows[..end]
            .iter()
            .map(|(m, rel, b)| {
                let tail = if b.is_empty() { String::new() } else { format!(" — {b}") };
                format!("- {}  {rel}{tail}", py::strftime_local("%b %d %H:%M", Some(*m)))
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }

    pub fn tool_update_description(&self, args: &Args) -> Result<String, String> {
        let path = req(args, "path")?;
        let folder = self.safe_path(Some(path))?;
        let rel = if folder == self.root { String::new() } else { rel_of(&self.root, &folder) };
        if folder == self.root || !folder.is_dir() || rules::kind(&folder, depth_of(&rel)) != Kind::Group {
            return Err(format!(
                "{} isn't a described Desktop folder (projects are described by their parent folder)",
                py_str(path)
            ));
        }
        let mut d = parse_at(&folder, 50_000);
        // Every value is forced onto one line: a newline in a summary or blurb would otherwise add headings or
        // bullets to description.md.
        if let Some(s) = or(args.get("summary")) {
            d.summary = Some(one_line(s, Some(160)));
        }
        if let Some(tags) = or(args.get("tags")) {
            let tags: Vec<String> = py_iter(tags)?
                .iter()
                .map(|t| strip(&lower(&one_line(t, None)).replace(',', " ")).to_string())
                .filter(|t| !t.is_empty())
                .take(6)
                .collect();
            d.tags = tags;
        }
        if let Some(notes) = args.get("notes").filter(|v| !v.is_null()) {
            let text = py_str(notes);
            d.notes = py::splitlines(&text)
                .into_iter()
                .filter(|l| !strip(l).is_empty() && !py::lstrip(l).starts_with('#'))
                .map(|l| strip(l).to_string())
                .collect();
        }
        let names: std::collections::HashSet<String> = rules::listdir(&folder).into_iter().collect();
        let mut unknown: Vec<String> = Vec::new();
        if let Some(blurbs) = or(args.get("blurbs")) {
            let Value::Object(blurbs) = blurbs else {
                return Err(format!("'{}' object has no attribute 'items'", pyjson::type_name(blurbs)));
            };
            for (name, blurb) in blurbs {
                let key = name.trim_end_matches('/');
                if names.contains(key) {
                    d.blurbs.insert(key.to_string(), one_line(blurb, Some(160)));
                } else {
                    unknown.push(name.clone());
                }
            }
        }
        self.write_desc(&folder, &d)?;
        self.commit_quiet()?;
        let note = if unknown.is_empty() { String::new() } else { format!(" (no such items: {})", unknown.join(", ")) };
        Ok(format!("Updated {rel}/description.md{note}"))
    }

    pub fn tool_list_questions(&self, _: &Args) -> Result<String, String> {
        let text = read_head(&self.questions_file(), 200_000);
        let qs: Vec<&str> = py::splitlines(&text).into_iter().map(strip).filter(|l| l.starts_with("- [ ]")).collect();
        Ok(if qs.is_empty() { "No open questions.".into() } else { qs.join("\n") })
    }

    pub fn tool_resolve_question(&self, args: &Args) -> Result<String, String> {
        let want = match req(args, "question")? {
            Value::String(s) => lower(strip(s)),
            other => return Err(format!("'{}' object has no attribute 'strip'", pyjson::type_name(other))),
        };
        let file = self.questions_file();
        let text = read_head(&file, 200_000);
        let mut lines: Vec<String> = py::splitlines(&text).into_iter().map(str::to_string).collect();
        for i in 0..lines.len() {
            let line = lines[i].clone();
            if !(strip(&line).starts_with("- [ ]") && lower(&line).contains(&want)) {
                continue;
            }
            let answer = one_line(req(args, "answer")?, Some(500));
            if let Some((folder_rel, question)) = question_parts(&line) {
                let folder_rel = strip(&folder_rel).trim_matches('/').to_string();
                let question = strip(&question).to_string();
                if let Ok(folder) = self.safe_rel(&folder_rel) {
                    let target = if rules::is_dir(&folder) && folder.join(DESC).exists() {
                        folder.clone()
                    } else {
                        folder.parent().map(Path::to_path_buf).unwrap_or_else(|| folder.clone())
                    };
                    let mut d = parse_at(&target, 50_000);
                    d.notes.push(format!("Q: {question} A: {answer}"));
                    // Deliberately stricter than mcp_server.py: the parent of the Desktop root (a question about
                    // the Desktop itself) is never written to.
                    if target.is_dir() && target != self.root && target.starts_with(&self.root) {
                        self.write_desc(&target, &d)?;
                    }
                }
            }
            lines[i] = line.replacen("- [ ]", "- [x]", 1) + &format!(" → {answer}");
            fs::write(&file, lines.join("\n") + "\n").map_err(|e| os_err(&e, &file))?;
            self.commit_quiet()?;
            return Ok("Saved. The answer is in that folder's Notes, so future indexing will use it.".into());
        }
        Err("No open question matches that text — call list_questions".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn question_regex() {
        assert_eq!(
            question_parts("- [ ] 2026-11-04 · Projects — What is web?"),
            Some(("Projects".into(), "What is web?".into()))
        );
        assert_eq!(question_parts("- [ ] 2026 · A — B — C"), Some(("A".into(), "B — C".into())));
        assert_eq!(question_parts("- [ ] no dot — x"), None);
        assert_eq!(question_parts("- [ ] ·  — x"), Some((" ".into(), "x".into())));
        assert_eq!(question_parts("- [ ] · a —x"), None);
    }
}
