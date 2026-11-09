//! Small re-implementations of the Python behaviours that `inventory.py` depends on, so the Rust output is
//! byte-identical: `str.isspace`/`strip`/`splitlines`, `pathlib` suffixes, text-mode reads with
//! `errors="ignore"` and universal newlines, `json.dumps` escaping and float `repr`, and `time.strftime`.

use serde_json::Value;
use std::fs::{File, Metadata};
use std::io::Read;
use std::path::Path;

/// `str.isspace()` for one character (Python counts U+001C..U+001F as whitespace; Rust doesn't).
#[inline]
pub fn is_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// `s.strip()`
#[inline]
pub fn strip(s: &str) -> &str {
    s.trim_matches(is_space)
}

/// `s.lstrip()`
#[inline]
pub fn lstrip(s: &str) -> &str {
    s.trim_start_matches(is_space)
}

/// `s.strip(chars)`
#[inline]
pub fn strip_chars<'a>(s: &'a str, chars: &str) -> &'a str {
    s.trim_matches(|c| chars.contains(c))
}

/// `s.splitlines()` (no keepends).
pub fn splitlines(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        let brk = matches!(
            c,
            '\n' | '\r' | '\u{0b}' | '\u{0c}' | '\u{1c}' | '\u{1d}' | '\u{1e}' | '\u{85}' | '\u{2028}' | '\u{2029}'
        );
        if brk {
            out.push(&s[start..i]);
            let mut end = i + c.len_utf8();
            if c == '\r' {
                if let Some(&(_, '\n')) = it.peek() {
                    it.next();
                    end += 1;
                }
            }
            start = end;
        }
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// `s[:n]` in code points.
#[inline]
pub fn take_chars(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// `len(s)` in code points.
#[inline]
pub fn char_len(s: &str) -> usize {
    s.chars().count()
}

/// `pathlib.PurePath(name).suffix`: the last dot-part, but not for dotfiles or names ending in a dot.
pub fn suffix(name: &str) -> &str {
    match name.rfind('.') {
        Some(i) if i > 0 && i < name.len() - 1 => &name[i..],
        _ => "",
    }
}

/// UTF-8 decode dropping invalid bytes (`errors="ignore"`). `eof`: an incomplete sequence at the end is dropped too.
fn decode_ignore(mut bytes: &[u8], out: &mut String, eof: bool) {
    loop {
        match std::str::from_utf8(bytes) {
            Ok(s) => {
                out.push_str(s);
                return;
            }
            Err(e) => {
                let good = e.valid_up_to();
                // SAFETY-free: the prefix is valid UTF-8 by construction.
                out.push_str(std::str::from_utf8(&bytes[..good]).unwrap());
                match e.error_len() {
                    Some(n) => bytes = &bytes[good + n..],
                    None => {
                        let _ = eof;
                        return;
                    }
                }
            }
        }
    }
}

/// Universal-newline translation of Python text mode: `\r\n` and lone `\r` become `\n`.
fn translate_newlines(s: String) -> String {
    if !s.contains('\r') {
        return s;
    }
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\r' {
            if it.peek() == Some(&'\n') {
                it.next();
            }
            out.push('\n');
        } else {
            out.push(c);
        }
    }
    out
}

/// `inventory.read_head`: `open(p, encoding="utf-8", errors="ignore").read(limit)`, or "" on any error.
pub fn read_head(path: &Path, limit: usize) -> String {
    let Ok(mut f) = File::open(path) else { return String::new() };
    let mut buf: Vec<u8> = Vec::new();
    let mut eof = false;
    loop {
        let chars = {
            let mut s = String::new();
            decode_ignore(&buf, &mut s, eof);
            let s = translate_newlines(s);
            let n = char_len(&s);
            if n >= limit || eof {
                return take_chars(&s, limit).to_string();
            }
            n
        };
        // Each remaining character needs at most 4 bytes; a few extra cover a `\r\n` split at the boundary.
        let want = (limit - chars).saturating_mul(4).saturating_add(8);
        let before = buf.len();
        match (&mut f).take(want as u64).read_to_end(&mut buf) {
            Ok(0) => eof = true,
            Ok(n) if n < want => eof = true,
            Ok(_) => {}
            Err(_) => {
                if before == 0 {
                    return String::new();
                }
                eof = true;
            }
        }
    }
}

/// `int(st.st_mtime)`: CPython builds the float as `sec + nsec * 1e-9`, then truncates toward zero.
pub fn mtime_float(m: &Metadata) -> f64 {
    use std::os::unix::fs::MetadataExt;
    m.mtime() as f64 + m.mtime_nsec() as f64 * 1e-9
}

pub fn mtime_int(m: &Metadata) -> i64 {
    mtime_float(m) as i64
}

/// `time.strftime(fmt, time.localtime(t))` in the C locale (Python doesn't set LC_TIME either).
pub fn strftime_local(fmt: &str, t: Option<f64>) -> String {
    let secs: libc::time_t = match t {
        Some(f) => f.floor() as libc::time_t,
        None => unsafe { libc::time(std::ptr::null_mut()) },
    };
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&secs, &mut tm) };
    let cfmt = std::ffi::CString::new(fmt).unwrap();
    let mut buf = [0u8; 256];
    let n = unsafe { libc::strftime(buf.as_mut_ptr() as *mut libc::c_char, buf.len(), cfmt.as_ptr(), &tm) };
    String::from_utf8_lossy(&buf[..n]).into_owned()
}

/// Python truthiness of a JSON value.
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// `str(v)` for a value loaded from JSON.
pub fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => py_repr(other),
    }
}

fn py_repr(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Number(n) => number_repr(n),
        Value::String(s) => {
            let q = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
            let mut out = String::from(q);
            for c in s.chars() {
                match c {
                    '\\' => out.push_str("\\\\"),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    c if c == q => {
                        out.push('\\');
                        out.push(c);
                    }
                    c if (c as u32) < 0x20 || c as u32 == 0x7f => out.push_str(&format!("\\x{:02x}", c as u32)),
                    c => out.push(c),
                }
            }
            out.push(q);
            out
        }
        Value::Array(a) => format!("[{}]", a.iter().map(py_repr).collect::<Vec<_>>().join(", ")),
        Value::Object(o) => format!(
            "{{{}}}",
            o.iter()
                .map(|(k, v)| format!("{}: {}", py_repr(&Value::String(k.clone())), py_repr(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn number_repr(n: &serde_json::Number) -> String {
    if n.is_f64() {
        float_repr(n.as_f64().unwrap())
    } else {
        n.to_string()
    }
}

/// CPython `float.__repr__` ('r' format with the add-".0" flag).
pub fn float_repr(x: f64) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    if x == 0.0 {
        return if x.is_sign_negative() { "-0.0".into() } else { "0.0".into() };
    }
    let e = format!("{:e}", x); // shortest round-trip digits, e.g. "-1.5e16"
    let (mant, exp) = e.split_once('e').unwrap();
    let exp: i32 = exp.parse().unwrap();
    let (neg, mant) = mant.strip_prefix('-').map_or((false, mant), |m| (true, m));
    let digits: String = mant.chars().filter(|c| *c != '.').collect();
    let sign = if neg { "-" } else { "" };
    let decpt = exp + 1;
    if !(-4 < decpt && decpt <= 16) {
        let rest = &digits[1..];
        let m = if rest.is_empty() { digits[..1].to_string() } else { format!("{}.{}", &digits[..1], rest) };
        let es = if exp < 0 { '-' } else { '+' };
        return format!("{sign}{m}e{es}{:02}", exp.abs());
    }
    if decpt <= 0 {
        format!("{sign}0.{}{}", "0".repeat((-decpt) as usize), digits)
    } else if decpt as usize >= digits.len() {
        format!("{sign}{}{}.0", digits, "0".repeat(decpt as usize - digits.len()))
    } else {
        format!("{sign}{}.{}", &digits[..decpt as usize], &digits[decpt as usize..])
    }
}

/// `json.dumps(v, separators=(",", ":"), ensure_ascii=...)`
pub fn dumps(v: &Value, ensure_ascii: bool) -> String {
    let mut out = String::with_capacity(4096);
    write_value(&mut out, v, ensure_ascii);
    out
}

fn write_value(out: &mut String, v: &Value, ascii: bool) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&number_repr(n)),
        Value::String(s) => write_str(out, s, ascii),
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_value(out, x, ascii);
            }
            out.push(']');
        }
        Value::Object(o) => {
            out.push('{');
            for (i, (k, x)) in o.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_str(out, k, ascii);
                out.push(':');
                write_value(out, x, ascii);
            }
            out.push('}');
        }
    }
}

fn write_str(out: &mut String, s: &str, ascii: bool) {
    use std::fmt::Write;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitlines_matches_python() {
        assert_eq!(splitlines("a\r\nb\rc\n\nd\u{2028}e\n"), vec!["a", "b", "c", "", "d", "e"]);
        assert_eq!(splitlines(""), Vec::<&str>::new());
        assert_eq!(splitlines("\n"), vec![""]);
    }

    #[test]
    fn suffix_matches_pathlib() {
        assert_eq!(suffix("a.txt"), ".txt");
        assert_eq!(suffix(".bashrc"), "");
        assert_eq!(suffix("a."), "");
        assert_eq!(suffix("a.tar.gz"), ".gz");
        assert_eq!(suffix("noext"), "");
    }

    #[test]
    fn float_repr_matches_python() {
        for (x, want) in [
            (1.5, "1.5"),
            (100.0, "100.0"),
            (1e16, "1e+16"),
            (1.5e16, "1.5e+16"),
            (1e15, "1000000000000000.0"),
            (0.0001, "0.0001"),
            (0.00001, "1e-05"),
            (-2.5e-7, "-2.5e-07"),
            (123.456, "123.456"),
        ] {
            assert_eq!(float_repr(x), want, "{x}");
        }
    }

    #[test]
    fn dumps_escapes_like_python() {
        let v = serde_json::json!({"a": "é\u{1f600}\u{7f}\u{1}\"", "b": [1, true, null]});
        let b = std::char::from_u32(92).unwrap();
        let want = format!(r#"{{"a":"{b}u00e9{b}ud83d{b}ude00{b}u007f{b}u0001{b}"","b":[1,true,null]}}"#);
        assert_eq!(dumps(&v, true), want);
        assert_eq!(dumps(&v, false), "{\"a\":\"é\u{1f600}\u{7f}\\u0001\\\"\",\"b\":[1,true,null]}");
    }
}
