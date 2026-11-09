//! `_Organize Log/desktop-index.md`, assembled from each folder's description.md, and the one-line status.

use crate::description;
use crate::manifest;
use crate::prefs;
use crate::py::{self, read_head};
use crate::rules::{self, Container, Kind, DESC, LOG_DIR};
use crate::scan::open_questions;
use std::fs;
use std::io;
use std::path::Path;

fn plural(n: usize) -> &'static str {
    if n != 1 { "s" } else { "" }
}

/// `Indexed 12 folders · 2 still undescribed · 1 question for you`
pub fn status_line(root: &Path) -> String {
    let conts = rules::containers(root, &prefs::load(root).skip);
    let missing = conts
        .iter()
        .filter(|c| description::parse(&read_head(&c.path.join(DESC), 20_000)).summary.is_none_or(|s| s.is_empty()))
        .count();
    let q = open_questions(root);
    let mut parts = vec![format!("Indexed {} folders", conts.len())];
    if missing > 0 {
        parts.push(format!("{missing} still undescribed"));
    }
    if q > 0 {
        parts.push(format!("{q} question{} for you", plural(q)));
    }
    parts.join(" · ")
}

/// The desktop-index.md text for these containers (`updated` is the "_Updated …" timestamp).
pub fn render_index(root: &Path, conts: &[Container], updated: &str) -> String {
    let mut body: Vec<String> = Vec::new();
    let mut total = 0usize;
    for c in conts {
        let names = rules::listdir(&c.path);
        total += names.len();
        let d = description::parse(&read_head(&c.path.join(DESC), 20_000));
        body.push(String::new());
        body.push(format!("{} {}/", "#".repeat((c.depth + 1).min(4)), c.rel));
        let summary = d.summary.as_deref().filter(|s| !s.is_empty()).unwrap_or("_No description yet._");
        body.push(format!("> {summary}"));
        if !d.tags.is_empty() {
            body.push(format!("Tags: {}", d.tags.join(", ")));
        }
        body.push(String::new());
        for n in &names {
            let blurb = d.blurbs.get(n).map(String::as_str);
            body.push(description::bullet(n, rules::is_dir(&c.path.join(n)), blurb));
        }
        if !d.notes.is_empty() {
            body.push(String::new());
            body.extend(d.notes.iter().cloned());
        }
    }
    let loose: Vec<String> = rules::listdir(root).into_iter().filter(|n| rules::kind(&root.join(n), 1) != Kind::Group).collect();
    let q = open_questions(root);
    let mut stamp = format!("_Updated {updated} · {} folders · {total} items", conts.len());
    if q > 0 {
        stamp += &format!(" · {q} open question{} in questions.md", plural(q));
    }
    stamp += " · assembled from each folder's description.md_";
    let mut head = vec!["# Desktop Index".to_string(), String::new(), stamp];
    if !loose.is_empty() {
        head.extend(["".into(), "## Loose on the Desktop".into(), "".into()]);
        head.extend(loose.iter().map(|n| format!("- {n}")));
    }
    head.extend(body);
    head.join("\n") + "\n"
}

/// `inventory.commit`: save the change manifest and rebuild desktop-index.md. Returns the status line.
pub fn commit(root: &Path) -> io::Result<String> {
    let conts = rules::containers(root, &prefs::load(root).skip);
    let manifest = manifest::build(&conts);
    let index = render_index(root, &conts, &py::strftime_local("%b %d, %Y %I:%M %p", None));
    let log = root.join(LOG_DIR);
    match fs::create_dir(&log) {
        Err(e) if e.kind() != io::ErrorKind::AlreadyExists => return Err(e),
        _ => {}
    }
    fs::write(log.join("desktop-index.md"), index)?;
    fs::write(log.join(manifest::FILE), manifest::serialize(&manifest))?;
    Ok(status_line(root))
}
