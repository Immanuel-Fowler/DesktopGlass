//! The Desktop model and indexing preferences (was Models.swift `Scanner`/`FolderDescription`, Store.swift's
//! search and IndexPreferences.swift's model). The logic lives in `dg_core::desktop_model` and `dg_core::prefs`.
//!
//! **UI link methods**
//! - `desktop/load {}` → `{root, items: [Item], indexed: secs|null, questions: n, undescribed: n}` — the whole
//!   tree the overlay shows: groups down to depth 3 with their children; projects and plain folders without
//!   (`desktop/list` loads them when opened). `indexed` is desktop-index.md's mtime, `questions` the open
//!   `- [ ]` lines in questions.md, `undescribed` the groups that have no description.md yet.
//!   `Item` = `{path, name, kind: group|project|folder|package|file, modified: secs|null, size: n|null,
//!   blurb: str|null, count: n, info?: {title, summary, tags, blurbs: {name: blurb}, notes}, children?: [Item]}`.
//! - `desktop/list {path}` → `{path, items: [Item]}` — a project's or folder's entries (path inside the Desktop).
//! - `desktop/search {query, limit?}` → `{query, paths: [path]}` — over the tree of the last `desktop/load`
//!   (scanned now if there wasn't one); same filter and ranking the overlay always had, at most 80.
//! - `desktop/describe {path}` → `{path, info: {…}|null}` — one folder's description.md, parsed.
//! - `prefs/get {}` → `Prefs` = `{version, style, private: [rel], skip: [rel], flagStatus, notes}`
//!   (`_Organize Log/preferences.json`).
//! - `prefs/set {data?: Prefs (any subset), folder?: rel, mode?: include|names_only|skip}` → `Prefs`; always
//!   writes preferences.json (so `prefs/set {}` creates it from what's on disk or the defaults).
//!
//! **Notification** — `prefs/changed Prefs` after an MCP client changed the preferences (`prefs.set`).
//!
//! **Control methods** (same params and results as AppControl.swift had): `prefs.get` →
//! `{style, flag_status, notes, names_only, skip}`; `prefs.set {style?, flag_status?, notes?, folder?,
//! folder_mode?}` → the same.

use crate::Engine;
use dg_core::desktop_model::{self as model, FolderDescription, Item};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};

pub const LOAD: &str = "desktop/load";
pub const LIST: &str = "desktop/list";
pub const SEARCH: &str = "desktop/search";
pub const DESCRIBE: &str = "desktop/describe";
pub const PREFS_GET: &str = "prefs/get";
pub const PREFS_SET: &str = "prefs/set";
/// Notification: the preferences changed outside the UI.
pub const PREFS_CHANGED: &str = "prefs/changed";

// ---------- per-engine state ----------

struct State {
    root: Mutex<PathBuf>,
    /// The tree of the last `desktop/load`, for search.
    tree: Mutex<Option<Arc<Vec<Item>>>>,
    /// Serializes preferences.json read-modify-write.
    prefs: Mutex<()>,
}

type Registry = Mutex<Vec<(Weak<Engine>, Arc<State>)>>;

fn registry() -> &'static Registry {
    static R: OnceLock<Registry> = OnceLock::new();
    R.get_or_init(Default::default)
}

fn state(engine: &Arc<Engine>) -> Arc<State> {
    let mut reg = registry().lock().unwrap();
    reg.retain(|(w, _)| w.strong_count() > 0);
    if let Some((_, s)) = reg.iter().find(|(w, _)| w.as_ptr() == Arc::as_ptr(engine)) {
        return s.clone();
    }
    let s = Arc::new(State {
        root: Mutex::new(dg_platform::desktop()),
        tree: Mutex::new(None),
        prefs: Mutex::new(()),
    });
    reg.push((Arc::downgrade(engine), s.clone()));
    s
}

/// Points this engine at another Desktop folder (tests).
pub fn set_root(engine: &Arc<Engine>, root: PathBuf) {
    let s = state(engine);
    *s.root.lock().unwrap() = root;
    *s.tree.lock().unwrap() = None;
}

pub fn root(engine: &Arc<Engine>) -> PathBuf {
    state(engine).root.lock().unwrap().clone()
}

// ---------- methods ----------

pub fn handle(engine: &Arc<Engine>, method: &str, params: &Value) -> Option<Result<Value, String>> {
    Some(match method {
        LOAD => Ok(load(engine)),
        LIST => list(engine, params),
        SEARCH => search(engine, params),
        DESCRIBE => describe(engine, params),
        PREFS_GET => Ok(prefs_json(&prefs_load(&root(engine)))),
        PREFS_SET => prefs_set_ui(engine, params),
        "prefs.get" => Ok(control_json(&prefs_load(&root(engine)))),
        "prefs.set" => prefs_set_control(engine, params),
        _ => return None,
    })
}

pub fn start(_engine: &Arc<Engine>) {}

fn load(engine: &Arc<Engine>) -> Value {
    let s = state(engine);
    let root = s.root.lock().unwrap().clone();
    let items = Arc::new(model::scan(&root));
    let out = json!({
        "root": root.to_string_lossy(),
        "items": items.iter().map(Item::to_json).collect::<Vec<_>>(),
        "indexed": model::last_indexed(&root),
        "questions": model::open_questions(&root),
        "undescribed": model::undescribed(&items),
    });
    *s.tree.lock().unwrap() = Some(items);
    out
}

/// A path the UI sent: absolute (or Desktop-relative), and inside the Desktop once symlinks are resolved.
/// Returned unresolved, so item paths keep the form the UI already has.
fn inside(engine: &Arc<Engine>, params: &Value) -> Result<PathBuf, String> {
    let raw = params.get("path").and_then(|p| p.as_str()).ok_or("path is required")?;
    let root = root(engine);
    let p = if Path::new(raw).is_absolute() { PathBuf::from(raw) } else { root.join(raw.trim_matches('/')) };
    let real_root = dg_core::paths::resolve(&root);
    let real = dg_core::paths::resolve(&p);
    if real != real_root && !real.starts_with(&real_root) {
        return Err("Path must be inside the Desktop".into());
    }
    Ok(p)
}

fn list(engine: &Arc<Engine>, params: &Value) -> Result<Value, String> {
    let p = inside(engine, params)?;
    if !dg_core::rules::is_dir(&p) {
        return Err(format!("Not a folder: {}", p.display()));
    }
    let items = model::list(&p);
    Ok(json!({"path": p.to_string_lossy(), "items": items.iter().map(Item::to_json).collect::<Vec<_>>()}))
}

fn search(engine: &Arc<Engine>, params: &Value) -> Result<Value, String> {
    let query = params.get("query").and_then(|q| q.as_str()).unwrap_or("");
    let limit = params.get("limit").and_then(|l| l.as_u64()).map(|l| l as usize).unwrap_or(model::MAX_RESULTS);
    let s = state(engine);
    let tree = {
        let cached = s.tree.lock().unwrap().clone();
        match cached {
            Some(t) => t,
            None => {
                let root = s.root.lock().unwrap().clone();
                let t = Arc::new(model::scan(&root));
                *s.tree.lock().unwrap() = Some(t.clone());
                t
            }
        }
    };
    let flat = model::flatten(&tree);
    let paths: Vec<String> =
        model::search(&flat, query, limit).iter().map(|i| i.path.to_string_lossy().into_owned()).collect();
    Ok(json!({"query": query, "paths": paths}))
}

fn describe(engine: &Arc<Engine>, params: &Value) -> Result<Value, String> {
    let p = inside(engine, params)?;
    Ok(json!({"path": p.to_string_lossy(), "info": FolderDescription::read(&p).map(|d| d.to_json())}))
}

// ---------- preferences ----------

/// IndexPreferencesData: what Settings edits and preferences.json holds.
#[derive(Clone, Debug, PartialEq)]
pub struct Prefs {
    pub version: i64,
    pub style: String,
    pub private: Vec<String>,
    pub skip: Vec<String>,
    pub flag_status: bool,
    pub notes: String,
}

/// preferences.json as the indexer reads it (`dg_core::prefs::load`, legacy preferences.md included), typed.
pub fn prefs_load(root: &Path) -> Prefs {
    let p = dg_core::prefs::load(root);
    Prefs {
        version: p.version.as_i64().unwrap_or(1),
        style: p.style.as_str().unwrap_or("terse").to_string(),
        private: p.private,
        skip: p.skip,
        flag_status: match &p.flag_status {
            Value::Bool(b) => *b,
            v => dg_core::py::truthy(v),
        },
        notes: p.notes.as_str().map(String::from).unwrap_or_default(),
    }
}

fn prefs_json(p: &Prefs) -> Value {
    json!({"version": p.version, "style": p.style, "private": p.private, "skip": p.skip,
           "flagStatus": p.flag_status, "notes": p.notes})
}

/// The shape AppControl's `prefs.get` returned.
fn control_json(p: &Prefs) -> Value {
    json!({"style": p.style, "flag_status": p.flag_status, "notes": p.notes, "names_only": p.private,
           "skip": p.skip})
}

/// A string the way Swift's JSONEncoder writes it (`/` escaped, other non-ASCII left as is).
fn swift_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '/' => out.push_str("\\/"),
            '\u{08}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{0c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn swift_list(items: &[String]) -> String {
    if items.is_empty() {
        return "[\n\n  ]".into();
    }
    let lines: Vec<String> = items.iter().map(|s| format!("    {}", swift_string(s))).collect();
    format!("[\n{}\n  ]", lines.join(",\n"))
}

/// preferences.json byte-for-byte as the app wrote it (`JSONEncoder`, `.prettyPrinted` + `.sortedKeys`).
pub fn prefs_render(p: &Prefs) -> String {
    format!(
        "{{\n  \"flagStatus\" : {},\n  \"notes\" : {},\n  \"private\" : {},\n  \"skip\" : {},\n  \"style\" : {},\n  \"version\" : {}\n}}",
        p.flag_status,
        swift_string(&p.notes),
        swift_list(&p.private),
        swift_list(&p.skip),
        swift_string(&p.style),
        p.version
    )
}

fn prefs_save(root: &Path, p: &Prefs) -> Result<(), String> {
    let dir = root.join(dg_core::rules::LOG_DIR);
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all(&dir)?;
        let file = dir.join("preferences.json");
        let tmp = dir.join(format!(".preferences.json.{}.tmp", std::process::id()));
        std::fs::write(&tmp, prefs_render(p))?;
        std::fs::rename(&tmp, &file)
    };
    write().map_err(|e| e.to_string())
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Include,
    NamesOnly,
    Skip,
}

/// `IndexPreferences.setMode`: a folder is in at most one list; both lists stay sorted.
pub fn set_mode(p: &mut Prefs, mode: Mode, path: &str) {
    p.skip.retain(|x| x != path);
    p.private.retain(|x| x != path);
    match mode {
        Mode::Include => {}
        Mode::NamesOnly => p.private.push(path.to_string()),
        Mode::Skip => p.skip.push(path.to_string()),
    }
    p.skip.sort();
    p.private.sort();
}

fn strings(v: &Value) -> Result<Vec<String>, String> {
    v.as_array()
        .ok_or("expected a list of folder paths")?
        .iter()
        .map(|s| s.as_str().map(String::from).ok_or_else(|| "expected a list of folder paths".to_string()))
        .collect()
}

/// A JSON bool, or a number the way NSNumber bridges to Bool.
fn boolish(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        Value::Number(n) => n.as_f64().map(|f| f != 0.0),
        _ => None,
    }
}

fn prefs_set_ui(engine: &Arc<Engine>, params: &Value) -> Result<Value, String> {
    let s = state(engine);
    let _guard = s.prefs.lock().unwrap();
    let root = s.root.lock().unwrap().clone();
    let mut p = prefs_load(&root);
    if let Some(d) = params.get("data").and_then(|d| d.as_object()) {
        apply_data(&mut p, d)?;
    }
    if let Some(folder) = params.get("folder").and_then(|f| f.as_str()) {
        let mode = match params.get("mode").and_then(|m| m.as_str()).unwrap_or("include") {
            "skip" => Mode::Skip,
            "names_only" | "private" => Mode::NamesOnly,
            "include" => Mode::Include,
            m => return Err(format!("Unknown folder mode {m}")),
        };
        set_mode(&mut p, mode, folder);
    }
    prefs_save(&root, &p)?;
    Ok(prefs_json(&p))
}

fn apply_data(p: &mut Prefs, d: &Map<String, Value>) -> Result<(), String> {
    for (k, v) in d {
        match k.as_str() {
            "version" => p.version = v.as_i64().ok_or("version must be a number")?,
            "style" => p.style = v.as_str().ok_or("style must be a string")?.to_string(),
            "private" => p.private = strings(v)?,
            "skip" => p.skip = strings(v)?,
            "flagStatus" => p.flag_status = boolish(v).ok_or("flagStatus must be a boolean")?,
            "notes" => p.notes = v.as_str().ok_or("notes must be a string")?.to_string(),
            _ => {}
        }
    }
    Ok(())
}

/// AppControl's `prefs.set`: unknown styles and wrong types are ignored, like before.
fn prefs_set_control(engine: &Arc<Engine>, params: &Value) -> Result<Value, String> {
    let s = state(engine);
    let p = {
        let _guard = s.prefs.lock().unwrap();
        let root = s.root.lock().unwrap().clone();
        let mut p = prefs_load(&root);
        if let Some(style) = params.get("style").and_then(|v| v.as_str()).filter(|s| ["terse", "sentence"].contains(s)) {
            p.style = style.to_string();
        }
        if let Some(flag) = params.get("flag_status").and_then(boolish) {
            p.flag_status = flag;
        }
        if let Some(notes) = params.get("notes").and_then(|v| v.as_str()) {
            p.notes = notes.to_string();
        }
        if let (Some(folder), Some(mode)) =
            (params.get("folder").and_then(|v| v.as_str()), params.get("folder_mode").and_then(|v| v.as_str()))
        {
            let m = match mode {
                "skip" => Mode::Skip,
                "names_only" | "private" => Mode::NamesOnly,
                _ => Mode::Include,
            };
            set_mode(&mut p, m, folder.trim_matches('/'));
        }
        prefs_save(&root, &p)?;
        p
    };
    engine.link.notify(PREFS_CHANGED, prefs_json(&p));
    Ok(control_json(&p))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[derive(Clone, Default)]
    struct Buf(Arc<Mutex<Vec<u8>>>);
    impl Write for Buf {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn engine(tag: &str) -> (crate::settings::tests::TempHome, Arc<Engine>, PathBuf, Buf) {
        let home = crate::settings::tests::temp_home(tag);
        let buf = Buf::default();
        let e = Engine::with_settings(Box::new(buf.clone()), crate::settings::Settings::open_at(home.path().join("s.json")));
        let root = home.path().join("Desktop");
        let w = |rel: &str, text: &str| {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        };
        w("Projects/description.md", "# Projects\n> Code\nTags: Swift\n## Contents\n- **App/** — the app\n");
        w("Projects/App/Cargo.toml", "");
        w("Projects/App/src/main.rs", "");
        w("Notes/a.txt", "");
        w("_Organize Log/questions.md", "- [ ] why?\n");
        set_root(&e, root.clone());
        (home, e, root, buf)
    }

    fn call(e: &Arc<Engine>, m: &str, p: Value) -> Result<Value, String> {
        e.handle_ui(m, &p)
    }

    #[test]
    fn load_list_search_describe() {
        let (_h, e, root, _) = engine("desktop-load");
        let r = call(&e, LOAD, json!({})).unwrap();
        assert_eq!(r["root"], json!(root.to_string_lossy()));
        assert_eq!(r["questions"], json!(1));
        assert_eq!(r["undescribed"], json!(1)); // Notes
        assert!(r["indexed"].is_null());
        let items = r["items"].as_array().unwrap();
        assert_eq!(items.iter().map(|i| i["name"].as_str().unwrap()).collect::<Vec<_>>(), ["Notes", "Projects"]);
        let projects = &items[1];
        assert_eq!(projects["info"]["tags"], json!(["swift"]));
        assert_eq!(projects["blurb"], json!("Code"));
        let app = &projects["children"][0];
        assert_eq!((app["kind"].as_str(), app["blurb"].as_str(), app["count"].as_u64()), (Some("project"), Some("the app"), Some(2)));
        assert!(app.get("children").is_none());

        let l = call(&e, LIST, json!({"path": app["path"]})).unwrap();
        let names: Vec<_> = l["items"].as_array().unwrap().iter().map(|i| (i["name"].clone(), i["kind"].clone())).collect();
        assert_eq!(names, [(json!("src"), json!("folder")), (json!("Cargo.toml"), json!("file"))]);
        assert!(call(&e, LIST, json!({"path": "/etc"})).unwrap_err().contains("inside the Desktop"));
        assert!(call(&e, LIST, json!({"path": format!("{}/../..", root.display())})).is_err());
        assert!(call(&e, LIST, json!({"path": "Projects/App"})).is_ok());

        let s = call(&e, SEARCH, json!({"query": "app"})).unwrap();
        assert_eq!(s["paths"], json!([app["path"]]));
        let s = call(&e, SEARCH, json!({"query": "  "})).unwrap();
        assert_eq!(s["paths"], json!([]));

        let d = call(&e, DESCRIBE, json!({"path": "Projects"})).unwrap();
        assert_eq!(d["info"]["title"], json!("Projects"));
        assert!(call(&e, DESCRIBE, json!({"path": "Notes"})).unwrap()["info"].is_null());
    }

    #[test]
    fn search_scans_when_nothing_is_loaded_yet() {
        let (_h, e, _, _) = engine("desktop-search");
        let s = call(&e, SEARCH, json!({"query": "swift"})).unwrap();
        assert_eq!(s["paths"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn prefs_round_trip_in_the_apps_format() {
        let (_h, e, root, buf) = engine("desktop-prefs");
        let file = root.join("_Organize Log/preferences.json");
        let p = call(&e, PREFS_GET, json!({})).unwrap();
        assert_eq!(p, json!({"version": 1, "style": "terse", "private": [], "skip": [], "flagStatus": true, "notes": ""}));
        assert!(!file.exists());
        call(&e, PREFS_SET, json!({})).unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "{\n  \"flagStatus\" : true,\n  \"notes\" : \"\",\n  \"private\" : [\n\n  ],\n  \"skip\" : [\n\n  ],\n  \"style\" : \"terse\",\n  \"version\" : 1\n}"
        );
        let p = call(&e, PREFS_SET, json!({"data": {"style": "sentence", "notes": "a/b \"q\"\n", "flagStatus": false}})).unwrap();
        assert_eq!(p["style"], json!("sentence"));
        call(&e, PREFS_SET, json!({"folder": "Projects/App", "mode": "skip"})).unwrap();
        call(&e, PREFS_SET, json!({"folder": "Notes", "mode": "names_only"})).unwrap();
        let p = call(&e, PREFS_SET, json!({"folder": "Archive", "mode": "skip"})).unwrap();
        assert_eq!(p["skip"], json!(["Archive", "Projects/App"]));
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "{\n  \"flagStatus\" : false,\n  \"notes\" : \"a\\/b \\\"q\\\"\\n\",\n  \"private\" : [\n    \"Notes\"\n  ],\n  \"skip\" : [\n    \"Archive\",\n    \"Projects\\/App\"\n  ],\n  \"style\" : \"sentence\",\n  \"version\" : 1\n}"
        );
        // a folder is in one list at most; include removes it
        let p = call(&e, PREFS_SET, json!({"folder": "Notes", "mode": "skip"})).unwrap();
        assert_eq!((p["private"].clone(), p["skip"].clone()), (json!([]), json!(["Archive", "Notes", "Projects/App"])));
        let p = call(&e, PREFS_SET, json!({"folder": "Archive", "mode": "include"})).unwrap();
        assert_eq!(p["skip"], json!(["Notes", "Projects/App"]));
        assert!(call(&e, PREFS_SET, json!({"folder": "x", "mode": "bogus"})).is_err());
        assert!(call(&e, PREFS_SET, json!({"data": {"skip": "nope"}})).is_err());
        // dg inventory reads the same file
        assert_eq!(dg_core::prefs::load(&root).skip, ["Notes", "Projects/App"]);
        assert!(!String::from_utf8_lossy(&buf.0.lock().unwrap()).contains(PREFS_CHANGED), "UI writes aren't echoed");
    }

    #[test]
    fn control_prefs_match_appcontrol() {
        let (_h, e, _root, buf) = engine("desktop-control");
        let r = e.handle_control("prefs.get", &json!({})).unwrap();
        assert_eq!(r, json!({"style": "terse", "flag_status": true, "notes": "", "names_only": [], "skip": []}));
        // gated like before
        assert!(e.handle_control("prefs.set", &json!({"style": "sentence"})).unwrap_err().contains("turned off"));
        let mut allow = Map::new();
        allow.insert("mcp.allow.control".into(), json!(true));
        e.settings.set(&allow).unwrap();
        let r = e
            .handle_control(
                "prefs.set",
                &json!({"style": "fancy", "flag_status": false, "notes": "n", "folder": "/Projects/", "folder_mode": "private"}),
            )
            .unwrap();
        assert_eq!(r, json!({"style": "terse", "flag_status": false, "notes": "n", "names_only": ["Projects"], "skip": []}));
        let r = e.handle_control("prefs.set", &json!({"folder": "Projects", "folder_mode": "whatever"})).unwrap();
        assert_eq!(r["names_only"], json!([]));
        let out = String::from_utf8_lossy(&buf.0.lock().unwrap()).into_owned();
        assert!(out.contains(PREFS_CHANGED) && out.contains("\"flagStatus\":false"), "{out}");
    }
}
