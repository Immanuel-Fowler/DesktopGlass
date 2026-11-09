//! The Desktop model the overlay shows: the folder tree with each group's parsed description.md, Finder-style
//! ordering and search. Ported from the app (Models.swift `Scanner` / `FolderDescription`, Store.swift search) so
//! the UI no longer touches the file system itself; `dg engine` serves it as `desktop/load|list|search`.
//!
//! This parser is deliberately *not* [`crate::description::parse`] (inventory.py's): the overlay has always used
//! Swift's slightly different rules (first `**` closes the name, one trailing `/` removed, title/summary not
//! re-trimmed, notes joined with spaces), and the overlay must keep showing exactly what it showed before.
//! Character classes follow Foundation: lines split on `CharacterSet.newlines`, trimmed with
//! `CharacterSet.whitespaces` (tab + Unicode Zs).

use crate::rules::{self, Kind};
use serde_json::{json, Map, Value};
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Search returns at most this many items (Store.swift `prefix(80)`).
pub const MAX_RESULTS: usize = 80;

/// Directories Finder treats as packages (LaunchServices `isPackage`) beyond the shared `rules::PACKAGE_EXT`.
/// The app always showed these as single items.
pub const MAC_PACKAGE_EXT: &[&str] = &[
    "sparsebundle", "plugin", "kext", "pkg", "mpkg", "scptd", "prefpane", "qlgenerator", "mdimporter", "saver",
    "wdgt", "xpc", "appex", "dsym", "docset", "xcarchive", "fcpbundle", "imovielibrary", "musiclibrary",
    "tvlibrary", "playgroundbook", "mlmodelc",
];

// ---------- Foundation character classes ----------

/// `CharacterSet.newlines`: U+000A–U+000D, U+0085, U+2028, U+2029.
fn is_newline(c: char) -> bool {
    matches!(c, '\n' | '\u{0b}' | '\u{0c}' | '\r' | '\u{85}' | '\u{2028}' | '\u{2029}')
}

/// `CharacterSet.whitespaces`: tab and Unicode general category Zs.
fn is_ws(c: char) -> bool {
    matches!(c, '\t' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{202f}' | '\u{205f}' | '\u{3000}')
}

/// `trimmingCharacters(in: .whitespaces)`
pub fn trim_ws(s: &str) -> &str {
    s.trim_matches(is_ws)
}

/// Scalars that join the previous character into one grapheme (combining marks, joiners, variation selectors,
/// emoji modifiers). Swift compares `Character`s, so `"# \u{301}x".hasPrefix("# ")` is false.
fn is_extender(c: char) -> bool {
    matches!(c as u32,
        0x0300..=0x036F | 0x0483..=0x0489 | 0x0591..=0x05BD | 0x0610..=0x061A | 0x064B..=0x065F | 0x0E31 | 0x0E34..=0x0E3A
        | 0x0E47..=0x0E4E | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x200C | 0x200D | 0x20D0..=0x20FF | 0x302A..=0x302F
        | 0x3099 | 0x309A | 0xFE00..=0xFE0F | 0xFE20..=0xFE2F | 0x1F3FB..=0x1F3FF | 0xE0020..=0xE007F
        | 0xE0100..=0xE01EF)
}

/// Swift `hasPrefix` for an ASCII prefix: it must end on a grapheme boundary.
fn has_prefix(s: &str, prefix: &str) -> bool {
    s.starts_with(prefix) && !s[prefix.len()..].chars().next().is_some_and(is_extender)
}

/// Swift `lowercased()`: per-scalar lowercase mapping (no context-dependent final sigma).
pub fn lower(s: &str) -> String {
    s.chars().flat_map(char::to_lowercase).collect()
}

// ---------- description.md, as the overlay reads it ----------

/// Parsed description.md: `# Title`, `> summary`, `Tags: a, b`, `## Contents` bullets `- **name/** — blurb`,
/// `## Notes`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FolderDescription {
    pub title: Option<String>,
    pub summary: Option<String>,
    pub tags: Vec<String>,
    pub blurbs: BTreeMap<String, String>,
    pub notes: Option<String>,
}

/// `- **name** — blurb` / `* **name**: blurb` on an already-trimmed line.
fn parse_bullet(line: &str) -> Option<(String, String)> {
    if !(has_prefix(line, "- **") || has_prefix(line, "* **")) {
        return None;
    }
    let body = &line[4..];
    let end = body.find("**")?;
    let mut name = trim_ws(&body[..end]).to_string();
    if name.ends_with('/') {
        name.pop();
    }
    let mut rest = trim_ws(&body[end + 2..]);
    for sep in ["—", "–", "-", ":"] {
        if has_prefix(rest, sep) {
            rest = trim_ws(&rest[sep.len()..]);
            break;
        }
    }
    Some((name, rest.to_string()))
}

/// Splits on `,` the way Swift's `split(separator: ",")` does (a comma carrying a combining mark isn't one).
fn split_commas(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        if c == ',' && !it.peek().is_some_and(|&(_, n)| is_extender(n)) {
            out.push(&s[start..i]);
            start = i + 1;
        }
    }
    out.push(&s[start..]);
    out
}

impl FolderDescription {
    /// `FolderDescription.parse`
    pub fn parse(text: &str) -> FolderDescription {
        let mut d = FolderDescription::default();
        let mut section = String::new();
        let mut notes: Vec<&str> = Vec::new();
        for raw in text.split(is_newline) {
            let line = trim_ws(raw);
            if has_prefix(line, "## ") {
                section = lower(&line[3..]);
                continue;
            }
            if has_prefix(line, "# ") && d.title.is_none() {
                d.title = Some(line[2..].to_string());
                continue;
            }
            if has_prefix(line, "> ") && d.summary.is_none() {
                d.summary = Some(line[2..].to_string());
                continue;
            }
            if has_prefix(&lower(line), "tags:") {
                // only ASCII letters lower-case to "tags:", so the original prefix is 5 bytes too
                d.tags = split_commas(&line[5..])
                    .into_iter()
                    .map(|t| lower(trim_ws(t)))
                    .filter(|t| !t.is_empty())
                    .collect();
                continue;
            }
            if section == "contents" {
                if let Some((name, blurb)) = parse_bullet(line) {
                    d.blurbs.insert(name, blurb);
                    continue;
                }
            }
            if section == "notes" && !line.is_empty() {
                notes.push(line);
            }
        }
        if !notes.is_empty() {
            d.notes = Some(notes.join(" "));
        }
        d
    }

    /// Reads and parses `<dir>/description.md`; None if it's missing or not UTF-8.
    pub fn read(dir: &Path) -> Option<FolderDescription> {
        let text = std::fs::read_to_string(dir.join(rules::DESC)).ok()?;
        Some(Self::parse(text.strip_prefix('\u{feff}').unwrap_or(&text)))
    }

    pub fn to_json(&self) -> Value {
        json!({
            "title": self.title, "summary": self.summary, "tags": self.tags,
            "blurbs": self.blurbs.iter().map(|(k, v)| (k.clone(), json!(v))).collect::<Map<String, Value>>(),
            "notes": self.notes,
        })
    }
}

// ---------- Finder ordering ----------

/// ASCII punctuation and symbols in ICU root-collation order (what `localizedStandardCompare` uses).
const PUNCT_ORDER: &str = "_-,;:!?.'\"()[]{}@*/\\&#%`^+<=>|~$";

fn fold(c: char) -> char {
    const TABLE: &[(&str, char)] = &[
        ("àáâãäåāăą", 'a'), ("çćĉċč", 'c'), ("ďđ", 'd'), ("èéêëēĕėęě", 'e'), ("ĝğġģ", 'g'), ("ĥħ", 'h'),
        ("ìíîïĩīĭįı", 'i'), ("ĵ", 'j'), ("ķ", 'k'), ("ĺļľŀł", 'l'), ("ñńņňŉ", 'n'), ("òóôõöøōŏő", 'o'),
        ("ŕŗř", 'r'), ("śŝşš", 's'), ("ţťŧ", 't'), ("ùúûüũūŭůűų", 'u'), ("ŵ", 'w'), ("ýÿŷ", 'y'), ("źżž", 'z'),
    ];
    TABLE.iter().find(|(set, _)| set.contains(c)).map(|&(_, b)| b).unwrap_or(c)
}

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone)]
enum Tok {
    /// (class, weight): 0 space/control, 1 ASCII punctuation, 2 other symbols, 3 numbers, 4 letters
    Ch(u8, u32),
    /// a digit run: (number of significant digits, the significant digits)
    Num(usize, String),
}

impl Tok {
    fn class(&self) -> u8 {
        match self {
            Tok::Ch(c, _) => *c,
            Tok::Num(..) => 3,
        }
    }
}

fn primary(s: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_ascii_digit() {
            let mut digits = String::from(c);
            while let Some(&d) = chars.peek() {
                if !d.is_ascii_digit() {
                    break;
                }
                digits.push(d);
                chars.next();
            }
            let sig = digits.trim_start_matches('0').to_string();
            out.push(Tok::Num(sig.len(), sig));
            continue;
        }
        if is_extender(c) {
            continue; // accents are a secondary difference
        }
        let l = char::to_lowercase(c).next().unwrap_or(c);
        if l == 'ß' {
            // sorts as "ss"
            out.push(Tok::Ch(4, 's' as u32));
            out.push(Tok::Ch(4, 's' as u32));
            continue;
        }
        let tok = if c.is_whitespace() || c.is_control() {
            Tok::Ch(0, c as u32)
        } else if let Some(i) = PUNCT_ORDER.find(c) {
            Tok::Ch(1, i as u32)
        } else if c.is_alphanumeric() {
            Tok::Ch(4, fold(l) as u32)
        } else {
            Tok::Ch(2, c as u32)
        };
        out.push(tok);
    }
    out
}

fn cmp_primary(a: &[Tok], b: &[Tok]) -> Ordering {
    for (x, y) in a.iter().zip(b) {
        let o = x.class().cmp(&y.class()).then_with(|| x.cmp(y));
        if o != Ordering::Equal {
            return o;
        }
    }
    a.len().cmp(&b.len())
}

/// Per-letter accent weights: unaccented first, then by the accented letter (ö before ø), combining marks last.
fn accents(s: &str) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::new();
    for c in s.chars() {
        if is_extender(c) {
            if let Some(last) = out.last_mut() {
                *last = u32::MAX - 0xFFFF + c as u32 % 0xFFFF;
            }
            continue;
        }
        if !c.is_alphabetic() {
            continue;
        }
        let l = char::to_lowercase(c).next().unwrap_or(c);
        out.push(if fold(l) != l { l as u32 } else { 0 });
    }
    out
}

/// Tie-breakers in string order: per letter, lower-case first; per number, fewer leading zeros first.
fn tertiary(s: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut run = 0;
    for c in s.chars() {
        if c.is_ascii_digit() {
            run += 1;
            continue;
        }
        if run > 0 {
            out.push(run);
            run = 0;
        }
        if c.is_alphabetic() {
            out.push(c.is_uppercase() as usize);
        }
    }
    if run > 0 {
        out.push(run);
    }
    out
}

/// `localizedStandardCompare`: Finder order — case-insensitive, numbers by value, punctuation before digits
/// before letters, accents and then case only as tie-breakers.
pub fn standard_compare(a: &str, b: &str) -> Ordering {
    cmp_primary(&primary(a), &primary(b))
        .then_with(|| accents(a).cmp(&accents(b)))
        .then_with(|| tertiary(a).cmp(&tertiary(b)))
        .then_with(|| a.cmp(b))
}

// ---------- scanning ----------

fn kind_rank(k: Kind) -> u8 {
    match k {
        Kind::Group => 0,
        Kind::Project => 1,
        Kind::Folder => 2,
        Kind::Package => 3,
        Kind::File => 4,
    }
}

/// One entry of the overlay's tree.
#[derive(Clone, Debug)]
pub struct Item {
    pub path: PathBuf,
    pub name: String,
    pub kind: Kind,
    /// Seconds since 1970.
    pub modified: Option<f64>,
    pub size: Option<u64>,
    /// The parent's `## Contents` line for this entry, or (for a group) its own summary.
    pub blurb: Option<String>,
    /// A group's own description.md.
    pub info: Option<FolderDescription>,
    /// A group's children (scanned up front). None for projects and plain folders, which load when opened
    /// (`children(path, MAX_DEPTH + 1)`), and for packages and files.
    pub children: Option<Vec<Item>>,
    /// Visible entries inside (groups, projects, folders).
    pub count: usize,
}

impl Item {
    pub fn to_json(&self) -> Value {
        let mut o = Map::new();
        o.insert("path".into(), json!(self.path.to_string_lossy()));
        o.insert("name".into(), json!(self.name));
        o.insert("kind".into(), json!(self.kind.as_str()));
        o.insert("modified".into(), json!(self.modified));
        o.insert("size".into(), json!(self.size));
        o.insert("blurb".into(), json!(self.blurb));
        o.insert("count".into(), json!(self.count));
        if let Some(info) = &self.info {
            o.insert("info".into(), info.to_json());
        }
        if let Some(children) = &self.children {
            o.insert("children".into(), Value::Array(children.iter().map(Item::to_json).collect()));
        }
        Value::Object(o)
    }
}

fn is_package_name(name: &str) -> bool {
    let ext = crate::py::suffix(name).trim_start_matches('.').to_lowercase();
    !ext.is_empty() && (rules::PACKAGE_EXT.contains(&ext.as_str()) || MAC_PACKAGE_EXT.contains(&ext.as_str()))
}

/// What the overlay calls an entry at `depth` (1 = top level). Same as `rules::kind` plus Finder packages.
pub fn kind(p: &Path, name: &str, depth: usize) -> Kind {
    if !rules::is_dir(p) {
        Kind::File
    } else if is_package_name(name) {
        Kind::Package
    } else if rules::is_project(p) {
        Kind::Project
    } else if depth <= rules::MAX_DEPTH {
        Kind::Group
    } else {
        Kind::Folder
    }
}

fn visible_count(p: &Path) -> usize {
    rules::raw_names(p).unwrap_or_default().iter().filter(|n| rules::visible(n)).count()
}

/// `Scanner.children(of:depth:blurbs:)`: the visible entries of `dir`, groups scanned recursively, sorted by
/// kind and then Finder order.
pub fn children(dir: &Path, depth: usize, blurbs: &BTreeMap<String, String>) -> Vec<Item> {
    let Some(names) = rules::raw_names(dir) else { return vec![] };
    let mut items: Vec<Item> = Vec::new();
    for name in names.into_iter().filter(|n| rules::visible(n)) {
        let path = dir.join(&name);
        let meta = std::fs::symlink_metadata(&path).ok();
        let kind = kind(&path, &name, depth);
        let modified = meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs_f64());
        let size = meta.as_ref().filter(|m| !m.is_dir()).map(|m| m.len());
        let mut item = Item {
            blurb: blurbs.get(&name).cloned(),
            path,
            name,
            kind,
            modified,
            size,
            info: None,
            children: None,
            count: 0,
        };
        match kind {
            Kind::Group => {
                item.info = FolderDescription::read(&item.path);
                let empty = BTreeMap::new();
                let kids = children(&item.path, depth + 1, item.info.as_ref().map(|i| &i.blurbs).unwrap_or(&empty));
                item.count = kids.len();
                item.children = Some(kids);
                if item.blurb.is_none() {
                    item.blurb = item.info.as_ref().and_then(|i| i.summary.clone());
                }
            }
            Kind::Project | Kind::Folder => item.count = visible_count(&item.path),
            _ => {}
        }
        items.push(item);
    }
    items.sort_by(|a, b| {
        kind_rank(a.kind).cmp(&kind_rank(b.kind)).then_with(|| standard_compare(&a.name, &b.name))
    });
    items
}

/// `Scanner.scan()`: the whole Desktop (groups down to `MAX_DEPTH`).
pub fn scan(root: &Path) -> Vec<Item> {
    children(root, 1, &BTreeMap::new())
}

/// The entries of a project or plain folder, opened in the overlay (always listed as below the group levels).
pub fn list(dir: &Path) -> Vec<Item> {
    children(dir, rules::MAX_DEPTH + 1, &BTreeMap::new())
}

/// Every item of the tree, depth-first (parents before their children).
pub fn flatten(items: &[Item]) -> Vec<&Item> {
    fn walk<'a>(items: &'a [Item], out: &mut Vec<&'a Item>) {
        for i in items {
            out.push(i);
            if let Some(c) = &i.children {
                walk(c, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(items, &mut out);
    out
}

/// Groups in the tree without a description.md yet.
pub fn undescribed(items: &[Item]) -> usize {
    flatten(items).iter().filter(|i| i.kind == Kind::Group && i.info.is_none()).count()
}

/// Store.swift search: every space-separated term must appear in the name, blurb or tags (case-insensitive);
/// names starting with the first term come first, then by kind, then Finder order. At most `limit`.
pub fn search<'a>(flat: &[&'a Item], query: &str, limit: usize) -> Vec<&'a Item> {
    let q = lower(query);
    let terms: Vec<&str> = q.split(' ').filter(|t| !t.is_empty()).collect();
    let Some(first) = terms.first().copied() else { return vec![] };
    let mut hits: Vec<&Item> = flat
        .iter()
        .copied()
        .filter(|item| {
            let tags = item.info.as_ref().map(|i| i.tags.join(" ")).unwrap_or_default();
            let hay = lower(&[item.name.as_str(), item.blurb.as_deref().unwrap_or(""), &tags].join(" "));
            terms.iter().all(|t| hay.contains(t))
        })
        .collect();
    hits.sort_by(|a, b| {
        let an = has_prefix(&lower(&a.name), first);
        let bn = has_prefix(&lower(&b.name), first);
        bn.cmp(&an)
            .then_with(|| kind_rank(a.kind).cmp(&kind_rank(b.kind)))
            .then_with(|| standard_compare(&a.name, &b.name))
    });
    hits.truncate(limit);
    hits
}

/// Unchecked `- [ ]` lines in `_Organize Log/questions.md` (as the app counted them).
pub fn open_questions(root: &Path) -> usize {
    let text = std::fs::read_to_string(root.join(rules::LOG_DIR).join("questions.md")).unwrap_or_default();
    text.split(is_newline).filter(|l| trim_ws(l).starts_with("- [ ]")).count()
}

/// When `_Organize Log/desktop-index.md` was last rebuilt (seconds since 1970).
pub fn last_indexed(root: &Path) -> Option<f64> {
    std::fs::metadata(root.join(rules::LOG_DIR).join("desktop-index.md"))
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn golden() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden/descriptions")
    }

    /// Port of FolderDescriptionTests.swift: the shared fixtures, then the inline cases.
    fn check(name: &str) {
        let text = std::fs::read_to_string(golden().join(format!("{name}.md"))).unwrap();
        let want: Value = serde_json::from_str(&std::fs::read_to_string(golden().join(format!("{name}.json"))).unwrap()).unwrap();
        let got = FolderDescription::parse(&text);
        assert_eq!(json!(got.title), want["title"], "{name}: title");
        assert_eq!(json!(got.summary), want["summary"], "{name}: summary");
        assert_eq!(json!(got.tags), want["tags"], "{name}: tags");
        let blurbs: BTreeMap<String, String> = serde_json::from_value(want["blurbs"].clone()).unwrap();
        assert_eq!(got.blurbs, blurbs, "{name}: blurbs");
        let notes: Vec<String> = want["notes"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
        assert_eq!(got.notes, if notes.is_empty() { None } else { Some(notes.join(" ")) }, "{name}: notes");
    }

    #[test]
    fn full_description() {
        check("full");
    }
    #[test]
    fn title_only() {
        check("minimal");
    }
    #[test]
    fn windows_line_endings() {
        check("crlf");
    }
    #[test]
    fn bullets_outside_contents_are_not_blurbs() {
        check("bullets-outside-contents");
    }

    #[test]
    fn empty_text() {
        assert_eq!(FolderDescription::parse(""), FolderDescription::default());
    }

    #[test]
    fn first_title_and_summary_win() {
        let d = FolderDescription::parse("# One\n> First\n# Two\n> Second\n");
        assert_eq!(d.title.as_deref(), Some("One"));
        assert_eq!(d.summary.as_deref(), Some("First"));
    }

    #[test]
    fn section_names_are_case_insensitive() {
        let d = FolderDescription::parse("## CONTENTS\n- **a.txt** — x\n## notes\nremember\n");
        assert_eq!(d.blurbs, BTreeMap::from([("a.txt".to_string(), "x".to_string())]));
        assert_eq!(d.notes.as_deref(), Some("remember"));
    }

    #[test]
    fn folder_names_lose_their_trailing_slash() {
        let d = FolderDescription::parse("## Contents\n- **Photos/** — pictures\n");
        assert_eq!(d.blurbs.get("Photos").map(String::as_str), Some("pictures"));
        assert!(!d.blurbs.contains_key("Photos/"));
    }

    /// Where the overlay's parser differs from inventory.py's: these are what Swift extracted.
    #[test]
    fn swift_specific_rules() {
        let d = FolderDescription::parse(
            "#  Spaced\n>  quoted\nTAGS: A ,, b\u{3000}\n## Contents\n- ****empty name\n- **a//** : b\n-  **two spaces** x\n- **x** —— y\n\u{0b}## Notes\nn1\n\n n2 \n",
        );
        assert_eq!(d.title.as_deref(), Some(" Spaced"));
        assert_eq!(d.summary.as_deref(), Some(" quoted"));
        assert_eq!(d.tags, vec!["a", "b"]);
        assert_eq!(d.blurbs.get("").map(String::as_str), Some("empty name"));
        assert_eq!(d.blurbs.get("a/").map(String::as_str), Some("b"));
        assert!(!d.blurbs.contains_key("two spaces"));
        assert_eq!(d.blurbs.get("x").map(String::as_str), Some("— y"));
        assert_eq!(d.notes.as_deref(), Some("n1 n2"));
        // a combining mark makes "# " a different first character
        assert_eq!(FolderDescription::parse("# \u{301}x").title, None);
        // the section header line itself is trimmed, but not after "## "
        assert!(FolderDescription::parse("##  Contents\n- **a** b").blurbs.is_empty());
    }

    #[test]
    fn finder_order() {
        let mut v = vec!["a", "B", "_x", "-y", "10", "9", "Ab", "ab", "a b", "é", "e", "f", "~z", "$m", "#h", ".dot"];
        v.sort_by(|a, b| standard_compare(a, b));
        // what `localizedStandardCompare` gives on macOS
        assert_eq!(v, ["_x", "-y", ".dot", "#h", "~z", "$m", "9", "10", "a", "a b", "ab", "Ab", "B", "e", "é", "f"]);
        assert_eq!(standard_compare("file2", "file10"), Ordering::Less);
        assert_eq!(standard_compare("a01", "a1"), Ordering::Greater);
        assert_eq!(standard_compare("e\u{301}", "f"), Ordering::Less);
    }

    struct Tmp(PathBuf);
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn desktop(tag: &str) -> Tmp {
        let d = std::env::temp_dir().join(format!("dg-desktop-model-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let w = |rel: &str, text: &str| {
            let p = d.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        };
        w("Projects/description.md", "# Projects\n> Code\nTags: Swift, Web\n## Contents\n- **App/** — the app\n- **notes.txt** — scratch\n");
        w("Projects/App/Package.swift", "");
        w("Projects/App/Sources/main.swift", "");
        w("Projects/notes.txt", "hello");
        w("Projects/Old/a.txt", "");
        w("Projects/Old/Deeper/Deepest/x.txt", "");
        w("Projects/Old/Deeper/Deepest/More/y.txt", "");
        w("Photos/img2.jpg", "");
        w("Photos/img10.jpg", "");
        w("todo.md", "");
        w(".hidden", "");
        w("~$lock.docx", "");
        w("Thing.app/Contents/Info.plist", "");
        w("Disk.sparsebundle/x", "");
        w("_Organize Log/questions.md", "# Q\n- [ ] one?\n  - [ ] two?\n- [x] done\n");
        w("_Organize Log/desktop-index.md", "x");
        Tmp(d)
    }

    #[test]
    fn scans_like_the_app() {
        let t = desktop("scan");
        let items = scan(&t.0);
        let names: Vec<(&str, &str)> = items.iter().map(|i| (i.name.as_str(), i.kind.as_str())).collect();
        assert_eq!(names, [("Photos", "group"), ("Projects", "group"), ("Disk.sparsebundle", "package"),
                           ("Thing.app", "package"), ("todo.md", "file")]);
        let projects = &items[1];
        assert_eq!(projects.blurb.as_deref(), Some("Code"));
        assert_eq!(projects.info.as_ref().unwrap().tags, ["swift", "web"]);
        let kids = projects.children.as_ref().unwrap();
        let names: Vec<(&str, &str, Option<&str>)> =
            kids.iter().map(|i| (i.name.as_str(), i.kind.as_str(), i.blurb.as_deref())).collect();
        assert_eq!(names, [("Old", "group", None), ("App", "project", Some("the app")), ("notes.txt", "file", Some("scratch"))]);
        assert_eq!(kids[1].count, 2);
        assert!(kids[1].children.is_none(), "projects load when opened");
        assert_eq!(kids[2].size, Some(5));
        // depth: Projects(1)/Old(2)/Deeper(3) are groups; Deepest (4) is a plain folder
        let deeper = &kids[0].children.as_ref().unwrap()[0];
        assert_eq!((deeper.name.as_str(), deeper.kind), ("Deeper", Kind::Group));
        let deepest = &deeper.children.as_ref().unwrap()[0];
        assert_eq!((deepest.kind, deepest.count, deepest.children.is_none()), (Kind::Folder, 2, true));
        let photos: Vec<&str> = items[0].children.as_ref().unwrap().iter().map(|i| i.name.as_str()).collect();
        assert_eq!(photos, ["img2.jpg", "img10.jpg"]);

        let listed = list(&t.0.join("Projects/App"));
        let names: Vec<(&str, &str)> = listed.iter().map(|i| (i.name.as_str(), i.kind.as_str())).collect();
        assert_eq!(names, [("Sources", "folder"), ("Package.swift", "file")]);
        assert_eq!(undescribed(&items), 3); // Photos, Old, Deeper
        assert_eq!(open_questions(&t.0), 2);
        assert!(last_indexed(&t.0).is_some());
    }

    #[test]
    fn search_ranks_like_the_store() {
        let t = desktop("search");
        let items = scan(&t.0);
        let flat = flatten(&items);
        let names = |q: &str| search(&flat, q, MAX_RESULTS).iter().map(|i| i.name.clone()).collect::<Vec<_>>();
        assert_eq!(names("app"), ["App", "Thing.app"]); // name prefix first; then by kind
        assert_eq!(names("SWIFT"), ["Projects"]); // tags
        assert_eq!(names("scratch notes"), ["notes.txt"]); // blurb + name, every term
        assert_eq!(names("o"), ["Old", "Photos", "Projects", "notes.txt", "todo.md"]);
        assert!(names("   ").is_empty());
        assert_eq!(search(&flat, "i", 2).len(), 2);
    }
}
