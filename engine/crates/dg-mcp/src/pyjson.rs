//! Python-compatible JSON and value helpers, so every byte `dg mcp` writes matches `mcp_server.py`:
//! `json.dumps` with its default separators (`", "`, `": "`), `indent=2`, `ensure_ascii`, and the bits of Python
//! semantics the tools rely on (`int()`, `float()`, `str()`, truthiness, slicing, `format(x, "g")`).

use dg_core::py::{float_repr, py_str, truthy};
use serde_json::Value;
use std::fmt::Write;

/// `json.dumps(v, indent=indent, ensure_ascii=ascii)` with the default separators.
pub fn dumps(v: &Value, indent: Option<usize>, ascii: bool) -> String {
    let mut out = String::with_capacity(1024);
    write_value(&mut out, v, indent, 0, ascii);
    out
}

fn newline(out: &mut String, indent: Option<usize>, level: usize) {
    if let Some(n) = indent {
        out.push('\n');
        out.push_str(&" ".repeat(n * level));
    }
}

fn write_value(out: &mut String, v: &Value, indent: Option<usize>, level: usize, ascii: bool) {
    let item_sep = if indent.is_some() { "," } else { ", " };
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&number(n)),
        Value::String(s) => write_str(out, s, ascii),
        Value::Array(a) if a.is_empty() => out.push_str("[]"),
        Value::Object(o) if o.is_empty() => out.push_str("{}"),
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push_str(item_sep);
                }
                newline(out, indent, level + 1);
                write_value(out, x, indent, level + 1, ascii);
            }
            newline(out, indent, level);
            out.push(']');
        }
        Value::Object(o) => {
            out.push('{');
            for (i, (k, x)) in o.iter().enumerate() {
                if i > 0 {
                    out.push_str(item_sep);
                }
                newline(out, indent, level + 1);
                write_str(out, k, ascii);
                out.push_str(": ");
                write_value(out, x, indent, level + 1, ascii);
            }
            newline(out, indent, level);
            out.push('}');
        }
    }
}

fn number(n: &serde_json::Number) -> String {
    if n.is_f64() {
        float_repr(n.as_f64().unwrap_or(0.0))
    } else {
        n.to_string()
    }
}

fn write_str(out: &mut String, s: &str, ascii: bool) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c if ascii && !(' '..='~').contains(&c) => {
                let mut units = [0u16; 2];
                for u in c.encode_utf16(&mut units) {
                    let _ = write!(out, "\\u{:04x}", u);
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// `as_text`: strings as they are, anything else as `json.dumps(v, indent=2, ensure_ascii=False)`.
pub fn as_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => dumps(other, Some(2), false),
    }
}

/// Python's type name for a JSON value, for error messages.
pub fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// `f"{x}"` / `str(x)`, with a missing value shown as `None` (what `dict.get` returns).
pub fn disp(v: Option<&Value>) -> String {
    match v {
        Some(v) => py_str(v),
        None => "None".into(),
    }
}

/// `repr()` of a str, the way a KeyError or ValueError message quotes it.
pub fn repr_str(s: &str) -> String {
    py_str(&Value::Array(vec![Value::String(s.to_string())])).trim_start_matches('[').trim_end_matches(']').to_string()
}

/// `x or default`: the value when it is truthy, else `None`.
pub fn or(v: Option<&Value>) -> Option<&Value> {
    v.filter(|v| truthy(v))
}

/// `int(v)`
pub fn py_int(v: &Value) -> Result<i64, String> {
    match v {
        Value::Bool(b) => Ok(*b as i64),
        Value::Number(n) => match n.as_i64() {
            Some(i) => Ok(i),
            None => {
                let f = n.as_f64().unwrap_or(0.0);
                if f.is_finite() {
                    Ok(f.trunc() as i64)
                } else {
                    Err("cannot convert float infinity to integer".into())
                }
            }
        },
        Value::String(s) => {
            let t = dg_core::py::strip(s).replace('_', "");
            t.parse::<i64>().map_err(|_| format!("invalid literal for int() with base 10: {}", repr_str(s)))
        }
        other => Err(format!(
            "int() argument must be a string, a bytes-like object or a real number, not '{}'",
            type_name(other)
        )),
    }
}

/// `float(v)`
pub fn py_float(v: &Value) -> Result<f64, String> {
    match v {
        Value::Bool(b) => Ok(*b as i64 as f64),
        Value::Number(n) => Ok(n.as_f64().unwrap_or(0.0)),
        Value::String(s) => {
            let t = dg_core::py::strip(s).replace('_', "").to_ascii_lowercase();
            let t = t.as_str();
            match t {
                "inf" | "+inf" | "infinity" | "+infinity" => Ok(f64::INFINITY),
                "-inf" | "-infinity" => Ok(f64::NEG_INFINITY),
                "nan" | "+nan" | "-nan" => Ok(f64::NAN),
                _ => t.parse::<f64>().map_err(|_| format!("could not convert string to float: {}", repr_str(s))),
            }
        }
        other => Err(format!("float() argument must be a string or a real number, not '{}'", type_name(other))),
    }
}

/// The end index of `seq[:n]` for a sequence of length `len`.
pub fn slice_end(len: usize, n: i64) -> usize {
    if n >= 0 {
        (n as usize).min(len)
    } else {
        len.saturating_sub(n.unsigned_abs() as usize)
    }
}

/// `format(x, "g")` (same as C's `%g`).
pub fn format_g(x: f64) -> String {
    let mut buf = [0u8; 64];
    let fmt = b"%g\0";
    let n = unsafe { libc::snprintf(buf.as_mut_ptr() as *mut libc::c_char, buf.len(), fmt.as_ptr() as *const libc::c_char, x) };
    String::from_utf8_lossy(&buf[..(n.max(0) as usize).min(buf.len() - 1)]).into_owned()
}

/// `for x in v` over a JSON value, the way Python iterates it.
pub fn py_iter(v: &Value) -> Result<Vec<Value>, String> {
    match v {
        Value::Array(a) => Ok(a.clone()),
        Value::String(s) => Ok(s.chars().map(|c| Value::String(c.to_string())).collect()),
        Value::Object(o) => Ok(o.keys().map(|k| Value::String(k.clone())).collect()),
        other => Err(format!("'{}' object is not iterable", type_name(other))),
    }
}

/// `v[key]` on a value expected to be a dict.
pub fn index<'a>(v: &'a Value, key: &str) -> Result<&'a Value, String> {
    match v {
        Value::Object(o) => o.get(key).ok_or_else(|| repr_str(key)),
        Value::String(_) => Err("string indices must be integers, not 'str'".into()),
        Value::Array(_) => Err("list indices must be integers or slices, not str".into()),
        other => Err(format!("'{}' object is not subscriptable", type_name(other))),
    }
}

/// `v.get(key)` on a value expected to be a dict.
pub fn get<'a>(v: &'a Value, key: &str) -> Result<Option<&'a Value>, String> {
    match v {
        Value::Object(o) => Ok(o.get(key)),
        other => Err(format!("'{}' object has no attribute 'get'", type_name(other))),
    }
}

/// `str(v.get(key, ""))`
pub fn get_text(v: &Value, key: &str) -> Result<String, String> {
    Ok(get(v, key)?.map(py_str).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn dumps_matches_python() {
        let v = json!({"a": [1, 2.0, "é"], "b": {}, "c": [], "d": null});
        assert_eq!(dumps(&v, None, false), r#"{"a": [1, 2.0, "é"], "b": {}, "c": [], "d": null}"#);
        assert_eq!(dumps(&v, None, true), "{\"a\": [1, 2.0, \"\\u00e9\"], \"b\": {}, \"c\": [], \"d\": null}");
        assert_eq!(
            dumps(&v, Some(2), false),
            "{\n  \"a\": [\n    1,\n    2.0,\n    \"é\"\n  ],\n  \"b\": {},\n  \"c\": [],\n  \"d\": null\n}"
        );
    }

    #[test]
    fn python_conversions() {
        assert_eq!(py_int(&json!("12")), Ok(12));
        assert_eq!(py_int(&json!(5.9)), Ok(5));
        assert_eq!(py_int(&json!("x")), Err("invalid literal for int() with base 10: 'x'".into()));
        assert_eq!(py_float(&json!("0.5")), Ok(0.5));
        assert_eq!(slice_end(5, 3), 3);
        assert_eq!(slice_end(5, -1), 4);
        assert_eq!(slice_end(2, -5), 0);
        assert_eq!(format_g(7.0), "7");
        assert_eq!(format_g(0.5), "0.5");
        assert_eq!(format_g(1234567.0), "1.23457e+06");
        assert_eq!(repr_str("it's"), "\"it's\"");
    }
}
