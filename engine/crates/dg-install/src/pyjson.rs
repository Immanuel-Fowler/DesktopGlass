//! JSON files the way mcp_install.py reads and writes them, so a config file comes out byte-identical:
//! `json.dumps(data, indent=2) + "\n"` (ASCII-escaped, key order kept), one-time `.desktopglass-backup`, write to a
//! temporary file and rename. A file that isn't a plain JSON object is never overwritten: `read_json` fails with
//! `Error::Parse` and the caller reports the app as skipped.

use crate::Error;
use serde_json::{Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

pub type Obj = Map<String, Value>;

/// `{}` for a missing or blank file; the object it holds otherwise; `Error::Parse` for anything else.
pub fn read_json(path: &Path) -> Result<Obj, Error> {
    if !path.exists() {
        return Ok(Obj::new());
    }
    let bytes = fs::read(path).map_err(|e| Error::Other(e.to_string()))?;
    let text = String::from_utf8(bytes).map_err(|e| Error::Parse(e.to_string()))?;
    if text.trim().is_empty() {
        return Ok(Obj::new());
    }
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(o)) => Ok(o),
        Ok(_) => Err(Error::Parse("not a JSON object".into())),
        Err(e) => Err(Error::Parse(e.to_string())),
    }
}

/// Writes `data` (indent 2, trailing newline), backing up an existing file once.
pub fn write_json(path: &Path, data: &Obj) -> Result<(), Error> {
    let io = |e: std::io::Error| Error::Other(e.to_string());
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(io)?;
    }
    if path.exists() {
        let backup = sibling(path, ".desktopglass-backup");
        if !backup.exists() {
            fs::copy(path, &backup).map_err(io)?;
        }
    }
    let tmp = sibling(path, ".tmp");
    fs::write(&tmp, dumps(&Value::Object(data.clone()), Some(2)) + "\n").map_err(io)?;
    fs::rename(&tmp, path).map_err(io)
}

/// `path` with `suffix` appended to its file name.
pub fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

/// `json.dumps(v)` (`indent=None`: ", " and ": " separators) or `json.dumps(v, indent=n)`, ASCII-escaped.
pub fn dumps(v: &Value, indent: Option<usize>) -> String {
    let mut out = String::new();
    write(&mut out, v, indent, 0);
    out
}

fn write(out: &mut String, v: &Value, indent: Option<usize>, level: usize) {
    let (open, close, items): (char, char, Vec<(Option<&String>, &Value)>) = match v {
        Value::Array(a) => ('[', ']', a.iter().map(|x| (None, x)).collect()),
        Value::Object(o) => ('{', '}', o.iter().map(|(k, x)| (Some(k), x)).collect()),
        leaf => {
            out.push_str(&dg_core::py::dumps(leaf, true));
            return;
        }
    };
    out.push(open);
    if items.is_empty() {
        out.push(close);
        return;
    }
    for (i, (k, x)) in items.iter().enumerate() {
        match indent {
            Some(n) => {
                out.push_str(if i > 0 { ",\n" } else { "\n" });
                out.push_str(&" ".repeat(n * (level + 1)));
            }
            None if i > 0 => out.push_str(", "),
            None => {}
        }
        if let Some(k) = k {
            out.push_str(&dg_core::py::dumps(&Value::String((*k).clone()), true));
            out.push_str(": ");
        }
        write(out, x, indent, level + 1);
    }
    if let Some(n) = indent {
        out.push('\n');
        out.push_str(&" ".repeat(n * level));
    }
    out.push(close);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn matches_python_formatting() {
        let v = json!({"a": [], "b": {}, "c": [1, 2.5, "é"], "d": {"x": null, "y": true}});
        assert_eq!(dumps(&v, None), r#"{"a": [], "b": {}, "c": [1, 2.5, "\u00e9"], "d": {"x": null, "y": true}}"#);
        assert_eq!(
            dumps(&v, Some(2)),
            "{\n  \"a\": [],\n  \"b\": {},\n  \"c\": [\n    1,\n    2.5,\n    \"\\u00e9\"\n  ],\n  \"d\": {\n    \"x\": null,\n    \"y\": true\n  }\n}"
        );
    }
}
