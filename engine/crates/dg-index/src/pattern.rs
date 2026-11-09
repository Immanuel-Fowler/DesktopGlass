//! A tiny backtracking matcher for the ERE subset the shell scripts' pattern tables use, so those tables stay
//! verbatim as data without pulling in a regex crate.
//!
//! Supported: literals, `.`, `[...]` / `[^...]` with ranges and `\` escapes, `\s` (whitespace), groups `( | )`,
//! the quantifiers `?`, `*`, `+`, and the anchors `^`/`$`. Matching is unanchored (a search, like `grep -E`).

#[derive(Debug, Clone)]
enum Node {
    Char(char),
    Any,
    Class { ranges: Vec<(char, char)>, space: bool, neg: bool },
    Start,
    End,
    Group(Vec<Vec<Item>>),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Quant {
    One,
    Opt,
    Star,
    Plus,
}

type Item = (Node, Quant);

#[derive(Debug, Clone)]
pub struct Pattern {
    alts: Vec<Vec<Item>>,
    ci: bool,
}

impl Pattern {
    /// Compiles `src`; `ci` = case-insensitive (`grep -i`). Panics on a malformed pattern (they're constants).
    pub fn new(src: &str, ci: bool) -> Pattern {
        let chars: Vec<char> = if ci { src.to_lowercase().chars().collect() } else { src.chars().collect() };
        let mut pos = 0;
        let alts = parse_alt(&chars, &mut pos);
        assert!(pos == chars.len(), "unbalanced pattern: {src}");
        Pattern { alts, ci }
    }

    /// True if the pattern matches anywhere in `text` (`.` matches newlines too; `^`/`$` are the text's ends).
    pub fn is_match(&self, text: &str) -> bool {
        let t: Vec<char> = if self.ci { text.to_lowercase().chars().collect() } else { text.chars().collect() };
        let whole = [(Node::Group(self.alts.clone()), Quant::One)];
        (0..=t.len()).any(|start| seq(&whole, &t, start, &|_| true))
    }

    /// `grep -E`: true if any line matches.
    pub fn any_line(&self, text: &str) -> bool {
        text.split('\n').any(|l| self.is_match(l))
    }
}

fn parse_alt(c: &[char], pos: &mut usize) -> Vec<Vec<Item>> {
    let mut alts = vec![parse_seq(c, pos)];
    while *pos < c.len() && c[*pos] == '|' {
        *pos += 1;
        alts.push(parse_seq(c, pos));
    }
    alts
}

fn parse_seq(c: &[char], pos: &mut usize) -> Vec<Item> {
    let mut items = Vec::new();
    while *pos < c.len() && c[*pos] != '|' && c[*pos] != ')' {
        let node = match c[*pos] {
            '(' => {
                *pos += 1;
                let g = parse_alt(c, pos);
                assert!(*pos < c.len() && c[*pos] == ')', "missing )");
                *pos += 1;
                Node::Group(g)
            }
            '[' => {
                *pos += 1;
                parse_class(c, pos)
            }
            '.' => {
                *pos += 1;
                Node::Any
            }
            '^' => {
                *pos += 1;
                Node::Start
            }
            '$' => {
                *pos += 1;
                Node::End
            }
            '\\' => {
                *pos += 2;
                match c[*pos - 1] {
                    's' => Node::Class { ranges: vec![], space: true, neg: false },
                    x => Node::Char(x),
                }
            }
            x => {
                *pos += 1;
                Node::Char(x)
            }
        };
        let q = match c.get(*pos) {
            Some('?') => Quant::Opt,
            Some('*') => Quant::Star,
            Some('+') => Quant::Plus,
            _ => Quant::One,
        };
        if q != Quant::One {
            *pos += 1;
        }
        items.push((node, q));
    }
    items
}

fn parse_class(c: &[char], pos: &mut usize) -> Node {
    let mut neg = false;
    if c.get(*pos) == Some(&'^') {
        neg = true;
        *pos += 1;
    }
    let mut ranges = Vec::new();
    let mut space = false;
    let mut first = true;
    loop {
        let ch = *c.get(*pos).expect("missing ]");
        if ch == ']' && !first {
            *pos += 1;
            break;
        }
        first = false;
        let lo = if ch == '\\' {
            *pos += 2;
            if c[*pos - 1] == 's' {
                space = true;
                continue;
            }
            c[*pos - 1]
        } else {
            *pos += 1;
            ch
        };
        if c.get(*pos) == Some(&'-') && c.get(*pos + 1).is_some_and(|n| *n != ']') {
            let hi = c[*pos + 1];
            *pos += 2;
            ranges.push((lo, hi));
        } else {
            ranges.push((lo, lo));
        }
    }
    Node::Class { ranges, space, neg }
}

/// Matches `items` at `pos`, then calls `k` with the end position (continuation-passing backtracking).
fn seq(items: &[Item], t: &[char], pos: usize, k: &dyn Fn(usize) -> bool) -> bool {
    let Some(((node, q), rest)) = items.split_first() else { return k(pos) };
    let then = |p: usize| seq(rest, t, p, k);
    match q {
        Quant::One => one(node, t, pos, &then),
        Quant::Opt => one(node, t, pos, &then) || then(pos),
        Quant::Star => star(node, t, pos, &then),
        Quant::Plus => one(node, t, pos, &|p| star(node, t, p, &then)),
    }
}

fn star(node: &Node, t: &[char], pos: usize, k: &dyn Fn(usize) -> bool) -> bool {
    one(node, t, pos, &|p| p > pos && star(node, t, p, k)) || k(pos)
}

fn one(node: &Node, t: &[char], pos: usize, k: &dyn Fn(usize) -> bool) -> bool {
    match node {
        Node::Start => pos == 0 && k(pos),
        Node::End => pos == t.len() && k(pos),
        Node::Group(alts) => alts.iter().any(|a| seq(a, t, pos, k)),
        _ => {
            let Some(&c) = t.get(pos) else { return false };
            let hit = match node {
                Node::Char(x) => c == *x,
                Node::Any => true,
                Node::Class { ranges, space, neg } => {
                    let inside = (*space && c.is_whitespace()) || ranges.iter().any(|(lo, hi)| *lo <= c && c <= *hi);
                    inside != *neg
                }
                _ => unreachable!(),
            };
            hit && k(pos + 1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basics() {
        let p = Pattern::new("authentication (is )?(required|failed|error)", true);
        assert!(p.is_match("Authentication required"));
        assert!(p.is_match("xx authentication is failed yy"));
        assert!(!p.is_match("authentication ok"));
        let n = Pattern::new("(^|[^0-9])401([^0-9]|$)", true);
        assert!(n.is_match("401"));
        assert!(n.is_match("HTTP 401: nope"));
        assert!(!n.is_match("4010"));
        assert!(!n.is_match("14012"));
        let r = Pattern::new(r#"run [`"']?[a-z-]+ (auth )?login"#, true);
        assert!(r.is_match("Please run `codex login` first"));
        assert!(r.is_match("run opencode auth login"));
        assert!(!r.is_match("run login"));
        let s = Pattern::new(r#""modelProvider"\s*:\s*"gemini""#, false);
        assert!(s.is_match(r#"{ "modelProvider" :  "gemini" }"#));
        assert!(!s.is_match(r#"{ "modelprovider": "gemini" }"#));
        let d = Pattern::new("login.*required", false);
        assert!(d.is_match("login is\nrequired"));
        let unauth = Pattern::new("unauthori[sz]ed", true);
        assert!(unauth.is_match("UNAUTHORISED"));
        let c = Pattern::new("no providers? (configured|found)", true);
        assert!(c.is_match("No provider found") && c.is_match("no providers configured"));
    }
}
