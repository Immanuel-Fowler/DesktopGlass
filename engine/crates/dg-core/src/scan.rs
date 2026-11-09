//! The compact, token-cheap scan that tells the indexer which folders need work. Its JSON is the contract with
//! SKILL.md and every agent, so the shape, key order and caps match `inventory.py` exactly.

use crate::description::{self, Description};
use crate::manifest;
use crate::prefs;
use crate::py::{self, read_head, strip, take_chars};
use crate::rules::{self, Kind, DESC, MAX_ENTRIES, NOTABLE, TEXT_EXT};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

/// First `n` meaningful lines of a README or text file, skipping markup and code.
pub fn first_lines(text: &str, n: usize, width: usize) -> String {
    const SKIP: &[&str] = &["![", "[!", "<", "---", "|", "import ", "from ", "#!", "{", "}", "\""];
    let mut out: Vec<&str> = Vec::new();
    for raw in py::splitlines(text) {
        let line = strip(py::strip_chars(strip(raw), "#*>`_ \t"));
        if py::char_len(line) < 4 || SKIP.iter().any(|p| line.starts_with(p)) {
            continue;
        }
        out.push(line);
        if out.len() >= n {
            break;
        }
    }
    take_chars(&out.join(" / "), width).to_string()
}

/// `human(n)`: 512B, 3KB, 1.2TB.
pub fn human(size: u64) -> String {
    let mut n = size as f64;
    for unit in ["B", "KB", "MB", "GB"] {
        if n < 1024.0 {
            return format!("{n:.0}{unit}");
        }
        n /= 1024.0;
    }
    format!("{n:.1}TB")
}

/// Change signature of one entry: `d<mtime>:<visible count>` for folders, `f<size>:<mtime>` for files, `x` if gone.
pub fn entry_sig(p: &Path) -> String {
    let Ok(st) = fs::metadata(p) else { return "x".into() };
    if rules::is_dir(p) {
        format!("d{}:{}", py::mtime_int(&st), rules::listdir(p).len())
    } else {
        format!("f{}:{}", st.len(), py::mtime_int(&st))
    }
}

fn read_desc(folder: &Path) -> String {
    read_head(&folder.join(DESC), 20_000)
}

fn top(names: &[String]) -> String {
    let first: Vec<&str> = names.iter().take(8).map(String::as_str).collect();
    take_chars(&first.join(","), 160).to_string()
}

/// Stack, git, README and description hints for a project folder.
pub fn project_hints(p: &Path) -> Map<String, Value> {
    let mut h = Map::new();
    let Some(raw) = rules::raw_names(p) else { return h };
    let names: HashSet<&str> = raw.iter().map(String::as_str).collect();
    let has = |n: &str| names.contains(n);
    let mut stack: Vec<String> = Vec::new();
    if has("package.json") {
        let pj = serde_json::from_str::<Value>(&read_head(&p.join("package.json"), 100_000)).unwrap_or(Value::Null);
        let mut deps: Vec<&str> = Vec::new();
        for key in ["dependencies", "devDependencies"] {
            if let Some(Value::Object(o)) = pj.get(key) {
                deps.extend(o.keys().map(String::as_str));
            }
        }
        let mut hits: Vec<&str> = deps.into_iter().filter(|d| NOTABLE.contains(d)).collect();
        hits.sort_unstable();
        hits.dedup();
        hits.truncate(6);
        stack.push(if hits.is_empty() { "node".into() } else { format!("node[{}]", hits.join(",")) });
        if let Some(Value::String(d)) = pj.get("description") {
            let d = strip(d);
            if !d.is_empty() {
                h.insert("desc".into(), json!(take_chars(d, 120)));
            }
        }
    }
    if ["requirements.txt", "pyproject.toml", "setup.py", "main.py", "app.py"].iter().any(|n| has(n)) {
        let text = read_head(&p.join("requirements.txt"), 4000);
        let reqs: Vec<&str> = py::splitlines(&text)
            .into_iter()
            .filter(|l| !strip(l).is_empty() && !py::lstrip(l).starts_with(['#', '-']))
            .map(|l| {
                let s = strip(l);
                s.split(|c: char| "<>=~![;".contains(c) || py::is_space(c)).next().unwrap_or("")
            })
            .collect();
        let shown: Vec<&str> = reqs.iter().take(6).copied().collect();
        stack.push(if reqs.is_empty() { "python".into() } else { format!("python[{}]", shown.join(",")) });
    }
    if has("Package.swift") || raw.iter().any(|n| n.ends_with(".xcodeproj") || n.ends_with(".xcworkspace")) {
        stack.push("swift".into());
    }
    for (marker, label) in
        [("Cargo.toml", "rust"), ("go.mod", "go"), ("pubspec.yaml", "flutter"), ("pom.xml", "java"), ("build.gradle", "gradle")]
    {
        if has(marker) {
            stack.push(label.into());
        }
    }
    if raw.iter().any(|n| n.ends_with(".sln") || n.ends_with(".csproj")) {
        stack.push("dotnet".into());
    }
    if has("index.html") && !has("package.json") {
        stack.push("static-web".into());
    }
    if !stack.is_empty() {
        h.insert("stack".into(), json!(stack.join(" ")));
    }
    if has(".git") {
        h.insert("git".into(), json!(1));
    }
    for r in ["README.md", "readme.md", "README.txt", "README"] {
        if has(r) {
            let line = first_lines(&read_head(&p.join(r), 2000), 2, 160);
            if !line.is_empty() {
                h.insert("readme".into(), json!(line));
            }
            break;
        }
    }
    let mut vis: Vec<String> = raw.iter().filter(|n| rules::visible(n)).cloned().collect();
    rules::sort_lower(&mut vis);
    h.insert("top".into(), json!(top(&vis)));
    h
}

/// One entry for the indexer: `{"n", "k", "mod", ...hints}`. Private folders send names and kinds only.
pub fn describe_entry(p: &Path, depth: usize, private: bool) -> Map<String, Value> {
    let k = rules::kind(p, depth);
    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut e = Map::new();
    e.insert("n".into(), json!(name));
    e.insert("k".into(), json!(k.as_str()));
    if private {
        return e;
    }
    let Ok(st) = fs::metadata(p) else { return e };
    e.insert("mod".into(), json!(py::strftime_local("%Y-%m", Some(py::mtime_float(&st)))));
    match k {
        Kind::File => {
            e.insert("size".into(), json!(human(st.len())));
            let ext = py::suffix(&name).to_lowercase();
            if TEXT_EXT.contains(&ext.as_str()) && st.len() < 2_000_000 {
                let line = first_lines(&read_head(p, 2000), 1, 120);
                if !line.is_empty() {
                    e.insert("head".into(), json!(line));
                }
            }
        }
        Kind::Project => e.extend(project_hints(p)),
        Kind::Group | Kind::Folder => {
            let kids = rules::listdir(p);
            e.insert("items".into(), json!(kids.len()));
            let summary = if k == Kind::Group { description::parse(&read_desc(p)).summary } else { None };
            match summary.filter(|s| !s.is_empty()) {
                Some(s) => e.insert("summary".into(), json!(s)),
                None => e.insert("top".into(), json!(top(&kids))),
            };
        }
        Kind::Package => {}
    }
    e
}

/// Number of unchecked `- [ ]` questions in questions.md.
pub fn open_questions(root: &Path) -> usize {
    let text = read_head(&root.join(rules::LOG_DIR).join("questions.md"), 200_000);
    py::splitlines(&text).iter().filter(|l| strip(l).starts_with("- [ ]")).count()
}

/// `+12 more: 5 .jpg, 4 folder, 3 .pdf` (largest groups first, ties in first-seen order, at most six).
fn more_summary(rest: &[String]) -> String {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for n in rest {
        let ext = py::suffix(n).to_lowercase();
        let key = if ext.is_empty() { "folder".to_string() } else { ext };
        match counts.iter_mut().find(|(k, _)| *k == key) {
            Some((_, c)) => *c += 1,
            None => counts.push((key, 1)),
        }
    }
    counts.sort_by_key(|c| std::cmp::Reverse(c.1)); // stable: ties keep first-seen order
    let parts: Vec<String> = counts.iter().take(6).map(|(k, v)| format!("{v} {k}")).collect();
    format!("+{} more: {}", rest.len(), parts.join(", "))
}

/// `inventory.scan`: folders whose description.md needs work, plus prefs and open questions.
pub fn scan(root: &Path, force: bool) -> Value {
    let old_all = manifest::load(root);
    let empty = Map::new();
    let old_containers = old_all.get("containers").and_then(Value::as_object).unwrap_or(&empty);
    let prefs = prefs::load(root);
    let mut out_containers: Vec<Value> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for c in rules::containers(root, &prefs.skip) {
        seen.insert(c.rel.clone());
        let is_private = rules::under(&c.rel, &prefs.private);
        let names = rules::listdir(&c.path);
        let sigs: HashMap<&str, String> = names.iter().map(|n| (n.as_str(), entry_sig(&c.path.join(n)))).collect();
        let old = old_containers
            .get(&c.rel)
            .and_then(|v| v.get("entries"))
            .and_then(Value::as_object)
            .unwrap_or(&empty);
        let desc_text = read_desc(&c.path);
        let existing: Option<Description> =
            if strip(&desc_text).is_empty() { None } else { Some(description::parse(&desc_text)) };
        let changed: HashSet<&str> = names
            .iter()
            .map(String::as_str)
            .filter(|n| old.get(*n).and_then(Value::as_str) != Some(sigs[n].as_str()))
            .collect();
        let mut gone: Vec<&String> = old.keys().filter(|k| !sigs.contains_key(k.as_str())).collect();
        gone.sort();
        let missing: HashSet<&str> = match &existing {
            Some(d) => names.iter().take(MAX_ENTRIES).map(String::as_str).filter(|n| !d.blurbs.contains_key(*n)).collect(),
            None => HashSet::new(),
        };
        if !force && existing.is_some() && changed.is_empty() && gone.is_empty() && missing.is_empty() {
            continue;
        }

        let mut item = Map::new();
        item.insert("path".into(), json!(c.rel));
        let why = if existing.is_none() { "new" } else if force { "all" } else { "changed" };
        item.insert("why".into(), json!(why));
        if is_private {
            item.insert("private".into(), json!(1));
        }
        if let Some(d) = &existing {
            item.insert("summary".into(), json!(d.summary));
            if !d.tags.is_empty() {
                item.insert("tags".into(), json!(d.tags.join(",")));
            }
            if !d.notes.is_empty() {
                item.insert("notes".into(), json!(take_chars(&d.notes.join(" "), 300)));
            }
        }
        let mut order: Vec<&String> = names.iter().collect();
        // Changed and undescribed entries first, then by name (stable over the listing order).
        let mut keyed: Vec<(bool, String, &String)> = order
            .iter()
            .map(|n| (!changed.contains(n.as_str()) && !missing.contains(n.as_str()), n.to_lowercase(), *n))
            .collect();
        keyed.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
        order = keyed.into_iter().map(|(_, _, n)| n).collect();

        let mut entries: Vec<Value> = Vec::new();
        for n in order.iter().take(MAX_ENTRIES) {
            let blurb = existing.as_ref().and_then(|d| d.blurbs.get(n.as_str()));
            if let Some(b) = blurb {
                if !b.is_empty() && !changed.contains(n.as_str()) && !force {
                    entries.push(json!({"n": n, "keep": b}));
                    continue;
                }
            }
            let mut e = describe_entry(&c.path.join(n), c.depth + 1, is_private);
            if let Some(b) = blurb.filter(|b| !b.is_empty()) {
                e.insert("old".into(), json!(b));
            }
            entries.push(Value::Object(e));
        }
        item.insert("entries".into(), Value::Array(entries));
        if order.len() > MAX_ENTRIES {
            let rest: Vec<String> = order[MAX_ENTRIES..].iter().map(|s| s.to_string()).collect();
            item.insert("more".into(), json!(more_summary(&rest)));
        }
        if !gone.is_empty() {
            item.insert("removed".into(), json!(gone));
        }
        out_containers.push(Value::Object(item));
    }
    let mut out = Map::new();
    out.insert("containers".into(), Value::Array(out_containers));
    let mut removed: Vec<&String> = old_containers.keys().filter(|k| !seen.contains(k.as_str())).collect();
    removed.sort();
    if !removed.is_empty() {
        out.insert("removed_folders".into(), json!(removed));
    }
    let mut p = Map::new();
    p.insert("style".into(), prefs.style.clone());
    p.insert("flagStatus".into(), prefs.flag_status.clone());
    let notes = prefs.notes_text();
    if !notes.is_empty() {
        p.insert("notes".into(), json!(take_chars(&notes, 600)));
    }
    out.insert("prefs".into(), Value::Object(p));
    let q = open_questions(root);
    if q > 0 {
        out.insert("open_questions".into(), json!(q));
    }
    Value::Object(out)
}

/// `check`: true when an indexing run has work to do.
pub fn needs_work(scan: &Value) -> bool {
    let has = |k: &str| scan.get(k).is_some_and(py::truthy);
    has("containers") || has("removed_folders")
}
