//! Port of `HTML/HTMLSource.swift` (the tolerant HTML source map, `HTMLTagText`) and `HTML/HTMLOps.swift`
//! (CSS declarations, style destinations and the text edits behind the visual HTML editor and the `html_*` MCP tools).
//!
//! These functions edit users' HTML and CSS files, so the output must match the Swift byte for byte. The Swift works
//! on `NSString` (UTF-16 code units) with a hand-written scanner plus a few `NSRegularExpression`s; this port keeps
//! the text as UTF-16 internally, reports every range in UTF-16 units (exactly like `NSRange`), and replaces each
//! regular expression with a hand-written matcher that follows ICU's leftmost-first semantics for that pattern.
//! `\s` follows ICU (`[\t\n\f\r\p{Z}]`), `.whitespaces` / `.whitespacesAndNewlines` follow Foundation.
//!
//! Known, deliberate difference: Swift counts `Character`s (grapheme clusters) where it truncates labels
//! (`plainText`'s limit, the 60-character `style` preview in the outline); this port counts Unicode scalars. The two
//! only differ for text with combining marks or multi-scalar emoji, and only in outline/label text, never in edits.

use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Component, Path, PathBuf};

// MARK: - Character classes

fn is_zs(c: u32) -> bool {
    matches!(c, 0x20 | 0xA0 | 0x1680 | 0x2000..=0x200A | 0x202F | 0x205F | 0x3000)
}

/// ICU regex `\s`: `[\t\n\f\r\p{Z}]`.
fn icu_ws(c: u32) -> bool {
    matches!(c, 0x09 | 0x0A | 0x0C | 0x0D | 0x2028 | 0x2029) || is_zs(c)
}

/// Foundation `CharacterSet.whitespaces`: tab and `Zs`.
fn swift_ws(c: u32) -> bool {
    c == 0x09 || is_zs(c)
}

/// Foundation `CharacterSet.whitespacesAndNewlines`.
fn swift_ws_nl(c: u32) -> bool {
    swift_ws(c) || matches!(c, 0x0A..=0x0D | 0x85 | 0x2028 | 0x2029)
}

fn trim_by(s: &str, f: fn(u32) -> bool) -> &str {
    s.trim_matches(|ch: char| f(ch as u32))
}

fn u16s(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

fn string(u: &[u16]) -> String {
    String::from_utf16_lossy(u)
}

fn lower_ascii(c: u16) -> u16 {
    if (65..=90).contains(&c) {
        c + 32
    } else {
        c
    }
}

/// Case-insensitive literal match of `pat` at `t[at..]` (ASCII folding; exact otherwise).
fn ci_at(t: &[u16], at: usize, pat: &[u16]) -> bool {
    at + pat.len() <= t.len() && t[at..at + pat.len()].iter().zip(pat).all(|(a, b)| lower_ascii(*a) == lower_ascii(*b))
}

fn exact_at(t: &[u16], at: usize, pat: &[u16]) -> bool {
    at + pat.len() <= t.len() && &t[at..at + pat.len()] == pat
}

/// `NSString.lineRange(for:)`'s start: just after the previous line terminator.
fn line_start(t: &[u16], pos: usize) -> usize {
    let mut s = pos;
    while s > 0 && matches!(t[s - 1], 0x0A | 0x0D | 0x85 | 0x2028 | 0x2029) == false {
        s -= 1;
    }
    s
}

/// Swift `split(separator:maxSplits:)` with `omittingEmptySubsequences: true`.
fn swift_split(s: &str, sep: char, max_splits: usize) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    let mut cur = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == sep && out.len() < max_splits {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
                if out.len() == max_splits {
                    let rest: String = chars.collect();
                    if !rest.is_empty() {
                        out.push(rest);
                    }
                    return out;
                }
            }
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn prefix_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

// MARK: - Ranges and errors

/// A UTF-16 range, like `NSRange`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NsRange {
    pub location: usize,
    pub length: usize,
}

impl NsRange {
    pub fn new(location: usize, length: usize) -> Self {
        NsRange { location, length }
    }
    /// `NSMaxRange`.
    pub fn end(&self) -> usize {
        self.location + self.length
    }
    pub fn to_json(&self) -> Value {
        json!({"location": self.location, "length": self.length})
    }
}

/// `HTMLOps.OpError`: a user-facing reason an edit can't be made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HtmlError(pub String);

impl fmt::Display for HtmlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for HtmlError {}

fn err<T>(m: impl Into<String>) -> Result<T, HtmlError> {
    Err(HtmlError(m.into()))
}

// MARK: - Source map (HTMLSource.swift)

pub const VOID_TAGS: &[&str] =
    &["area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source", "track", "wbr"];
pub const RAW_TEXT_TAGS: &[&str] = &["script", "style", "textarea", "title", "xmp"];
pub const NON_VISUAL: &[&str] =
    &["html", "head", "meta", "link", "script", "style", "title", "base", "noscript", "template"];
/// Start tags that close an open <p> (HTML spec "button scope").
const CLOSES_P: &[&str] = &[
    "address", "article", "aside", "blockquote", "details", "dialog", "div", "dl", "fieldset", "figcaption", "figure",
    "footer", "form", "h1", "h2", "h3", "h4", "h5", "h6", "header", "hgroup", "hr", "main", "menu", "nav", "ol", "p",
    "pre", "search", "section", "table", "ul", "li", "dd", "dt",
];
const P_SCOPE_STOPS: &[&str] = &["html", "table", "td", "th", "caption", "button", "object", "template", "marquee", "applet"];

/// `HTMLNode`: one element and its exact place in the source text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HtmlNode {
    pub index: usize,
    pub tag: String,
    pub start_tag: NsRange,
    pub end_tag: Option<NsRange>,
    pub children: Vec<usize>,
    pub parent: Option<usize>,
    pub selectable: bool,
}

impl HtmlNode {
    /// `HTMLNode.fullRange`.
    pub fn full_range(&self) -> NsRange {
        match self.end_tag {
            Some(e) => NsRange::new(self.start_tag.location, e.end() - self.start_tag.location),
            None => self.start_tag,
        }
    }
    /// `HTMLNode.innerRange`.
    pub fn inner_range(&self) -> Option<NsRange> {
        self.end_tag.map(|e| NsRange::new(self.start_tag.end(), e.location - self.start_tag.end()))
    }
}

/// `HTMLSourceMap`: a tolerant map from every element in an HTML file to its exact place in the source text.
#[derive(Clone, Debug)]
pub struct HtmlSourceMap {
    text: Vec<u16>,
    pub nodes: Vec<HtmlNode>,
    pub roots: Vec<usize>,
}

impl HtmlSourceMap {
    /// Replaces `HTMLSourceMap.init(_:)`.
    pub fn new(s: &str) -> Self {
        let mut m = HtmlSourceMap { text: u16s(s), nodes: vec![], roots: vec![] };
        m.parse();
        m
    }

    /// The source as UTF-16 (what every `NsRange` indexes).
    pub fn utf16(&self) -> &[u16] {
        &self.text
    }

    /// The source text of a range.
    pub fn substring(&self, r: NsRange) -> String {
        string(&self.text[r.location..r.end()])
    }

    fn parse(&mut self) {
        let len = self.text.len();
        if len == 0 {
            return;
        }
        let buf = self.text.clone();
        let mut stack: Vec<usize> = vec![];
        let mut i = 0usize;

        let is_letter = |c: u16| (65..=90).contains(&c) || (97..=122).contains(&c);
        let is_name = |c: u16| is_letter(c) || (48..=57).contains(&c) || c == 45 || c == 58 || c == 95;
        let tag_end = |j: usize| -> Option<usize> {
            let mut k = j;
            let mut quote = 0u16;
            while k < len {
                let c = buf[k];
                if quote != 0 {
                    if c == quote {
                        quote = 0
                    }
                } else if c == 34 || c == 39 {
                    quote = c
                } else if c == 62 {
                    return Some(k);
                }
                k += 1;
            }
            None
        };

        fn close_implied(nodes: &mut [HtmlNode], stack: &mut Vec<usize>, k: usize, pos: usize) {
            for &idx in &stack[k..] {
                if nodes[idx].end_tag.is_none() {
                    nodes[idx].end_tag = Some(NsRange::new(pos, 0));
                }
            }
            stack.truncate(k);
        }
        fn in_scope(nodes: &[HtmlNode], stack: &[usize], targets: &[&str], stops: &[&str]) -> Option<usize> {
            for k in (0..stack.len()).rev() {
                let t = nodes[stack[k]].tag.as_str();
                if targets.contains(&t) {
                    return Some(k);
                }
                if stops.contains(&t) {
                    return None;
                }
            }
            None
        }
        fn close_implied_before(nodes: &mut [HtmlNode], stack: &mut Vec<usize>, name: &str, pos: usize) {
            if CLOSES_P.contains(&name) {
                if let Some(k) = in_scope(nodes, stack, &["p"], P_SCOPE_STOPS) {
                    close_implied(nodes, stack, k, pos);
                }
            }
            let last_is_option = stack.last().map(|&l| nodes[l].tag == "option").unwrap_or(false);
            let rule: Option<(&[&str], &[&str])> = match name {
                "li" => Some((&["li"], &["ul", "ol", "menu", "table", "td", "th", "template"])),
                "dt" | "dd" => Some((&["dt", "dd"], &["dl", "table", "td", "th", "template"])),
                "tr" => Some((&["tr"], &["table", "template"])),
                "td" | "th" => Some((&["td", "th"], &["tr", "table", "template"])),
                "tbody" | "thead" | "tfoot" => Some((&["tbody", "thead", "tfoot"], &["table", "template"])),
                "optgroup" => Some((&["optgroup", "option"], &["select", "datalist", "template"])),
                "option" if last_is_option => Some((&["option"], &[])),
                _ => None,
            };
            if let Some((targets, stops)) = rule {
                if let Some(k) = in_scope(nodes, stack, targets, stops) {
                    close_implied(nodes, stack, k, pos);
                }
            }
        }

        let comment_open = u16s("<!--");
        while i < len {
            if buf[i] != 60 {
                i += 1;
                continue;
            }
            if ci_at(&buf, i, &comment_open) {
                let mut j = i + 4;
                while j + 2 < len && !(buf[j] == 45 && buf[j + 1] == 45 && buf[j + 2] == 62) {
                    j += 1;
                }
                i = j + 3;
                continue;
            }
            if i + 1 >= len {
                break;
            }
            if buf[i + 1] == 33 || buf[i + 1] == 63 {
                i = tag_end(i).unwrap_or(len - 1) + 1;
                continue;
            }
            if buf[i + 1] == 47 {
                let mut j = i + 2;
                while j < len && is_name(buf[j]) {
                    j += 1;
                }
                let name = string(&buf[i + 2..j]).to_ascii_lowercase();
                let Some(end) = tag_end(j) else { break };
                if let Some(pos) = stack.iter().rposition(|&n| self.nodes[n].tag == name) {
                    let idx = stack[pos];
                    self.nodes[idx].end_tag = Some(NsRange::new(i, end - i + 1));
                    if pos + 1 < stack.len() {
                        close_implied(&mut self.nodes, &mut stack, pos + 1, i);
                    }
                    stack.truncate(pos);
                }
                i = end + 1;
                continue;
            }
            if !is_letter(buf[i + 1]) {
                i += 1;
                continue;
            }
            let mut j = i + 1;
            while j < len && is_name(buf[j]) {
                j += 1;
            }
            let name = string(&buf[i + 1..j]).to_ascii_lowercase();
            let Some(end) = tag_end(j) else { break };
            let self_closing = end > 0 && buf[end - 1] == 47;
            close_implied_before(&mut self.nodes, &mut stack, &name, i);
            let index = self.nodes.len();
            let parent = stack.last().copied();
            self.nodes.push(HtmlNode {
                index,
                tag: name.clone(),
                start_tag: NsRange::new(i, end - i + 1),
                end_tag: None,
                children: vec![],
                parent,
                selectable: !NON_VISUAL.contains(&name.as_str()),
            });
            match parent {
                Some(p) => self.nodes[p].children.push(index),
                None => self.roots.push(index),
            }
            i = end + 1;
            if VOID_TAGS.contains(&name.as_str()) || self_closing {
                continue;
            }
            if RAW_TEXT_TAGS.contains(&name.as_str()) {
                let close_pat = u16s(&format!("</{name}"));
                let mut k = i;
                while k < len && !ci_at(&buf, k, &close_pat) {
                    k += 1;
                }
                match if k < len { tag_end(k) } else { None } {
                    Some(close) => {
                        self.nodes[index].end_tag = Some(NsRange::new(k, close - k + 1));
                        i = close + 1;
                    }
                    None => i = len,
                }
                continue;
            }
            stack.push(index);
        }
        if !stack.is_empty() {
            close_implied(&mut self.nodes, &mut stack, 0, len);
        }
    }

    /// `annotated()`: the source with `data-dg-id="N"` on every visual element, for the visual editor's renderer.
    pub fn annotated(&self) -> String {
        let mut out = self.text.clone();
        for node in self.nodes.iter().rev().filter(|n| n.selectable) {
            let r = node.start_tag;
            let mut at = r.end() - 1;
            if at > r.location && self.text[at - 1] == 47 {
                at -= 1;
            }
            let ins = u16s(&format!(" data-dg-id=\"{}\"", node.index));
            out.splice(at..at, ins);
        }
        string(&out)
    }

    /// `startTagText(_:)`.
    pub fn start_tag_text(&self, i: usize) -> String {
        self.substring(self.nodes[i].start_tag)
    }
    /// `fullText(_:)`.
    pub fn full_text(&self, i: usize) -> String {
        self.substring(self.nodes[i].full_range())
    }
    /// `attribute(_:of:)`.
    pub fn attribute(&self, name: &str, i: usize) -> Option<String> {
        tag_text::attribute(name, &self.start_tag_text(i))
    }
    /// `classes(of:)`.
    pub fn classes(&self, i: usize) -> Vec<String> {
        swift_split(&self.attribute("class", i).unwrap_or_default(), ' ', usize::MAX)
    }
    /// `elementChildren(_:)`.
    pub fn element_children(&self, i: usize) -> Vec<usize> {
        self.nodes[i].children.iter().copied().filter(|&c| self.nodes[c].selectable).collect()
    }

    /// `plainText(_:limit:)`: visible text inside an element, markup stripped (for outlines and labels).
    pub fn plain_text(&self, i: usize, limit: usize) -> String {
        let Some(inner) = self.nodes[i].inner_range() else { return String::new() };
        let raw: Vec<char> = self.substring(inner).chars().collect();
        let a = strip_script_style(&raw);
        let b = strip_tags(&a, " ");
        let c = collapse_ws(&b);
        let s: String = c.into_iter().collect();
        let stripped = trim_by(&s, swift_ws);
        if stripped.chars().count() > limit {
            prefix_chars(stripped, limit) + "…"
        } else {
            stripped.to_string()
        }
    }

    /// `isTextOnly(_:)`: only text inside (no child elements), safe to edit as text.
    pub fn is_text_only(&self, i: usize) -> bool {
        self.nodes[i].inner_range().is_some() && self.nodes[i].children.is_empty()
    }

    /// `path(of:)`.
    pub fn path(&self, index: usize) -> Vec<usize> {
        let mut p = vec![];
        let mut cur = index;
        while let Some(parent) = self.nodes[cur].parent {
            p.insert(0, self.nodes[parent].children.iter().position(|&c| c == cur).unwrap_or(0));
            cur = parent;
        }
        p.insert(0, self.roots.iter().position(|&r| r == cur).unwrap_or(0));
        p
    }

    /// `node(atPath:)`.
    pub fn node_at_path(&self, path: &[usize]) -> Option<usize> {
        let first = *path.first()?;
        if first >= self.roots.len() {
            return None;
        }
        let mut cur = self.roots[first];
        for &k in &path[1..] {
            let ch = &self.nodes[cur].children;
            if k >= ch.len() {
                return Some(cur);
            }
            cur = ch[k];
        }
        Some(cur)
    }

    /// `lineIndent(_:)`.
    pub fn line_indent(&self, i: usize) -> String {
        let loc = self.nodes[i].start_tag.location;
        let ls = line_start(&self.text, loc);
        self.text[ls..loc].iter().take_while(|&&c| c == 32 || c == 9).map(|&c| c as u8 as char).collect()
    }

    /// `label(_:)`: `tag#id.class1.class2.class3`.
    pub fn label(&self, i: usize) -> String {
        let mut s = self.nodes[i].tag.clone();
        if let Some(id) = self.attribute("id", i) {
            if !id.is_empty() {
                s += "#";
                s += &id;
            }
        }
        let cls = self.classes(i);
        if !cls.is_empty() {
            s += ".";
            s += &cls.iter().take(3).cloned().collect::<Vec<_>>().join(".");
        }
        s
    }

    /// `resolve(_:)`: finds an element by "0.1.2" path, "#id", ".class", "tag", "tag:N" (Nth, from 1) or "@index".
    pub fn resolve(&self, spec: &str) -> Option<usize> {
        let s = trim_by(spec, swift_ws);
        if let Some(rest) = s.strip_prefix('@') {
            if let Ok(n) = rest.parse::<i64>() {
                if n >= 0 && (n as usize) < self.nodes.len() {
                    return Some(n as usize);
                }
            }
        }
        if is_dotted_digits(s) {
            let path: Vec<usize> = s.split('.').filter_map(|p| p.parse::<usize>().ok()).collect();
            return self.node_at_path(&path);
        }
        let visual: Vec<usize> = (0..self.nodes.len()).filter(|&i| self.nodes[i].selectable).collect();
        if let Some(id) = s.strip_prefix('#') {
            return visual.into_iter().find(|&i| self.attribute("id", i).as_deref() == Some(id));
        }
        if let Some(c) = s.strip_prefix('.') {
            return visual.into_iter().find(|&i| self.classes(i).iter().any(|x| x == c));
        }
        let parts = swift_split(&s.to_lowercase(), ':', usize::MAX);
        let tag = parts.first().cloned().unwrap_or_default();
        let nth: i64 = if parts.len() > 1 { parts[1].parse::<i64>().unwrap_or(1) } else { 1 };
        let matches: Vec<usize> = visual.into_iter().filter(|&i| self.nodes[i].tag == tag).collect();
        if nth >= 1 && ((nth - 1) as usize) < matches.len() {
            Some(matches[(nth - 1) as usize])
        } else {
            None
        }
    }

    /// `firstStyleBlock`: the first `<style>` block's inner range, if any.
    pub fn first_style_block(&self) -> Option<NsRange> {
        self.nodes.iter().find(|n| n.tag == "style" && n.end_tag.is_some()).and_then(|n| n.inner_range())
    }

    /// Every node as JSON (index, tag, path, label, ranges in UTF-16 units, parent, children, selectable).
    pub fn to_json(&self) -> Value {
        Value::Array(
            self.nodes
                .iter()
                .map(|n| {
                    json!({
                        "index": n.index, "tag": n.tag, "label": self.label(n.index),
                        "path": self.path(n.index).iter().map(|x| x.to_string()).collect::<Vec<_>>().join("."),
                        "startTag": n.start_tag.to_json(), "endTag": n.end_tag.map(|e| e.to_json()),
                        "parent": n.parent, "children": n.children, "selectable": n.selectable,
                    })
                })
                .collect(),
        )
    }
}

fn is_dotted_digits(s: &str) -> bool {
    // ^\d+(\.\d+)*$
    !s.is_empty() && s.split('.').all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

/// `(?s)<(script|style)[^>]*>.*?</\1>` (case-insensitive) → " ".
fn strip_script_style(t: &[char]) -> Vec<char> {
    let ci = |at: usize, pat: &str| -> bool {
        let p: Vec<char> = pat.chars().collect();
        at + p.len() <= t.len() && t[at..at + p.len()].iter().zip(&p).all(|(a, b)| a.to_ascii_lowercase() == *b)
    };
    let mut out = vec![];
    let mut i = 0;
    'outer: while i < t.len() {
        if t[i] == '<' {
            for name in ["script", "style"] {
                if !ci(i + 1, name) {
                    continue;
                }
                let captured: Vec<char> = t[i + 1..i + 1 + name.len()].to_vec();
                let mut j = i + 1 + name.len();
                while j < t.len() && t[j] != '>' {
                    j += 1;
                }
                if j >= t.len() {
                    continue;
                }
                let mut k = j + 1;
                while k < t.len() {
                    let close = k + 2 + captured.len();
                    if t[k] == '<'
                        && k + 1 < t.len()
                        && t[k + 1] == '/'
                        && close < t.len()
                        && t[k + 2..close].iter().zip(&captured).all(|(a, b)| a.to_lowercase().eq(b.to_lowercase()))
                        && t[close] == '>'
                    {
                        out.push(' ');
                        i = close + 1;
                        continue 'outer;
                    }
                    k += 1;
                }
            }
        }
        out.push(t[i]);
        i += 1;
    }
    out
}

/// `<[^>]+>` → `with`.
fn strip_tags(t: &[char], with: &str) -> Vec<char> {
    let mut out = vec![];
    let mut i = 0;
    while i < t.len() {
        if t[i] == '<' && i + 1 < t.len() && t[i + 1] != '>' {
            if let Some(off) = t[i + 1..].iter().position(|&c| c == '>') {
                out.extend(with.chars());
                i = i + 1 + off + 1;
                continue;
            }
        }
        out.push(t[i]);
        i += 1;
    }
    out
}

/// `\s+` → " " (ICU `\s`).
fn collapse_ws(t: &[char]) -> Vec<char> {
    let mut out = vec![];
    let mut i = 0;
    while i < t.len() {
        if icu_ws(t[i] as u32) {
            while i < t.len() && icu_ws(t[i] as u32) {
                i += 1;
            }
            out.push(' ');
        } else {
            out.push(t[i]);
            i += 1;
        }
    }
    out
}

/// `HTMLTagText`: reading and setting attributes on a start tag's text.
pub mod tag_text {
    use super::*;

    /// The match of `\sNAME\s*=\s*("([^"]*)"|'([^']*)'|([^\s>"']+))` (case-insensitive):
    /// (whole match start, whole match end, value start, value end).
    fn find(name: &[u16], t: &[u16]) -> Option<(usize, usize, usize, usize)> {
        let len = t.len();
        for p in 0..len {
            if !icu_ws(t[p] as u32) || !ci_at(t, p + 1, name) {
                continue;
            }
            let mut q = p + 1 + name.len();
            while q < len && icu_ws(t[q] as u32) {
                q += 1;
            }
            if q >= len || t[q] != 61 {
                continue;
            }
            q += 1;
            while q < len && icu_ws(t[q] as u32) {
                q += 1;
            }
            if q >= len {
                continue;
            }
            match t[q] {
                34 | 39 => {
                    if let Some(off) = t[q + 1..].iter().position(|&c| c == t[q]) {
                        let k = q + 1 + off;
                        return Some((p, k + 1, q + 1, k));
                    }
                }
                _ => {
                    let mut k = q;
                    while k < len && !icu_ws(t[k] as u32) && !matches!(t[k], 62 | 34 | 39) {
                        k += 1;
                    }
                    if k > q {
                        return Some((p, k, q, k));
                    }
                }
            }
        }
        None
    }

    /// `HTMLTagText.attribute(_:in:)`: an attribute's value ("" for a boolean attribute), or None.
    pub fn attribute(name: &str, tag: &str) -> Option<String> {
        let t = u16s(tag);
        let n = u16s(name);
        match find(&n, &t) {
            Some((_, _, vs, ve)) => Some(string(&t[vs..ve]).replace("&quot;", "\"").replace("&amp;", "&")),
            None => {
                // \sNAME(?=[\s/>])
                let present = (0..t.len()).any(|p| {
                    icu_ws(t[p] as u32) && ci_at(&t, p + 1, &n) && {
                        let k = p + 1 + n.len();
                        k < t.len() && (icu_ws(t[k] as u32) || t[k] == 47 || t[k] == 62)
                    }
                });
                if present {
                    Some(String::new())
                } else {
                    None
                }
            }
        }
    }

    /// `HTMLTagText.setting(_:to:in:)`: sets (or with None removes) an attribute on a start tag's text.
    pub fn setting(name: &str, value: Option<&str>, tag: &str) -> String {
        let mut t = u16s(tag);
        let escaped = value.map(|v| v.replace('&', "&amp;").replace('"', "&quot;"));
        if let Some((s, e, _, _)) = find(&u16s(name), &t) {
            let rep = escaped.map(|v| format!(" {name}=\"{v}\"")).unwrap_or_default();
            t.splice(s..e, u16s(&rep));
            return string(&t);
        }
        let Some(v) = escaped else { return tag.to_string() };
        if t.is_empty() {
            return tag.to_string();
        }
        let mut at = t.len() - 1;
        if at > 0 && t[at - 1] == 47 {
            at -= 1;
        }
        t.splice(at..at, u16s(&format!(" {name}=\"{v}\"")));
        string(&t)
    }
}

// MARK: - CSS (HTMLOps.swift)

/// A list of `(property, value)` CSS declarations, in source order.
pub type Declarations = Vec<(String, String)>;
/// Property changes: `None` (or a blank value) removes the property. Applied in key order, like the Swift.
pub type Changes = BTreeMap<String, Option<String>>;

/// `CSSDeclarations`.
pub mod declarations {
    use super::*;

    /// `CSSDeclarations.parse(_:)`: splits on ';' outside parentheses (url(), rgb()).
    pub fn parse(s: &str) -> Declarations {
        let mut out = vec![];
        let mut depth = 0i64;
        let mut current = String::new();
        for ch in s.chars() {
            if ch == '(' {
                depth += 1
            } else if ch == ')' {
                depth = (depth - 1).max(0)
            }
            if ch == ';' && depth == 0 {
                append(&current, &mut out);
                current.clear();
            } else {
                current.push(ch)
            }
        }
        append(&current, &mut out);
        out
    }

    fn append(part: &str, out: &mut Declarations) {
        let kv = swift_split(part, ':', 1);
        if kv.len() != 2 {
            return;
        }
        let k = trim_by(&kv[0], swift_ws_nl).to_lowercase();
        let v = trim_by(&kv[1], swift_ws_nl).to_string();
        if !k.is_empty() && !v.is_empty() {
            out.push((k, v));
        }
    }

    /// `CSSDeclarations.setting(_:in:)`.
    pub fn setting(changes: &Changes, decls: &[(String, String)]) -> Declarations {
        let mut d: Declarations = decls.to_vec();
        for (prop, value) in changes {
            let v = value.as_deref().map(|v| trim_by(v, swift_ws).to_string()).unwrap_or_default();
            if let Some(i) = d.iter().position(|(k, _)| k == prop) {
                if v.is_empty() {
                    d.remove(i);
                } else {
                    d[i].1 = v;
                }
            } else if !v.is_empty() {
                d.push((prop.clone(), v));
            }
        }
        d
    }

    /// `CSSDeclarations.inline(_:)`: `a: 1; b: 2`.
    pub fn inline(d: &[(String, String)]) -> String {
        d.iter().map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>().join("; ")
    }

    /// Declarations as a JSON array of `[property, value]` pairs.
    pub fn to_json(d: &[(String, String)]) -> Value {
        Value::Array(d.iter().map(|(k, v)| json!([k, v])).collect())
    }

    pub(crate) fn changes_from(d: &[(String, String)]) -> Changes {
        d.iter().map(|(k, v)| (k.clone(), Some(v.clone()))).collect()
    }
}

/// `CSSRules`: finding and editing `selector { … }` in CSS text.
pub mod rules {
    use super::*;

    /// `(^|[\s}])(SELECTOR)\s*\{([^}]*)\}` with `.anchorsMatchLines`, first match within `range`:
    /// (selector location, body range).
    pub(crate) fn find(css: &[u16], range: NsRange, selector: &[u16]) -> Option<(usize, NsRange)> {
        let (lo, hi) = (range.location, range.end());
        let rest = |q: usize| -> Option<(usize, NsRange)> {
            if q + selector.len() > hi || !exact_at(css, q, selector) {
                return None;
            }
            let mut r = q + selector.len();
            while r < hi && icu_ws(css[r] as u32) {
                r += 1;
            }
            if r >= hi || css[r] != 123 {
                return None;
            }
            let off = css[r + 1..hi].iter().position(|&c| c == 125)?;
            Some((q, NsRange::new(r + 1, off)))
        };
        let line_start = |p: usize| -> bool {
            if p == lo {
                return true;
            }
            let prev = css[p - 1];
            if prev == 0x0D {
                return !(p < css.len() && css[p] == 0x0A);
            }
            matches!(prev, 0x0A | 0x0B | 0x0C | 0x85 | 0x2028 | 0x2029)
        };
        for p in lo..hi {
            if line_start(p) {
                if let Some(m) = rest(p) {
                    return Some(m);
                }
            }
            if icu_ws(css[p] as u32) || css[p] == 125 {
                if let Some(m) = rest(p + 1) {
                    return Some(m);
                }
            }
        }
        None
    }

    /// `CSSRules.declarations(_:_:selector:)`.
    pub fn declarations(css: &[u16], range: NsRange, selector: &str) -> Declarations {
        match find(css, range, &u16s(selector)) {
            Some((_, body)) => declarations::parse(&string(&css[body.location..body.end()])),
            None => vec![],
        }
    }

    /// `CSSRules.edit(...)`: the edit that sets/removes properties in `selector { … }` within `range`, adding the
    /// rule at the end if missing.
    pub fn edit(css: &[u16], range: NsRange, selector: &str, changes: &Changes, rule_indent: &str) -> Edit {
        if let Some((sel_loc, body)) = find(css, range, &u16s(selector)) {
            let decls = declarations::setting(changes, &declarations::parse(&string(&css[body.location..body.end()])));
            let ls = line_start(css, sel_loc);
            let indent: String =
                css[ls..sel_loc].iter().take_while(|&&c| c == 32 || c == 9).map(|&c| c as u8 as char).collect();
            let text = if decls.is_empty() {
                " ".to_string()
            } else {
                "\n".to_string()
                    + &decls.iter().map(|(k, v)| format!("{indent}  {k}: {v};")).collect::<Vec<_>>().join("\n")
                    + "\n"
                    + &indent
            };
            return Edit { range: body, text };
        }
        let decls = declarations::setting(changes, &[]);
        let existing = &css[range.location..range.end()];
        let lead = if existing.is_empty() {
            ""
        } else if existing.last() == Some(&10) {
            "\n"
        } else {
            "\n\n"
        };
        let rule = format!("{rule_indent}{selector} {{\n")
            + &decls.iter().map(|(k, v)| format!("{rule_indent}  {k}: {v};")).collect::<Vec<_>>().join("\n")
            + if decls.is_empty() { "" } else { "\n" }
            + &format!("{rule_indent}}}\n");
        Edit { range: NsRange::new(range.end(), 0), text: lead.to_string() + &rule }
    }
}

// MARK: - Style destinations, context and edits

/// `StyleDestination`: where styles are written.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum StyleDestination {
    /// A local .css file linked from the page.
    Stylesheet(PathBuf),
    /// `<style>` inside the HTML file.
    StyleBlock,
    /// `style="…"` on the element (properties only).
    Inline,
}

impl StyleDestination {
    /// `StyleDestination.label`.
    pub fn label(&self) -> String {
        match self {
            StyleDestination::Stylesheet(p) => last_component(p),
            StyleDestination::StyleBlock => "<style> in this HTML".into(),
            StyleDestination::Inline => "Inline (style attribute)".into(),
        }
    }
    /// `isStylesheet`.
    pub fn is_stylesheet(&self) -> bool {
        matches!(self, StyleDestination::Stylesheet(_))
    }
    /// `isInHTML`.
    pub fn is_in_html(&self) -> bool {
        !self.is_stylesheet()
    }
    /// Parses `"inline"`, `"style"` / `"styleBlock"`, or a stylesheet path (relative paths resolve against `page`'s
    /// folder).
    pub fn parse(s: &str, page: &Path) -> StyleDestination {
        match s {
            "inline" => StyleDestination::Inline,
            "style" | "styleBlock" | "style_block" | "" => StyleDestination::StyleBlock,
            p => {
                let p = Path::new(p);
                StyleDestination::Stylesheet(if p.is_absolute() {
                    p.to_path_buf()
                } else {
                    standardize(&page.parent().unwrap_or(Path::new("/")).join(p))
                })
            }
        }
    }
    pub fn to_json(&self) -> Value {
        match self {
            StyleDestination::Stylesheet(p) => json!({"kind": "stylesheet", "path": p.to_string_lossy(), "label": self.label()}),
            StyleDestination::StyleBlock => json!({"kind": "styleBlock", "label": self.label()}),
            StyleDestination::Inline => json!({"kind": "inline", "label": self.label()}),
        }
    }
}

fn last_component(p: &Path) -> String {
    p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Lexical `standardizedFileURL`: removes `.` and resolves `..`.
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

/// One text edit: replace `range` (UTF-16 units) with `text`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub range: NsRange,
    pub text: String,
}

impl Edit {
    pub fn new(range: NsRange, text: impl Into<String>) -> Self {
        Edit { range, text: text.into() }
    }
}

/// `FileEdits`: a set of text edits across files (the HTML page and/or its stylesheets).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileEdits {
    /// Edits per file, files in the order they were first touched.
    pub by_file: Vec<(PathBuf, Vec<Edit>)>,
    pub message: String,
    pub structural: bool,
}

impl FileEdits {
    pub fn add(&mut self, path: &Path, e: Edit) {
        match self.by_file.iter_mut().find(|(p, _)| p == path) {
            Some((_, v)) => v.push(e),
            None => self.by_file.push((path.to_path_buf(), vec![e])),
        }
    }
    /// The edits for one file (empty if none).
    pub fn edits_for(&self, path: &Path) -> &[Edit] {
        self.by_file.iter().find(|(p, _)| p == path).map(|(_, v)| v.as_slice()).unwrap_or(&[])
    }
    pub fn is_empty(&self) -> bool {
        self.by_file.is_empty()
    }
    /// The new text of every touched file, reading current text with `read` (files it can't read are skipped).
    pub fn results(&self, read: &dyn Fn(&Path) -> Option<String>) -> Vec<(PathBuf, String)> {
        self.by_file.iter().filter_map(|(p, e)| read(p).map(|t| (p.clone(), apply(e, &t)))).collect()
    }
    /// `{"message", "structural", "files": [{"path", "edits": [{"location", "length", "text"}]}]}`; ranges are UTF-16.
    pub fn to_json(&self) -> Value {
        json!({
            "message": self.message,
            "structural": self.structural,
            "files": self.by_file.iter().map(|(p, e)| json!({
                "path": p.to_string_lossy(),
                "edits": e.iter().map(|x| json!({"location": x.range.location, "length": x.range.length, "text": x.text}))
                    .collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        })
    }
}

/// `HTMLOps.apply(_:to:)`: applies edits from the last location backwards (stable for equal locations).
pub fn apply(edits: &[Edit], text: &str) -> String {
    let mut out = u16s(text);
    let mut sorted: Vec<&Edit> = edits.iter().collect();
    sorted.sort_by(|a, b| b.range.location.cmp(&a.range.location));
    for e in sorted {
        out.splice(e.range.location..e.range.end(), u16s(&e.text));
    }
    string(&out)
}

/// `HTMLContext`: the page, its parse, and a way to read its CSS files.
pub struct HtmlContext {
    pub path: PathBuf,
    pub text: String,
    pub map: HtmlSourceMap,
    read_file: Box<dyn Fn(&Path) -> Option<String>>,
}

impl HtmlContext {
    /// Replaces `HTMLContext.init(url:text:readFile:)`.
    pub fn new(path: impl Into<PathBuf>, text: impl Into<String>, read_file: Box<dyn Fn(&Path) -> Option<String>>) -> Self {
        let text = text.into();
        let map = HtmlSourceMap::new(&text);
        HtmlContext { path: path.into(), text, map, read_file }
    }

    /// A context for a page on disk, reading stylesheets from disk.
    pub fn from_file(path: &Path) -> std::io::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        Ok(Self::new(path, text, Box::new(|p: &Path| std::fs::read_to_string(p).ok())))
    }

    pub fn read_file(&self, p: &Path) -> Option<String> {
        (self.read_file)(p)
    }

    /// `stylesheets`: local stylesheets linked with `<link rel="stylesheet" href="…">` that exist.
    pub fn stylesheets(&self) -> Vec<PathBuf> {
        let m = &self.map;
        (0..m.nodes.len())
            .filter_map(|i| {
                if m.nodes[i].tag != "link" {
                    return None;
                }
                let rel = m.attribute("rel", i).unwrap_or_default().to_lowercase();
                if !swift_split(&rel, ' ', usize::MAX).iter().any(|r| r == "stylesheet") {
                    return None;
                }
                let href = m.attribute("href", i)?;
                if href.is_empty() || href.contains("://") || href.starts_with("//") || href.starts_with("data:") {
                    return None;
                }
                let clean = href.split('?').next().unwrap_or("").split('#').next().unwrap_or("");
                let file = if clean.starts_with('/') {
                    PathBuf::from(clean)
                } else {
                    standardize(&self.path.parent().unwrap_or(Path::new("/")).join(clean))
                };
                if file.exists() {
                    Some(file)
                } else {
                    None
                }
            })
            .collect()
    }

    /// `defaultDestination`: the page's first local stylesheet, else its `<style>` block.
    pub fn default_destination(&self) -> StyleDestination {
        self.stylesheets().into_iter().next().map(StyleDestination::Stylesheet).unwrap_or(StyleDestination::StyleBlock)
    }
}

/// `HTMLOps.inlineTags`: inline formatting that in-place text editing keeps.
pub const INLINE_TAGS: &[&str] = &[
    "a", "abbr", "b", "bdi", "bdo", "br", "cite", "code", "data", "del", "dfn", "em", "i", "ins", "kbd", "mark", "q", "s",
    "samp", "small", "span", "strong", "sub", "sup", "time", "u", "var", "wbr",
];

// MARK: Reading styles

/// `HTMLOps.declarations(_:_:selector:)`: the declarations of `selector` at a destination.
pub fn declarations_at(ctx: &HtmlContext, dest: &StyleDestination, selector: &str) -> Declarations {
    match dest {
        StyleDestination::Inline => vec![],
        StyleDestination::StyleBlock => match ctx.map.first_style_block() {
            Some(b) => rules::declarations(ctx.map.utf16(), b, selector),
            None => vec![],
        },
        StyleDestination::Stylesheet(f) => match ctx.read_file(f) {
            Some(css) => {
                let u = u16s(&css);
                let n = u.len();
                rules::declarations(&u, NsRange::new(0, n), selector)
            }
            None => vec![],
        },
    }
}

/// `HTMLOps.inlineDeclarations(_:_:)`.
pub fn inline_declarations(ctx: &HtmlContext, i: usize) -> Declarations {
    declarations::parse(&ctx.map.attribute("style", i).unwrap_or_default())
}

// MARK: Writing styles

/// `HTMLOps.ruleEdit(...)`: one CSS rule edit at the destination (creating a `<style>` block if the page has none).
pub fn rule_edit(
    ctx: &HtmlContext,
    dest: &StyleDestination,
    selector: &str,
    changes: &Changes,
    edits: &mut FileEdits,
) -> Result<(), HtmlError> {
    match dest {
        StyleDestination::Inline => return err("Rules can't be inline — pick a stylesheet or <style>"),
        StyleDestination::Stylesheet(file) => {
            let Some(css) = ctx.read_file(file) else { return err(format!("Can't read {}", last_component(file))) };
            let u = u16s(&css);
            let n = u.len();
            edits.add(file, rules::edit(&u, NsRange::new(0, n), selector, changes, ""));
        }
        StyleDestination::StyleBlock => {
            if let Some(block) = ctx.map.first_style_block() {
                edits.add(&ctx.path, rules::edit(ctx.map.utf16(), block, selector, changes, "  "));
            } else {
                let decls = declarations::setting(changes, &[]);
                let rule = format!("<style>\n  {selector} {{\n")
                    + &decls.iter().map(|(k, v)| format!("    {k}: {v};")).collect::<Vec<_>>().join("\n")
                    + "\n  }\n</style>\n";
                let head_end = ctx.map.nodes.iter().find(|n| n.tag == "head").and_then(|h| h.end_tag);
                let at = if let Some(end) = head_end {
                    end.location
                } else if let Some(body) = ctx.map.nodes.iter().find(|n| n.tag == "body") {
                    body.start_tag.location
                } else {
                    0
                };
                edits.add(&ctx.path, Edit::new(NsRange::new(at, 0), rule));
            }
        }
    }
    Ok(())
}

/// `HTMLOps.setProperties(...)`: sets properties on an element, inline or in a rule for `selector` at `dest`.
pub fn set_properties(
    ctx: &HtmlContext,
    i: usize,
    dest: &StyleDestination,
    selector: Option<&str>,
    changes: &Changes,
) -> Result<FileEdits, HtmlError> {
    let mut edits = FileEdits::default();
    if *dest == StyleDestination::Inline {
        let tag = ctx.map.start_tag_text(i);
        let decls = declarations::setting(changes, &inline_declarations(ctx, i));
        let style = if decls.is_empty() { None } else { Some(declarations::inline(&decls)) };
        edits.add(&ctx.path, Edit::new(ctx.map.nodes[i].start_tag, tag_text::setting("style", style.as_deref(), &tag)));
        edits.message = format!("Inline style on <{}>", ctx.map.label(i));
    } else {
        let sel = match selector {
            Some(s) if !s.is_empty() => s,
            _ => return err("Choose a class or id for the rule"),
        };
        rule_edit(ctx, dest, sel, changes, &mut edits)?;
        edits.message = format!("{sel} in {}", dest.label());
    }
    Ok(edits)
}

/// Start tag text with a class list change and optional inline properties removed (one edit per element).
fn retag(ctx: &HtmlContext, i: usize, classes: &[String], remove_inline: &BTreeSet<String>) -> Edit {
    let joined = classes.join(" ");
    let mut tag =
        tag_text::setting("class", if classes.is_empty() { None } else { Some(&joined) }, &ctx.map.start_tag_text(i));
    if !remove_inline.is_empty() {
        let kept: Declarations =
            inline_declarations(ctx, i).into_iter().filter(|(k, _)| !remove_inline.contains(k)).collect();
        let style = if kept.is_empty() { None } else { Some(declarations::inline(&kept)) };
        tag = tag_text::setting("style", style.as_deref(), &tag);
    }
    Edit::new(ctx.map.nodes[i].start_tag, tag)
}

/// `HTMLOps.splitStyle(...)`: gives one element its own class so it can be styled apart from the others sharing
/// `base_class`. With `copy`, the new class starts with the base's declarations and replaces it on this element;
/// otherwise the element keeps the base class and the new one only holds overrides (a variant).
pub fn split_style(
    ctx: &HtmlContext,
    i: usize,
    base_class: &str,
    new_class: &str,
    copy: bool,
    move_inline: bool,
    dest: &StyleDestination,
) -> Result<FileEdits, HtmlError> {
    if *dest == StyleDestination::Inline {
        return err("Splitting a style needs a stylesheet or <style> to write the new rule into");
    }
    let mut cls = ctx.map.classes(i);
    if new_class.is_empty() || cls.iter().any(|c| c == new_class) {
        return err("Pick a new class name");
    }
    let mut decls: Declarations = vec![];
    if copy && !base_class.is_empty() {
        decls = declarations_at(ctx, dest, &format!(".{base_class}"));
        if decls.is_empty() && dest.is_stylesheet() {
            decls = declarations_at(ctx, &StyleDestination::StyleBlock, &format!(".{base_class}"));
        }
    }
    let inline = if move_inline { inline_declarations(ctx, i) } else { vec![] };
    let merged = declarations::setting(&declarations::changes_from(&inline), &decls);
    match cls.iter().position(|c| c == base_class) {
        Some(k) if copy => cls[k] = new_class.to_string(),
        _ => cls.push(new_class.to_string()),
    }
    let mut edits = FileEdits::default();
    let remove: BTreeSet<String> = inline.iter().map(|(k, _)| k.clone()).collect();
    edits.add(&ctx.path, retag(ctx, i, &cls, &remove));
    rule_edit(ctx, dest, &format!(".{new_class}"), &declarations::changes_from(&merged), &mut edits)?;
    edits.message = if copy {
        format!("Split .{new_class} off .{base_class} in {}", dest.label())
    } else {
        format!("Added variant .{new_class} in {}", dest.label())
    };
    edits.structural = true;
    Ok(edits)
}

/// `HTMLOps.groupStyle(...)`: a new class shared by the selected elements, holding the given properties.
pub fn group_style(
    ctx: &HtmlContext,
    indices: &[usize],
    class_name: &str,
    properties: &BTreeMap<String, String>,
    move_inline: bool,
    dest: &StyleDestination,
) -> Result<FileEdits, HtmlError> {
    if *dest == StyleDestination::Inline {
        return err("A style group needs a stylesheet or <style> to live in");
    }
    if indices.is_empty() {
        return err("Select the elements to group (shift-click to add more)");
    }
    if class_name.is_empty() {
        return err("Name the new class");
    }
    let unique: BTreeSet<usize> = indices.iter().copied().collect();
    let mut edits = FileEdits::default();
    let remove: BTreeSet<String> = if move_inline { properties.keys().cloned().collect() } else { BTreeSet::new() };
    for &i in &unique {
        let mut cls = ctx.map.classes(i);
        if !cls.iter().any(|c| c == class_name) {
            cls.push(class_name.to_string());
        }
        edits.add(&ctx.path, retag(ctx, i, &cls, &remove));
    }
    let changes: Changes = properties.iter().map(|(k, v)| (k.clone(), Some(v.clone()))).collect();
    rule_edit(ctx, dest, &format!(".{class_name}"), &changes, &mut edits)?;
    edits.message = format!("Grouped {} element(s) as .{class_name} in {}", unique.len(), dest.label());
    edits.structural = true;
    Ok(edits)
}

/// `HTMLOps.addClass(_:_:_:)`.
pub fn add_class(ctx: &HtmlContext, i: usize, name: &str) -> FileEdits {
    let mut cls = ctx.map.classes(i);
    let mut e = FileEdits::default();
    if !cls.iter().any(|c| c == name) {
        cls.push(name.to_string());
        e.add(&ctx.path, retag(ctx, i, &cls, &BTreeSet::new()));
    }
    e.message = format!("Added class .{name}");
    e
}

/// `HTMLOps.setID(_:_:_:)`.
pub fn set_id(ctx: &HtmlContext, i: usize, id: &str) -> FileEdits {
    let mut e = FileEdits::default();
    e.add(&ctx.path, Edit::new(ctx.map.nodes[i].start_tag, tag_text::setting("id", Some(id), &ctx.map.start_tag_text(i))));
    e.message = format!("Set id #{id}");
    e
}

/// `HTMLOps.setText(_:_:_:)`: replaces a text-only element's contents with escaped text.
pub fn set_text(ctx: &HtmlContext, i: usize, text: &str) -> Result<FileEdits, HtmlError> {
    let inner = match ctx.map.nodes[i].inner_range() {
        Some(r) if ctx.map.is_text_only(i) => r,
        _ => return err(format!("<{}> contains other elements — edit its children instead", ctx.map.label(i))),
    };
    let escaped = text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let mut e = FileEdits::default();
    e.add(&ctx.path, Edit::new(inner, escaped));
    e.message = format!("Updated text of <{}>", ctx.map.label(i));
    Ok(e)
}

/// `HTMLOps.isInlineOnly(_:_:)`.
pub fn is_inline_only(map: &HtmlSourceMap, i: usize) -> bool {
    map.nodes[i].children.iter().all(|&c| INLINE_TAGS.contains(&map.nodes[c].tag.as_str()) && is_inline_only(map, c))
}

/// `\son[a-z]+\s*=`
fn has_event_handler(lower: &str) -> bool {
    let t: Vec<char> = lower.chars().collect();
    (0..t.len()).any(|p| {
        if !icu_ws(t[p] as u32) || t.get(p + 1) != Some(&'o') || t.get(p + 2) != Some(&'n') {
            return false;
        }
        let mut k = p + 3;
        while k < t.len() && t[k].is_ascii_lowercase() {
            k += 1;
        }
        if k == p + 3 {
            return false;
        }
        while k < t.len() && icu_ws(t[k] as u32) {
            k += 1;
        }
        t.get(k) == Some(&'=')
    })
}

/// `HTMLOps.setInnerHTML(_:_:_:)`: replaces an element's contents with text + inline formatting.
pub fn set_inner_html(ctx: &HtmlContext, i: usize, html: &str) -> Result<FileEdits, HtmlError> {
    let map = &ctx.map;
    let Some(inner) = map.nodes[i].inner_range() else {
        return err(format!("<{}> can't hold text", map.nodes[i].tag));
    };
    if !is_inline_only(map, i) {
        return err(format!("<{}> contains block elements — edit the text inside them instead", map.label(i)));
    }
    if map.substring(inner).contains("<!--") {
        return err(format!("<{}> contains an HTML comment — edit it in Source so the comment isn't lost", map.label(i)));
    }
    let fragment = HtmlSourceMap::new(html);
    if let Some(bad) = fragment.nodes.iter().find(|n| !INLINE_TAGS.contains(&n.tag.as_str())) {
        return err(format!("<{}> isn't allowed in text — only inline tags like b, i, a, span, code, br", bad.tag));
    }
    let lower = html.to_lowercase();
    if lower.contains("javascript:") || has_event_handler(&lower) {
        return err("Scripts and event handlers aren't allowed in text");
    }
    let mut e = FileEdits { message: format!("Updated text of <{}>", map.label(i)), structural: true, ..Default::default() };
    e.add(&ctx.path, Edit::new(inner, html));
    Ok(e)
}

/// `\sid\s*=\s*("[^"]*"|'[^']*'|[^\s>]+)` → "" (all matches).
fn strip_ids(s: &str) -> String {
    let t: Vec<char> = s.chars().collect();
    let ws = |c: char| icu_ws(c as u32);
    let at = |p: usize| -> Option<usize> {
        if !ws(t[p]) || t.get(p + 1) != Some(&'i') || t.get(p + 2) != Some(&'d') {
            return None;
        }
        let mut q = p + 3;
        while q < t.len() && ws(t[q]) {
            q += 1;
        }
        if t.get(q) != Some(&'=') {
            return None;
        }
        q += 1;
        while q < t.len() && ws(t[q]) {
            q += 1;
        }
        if q >= t.len() {
            return None;
        }
        if t[q] == '"' || t[q] == '\'' {
            if let Some(off) = t[q + 1..].iter().position(|&c| c == t[q]) {
                return Some(q + 1 + off + 1);
            }
        }
        let mut k = q;
        while k < t.len() && !ws(t[k]) && t[k] != '>' {
            k += 1;
        }
        if k > q {
            Some(k)
        } else {
            None
        }
    };
    let mut out = String::new();
    let mut p = 0;
    while p < t.len() {
        if let Some(end) = at(p) {
            p = end;
        } else {
            out.push(t[p]);
            p += 1;
        }
    }
    out
}

/// `HTMLOps.duplicate(_:_:)`: inserts a copy (without ids) after the element, on its own line.
pub fn duplicate(ctx: &HtmlContext, i: usize) -> FileEdits {
    let copy = strip_ids(&ctx.map.full_text(i));
    let mut e = FileEdits { message: format!("Duplicated <{}>", ctx.map.label(i)), structural: true, ..Default::default() };
    e.add(
        &ctx.path,
        Edit::new(NsRange::new(ctx.map.nodes[i].full_range().end(), 0), format!("\n{}{}", ctx.map.line_indent(i), copy)),
    );
    e
}

/// `HTMLOps.remove(_:_:)`: removes the element, and its whole line when it's alone on it.
pub fn remove(ctx: &HtmlContext, i: usize) -> FileEdits {
    let t = ctx.map.utf16();
    let mut r = ctx.map.nodes[i].full_range();
    let mut s = r.location;
    while s > 0 && (t[s - 1] == 32 || t[s - 1] == 9) {
        s -= 1;
    }
    if s == 0 || t[s - 1] == 10 {
        let mut end = r.end();
        if end < t.len() && t[end] == 10 {
            end += 1;
        }
        r = NsRange::new(s, end - s);
    }
    let mut e = FileEdits { message: format!("Removed <{}>", ctx.map.label(i)), structural: true, ..Default::default() };
    e.add(&ctx.path, Edit::new(r, ""));
    e
}

/// `HTMLOps.outline(_:maxNodes:)` (the `html_outline` MCP tool). Swift's default `max_nodes` is 400.
pub fn outline(ctx: &HtmlContext, max_nodes: usize) -> String {
    let map = &ctx.map;
    let mut lines: Vec<String> = vec![];
    let sheets = ctx.stylesheets();
    lines.push(
        "Stylesheets: ".to_string()
            + &if sheets.is_empty() {
                "none linked (rules go in <style>)".to_string()
            } else {
                sheets.iter().map(|p| last_component(p)).collect::<Vec<_>>().join(", ")
            },
    );
    fn walk(map: &HtmlSourceMap, i: usize, depth: usize, lines: &mut Vec<String>, max: usize) {
        if lines.len() >= max {
            return;
        }
        let n = &map.nodes[i];
        if n.selectable {
            let path = map.path(i).iter().map(|x| x.to_string()).collect::<Vec<_>>().join(".");
            let text = if map.is_text_only(i) { map.plain_text(i, 50) } else { String::new() };
            let inline = map.attribute("style", i).map(|s| format!(" style=\"{}\"", prefix_chars(&s, 60))).unwrap_or_default();
            lines.push(
                "  ".repeat(depth)
                    + &format!("{path}  <{}>{inline}", map.label(i))
                    + &if text.is_empty() { String::new() } else { format!("  “{text}”") },
            );
        }
        for &c in &n.children {
            walk(map, c, if n.selectable { depth + 1 } else { depth }, lines, max);
        }
    }
    for &r in &map.roots {
        walk(map, r, 0, &mut lines, max_nodes);
    }
    if lines.len() >= max_nodes {
        lines.push("… (outline truncated)".into());
    }
    lines.join("\n")
}

#[cfg(test)]
mod source_tests {
    //! Port of Tests/DesktopGlassTests/HTMLSourceTests.swift (one test per Swift test).
    use super::*;

    fn tags(m: &HtmlSourceMap, idx: &[usize]) -> Vec<String> {
        idx.iter().map(|&i| m.nodes[i].tag.clone()).collect()
    }
    fn pt(m: &HtmlSourceMap, i: usize) -> String {
        m.plain_text(i, 60)
    }

    #[test]
    fn implied_list_item_and_paragraph_ends() {
        let m = HtmlSourceMap::new("<ul><li>a<li>b</ul><p>one<div>two</div>");
        let ul = m.resolve("ul").unwrap();
        assert_eq!(tags(&m, &m.nodes[ul].children), ["li", "li"]);
        assert_eq!(pt(&m, m.nodes[ul].children[0]), "a");
        assert_eq!(pt(&m, m.nodes[ul].children[1]), "b");
        let p = m.resolve("p").unwrap();
        let div = m.resolve("div").unwrap();
        assert_eq!(pt(&m, p), "one");
        assert_eq!(m.nodes[div].parent, None, "a <div> closes the open <p>, so it isn't inside it");
        assert_eq!(tags(&m, &m.roots), ["ul", "p", "div"]);
    }

    #[test]
    fn implied_table_cell_ends() {
        let m = HtmlSourceMap::new("<table><tr><td>1<td>2</tr></table>");
        let tr = m.resolve("tr").unwrap();
        assert_eq!(tags(&m, &m.nodes[tr].children), ["td", "td"]);
        assert_eq!(m.nodes[tr].children.iter().map(|&c| pt(&m, c)).collect::<Vec<_>>(), ["1", "2"]);
    }

    #[test]
    fn script_contents_are_not_markup() {
        let m = HtmlSourceMap::new(r#"<script>if (a<b) { s = "<div>" }</script><div id="real">x</div>"#);
        assert_eq!(tags(&m, &[0, 1]), ["script", "div"]);
        assert_eq!(m.nodes.len(), 2);
        assert!(!m.nodes[0].selectable);
        assert_eq!(m.resolve("#real"), Some(1));
        assert_eq!(m.resolve("div"), Some(1));
    }

    #[test]
    fn comments_and_doctype_are_skipped() {
        let m = HtmlSourceMap::new(r#"<!doctype html><!-- <div id="ghost"> --><span>hi</span>"#);
        assert_eq!(m.nodes.iter().map(|n| n.tag.as_str()).collect::<Vec<_>>(), ["span"]);
        assert_eq!(m.resolve("#ghost"), None);
    }

    #[test]
    fn greater_than_inside_quoted_attribute() {
        let m = HtmlSourceMap::new(r#"<a title="a>b" href='x'>t</a>"#);
        assert_eq!(m.nodes.len(), 1);
        assert_eq!(m.attribute("title", 0).as_deref(), Some("a>b"));
        assert_eq!(m.attribute("href", 0).as_deref(), Some("x"));
        assert_eq!(pt(&m, 0), "t");
    }

    #[test]
    fn uppercase_tags_and_unquoted_attributes() {
        let m = HtmlSourceMap::new("<DIV CLASS=box>Hi</DIV>");
        assert_eq!(m.nodes[0].tag, "div");
        assert_eq!(m.classes(0), ["box"]);
        assert!(m.nodes[0].end_tag.is_some());
        assert_eq!(pt(&m, 0), "Hi");
    }

    #[test]
    fn void_and_self_closing_elements_have_no_children() {
        let m = HtmlSourceMap::new(r#"<p>a<br>b<img src="x.png"/></p>"#);
        let p = m.resolve("p").unwrap();
        assert_eq!(tags(&m, &m.nodes[p].children), ["br", "img"]);
        assert_eq!(m.nodes[m.nodes[p].children[0]].end_tag, None);
        assert_eq!(pt(&m, p), "a b");
    }

    #[test]
    fn unclosed_elements_end_at_end_of_file() {
        let m = HtmlSourceMap::new("<div><span>open");
        let span = m.resolve("span").unwrap();
        assert_eq!(pt(&m, span), "open");
        assert_eq!(m.nodes[span].parent, m.resolve("div"));
    }

    #[test]
    fn empty_and_text_only_input() {
        assert!(HtmlSourceMap::new("").nodes.is_empty());
        assert!(HtmlSourceMap::new("just text, a < b").nodes.is_empty());
    }

    #[test]
    fn non_visual_elements_are_not_selectable() {
        let m = HtmlSourceMap::new("<html><head><title>T</title><style>p{}</style></head><body><p>x</p></body></html>");
        let sel: Vec<&str> = m.nodes.iter().filter(|n| n.selectable).map(|n| n.tag.as_str()).collect();
        assert_eq!(sel, ["body", "p"]);
        assert!(m.first_style_block().is_some());
    }

    #[test]
    fn paths_round_trip() {
        let m = HtmlSourceMap::new("<html><head><title>T</title></head><body><main><p>a</p><p>b</p></main></body></html>");
        for i in 0..m.nodes.len() {
            assert_eq!(m.node_at_path(&m.path(i)), Some(i), "node {i} <{}>", m.nodes[i].tag);
        }
        assert_eq!(m.path(m.resolve("p:2").unwrap()), [0, 1, 0, 1]);
    }

    #[test]
    fn resolve_specs() {
        let m = HtmlSourceMap::new(r#"<div id="a" class="card big"><p>1</p><p>2</p></div>"#);
        assert_eq!(m.resolve("#a"), Some(0));
        assert_eq!(m.resolve(".big"), Some(0));
        assert_eq!(m.resolve("p"), Some(1));
        assert_eq!(m.resolve("p:2"), Some(2));
        assert_eq!(m.resolve(" P:2 "), Some(2));
        assert_eq!(m.resolve("@1"), Some(1));
        assert_eq!(m.resolve("0.1"), Some(2));
        assert_eq!(m.resolve("p:3"), None);
        assert_eq!(m.resolve("#missing"), None);
        assert_eq!(m.resolve("@99"), None);
        assert_eq!(m.resolve("5"), None);
    }

    #[test]
    fn label() {
        let m = HtmlSourceMap::new(r#"<section id="hero" class="a b c d">x</section>"#);
        assert_eq!(m.label(0), "section#hero.a.b.c");
    }

    #[test]
    fn annotated_marks_only_visual_elements() {
        let m = HtmlSourceMap::new("<div><br/><script>x</script></div>");
        assert_eq!(m.annotated(), r#"<div data-dg-id="0"><br data-dg-id="1"/><script>x</script></div>"#);
    }

    #[test]
    fn setting_attributes() {
        use tag_text::setting;
        assert_eq!(setting("id", Some("x"), "<p>"), r#"<p id="x">"#);
        assert_eq!(setting("class", Some("b"), "<p class='a' id=z>"), r#"<p class="b" id=z>"#);
        assert_eq!(setting("class", None, r#"<p class="a" id="z">"#), r#"<p id="z">"#);
        assert_eq!(setting("alt", Some("x"), "<img/>"), r#"<img alt="x"/>"#);
        assert_eq!(setting("class", None, "<p>"), "<p>");
    }

    #[test]
    fn attribute_values_are_escaped_and_read_back() {
        let tag = tag_text::setting("title", Some(r#"say "hi" & bye"#), "<a>");
        assert_eq!(tag, r#"<a title="say &quot;hi&quot; &amp; bye">"#);
        assert_eq!(tag_text::attribute("title", &tag).as_deref(), Some(r#"say "hi" & bye"#));
    }

    #[test]
    fn attribute_lookup_needs_a_whole_name() {
        assert_eq!(tag_text::attribute("class", r#"<p data-class="x">"#), None);
        assert_eq!(tag_text::attribute("disabled", "<button disabled>").as_deref(), Some(""));
        assert_eq!(tag_text::attribute("hidden", "<p>"), None);
    }

    // Extra (not in Swift): UTF-16 ranges stay aligned past non-BMP text.
    #[test]
    fn ranges_are_utf16_like_nsrange() {
        let ctx = HtmlContext::new("/p.html", "<p>😀</p><b>x</b>", Box::new(|_: &Path| None));
        assert_eq!(ctx.map.nodes[1].start_tag, NsRange::new(9, 3));
        let e = set_text(&ctx, 1, "y").unwrap();
        assert_eq!(apply(e.edits_for(Path::new("/p.html")), &ctx.text), "<p>😀</p><b>y</b>");
    }
}

#[cfg(test)]
mod ops_tests {
    //! Port of Tests/DesktopGlassTests/HTMLOpsTests.swift (one test per Swift test).
    use super::*;
    use std::collections::HashMap;

    const PAGE: &str = "/tmp/dg-tests/page.html";
    const SHEET: &str = "/tmp/dg-tests/site.css";

    fn context(html: &str, css: Option<&str>) -> HtmlContext {
        let mut files: HashMap<PathBuf, String> = HashMap::new();
        if let Some(c) = css {
            files.insert(PathBuf::from(SHEET), c.to_string());
        }
        HtmlContext::new(PAGE, html, Box::new(move |p: &Path| files.get(p).cloned()))
    }
    fn result(e: &FileEdits, ctx: &HtmlContext) -> String {
        apply(e.edits_for(Path::new(PAGE)), &ctx.text)
    }
    fn css_result(e: &FileEdits, css: &str) -> String {
        apply(e.edits_for(Path::new(SHEET)), css)
    }
    fn pairs(d: &[(String, String)]) -> Vec<String> {
        d.iter().map(|(k, v)| format!("{k}={v}")).collect()
    }
    fn ch(kv: &[(&str, Option<&str>)]) -> Changes {
        kv.iter().map(|(k, v)| (k.to_string(), v.map(String::from))).collect()
    }
    fn sheet() -> StyleDestination {
        StyleDestination::Stylesheet(PathBuf::from(SHEET))
    }

    #[test]
    fn parse_declarations() {
        let d = declarations::parse("color: red; background: url(data:image/png;base64,AA==); ;bad; Margin : 0 ");
        assert_eq!(pairs(&d), ["color=red", "background=url(data:image/png;base64,AA==)", "margin=0"]);
    }

    #[test]
    fn setting_declarations() {
        let d = declarations::setting(
            &ch(&[("color", None), ("padding", Some("4px")), ("margin", Some("  "))]),
            &[("color".into(), "red".into()), ("margin".into(), "0".into())],
        );
        assert_eq!(pairs(&d), ["padding=4px"]);
        assert_eq!(declarations::inline(&[("a".into(), "1".into()), ("b".into(), "2".into())]), "a: 1; b: 2");
    }

    #[test]
    fn apply_edits_in_any_order() {
        let out = apply(&[Edit::new(NsRange::new(0, 1), "A"), Edit::new(NsRange::new(4, 2), "EF!")], "abcdef");
        assert_eq!(out, "AbcdEF!");
    }

    #[test]
    fn inline_properties() {
        let ctx = context(r#"<p class="a" style="color: red">x</p>"#, None);
        let e = set_properties(&ctx, 0, &StyleDestination::Inline, None, &ch(&[("color", Some("blue")), ("margin", Some("0"))]))
            .unwrap();
        assert_eq!(result(&e, &ctx), r#"<p class="a" style="color: blue; margin: 0">x</p>"#);
        assert_eq!(e.message, "Inline style on <p.a>");
    }

    #[test]
    fn removing_the_last_inline_property_drops_the_attribute() {
        let ctx = context(r#"<p class="a" style="color: red">x</p>"#, None);
        let e = set_properties(&ctx, 0, &StyleDestination::Inline, None, &ch(&[("color", None)])).unwrap();
        assert_eq!(result(&e, &ctx), r#"<p class="a">x</p>"#);
    }

    #[test]
    fn existing_rule_in_style_block() {
        let ctx = context("<style>\n  .card { color: red; }\n</style>\n<div class=\"card\">x</div>", None);
        let e = set_properties(&ctx, 1, &StyleDestination::StyleBlock, Some(".card"), &ch(&[("padding", Some("4px"))])).unwrap();
        assert_eq!(
            result(&e, &ctx),
            "<style>\n  .card {\n    color: red;\n    padding: 4px;\n  }\n</style>\n<div class=\"card\">x</div>"
        );
    }

    #[test]
    fn new_style_block_goes_at_the_end_of_head() {
        let ctx = context("<html><head><title>t</title></head><body><p class=\"x\">y</p></body></html>", None);
        let p = ctx.map.resolve("p").unwrap();
        let e = set_properties(&ctx, p, &StyleDestination::StyleBlock, Some(".x"), &ch(&[("color", Some("red"))])).unwrap();
        assert_eq!(
            result(&e, &ctx),
            "<html><head><title>t</title><style>\n  .x {\n    color: red;\n  }\n</style>\n</head><body><p class=\"x\">y</p></body></html>"
        );
    }

    #[test]
    fn new_rule_is_appended_to_stylesheet() {
        let css = "body { margin: 0; }\n";
        let ctx = context(r#"<h1 id="hero">Hi</h1>"#, Some(css));
        let e = set_properties(&ctx, 0, &sheet(), Some("#hero"), &ch(&[("color", Some("red"))])).unwrap();
        assert_eq!(css_result(&e, css), "body { margin: 0; }\n\n#hero {\n  color: red;\n}\n");
        assert!(e.edits_for(Path::new(PAGE)).is_empty(), "the HTML file isn't touched when styles go to the stylesheet");
    }

    #[test]
    fn existing_rule_in_stylesheet_is_rewritten_in_place() {
        let css = "#hero {\n  color: red;\n}\n.other { color: blue; }\n";
        let ctx = context(r#"<h1 id="hero">Hi</h1>"#, Some(css));
        let e = set_properties(&ctx, 0, &sheet(), Some("#hero"), &ch(&[("color", None), ("margin", Some("0"))])).unwrap();
        assert_eq!(css_result(&e, css), "#hero {\n  margin: 0;\n}\n.other { color: blue; }\n");
    }

    #[test]
    fn similar_selectors_are_not_confused() {
        let css = ".card-alt { color: blue; }\n";
        let ctx = context(r#"<div class="card">x</div>"#, Some(css));
        let e = set_properties(&ctx, 0, &sheet(), Some(".card"), &ch(&[("color", Some("red"))])).unwrap();
        assert_eq!(css_result(&e, css), ".card-alt { color: blue; }\n\n.card {\n  color: red;\n}\n");
    }

    #[test]
    fn rules_need_a_selector_and_cant_be_inline() {
        let ctx = context("<p>x</p>", None);
        assert!(set_properties(&ctx, 0, &StyleDestination::StyleBlock, None, &ch(&[("color", Some("red"))])).is_err());
        let mut edits = FileEdits::default();
        assert!(rule_edit(&ctx, &StyleDestination::Inline, ".x", &ch(&[("color", Some("red"))]), &mut edits).is_err());
        assert!(
            set_properties(&ctx, 0, &sheet(), Some(".x"), &ch(&[("color", Some("red"))])).is_err(),
            "an unreadable stylesheet is an error, not a silent no-op"
        );
    }

    #[test]
    fn split_style_copies_the_base_rule_and_moves_inline_styles() {
        let ctx = context(
            "<style>.card { padding: 4px; color: red; }</style>\n<div class=\"card\">A</div>\n<div class=\"card\" style=\"color: blue\">B</div>",
            None,
        );
        let e = split_style(&ctx, 2, "card", "card-alt", true, true, &StyleDestination::StyleBlock).unwrap();
        assert_eq!(
            result(&e, &ctx),
            "<style>.card { padding: 4px; color: red; }\n\n  .card-alt {\n    color: blue;\n    padding: 4px;\n  }\n</style>\n"
                .to_string()
                + "<div class=\"card\">A</div>\n<div class=\"card-alt\">B</div>"
        );
        assert!(e.structural);
    }

    #[test]
    fn split_style_as_variant_keeps_the_base_class() {
        let css = ".card { padding: 4px; }\n";
        let ctx = context(r#"<div class="card">A</div>"#, Some(css));
        let e = split_style(&ctx, 0, "card", "card-wide", false, false, &sheet()).unwrap();
        assert_eq!(result(&e, &ctx), r#"<div class="card card-wide">A</div>"#);
        assert_eq!(css_result(&e, css), ".card { padding: 4px; }\n\n.card-wide {\n}\n");
    }

    #[test]
    fn split_style_rejects_bad_names_and_inline() {
        let ctx = context(r#"<div class="card">A</div>"#, None);
        assert!(split_style(&ctx, 0, "card", "card", true, false, &StyleDestination::StyleBlock).is_err());
        assert!(split_style(&ctx, 0, "card", "", true, false, &StyleDestination::StyleBlock).is_err());
        assert!(split_style(&ctx, 0, "card", "x", true, false, &StyleDestination::Inline).is_err());
    }

    #[test]
    fn group_style_writes_one_rule_and_tags_each_element() {
        let css = "p { margin: 0; }\n";
        let ctx = context(r#"<p style="font-weight: 700; color: red">a</p><p class="x">b</p>"#, Some(css));
        let props: BTreeMap<String, String> = [("font-weight".to_string(), "600".to_string())].into();
        let e = group_style(&ctx, &[0, 1, 0], "lead", &props, true, &sheet()).unwrap();
        assert_eq!(result(&e, &ctx), r#"<p style="color: red" class="lead">a</p><p class="x lead">b</p>"#);
        assert_eq!(css_result(&e, css), "p { margin: 0; }\n\n.lead {\n  font-weight: 600;\n}\n");
        assert_eq!(e.message, "Grouped 2 element(s) as .lead in site.css");
    }

    #[test]
    fn group_style_needs_elements_and_a_name() {
        let ctx = context("<p>a</p>", None);
        assert!(group_style(&ctx, &[], "x", &BTreeMap::new(), false, &StyleDestination::StyleBlock).is_err());
        assert!(group_style(&ctx, &[0], "", &BTreeMap::new(), false, &StyleDestination::StyleBlock).is_err());
    }

    #[test]
    fn add_class_and_set_id() {
        let ctx = context(r#"<p class="a">x</p>"#, None);
        assert!(add_class(&ctx, 0, "a").is_empty(), "adding a class it already has changes nothing");
        assert_eq!(result(&add_class(&ctx, 0, "b"), &ctx), r#"<p class="a b">x</p>"#);
        assert_eq!(result(&set_id(&ctx, 0, "intro"), &ctx), r#"<p class="a" id="intro">x</p>"#);
    }

    #[test]
    fn set_text_escapes_markup() {
        let ctx = context(r#"<h1 id="t">Hi</h1>"#, None);
        assert_eq!(result(&set_text(&ctx, 0, "A & <B>").unwrap(), &ctx), r#"<h1 id="t">A &amp; &lt;B&gt;</h1>"#);
    }

    #[test]
    fn set_text_refuses_elements_with_children() {
        let ctx = context("<div><p>x</p></div>", None);
        assert!(set_text(&ctx, 0, "y").is_err());
    }

    #[test]
    fn set_inner_html_keeps_inline_formatting() {
        let ctx = context("<p>Hello</p>", None);
        let e = set_inner_html(&ctx, 0, r#"Save <b>40%</b> with <a href="/plans">plans</a>"#).unwrap();
        assert_eq!(result(&e, &ctx), r#"<p>Save <b>40%</b> with <a href="/plans">plans</a></p>"#);
    }

    #[test]
    fn set_inner_html_rejects_unsafe_or_structural_content() {
        let ctx = context("<p>Hello</p>", None);
        for bad in [
            "<div>x</div>",
            r#"<a href="javascript:alert(1)">x</a>"#,
            r#"<a href="JavaScript:x">x</a>"#,
            r#"<span onclick="x()">x</span>"#,
            "<b onmouseover = 'x'>x</b>",
            "<script>x</script>",
            "<img src=x>",
        ] {
            assert!(set_inner_html(&ctx, 0, bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn set_inner_html_refuses_blocks_comments_and_void_elements() {
        assert!(set_inner_html(&context("<div><p>x</p></div>", None), 0, "y").is_err());
        assert!(set_inner_html(&context("<p>a<!-- keep -->b</p>", None), 0, "y").is_err());
        assert!(set_inner_html(&context("<p><img src=\"a.png\"></p>", None), 1, "y").is_err());
    }

    #[test]
    fn duplicate_drops_the_id() {
        let ctx = context("<ul>\n  <li id=\"a\" class=\"x\">A</li>\n</ul>", None);
        assert_eq!(result(&duplicate(&ctx, 1), &ctx), "<ul>\n  <li id=\"a\" class=\"x\">A</li>\n  <li class=\"x\">A</li>\n</ul>");
    }

    #[test]
    fn remove_takes_the_whole_line() {
        let ctx = context("<ul>\n  <li id=\"a\" class=\"x\">A</li>\n</ul>", None);
        assert_eq!(result(&remove(&ctx, 1), &ctx), "<ul>\n</ul>");
    }

    #[test]
    fn remove_inline_element_keeps_the_rest_of_the_line() {
        let ctx = context("<p>a <b>b</b> c</p>", None);
        assert_eq!(result(&remove(&ctx, 1), &ctx), "<p>a  c</p>");
    }

    #[test]
    fn linked_stylesheets_are_local_existing_files_only() {
        let dir = std::env::temp_dir().join(format!("dg-html-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("css")).unwrap();
        std::fs::write(dir.join("css/site.css"), "body {}").unwrap();
        let html = "<link rel=\"stylesheet\" href=\"css/site.css?v=2\">\n\
                    <link rel=\"stylesheet\" href=\"https://cdn.example.com/x.css\">\n\
                    <link rel=\"stylesheet\" href=\"//cdn.example.com/y.css\">\n\
                    <link rel=\"icon\" href=\"css/site.css\">\n\
                    <link rel=\"preload stylesheet\" href=\"missing.css\">";
        let ctx = HtmlContext::new(dir.join("index.html"), html, Box::new(|_: &Path| None));
        assert_eq!(ctx.stylesheets().iter().map(|p| last_component(p)).collect::<Vec<_>>(), ["site.css"]);
        match ctx.default_destination() {
            StyleDestination::Stylesheet(u) => assert_eq!(last_component(&u), "site.css"),
            other => panic!("expected the stylesheet, got {other:?}"),
        }
        assert_eq!(context("<p>x</p>", None).default_destination(), StyleDestination::StyleBlock);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn outline() {
        let out = super::outline(&context(r#"<body><h1 id="t" style="color: red">Hi</h1></body>"#, None), 400);
        assert_eq!(
            out.split('\n').collect::<Vec<_>>(),
            [
                "Stylesheets: none linked (rules go in <style>)",
                "0  <body>",
                "  0.0  <h1#t> style=\"color: red\"  “Hi”",
            ]
        );
    }
}
