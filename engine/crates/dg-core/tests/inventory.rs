//! Port of Tests/python/test_inventory.py: description.md parsing (shared fixtures with the Swift tests), folder
//! kinds, preferences, and the scan → commit cycle that decides what an indexing run has to look at.

use dg_core::description::{self, Description};
use dg_core::rules::{self, Kind, MAX_DEPTH};
use dg_core::{assemble, paths, prefs, scan};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// A temporary Desktop folder, resolved so paths compare equal to what the code produces.
struct Desktop {
    root: PathBuf,
}

impl Desktop {
    fn new() -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("dg-tests-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Desktop { root: paths::resolve(&dir) }
    }
    fn file(&self, rel: &str, text: &str) -> PathBuf {
        let p = self.root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, text).unwrap();
        p
    }
    fn dir(&self, rel: &str) -> PathBuf {
        let p = self.root.join(rel);
        fs::create_dir_all(&p).unwrap();
        p
    }
    fn prefs(&self, v: Value) {
        self.file("_Organize Log/preferences.json", &v.to_string());
    }
    fn p(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }
}

impl Drop for Desktop {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn golden() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden/descriptions")
}

fn desc_json(d: &Description) -> Value {
    let blurbs: serde_json::Map<String, Value> = d.blurbs.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
    json!({"title": d.title, "summary": d.summary, "tags": d.tags, "blurbs": blurbs, "notes": d.notes})
}

fn sorted_eq(a: &Value, b: &Value) -> bool {
    // serde_json objects compare in insertion order with preserve_order; compare as maps instead.
    fn norm(v: &Value) -> Value {
        match v {
            Value::Object(o) => {
                let mut m: Vec<(&String, Value)> = o.iter().map(|(k, v)| (k, norm(v))).collect();
                m.sort_by(|x, y| x.0.cmp(y.0));
                Value::Object(m.into_iter().map(|(k, v)| (k.clone(), v)).collect())
            }
            Value::Array(a) => Value::Array(a.iter().map(norm).collect()),
            v => v.clone(),
        }
    }
    norm(a) == norm(b)
}

fn rels(conts: &[rules::Container]) -> Vec<String> {
    conts.iter().map(|c| c.rel.clone()).collect()
}

// ---------- ParseDescription ----------

#[test]
fn fixtures_match_the_app() {
    let mut n = 0;
    let mut mds: Vec<PathBuf> = fs::read_dir(golden()).unwrap().map(|e| e.unwrap().path()).collect();
    mds.sort();
    for md in mds.into_iter().filter(|p| p.extension().is_some_and(|e| e == "md")) {
        let text = fs::read_to_string(&md).unwrap();
        let want: Value = serde_json::from_str(&fs::read_to_string(md.with_extension("json")).unwrap()).unwrap();
        let got = desc_json(&description::parse(&text));
        assert!(sorted_eq(&got, &want), "{}: got {got} want {want}", md.display());
        n += 1;
    }
    assert!(n >= 4, "expected the shared description fixtures");
}

#[test]
fn empty() {
    assert_eq!(description::parse(""), Description::default());
    assert_eq!(
        desc_json(&description::parse("")),
        json!({"title": null, "summary": null, "tags": [], "blurbs": {}, "notes": []})
    );
}

#[test]
fn one_line_collapses_newlines() {
    assert_eq!(description::one_line("a\n## Notes\n- **x**  y", None), "a ## Notes - **x** y");
    assert_eq!(description::one_line("  long text  ", Some(4)), "long");
    assert_eq!(description::one_line("", None), "");
}

#[test]
fn first_lines_skips_code_and_markup() {
    let text = "# Title\n\n![badge](x.svg)\nimport os\n<div>\nA real sentence here.\nSecond line.\nThird.";
    assert_eq!(scan::first_lines(text, 2, 160), "Title / A real sentence here.");
    assert_eq!(scan::first_lines(text, 1, 5), "Title");
}

// ---------- Kinds ----------

#[test]
fn groups_projects_packages_and_files() {
    let d = Desktop::new();
    d.file("Projects/web/package.json", "{}");
    d.file("Projects/notes.txt", "hi");
    d.dir("Projects/Tool.app");
    d.dir("Projects/Swift/App.xcodeproj");
    assert_eq!(rules::kind(&d.p("Projects"), 1), Kind::Group);
    assert_eq!(rules::kind(&d.p("Projects/web"), 2), Kind::Project);
    assert_eq!(rules::kind(&d.p("Projects/notes.txt"), 2), Kind::File);
    assert_eq!(rules::kind(&d.p("Projects/Tool.app"), 2), Kind::Package);
    assert_eq!(rules::kind(&d.p("Projects/Swift"), 2), Kind::Project, "a folder holding an .xcodeproj is a project");
    assert_eq!(rules::kind(&d.p("Projects/Swift/App.xcodeproj"), 3), Kind::Package);
}

#[test]
fn depth_limit() {
    let d = Desktop::new();
    d.dir("A/B/C/D");
    assert_eq!(rules::kind(&d.p("A/B/C"), MAX_DEPTH), Kind::Group);
    assert_eq!(rules::kind(&d.p("A/B/C/D"), MAX_DEPTH + 1), Kind::Folder);
    assert_eq!(rels(&rules::containers::<&str>(&d.root, &[])), ["A", "A/B", "A/B/C"]);
}

#[test]
fn hidden_and_tool_files_are_invisible() {
    for n in [".git", ".DS_Store", "~$doc.docx", "node_modules", "__pycache__", "_Organize Log", "description.md"] {
        assert!(!rules::visible(n), "{n}");
    }
    let d = Desktop::new();
    d.file("Docs/.hidden", "x");
    d.file("Docs/description.md", "# Docs");
    d.file("Docs/a.txt", "x");
    assert_eq!(rules::listdir(&d.p("Docs")), ["a.txt"]);
}

#[test]
fn symlinked_folders_are_not_walked() {
    let d = Desktop::new();
    let outside = Desktop::new();
    outside.dir("Secret/Inner");
    std::os::unix::fs::symlink(outside.p("Secret"), d.p("Link")).unwrap();
    assert_eq!(rules::kind(&d.p("Link"), 1), Kind::File);
    assert!(rules::containers::<&str>(&d.root, &[]).is_empty());
}

#[test]
fn skip_prefs_prune_whole_subtrees() {
    let d = Desktop::new();
    d.dir("Keep/Sub");
    d.dir("Games/Switch");
    assert_eq!(rels(&rules::containers(&d.root, &["Games"])), ["Keep", "Keep/Sub"]);
    assert!(rules::under("Games/Switch", &["Games"]));
    assert!(!rules::under("GamesArchive", &["Games"]), "a prefix of a name isn't the same folder");
}

// ---------- Preferences ----------

#[test]
fn prefs_defaults() {
    let d = Desktop::new();
    assert_eq!(prefs::load(&d.root), prefs::Prefs::default());
}

#[test]
fn prefs_json_is_cleaned_and_unknown_keys_dropped() {
    let d = Desktop::new();
    d.prefs(json!({"style": "detailed", "skip": ["Games/", "", "Old"], "private": ["/Taxes/"], "evil": "x"}));
    let p = prefs::load(&d.root);
    assert_eq!(p.style, json!("detailed"));
    assert_eq!(p.skip, ["Games", "Old"]);
    assert_eq!(p.private, ["Taxes"]);
}

#[test]
fn prefs_broken_json_falls_back_to_defaults() {
    let d = Desktop::new();
    d.file("_Organize Log/preferences.json", "{ nope");
    assert_eq!(prefs::load(&d.root), prefs::Prefs::default());
}

#[test]
fn prefs_legacy_markdown() {
    let d = Desktop::new();
    d.file("_Organize Log/preferences.md", "- skip: `Games/`, Old\n- private: Taxes\n- style: detailed\n");
    let p = prefs::load(&d.root);
    assert_eq!((p.skip, p.private, p.style), (vec!["Games".to_string(), "Old".into()], vec!["Taxes".to_string()], json!("detailed")));
}

// ---------- ScanAndCommit ----------

fn scan_desktop() -> Desktop {
    let d = Desktop::new();
    d.file("Projects/notes.txt", "Plans for the week");
    d.file("Projects/web/package.json", &json!({"description": "Portfolio site", "dependencies": {"react": "1"}}).to_string());
    d
}

const PROJECTS_DESC: &str = "# Projects\n> Code\n\n## Contents\n- **notes.txt** — plans\n- **web/** — site\n";

fn entries_by_name(c: &Value) -> HashMap<String, Value> {
    c["entries"].as_array().unwrap().iter().map(|e| (e["n"].as_str().unwrap().to_string(), e.clone())).collect()
}

#[test]
fn new_folder_then_nothing_to_do() {
    let d = scan_desktop();
    let first = scan::scan(&d.root, false);
    let conts = first["containers"].as_array().unwrap();
    assert_eq!(conts.iter().map(|c| c["path"].as_str().unwrap()).collect::<Vec<_>>(), ["Projects"]);
    assert_eq!(conts[0]["why"], "new");
    let entries = entries_by_name(&conts[0]);
    assert_eq!(entries["web"]["k"], "project");
    assert_eq!(entries["web"]["desc"], "Portfolio site");
    assert!(entries["web"]["stack"].as_str().unwrap().contains("react"));
    assert_eq!(entries["notes.txt"]["head"], "Plans for the week");

    d.file("Projects/description.md", PROJECTS_DESC);
    assemble::commit(&d.root).unwrap();
    assert_eq!(scan::scan(&d.root, false)["containers"], json!([]), "an unchanged, fully described Desktop costs nothing");
    assert!(!scan::needs_work(&scan::scan(&d.root, false)));
    let index = fs::read_to_string(d.p("_Organize Log/desktop-index.md")).unwrap();
    assert!(index.contains("- **web/** — site"));
}

#[test]
fn changes_and_removals_are_reported() {
    let d = scan_desktop();
    d.file("Projects/description.md", PROJECTS_DESC);
    assemble::commit(&d.root).unwrap();
    // Entry signatures use whole-second mtimes; push the edit's mtime forward instead of sleeping.
    let notes = d.file("Projects/notes.txt", "Changed plans, now longer");
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
    fs::File::options().write(true).open(&notes).unwrap().set_modified(later).unwrap();
    d.file("Projects/new.md", "# New");
    fs::remove_dir_all(d.p("Projects/web")).unwrap();
    let out = scan::scan(&d.root, false);
    let c = &out["containers"][0];
    assert_eq!(c["why"], "changed");
    assert_eq!(c["removed"], json!(["web"]));
    let order: Vec<&str> = c["entries"].as_array().unwrap().iter().map(|e| e["n"].as_str().unwrap()).collect();
    assert_eq!(order[..2], ["new.md", "notes.txt"], "changed and undescribed entries come first");
    assert_eq!(entries_by_name(c)["notes.txt"]["old"], "plans", "the previous blurb is offered for reuse");
}

#[test]
fn private_folders_send_names_only() {
    let d = scan_desktop();
    d.prefs(json!({"private": ["Projects"]}));
    let out = scan::scan(&d.root, false);
    for e in out["containers"][0]["entries"].as_array().unwrap() {
        assert!(e.as_object().unwrap().keys().all(|k| k == "n" || k == "k"), "{e}");
    }
}

#[test]
fn removed_folders() {
    let d = scan_desktop();
    assemble::commit(&d.root).unwrap();
    fs::remove_dir_all(d.p("Projects")).unwrap();
    assert_eq!(scan::scan(&d.root, false)["removed_folders"], json!(["Projects"]));
}

// ---------- beyond the Python suite ----------

#[test]
fn caps_entries_and_summarizes_the_rest() {
    let d = Desktop::new();
    for i in 0..45 {
        d.file(&format!("Big/f{i:02}.{}", if i % 3 == 0 { "png" } else { "jpg" }), "x");
    }
    let out = scan::scan(&d.root, false);
    let c = &out["containers"][0];
    assert_eq!(c["entries"].as_array().unwrap().len(), rules::MAX_ENTRIES);
    assert_eq!(c["more"], "+5 more: 4 .jpg, 1 .png");
}

#[test]
fn unchanged_blurbs_are_kept_verbatim() {
    let d = scan_desktop();
    d.file("Projects/description.md", "# Projects\n> Code\n\n## Contents\n- **notes.txt** — plans\n");
    assemble::commit(&d.root).unwrap();
    let out = scan::scan(&d.root, false);
    let c = &out["containers"][0];
    assert_eq!(c["entries"][0]["n"], "web", "the undescribed entry comes first");
    assert_eq!(c["entries"][1], json!({"n": "notes.txt", "keep": "plans"}));
}

#[test]
fn status_line_counts() {
    let d = scan_desktop();
    d.file("_Organize Log/questions.md", "- [ ] a\n- [x] b\n");
    assert_eq!(assemble::status_line(&d.root), "Indexed 1 folders · 1 still undescribed · 1 question for you");
}
