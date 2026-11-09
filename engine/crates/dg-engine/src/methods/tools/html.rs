//! The visual HTML editor's and the `html_*` MCP tools' engine (dg-core `html`, ported from HTMLSource/HTMLOps).
//!
//! Every method takes the page `path` and, optionally, `source` (the page text) and `buffers` (`{path: text}`): the
//! text of files open in the UI's editor, which may be unsaved. Files not in `buffers` are read from disk. Edits are
//! returned as `{message, structural, files: [{path, written, edits: [{location, length, text}]}]}` with UTF-16
//! ranges (`NSRange`); with `write: true` the engine writes the files that aren't open itself (`written: true`) and
//! the UI applies the rest to its open documents as one undoable change each.
//!
//! UI link methods:
//! - `html/parse {path, source?}` → `{annotated, stylesheets: [path], nodes: [{index, tag, label, path: [int], parent,
//!   children, selectable, startTag, endTag, id, classes, inline: [[k, v]], textOnly}]}`
//! - `html/declarations {path, source?, buffers?, destination, selector}` → `[[k, v]]`
//! - `html/outline {path, source?, buffers?}` → string
//! - `html/edit {path, source?, buffers?, write?, op, element|elements, …}` → edits (ops below)
//! - `html/control {method, params, buffers?}` → `{result, files}`: an `html.*` control-channel call (MCP
//!   `html_*` tools) with its MCP parameters; `files` holds the edits the UI must apply to its open buffers.
//!
//! `destination` is `"inline"`, `"styleBlock"` or a stylesheet path. Ops for `html/edit`: `style {element,
//! destination, selector?, properties: {k: v|null}}`, `split_style {element, baseClass, newClass, copy, moveInline,
//! destination}`, `group_style {elements, className, properties: {k: v}, moveInline, destination}`, `add_class
//! {element, name}`, `set_id {element, id}`, `set_text {element, text}`, `set_html {element, html}`, `duplicate
//! {element}`, `remove {element}`. `element` is a node index (or any `html_outline` spec string).

use super::{bool_param, required, str_param};
use crate::Engine;
use dg_core::artifact_policy::{self, Target};
use dg_core::html::{self as h, declarations, Changes, FileEdits, HtmlContext, HtmlSourceMap, StyleDestination};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

pub fn handle(_engine: &Arc<Engine>, method: &str, p: &Value) -> Option<Result<Value, String>> {
    Some(match method {
        "html/parse" => parse(p),
        "html/declarations" => declarations_at(p),
        "html/outline" => page(p).map(|ctx| Value::String(h::outline(&ctx, 400))),
        "html/edit" => edit(p),
        "html/control" => control(p),
        _ => return None,
    })
}

// ---- reading files: open buffers first, then disk ----

/// Lexical `standardizedFileURL.path`: removes `.` and resolves `..` (how the UI matches open documents).
fn standardize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn buffers(p: &Value) -> HashMap<PathBuf, String> {
    p.get("buffers")
        .and_then(|b| b.as_object())
        .map(|m| {
            m.iter().filter_map(|(k, v)| v.as_str().map(|t| (standardize(Path::new(k)), t.to_string()))).collect()
        })
        .unwrap_or_default()
}

fn reader(buffers: HashMap<PathBuf, String>) -> Box<dyn Fn(&Path) -> Option<String>> {
    Box::new(move |p: &Path| buffers.get(&standardize(p)).cloned().or_else(|| std::fs::read_to_string(p).ok()))
}

/// The page context: `path` + `source` (or its buffer / file), stylesheets read through `buffers`.
fn page(p: &Value) -> Result<HtmlContext, String> {
    let path = PathBuf::from(required(p, "path")?);
    let bufs = buffers(p);
    let read = reader(bufs);
    let text = match str_param(p, "source") {
        Some(s) => s.to_string(),
        None => read(&path).ok_or_else(|| format!("Can't read {}", path.display()))?,
    };
    Ok(HtmlContext::new(path, text, read))
}

fn destination(p: &Value, ctx: &HtmlContext) -> StyleDestination {
    StyleDestination::parse(str_param(p, "destination").unwrap_or(""), &ctx.path)
}

// ---- html/parse ----

fn parse(p: &Value) -> Result<Value, String> {
    let ctx = page(p)?;
    let m = &ctx.map;
    let nodes: Vec<Value> = m
        .nodes
        .iter()
        .map(|n| {
            let i = n.index;
            json!({
                "index": i, "tag": n.tag, "label": m.label(i), "path": m.path(i), "parent": n.parent,
                "children": n.children, "selectable": n.selectable, "startTag": n.start_tag.to_json(),
                "endTag": n.end_tag.map(|e| e.to_json()), "id": m.attribute("id", i), "classes": m.classes(i),
                "inline": declarations::to_json(&h::inline_declarations(&ctx, i)), "textOnly": m.is_text_only(i),
            })
        })
        .collect();
    Ok(json!({
        "annotated": m.annotated(),
        "stylesheets": ctx.stylesheets().iter().map(|s| s.to_string_lossy().into_owned()).collect::<Vec<_>>(),
        "nodes": nodes,
    }))
}

// ---- html/declarations ----

fn declarations_at(p: &Value) -> Result<Value, String> {
    let ctx = page(p)?;
    let selector = str_param(p, "selector").unwrap_or("");
    if selector.is_empty() {
        return Ok(json!([]));
    }
    Ok(declarations::to_json(&h::declarations_at(&ctx, &destination(p, &ctx), selector)))
}

// ---- html/edit ----

fn element_of(v: Option<&Value>, map: &HtmlSourceMap) -> Result<usize, String> {
    let i = match v {
        Some(Value::Number(n)) => n.as_u64().map(|n| n as usize),
        Some(Value::String(s)) => map.resolve(s),
        _ => None,
    };
    match i {
        Some(i) if i < map.nodes.len() => Ok(i),
        _ => Err(format!("No element {}", v.map(|v| v.to_string()).unwrap_or_else(|| "given".into()))),
    }
}

/// `{k: v|null}` → changes (a null or blank value removes the property).
fn changes(v: Option<&Value>) -> Changes {
    let mut out = Changes::new();
    for (k, v) in v.and_then(|v| v.as_object()).into_iter().flatten() {
        out.insert(k.clone(), v.as_str().filter(|s| !s.is_empty()).map(String::from));
    }
    out
}

fn compute(ctx: &HtmlContext, p: &Value) -> Result<FileEdits, String> {
    let op = required(p, "op")?;
    let el = || element_of(p.get("element"), &ctx.map);
    let dest = destination(p, ctx);
    let r = match op {
        "style" => {
            let sel = str_param(p, "selector");
            h::set_properties(ctx, el()?, &dest, sel, &changes(p.get("properties")))
        }
        "split_style" => h::split_style(
            ctx,
            el()?,
            str_param(p, "baseClass").unwrap_or(""),
            str_param(p, "newClass").unwrap_or(""),
            bool_param(p, "copy", true),
            bool_param(p, "moveInline", true),
            &dest,
        ),
        "group_style" => {
            let indices = p
                .get("elements")
                .and_then(|e| e.as_array())
                .map(|a| a.iter().map(|v| element_of(Some(v), &ctx.map)).collect::<Result<Vec<_>, _>>())
                .transpose()?
                .unwrap_or_default();
            let props: BTreeMap<String, String> =
                changes(p.get("properties")).into_iter().filter_map(|(k, v)| v.map(|v| (k, v))).collect();
            h::group_style(ctx, &indices, str_param(p, "className").unwrap_or(""), &props, bool_param(p, "moveInline", true), &dest)
        }
        "add_class" => Ok(h::add_class(ctx, el()?, required(p, "name")?)),
        "set_id" => Ok(h::set_id(ctx, el()?, required(p, "id")?)),
        "set_text" => h::set_text(ctx, el()?, str_param(p, "text").unwrap_or("")),
        "set_html" => h::set_inner_html(ctx, el()?, str_param(p, "html").unwrap_or("")),
        "duplicate" => Ok(h::duplicate(ctx, el()?)),
        "remove" => Ok(h::remove(ctx, el()?)),
        other => return Err(format!("Unknown html op {other}")),
    };
    r.map_err(|e| e.0)
}

/// The edits as JSON; with `write`, files that aren't open buffers are written to disk here (`written: true`).
fn finish(edits: &FileEdits, open: &HashMap<PathBuf, String>, write: bool) -> Result<Value, String> {
    let mut files = vec![];
    for (path, list) in &edits.by_file {
        let is_open = open.contains_key(&standardize(path));
        let written = write && !is_open;
        if written {
            let text = std::fs::read_to_string(path).map_err(|e| format!("Can't read {}: {e}", path.display()))?;
            write_atomically(path, &h::apply(list, &text))?;
        }
        files.push(json!({
            "path": path.to_string_lossy(), "written": written,
            "edits": list.iter().map(|e| json!({"location": e.range.location, "length": e.range.length, "text": e.text}))
                .collect::<Vec<_>>(),
        }));
    }
    Ok(json!({"message": edits.message, "structural": edits.structural, "files": files}))
}

fn write_atomically(path: &Path, text: &str) -> Result<(), String> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.dg-{}", std::process::id()));
    std::fs::write(&tmp, text)
        .and_then(|_| std::fs::rename(&tmp, path))
        .map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("Can't write {}: {e}", path.display())
        })
}

fn edit(p: &Value) -> Result<Value, String> {
    let ctx = page(p)?;
    let edits = compute(&ctx, p)?;
    let mut open = buffers(p);
    // The page itself counts as open when the UI sent its text.
    if p.get("source").is_some() {
        open.entry(standardize(&ctx.path)).or_default();
    }
    finish(&edits, &open, bool_param(p, "write", false))
}

// ---- html/control: the html.* control-channel methods with their MCP parameters (was AppControl.swift) ----

fn control(p: &Value) -> Result<Value, String> {
    let method = required(p, "method")?;
    let params = p.get("params").cloned().unwrap_or(Value::Object(Map::new()));
    let open = buffers(p);
    let ctx = mcp_context(&params, open.clone(), &dg_platform::desktop())?;
    let s = |k: &str| str_param(&params, k);
    let edits = match method {
        "html.outline" => return Ok(json!({"result": h::outline(&ctx, 400), "files": []})),
        "html.style" => {
            let i = mcp_element(params.get("element"), &ctx)?;
            let dest = mcp_destination(&params, &ctx, true)?;
            let selector = if dest == StyleDestination::Inline { None } else { Some(rule_selector(&params, &ctx, i)?) };
            h::set_properties(&ctx, i, &dest, selector.as_deref(), &mcp_properties(params.get("properties")))
        }
        "html.split_style" => {
            let i = mcp_element(params.get("element"), &ctx)?;
            let base = s("base_class").map(String::from).or_else(|| ctx.map.classes(i).into_iter().next()).unwrap_or_default();
            let new_class = s("new_class").filter(|c| !c.is_empty()).ok_or("new_class is required")?;
            let copy = s("mode").unwrap_or("copy") != "variant";
            let move_inline = bool_param(&params, "move_inline", true);
            h::split_style(&ctx, i, &base, new_class, copy, move_inline, &mcp_destination(&params, &ctx, false)?)
        }
        "html.group_style" => {
            let specs: Vec<Value> = params.get("elements").and_then(|e| e.as_array()).cloned().unwrap_or_default();
            // Swift: `p["elements"] as? [String] ?? []` — an array with a non-string drops the whole list.
            let specs: Vec<Value> = if specs.iter().all(|v| v.is_string()) { specs } else { vec![] };
            let indices = specs.iter().map(|v| mcp_element(Some(v), &ctx)).collect::<Result<Vec<_>, _>>()?;
            let name = s("class_name").filter(|c| !c.is_empty()).ok_or("class_name is required")?;
            let props: BTreeMap<String, String> =
                mcp_properties(params.get("properties")).into_iter().filter_map(|(k, v)| v.map(|v| (k, v))).collect();
            let move_inline = bool_param(&params, "move_inline", true);
            h::group_style(&ctx, &indices, name, &props, move_inline, &mcp_destination(&params, &ctx, false)?)
        }
        "html.set_text" => h::set_text(&ctx, mcp_element(params.get("element"), &ctx)?, s("text").unwrap_or("")),
        "html.set_html" => h::set_inner_html(&ctx, mcp_element(params.get("element"), &ctx)?, s("html").unwrap_or("")),
        "html.duplicate" => Ok(h::duplicate(&ctx, mcp_element(params.get("element"), &ctx)?)),
        "html.remove" => Ok(h::remove(&ctx, mcp_element(params.get("element"), &ctx)?)),
        other => return Err(format!("Unknown method {other}")),
    }
    .map_err(|e| e.0)?;
    let out = finish(&edits, &open, true)?;
    let mut names: Vec<String> = edits
        .by_file
        .iter()
        .map(|(p, _)| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
        .collect();
    names.sort();
    names.dedup();
    let unwritten: Vec<Value> =
        out["files"].as_array().into_iter().flatten().filter(|f| f["written"] == json!(false)).cloned().collect();
    Ok(json!({"result": format!("{} (wrote {})", edits.message, names.join(", ")), "files": unwritten}))
}

/// `AppControl.htmlContext`: `path` must resolve (absolute, ~/ or Desktop-relative) to an .html/.htm file.
fn mcp_context(params: &Value, open: HashMap<PathBuf, String>, desktop: &Path) -> Result<HtmlContext, String> {
    let bad = || "path must be an .html file (absolute, ~/ or Desktop-relative)".to_string();
    let raw = str_param(params, "path").ok_or_else(bad)?;
    let path = match artifact_policy::resolve(raw, desktop).ok_or_else(bad)? {
        Target::File(p) => p,
        Target::Url(u) => {
            let parts = artifact_policy::parse(&u);
            if parts.scheme.as_deref().map(|s| s.eq_ignore_ascii_case("file")) != Some(true) {
                return Err(bad());
            }
            PathBuf::from(parts.path)
        }
    };
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if ext != "html" && ext != "htm" {
        return Err(bad());
    }
    let read = reader(open);
    let text = read(&path).ok_or_else(|| format!("Can't read {}", path.display()))?;
    Ok(HtmlContext::new(path, text, read))
}

fn mcp_element(spec: Option<&Value>, ctx: &HtmlContext) -> Result<usize, String> {
    if let Some(Value::String(s)) = spec {
        if let Some(i) = ctx.map.resolve(s) {
            if ctx.map.nodes[i].selectable {
                return Ok(i);
            }
        }
    }
    let shown = match spec {
        None | Some(Value::Null) => "nil".to_string(),
        Some(Value::String(s)) => s.clone(),
        Some(v) => v.to_string(),
    };
    Err(format!("No element matches {shown} — use a path from html_outline, #id, .class, tag or tag:N"))
}

fn mcp_destination(params: &Value, ctx: &HtmlContext, allow_inline: bool) -> Result<StyleDestination, String> {
    match str_param(params, "to").unwrap_or("").to_lowercase().as_str() {
        "inline" => {
            if !allow_inline {
                return Err("This needs a rule: use to=stylesheet or to=style_block".into());
            }
            Ok(StyleDestination::Inline)
        }
        "style_block" | "style" | "html" => Ok(StyleDestination::StyleBlock),
        "stylesheet" => {
            let sheets = ctx.stylesheets();
            let file_name = |p: &PathBuf| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if let Some(name) = str_param(params, "stylesheet").filter(|n| !n.is_empty()) {
                let suffix = format!("/{name}");
                return sheets
                    .iter()
                    .find(|s| file_name(s) == name || s.to_string_lossy().ends_with(&suffix))
                    .map(|s| StyleDestination::Stylesheet(s.clone()))
                    .ok_or_else(|| {
                        let linked: Vec<String> = sheets.iter().map(|s| format!("\"{}\"", file_name(s))).collect();
                        format!("{name} isn't a local stylesheet linked from this page. Linked: [{}]", linked.join(", "))
                    });
            }
            sheets
                .first()
                .map(|s| StyleDestination::Stylesheet(s.clone()))
                .ok_or_else(|| "No local stylesheet is linked — use to=style_block".into())
        }
        _ => Ok(ctx.default_destination()),
    }
}

fn rule_selector(params: &Value, ctx: &HtmlContext, i: usize) -> Result<String, String> {
    if let Some(s) = str_param(params, "selector").filter(|s| !s.is_empty()) {
        return Ok(s.to_string());
    }
    if let Some(c) = ctx.map.classes(i).first() {
        return Ok(format!(".{c}"));
    }
    if let Some(id) = ctx.map.attribute("id", i).filter(|id| !id.is_empty()) {
        return Ok(format!("#{id}"));
    }
    Err("That element has no class or id — pass selector (e.g. .card), or to=inline".into())
}

/// `AppControl.properties`: keys lowercased; strings as given, null → removed, numbers/bools as Swift prints them.
fn mcp_properties(raw: Option<&Value>) -> Changes {
    let mut out = Changes::new();
    for (k, v) in raw.and_then(|r| r.as_object()).into_iter().flatten() {
        let s = match v {
            Value::String(s) => s.clone(),
            Value::Null => String::new(),
            Value::Bool(b) => if *b { "1" } else { "0" }.to_string(),
            other => other.to_string(),
        };
        out.insert(k.to_lowercase(), if s.is_empty() { None } else { Some(s) });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{engine, temp_dir};
    use super::*;

    fn call(method: &str, p: Value) -> Result<Value, String> {
        let dir = temp_dir("html-call");
        let e = engine(&dir);
        let r = handle(&e, method, &p).expect("handled");
        let _ = std::fs::remove_dir_all(&dir);
        r
    }

    const PAGE: &str = "<html><head>\n  <link rel=\"stylesheet\" href=\"site.css\">\n</head>\n<body>\n  <h1 class=\"title\">Hello</h1>\n  <p id=\"lead\" style=\"color: red\">Some <b>bold</b> text</p>\n</body></html>\n";

    fn project() -> (PathBuf, PathBuf, PathBuf) {
        let dir = temp_dir("html");
        let page = dir.join("index.html");
        let css = dir.join("site.css");
        std::fs::write(&page, PAGE).unwrap();
        std::fs::write(&css, ".title {\n  color: blue;\n}\n").unwrap();
        (dir, page, css)
    }

    #[test]
    fn parse_reports_nodes_stylesheets_and_annotated_source() {
        let (dir, page, css) = project();
        let r = call("html/parse", json!({"path": page, "source": PAGE})).unwrap();
        assert_eq!(r["stylesheets"], json!([css.to_string_lossy()]));
        let nodes = r["nodes"].as_array().unwrap();
        let h1 = nodes.iter().find(|n| n["tag"] == "h1").unwrap();
        assert_eq!(h1["classes"], json!(["title"]));
        assert_eq!(h1["label"], "h1.title");
        assert_eq!(h1["textOnly"], true);
        let p = nodes.iter().find(|n| n["tag"] == "p").unwrap();
        assert_eq!(p["id"], "lead");
        assert_eq!(p["inline"], json!([["color", "red"]]));
        assert_eq!(p["textOnly"], false);
        assert!(r["annotated"].as_str().unwrap().contains(&format!("data-dg-id=\"{}\"", h1["index"])));
        // The path is the position in the tree, root first.
        let n = &nodes[h1["index"].as_u64().unwrap() as usize];
        assert_eq!(n["path"], json!([0, 1, 0]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn edits_use_open_buffers_and_write_closed_files() {
        let (dir, page, css) = project();
        let parsed = call("html/parse", json!({"path": page, "source": PAGE})).unwrap();
        let h1 = parsed["nodes"].as_array().unwrap().iter().find(|n| n["tag"] == "h1").unwrap()["index"].clone();

        // The stylesheet is closed: the engine writes it; the page buffer gets nothing.
        let r = call("html/edit", json!({"path": page, "source": PAGE, "op": "style", "element": h1,
            "destination": css, "selector": ".title", "properties": {"color": "green"}, "write": true})).unwrap();
        assert_eq!(r["message"], ".title in site.css");
        assert_eq!(r["files"][0]["written"], true);
        assert_eq!(std::fs::read_to_string(&css).unwrap(), ".title {\n  color: green;\n}\n");

        // With the stylesheet open (unsaved text), its buffer is read and nothing is written.
        let unsaved = ".title { color: pink; }\n";
        let r = call("html/edit", json!({"path": page, "source": PAGE, "op": "style", "element": h1,
            "destination": css, "selector": ".title", "properties": {"font-size": "2em"}, "write": true,
            "buffers": {css.to_string_lossy(): unsaved}})).unwrap();
        assert_eq!(r["files"][0]["written"], false);
        let e = &r["files"][0]["edits"][0];
        let applied = h::apply(
            &[h::Edit::new(h::NsRange::new(e["location"].as_u64().unwrap() as usize, e["length"].as_u64().unwrap() as usize),
                e["text"].as_str().unwrap())],
            unsaved,
        );
        assert_eq!(applied, ".title {\n  color: pink;\n  font-size: 2em;\n}\n");
        assert_eq!(std::fs::read_to_string(&css).unwrap(), ".title {\n  color: green;\n}\n", "disk untouched");

        // The page itself (sent as source) is never written by the engine.
        let r = call("html/edit", json!({"path": page, "source": PAGE, "op": "set_text", "element": h1, "text": "A & B",
            "write": true})).unwrap();
        assert_eq!(r["files"][0]["written"], false);
        assert_eq!(r["files"][0]["edits"][0]["text"], "A &amp; B");
        assert_eq!(std::fs::read_to_string(&page).unwrap(), PAGE);

        // Refusals come back as errors with the Swift message.
        let p_index = parsed["nodes"].as_array().unwrap().iter().find(|n| n["tag"] == "p").unwrap()["index"].clone();
        let err = call("html/edit", json!({"path": page, "source": PAGE, "op": "set_html", "element": p_index,
            "html": "<div>x</div>"})).unwrap_err();
        assert!(err.contains("isn't allowed in text"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn declarations_at_destinations() {
        let (dir, page, css) = project();
        let r = call("html/declarations", json!({"path": page, "source": PAGE, "destination": css, "selector": ".title"}))
            .unwrap();
        assert_eq!(r, json!([["color", "blue"]]));
        let r = call("html/declarations", json!({"path": page, "source": PAGE, "destination": "styleBlock",
            "selector": ".title"})).unwrap();
        assert_eq!(r, json!([]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn control_methods_match_appcontrol() {
        let (dir, page, css) = project();
        let pp = page.to_string_lossy().into_owned();
        let outline = call("html/control", json!({"method": "html.outline", "params": {"path": pp}})).unwrap();
        assert!(outline["result"].as_str().unwrap().starts_with("Stylesheets: site.css"));

        // Default destination is the linked stylesheet; the selector defaults to the first class.
        let r = call("html/control", json!({"method": "html.style",
            "params": {"path": pp, "element": "h1", "properties": {"Color": "teal", "margin": 0}}})).unwrap();
        assert_eq!(r["result"], ".title in site.css (wrote site.css)");
        assert_eq!(r["files"], json!([]));
        assert_eq!(std::fs::read_to_string(&css).unwrap(), ".title {\n  color: teal;\n  margin: 0;\n}\n");

        // An open page buffer is edited by the UI, not written.
        let r = call("html/control", json!({"method": "html.style", "buffers": {pp.clone(): PAGE},
            "params": {"path": pp, "element": "#lead", "to": "inline", "properties": {"color": null}}})).unwrap();
        assert_eq!(r["result"], "Inline style on <p#lead> (wrote index.html)");
        assert_eq!(r["files"][0]["path"], pp);
        assert_eq!(std::fs::read_to_string(&page).unwrap(), PAGE);

        for (params, want) in [
            (json!({"path": pp, "element": "nav"}), "No element matches nav — use a path from html_outline"),
            (json!({"path": pp}), "No element matches nil"),
            (json!({"path": dir.join("x.txt")}), "path must be an .html file"),
            (json!({"path": pp, "element": "h1", "to": "stylesheet", "stylesheet": "other.css"}),
                "other.css isn't a local stylesheet linked from this page. Linked: [\"site.css\"]"),
        ] {
            let err = call("html/control", json!({"method": "html.style", "params": params})).unwrap_err();
            assert!(err.starts_with(want), "{err}");
        }
        let err = call("html/control", json!({"method": "html.split_style", "params": {"path": pp, "element": "h1"}}))
            .unwrap_err();
        assert_eq!(err, "new_class is required");
        let err = call("html/control", json!({"method": "html.split_style",
            "params": {"path": pp, "element": "h1", "new_class": "x", "to": "inline"}})).unwrap_err();
        assert_eq!(err, "This needs a rule: use to=stylesheet or to=style_block");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
