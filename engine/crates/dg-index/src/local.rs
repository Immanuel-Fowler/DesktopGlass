//! On-device indexing (local_index.py). Local models can't run tools, so dg does the agent's job: it takes the
//! compact scan, asks the model for one description.md per folder (batched), strips `<think>` blocks and code
//! fences, validates the reply (one firm retry), keeps unchanged blurbs verbatim, writes the files and rebuilds
//! the index.
//!
//! Backends: Ollama (`/api/*`), LM Studio and llama.cpp (OpenAI-compatible), MLX through `mlx_lm.server`
//! (OpenAI-compatible; started on a free port when none is running and a Python with mlx-lm exists), and Apple
//! Foundation Models through the app binary's `--foundation-models` entry point (macOS only).
//! Exit codes: 0 ok · 78 model/server not available · 1 every folder failed.

use crate::harness::label;
use crate::http::{self, Error as HttpError};
use crate::{exit, process};
use dg_core::description::{self, Description};
use dg_core::py::{self, strip, take_chars};
use dg_core::rules::{self, DESC};
use dg_core::{assemble, scan};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub fn default_endpoint(backend: &str) -> &'static str {
    match backend {
        "ollama" => "http://localhost:11434",
        "lmstudio" => "http://localhost:1234",
        // llama.cpp's llama-server and mlx_lm.server both default to port 8080
        _ => "http://localhost:8080",
    }
}

/// Apple's on-device model has a ~4k token window; the others comfortably take more per request.
pub fn batch_chars(backend: &str) -> usize {
    if backend == "apple" {
        2200
    } else {
        7000
    }
}

pub const SYSTEM: &str = "You write short description.md files for folders on a Mac Desktop.
Reply with ONLY the file content: no code fences, no commentary, no thinking.
Exact format:
# <Folder name>
> <one-sentence summary, at most 120 characters>
Tags: <3 to 6 lowercase tags, comma-separated>

## Contents
- **<folder entry>/** — <what it is, at most 90 characters>
- **<file entry>** — <what it is, at most 90 characters>

## Notes
<optional: one or two short lines>

Rules: one bullet per entry, in the given order, with the exact entry names (add / after folders).
Use only the given hints; if unsure, write a cautious guess ending with (?).
For projects, say what it does, then the stack. Say when something looks like a backup or copy of a sibling.";

const PING: Duration = Duration::from_secs(5);
const CHAT: Duration = Duration::from_secs(600);

/// A chat failure: `Stop` = the server stopped answering (exit 78), `Skip` = this folder only.
#[derive(Debug)]
pub enum ChatError {
    Stop(String),
    Skip(String),
}

impl From<HttpError> for ChatError {
    fn from(e: HttpError) -> Self {
        match e {
            HttpError::Transport(s) => ChatError::Stop(s),
            HttpError::Other(s) => ChatError::Skip(s),
        }
    }
}

pub trait Backend {
    /// Ok, or the "not available" message (exit 78).
    fn ping(&mut self) -> Result<(), String>;
    fn chat(&mut self, system: &str, user: &str) -> Result<String, ChatError>;
    fn model(&self) -> String;
}

fn messages(system: &str, user: &str) -> Value {
    json!([{"role": "system", "content": system}, {"role": "user", "content": user}])
}

fn reply_text(v: Option<&Value>) -> Result<String, ChatError> {
    match v {
        Some(Value::String(s)) => Ok(s.clone()),
        Some(Value::Null) => Ok(String::new()),
        _ => Err(ChatError::Skip("unexpected reply from the model server".into())),
    }
}

// ---------- Ollama ----------

pub struct Ollama {
    base: String,
    model: String,
}

impl Backend for Ollama {
    fn ping(&mut self) -> Result<(), String> {
        let data = http::json(&format!("{}/api/tags", self.base), None, PING).map_err(|_| {
            format!("Ollama isn't running at {} — open the Ollama app or run `ollama serve`", self.base)
        })?;
        let names: Vec<String> = data
            .get("models")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|m| m.get("name")?.as_str().filter(|s| !s.is_empty()).map(String::from)).collect())
            .unwrap_or_default();
        if names.is_empty() {
            return Err("Ollama has no models — run `ollama pull llama3.2` (or any model) first".into());
        }
        if self.model.is_empty() {
            self.model = names[0].clone();
        } else if !names.contains(&self.model) && !names.contains(&format!("{}:latest", self.model)) {
            return Err(format!("Ollama model '{0}' isn't pulled — run `ollama pull {0}`", self.model));
        }
        Ok(())
    }

    fn chat(&mut self, system: &str, user: &str) -> Result<String, ChatError> {
        let r = http::json(
            &format!("{}/api/chat", self.base),
            Some(&json!({"model": self.model, "stream": false, "options": {"temperature": 0.2, "num_ctx": 8192},
                         "messages": messages(system, user)})),
            CHAT,
        )?;
        match r.get("message").and_then(|m| m.get("content")) {
            None => Ok(String::new()),
            v => reply_text(v),
        }
    }

    fn model(&self) -> String {
        self.model.clone()
    }
}

// ---------- OpenAI-compatible: LM Studio, llama.cpp, mlx_lm.server ----------

pub struct OpenAICompat {
    name: String,
    base: String,
    model: String,
    max_tokens: Option<u32>,
}

impl OpenAICompat {
    fn models(&self, timeout: Duration) -> Result<Vec<String>, HttpError> {
        let data = http::json(&format!("{}/v1/models", self.base), None, timeout)?;
        Ok(data
            .get("data")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|m| m.get("id")?.as_str().filter(|s| !s.is_empty()).map(String::from)).collect())
            .unwrap_or_default())
    }
}

impl Backend for OpenAICompat {
    fn ping(&mut self) -> Result<(), String> {
        let ids = self.models(PING).map_err(|_| {
            let hint = if self.name == "lmstudio" { "open LM Studio and start its local server" } else { "start llama-server" };
            format!("{} isn't running at {} — {hint}", label(&self.name), self.base)
        })?;
        if self.model.is_empty() {
            match ids.first() {
                Some(m) => self.model = m.clone(),
                None => return Err(format!("{} has no model loaded", label(&self.name))),
            }
        }
        Ok(())
    }

    fn chat(&mut self, system: &str, user: &str) -> Result<String, ChatError> {
        let mut body = json!({"model": self.model, "temperature": 0.2, "stream": false, "messages": messages(system, user)});
        if let Some(n) = self.max_tokens {
            body["max_tokens"] = json!(n);
        }
        let r = http::json(&format!("{}/v1/chat/completions", self.base), Some(&body), CHAT)?;
        reply_text(r.pointer("/choices/0/message/content"))
    }

    fn model(&self) -> String {
        self.model.clone()
    }
}

// ---------- MLX (mlx_lm.server) ----------

pub const MLX_DEFAULT: &str = "mlx-community/Qwen2.5-7B-Instruct-4bit";

/// The first instruct/chat MLX model in the Hugging Face cache.
pub fn mlx_cached_default(home: &Path) -> Option<String> {
    let mut dirs: Vec<String> = std::fs::read_dir(home.join(".cache/huggingface/hub"))
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("models--mlx-community--"))
        .collect();
    dirs.sort();
    dirs.into_iter()
        .map(|d| d.replacen("models--", "", 1).replacen("--", "/", 1))
        .find(|n| {
            let l = n.to_lowercase();
            l.contains("instruct") || l.contains("-it") || l.contains("chat")
        })
}

pub struct Mlx {
    api: OpenAICompat,
    /// A Python that can import mlx_lm (to start a server); None when a server was already answering.
    python: Option<String>,
    /// The user gave an endpoint: never start a server elsewhere.
    explicit: bool,
    /// --ping: report readiness without starting a server.
    probe_only: bool,
    server: Option<Child>,
}

impl Mlx {
    fn start_server(&mut self, python: &str) -> Result<(), String> {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|l| l.local_addr())
            .map(|a| a.port())
            .map_err(|e| format!("MLX couldn't pick a port: {e}"))?;
        let mut cmd = Command::new(python);
        cmd.args(["-m", "mlx_lm.server", "--model", &self.api.model, "--host", "127.0.0.1", "--port", &port.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child = cmd.spawn().map_err(|e| format!("MLX server couldn't start ({e})"))?;
        self.server = Some(child);
        self.api.base = format!("http://127.0.0.1:{port}");
        // The first start may download the model from Hugging Face.
        let deadline = Instant::now() + Duration::from_secs(900);
        while Instant::now() < deadline {
            if let Some(c) = self.server.as_mut() {
                if let Ok(Some(st)) = c.try_wait() {
                    return Err(format!(
                        "MLX server exited ({}) — run `{python} -m mlx_lm.server --model {}` in Terminal to see why",
                        process::shell_code(st),
                        self.api.model
                    ));
                }
            }
            if self.api.models(Duration::from_secs(2)).is_ok() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        Err("MLX server didn't start within 15 minutes".into())
    }
}

impl Backend for Mlx {
    fn ping(&mut self) -> Result<(), String> {
        if self.api.models(PING).is_ok() {
            return Ok(());
        }
        match (self.python.clone(), self.explicit) {
            (Some(_), false) if self.probe_only => Ok(()),
            (Some(py), false) => self.start_server(&py),
            _ => Err(format!(
                "MLX server isn't running at {} — start it with `mlx_lm.server --model {}`",
                self.api.base, self.api.model
            )),
        }
    }

    fn chat(&mut self, system: &str, user: &str) -> Result<String, ChatError> {
        self.api.chat(system, user)
    }

    fn model(&self) -> String {
        self.api.model.clone()
    }
}

impl Drop for Mlx {
    fn drop(&mut self) {
        if let Some(mut c) = self.server.take() {
            unsafe { libc::kill(c.id() as libc::pid_t, libc::SIGTERM) };
            let _ = process::wait_timeout(&mut c, Some(Duration::from_secs(5)));
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

// ---------- Apple Foundation Models ----------

pub struct Apple {
    bin: String,
}

/// Runs the app binary with `arg`, `input` on stdin; returns (stdout, stderr, code).
fn run_app(bin: &str, arg: &str, input: Option<&str>, timeout: Duration) -> (String, String, i32) {
    let tin = process::temp_file("fm-in");
    let tout = process::temp_file("fm-out");
    let terr = process::temp_file("fm-err");
    let _ = std::fs::write(&tin, input.unwrap_or(""));
    let mut cmd = Command::new(bin);
    cmd.arg(arg);
    let code = match (std::fs::File::open(&tin), std::fs::File::create(&tout), std::fs::File::create(&terr)) {
        (Ok(i), Ok(o), Ok(e)) => match cmd.stdin(i).stdout(o).stderr(e).spawn() {
            Ok(mut c) => process::wait_timeout(&mut c, Some(timeout)).map(process::shell_code).unwrap_or(1),
            Err(_) => 127,
        },
        _ => 1,
    };
    let read = |p: &PathBuf| String::from_utf8_lossy(&std::fs::read(p).unwrap_or_default()).into_owned();
    let (o, e) = (read(&tout), read(&terr));
    for p in [&tin, &tout, &terr] {
        let _ = std::fs::remove_file(p);
    }
    (o, e, code)
}

impl Backend for Apple {
    fn ping(&mut self) -> Result<(), String> {
        if self.bin.is_empty() || !Path::new(&self.bin).exists() {
            return Err("Apple Foundation Models run through the DesktopGlass app — start a run from the app".into());
        }
        let (o, e, code) = run_app(&self.bin, "--foundation-models-check", None, Duration::from_secs(30));
        if code != 0 {
            let msg = strip(if o.is_empty() { &e } else { &o }).to_string();
            return Err(if msg.is_empty() { "Apple Intelligence isn't available on this Mac".into() } else { msg });
        }
        Ok(())
    }

    fn chat(&mut self, system: &str, user: &str) -> Result<String, ChatError> {
        let input = json!({"system": system, "prompt": user}).to_string();
        let (o, e, code) = run_app(&self.bin, "--foundation-models", Some(&input), CHAT);
        if code != 0 {
            let msg = strip(if o.is_empty() { &e } else { &o });
            return Err(ChatError::Skip(take_chars(msg, 300).to_string()));
        }
        Ok(o)
    }

    fn model(&self) -> String {
        "on-device".into()
    }
}

// ---------- options ----------

pub struct Options {
    pub backend: String,
    pub model: String,
    /// DESKTOP_INDEX_ENDPOINT (empty: the backend's default).
    pub endpoint: String,
    pub root: PathBuf,
    pub all: bool,
    pub ping: bool,
    pub home: PathBuf,
    /// Apple: the app binary. MLX: a Python with mlx-lm, or "server" when one was already answering.
    pub bin: String,
}

pub fn make_backend(o: &Options) -> Option<Box<dyn Backend>> {
    let base = |b: &str| {
        let e = if o.endpoint.is_empty() { default_endpoint(b) } else { o.endpoint.as_str() };
        e.trim_end_matches('/').to_string()
    };
    Some(match o.backend.as_str() {
        "ollama" => Box::new(Ollama { base: base("ollama"), model: o.model.clone() }),
        "lmstudio" | "llamacpp" => Box::new(OpenAICompat {
            name: o.backend.clone(),
            base: base(&o.backend),
            model: o.model.clone(),
            max_tokens: None,
        }),
        "mlx" => {
            let model = if o.model.is_empty() {
                mlx_cached_default(&o.home).unwrap_or_else(|| MLX_DEFAULT.into())
            } else {
                o.model.clone()
            };
            Box::new(Mlx {
                api: OpenAICompat { name: "mlx".into(), base: base("mlx"), model, max_tokens: Some(1500) },
                python: (o.bin != "server" && !o.bin.is_empty()).then(|| o.bin.clone()),
                explicit: !o.endpoint.is_empty(),
                probe_only: o.ping,
                server: None,
            })
        }
        "apple" if cfg!(target_os = "macos") => Box::new(Apple { bin: o.bin.clone() }),
        _ => return None,
    })
}

// ---------- prompting ----------

/// `json.dumps(v, ensure_ascii=False)` with Python's default `", "` / `": "` separators (for batch sizes).
fn dumps_spaced(v: &Value) -> String {
    match v {
        Value::Array(a) => format!("[{}]", a.iter().map(dumps_spaced).collect::<Vec<_>>().join(", ")),
        Value::Object(o) => format!(
            "{{{}}}",
            o.iter()
                .map(|(k, x)| format!("{}: {}", py::dumps(&Value::String(k.clone()), false), dumps_spaced(x)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        other => py::dumps(other, false),
    }
}

fn truthy(v: Option<&Value>) -> bool {
    v.is_some_and(py::truthy)
}

fn text(v: Option<&Value>) -> String {
    v.map(py::py_str).unwrap_or_default()
}

pub fn build_prompt(c: &Value, entries: &[Value], prefs: &Value, part: Option<(usize, usize)>) -> String {
    let terse = prefs.get("style").is_none_or(|s| s == "terse");
    let style = if terse { "short fragments" } else { "one full sentence each" };
    let mut lines = vec![format!("Folder: {}", text(c.get("path"))), format!("Blurb style: {style}.")];
    if prefs.get("flagStatus").is_none_or(py::truthy) {
        lines.push("Mark each project as active, backup or archived when dates or names suggest it.".into());
    }
    if truthy(prefs.get("notes")) {
        lines.push(format!("Notes from the user: {}", text(prefs.get("notes"))));
    }
    if truthy(c.get("private")) {
        lines.push("This folder is private: describe entries from their names only.".into());
    }
    if truthy(c.get("summary")) {
        lines.push(format!("Previous summary: {}", text(c.get("summary"))));
    }
    if let Some((i, n)) = part {
        lines.push(format!("(Part {i} of {n} of this folder's entries.)"));
    }
    lines.push("Entries as JSON (n = name, k = kind; keep = existing blurb, reuse it unchanged):".into());
    lines.push(py::dumps(&Value::Array(entries.to_vec()), false));
    if truthy(c.get("more")) {
        lines.push(format!("Also: {} — mention this in Notes.", text(c.get("more"))));
    }
    lines.join("\n")
}

/// Removes `<think>…</think>` blocks and code-fence lines, then starts at the first `# `.
pub fn clean(text: &str) -> String {
    // re.sub(r"(?s)<think>.*?</think>", "", text)
    let mut t = String::new();
    let mut rest = text;
    while let Some(i) = rest.find("<think>") {
        match rest[i + 7..].find("</think>") {
            Some(j) => {
                t.push_str(&rest[..i]);
                rest = &rest[i + 7 + j + 8..];
            }
            None => break,
        }
    }
    t.push_str(rest);
    // re.sub(r"(?m)^```[a-zA-Z]*\s*$", "", t)
    let chars: Vec<char> = t.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let line_start = i == 0 || chars[i - 1] == '\n';
        if line_start && chars[i..].starts_with(&['`', '`', '`']) {
            let mut j = i + 3;
            while j < chars.len() && chars[j].is_ascii_alphabetic() {
                j += 1;
            }
            let mut k = j;
            while k < chars.len() && py::is_space(chars[k]) {
                k += 1;
            }
            // greedy \s* then $: back off to the last position that is the end or sits before a newline
            let end = (j..=k).rev().find(|&p| p == chars.len() || chars[p] == '\n');
            if let Some(e) = end {
                i = e;
                if i < chars.len() {
                    out.push(chars[i]);
                    i += 1;
                }
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    match out.find("# ") {
        Some(i) => strip(&out[i..]).to_string(),
        None => strip(&out).to_string(),
    }
}

pub fn batches(entries: &[Value], limit: usize) -> Vec<Vec<Value>> {
    let mut out = vec![];
    let mut cur: Vec<Value> = vec![];
    let mut size = 0;
    for e in entries {
        let s = py::char_len(&dumps_spaced(e));
        if !cur.is_empty() && size + s > limit {
            out.push(std::mem::take(&mut cur));
            size = 0;
        }
        cur.push(e.clone());
        size += s;
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Merged across batches: summary, tags, blurbs, notes.
pub struct Described {
    pub summary: Option<String>,
    pub tags: Vec<String>,
    pub blurbs: HashMap<String, String>,
    pub notes: Vec<String>,
}

fn nonempty(s: &Option<String>) -> bool {
    s.as_deref().is_some_and(|s| !s.is_empty())
}

pub fn describe(backend: &mut dyn Backend, name: &str, c: &Value, prefs: &Value) -> Result<Described, ChatError> {
    let entries: Vec<Value> = c.get("entries").and_then(Value::as_array).cloned().unwrap_or_default();
    let todo: Vec<Value> = entries.iter().filter(|e| e.get("keep").is_none()).cloned().collect();
    let groups = batches(if todo.is_empty() { &entries } else { &todo }, batch_chars(name));
    let mut d = Described { summary: None, tags: vec![], blurbs: HashMap::new(), notes: vec![] };
    let n = groups.len();
    for (i, group) in groups.iter().enumerate() {
        let part = (n > 1).then_some((i + 1, n));
        let prompt = build_prompt(c, if todo.is_empty() { &entries } else { group }, prefs, part);
        let mut p: Description = description::parse(&clean(&backend.chat(SYSTEM, &prompt)?));
        if !nonempty(&p.summary) && p.blurbs.is_empty() {
            // model ignored the format: one firm retry
            let retry = format!("{prompt}\n\nFollow the exact format. Start with '# '.");
            p = description::parse(&clean(&backend.chat(SYSTEM, &retry)?));
        }
        if !nonempty(&d.summary) {
            d.summary = p.summary;
        }
        if d.tags.is_empty() {
            d.tags = p.tags;
        }
        if d.notes.is_empty() {
            d.notes = p.notes;
        }
        d.blurbs.extend(p.blurbs);
    }
    Ok(d)
}

pub fn write_description(root: &Path, c: &Value, d: &Described) -> std::io::Result<()> {
    let folder = root.join(text(c.get("path")));
    let existing = description::parse(&py::read_head(&folder.join(DESC), 20000));
    let entries: Vec<Value> = c.get("entries").and_then(Value::as_array).cloned().unwrap_or_default();
    let str_of = |e: &Value, k: &str| e.get(k).map(py::py_str);
    let keep: HashMap<String, String> =
        entries.iter().filter(|e| e.get("keep").is_some()).map(|e| (text(e.get("n")), text(e.get("keep")))).collect();
    let old: HashMap<String, String> =
        entries.iter().filter(|e| truthy(e.get("old"))).map(|e| (text(e.get("n")), str_of(e, "old").unwrap_or_default())).collect();
    let pick = |v: Option<&String>| v.filter(|s| !s.is_empty()).cloned();
    let summary = pick(d.summary.as_ref())
        .or_else(|| truthy(c.get("summary")).then(|| text(c.get("summary"))))
        .or_else(|| pick(existing.summary.as_ref()))
        .unwrap_or_else(|| "(?)".into());
    let name = folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut lines = vec![format!("# {name}"), format!("> {}", take_chars(strip(&summary), 160))];
    if !d.tags.is_empty() {
        let tags: Vec<String> = d.tags.iter().take(6).map(|t| strip(t).to_lowercase()).collect();
        lines.push(format!("Tags: {}", tags.join(", ")));
    }
    lines.push(String::new());
    lines.push("## Contents".into());
    for n in rules::listdir(&folder) {
        let b = pick(keep.get(&n))
            .or_else(|| pick(d.blurbs.get(&n)))
            .or_else(|| pick(d.blurbs.get(n.trim_end_matches('/'))))
            .or_else(|| pick(old.get(&n)))
            .or_else(|| pick(existing.blurbs.get(&n)));
        let slash = if rules::is_dir(&folder.join(&n)) { "/" } else { "" };
        let tail = b.map(|b| format!(" — {}", take_chars(strip(&b), 140))).unwrap_or_default();
        lines.push(format!("- **{n}{slash}**{tail}"));
    }
    if !d.notes.is_empty() {
        lines.push(String::new());
        lines.push("## Notes".into());
        lines.extend(d.notes.iter().take(4).cloned());
    }
    std::fs::write(folder.join(DESC), lines.join("\n") + "\n")
}

/// The whole on-device run. `emit` gets each output line (stdout and stderr lines, in order).
pub fn run(o: &Options, emit: &mut dyn FnMut(&str)) -> i32 {
    let Some(mut backend) = make_backend(o) else {
        emit(&format!("unknown backend {}", o.backend));
        return exit::USAGE;
    };
    let name = label(&o.backend);
    if let Err(msg) = backend.ping() {
        emit(&msg);
        return exit::NOT_READY;
    }
    if o.ping {
        emit(&format!("{name} is ready · {}", backend.model()));
        return exit::OK;
    }
    let s = scan::scan(&o.root, o.all);
    let prefs = s.get("prefs").cloned().unwrap_or(json!({}));
    let (mut done, mut failed) = (0, 0);
    for c in s.get("containers").and_then(Value::as_array).cloned().unwrap_or_default() {
        let path = text(c.get("path"));
        let result = describe(backend.as_mut(), &o.backend, &c, &prefs)
            .and_then(|d| write_description(&o.root, &c, &d).map_err(|e| ChatError::Skip(e.to_string())));
        match result {
            Ok(()) => {
                done += 1;
                emit(&format!("described {path}"));
            }
            Err(ChatError::Stop(e)) => {
                emit(&format!("{name} stopped responding ({e}) — is it still running?"));
                return exit::NOT_READY;
            }
            Err(ChatError::Skip(e)) => {
                // one bad folder shouldn't stop the rest
                failed += 1;
                emit(&format!("skipped {path}: {e}"));
            }
        }
    }
    if let Err(e) = assemble::commit(&o.root) {
        emit(&format!("couldn't rebuild the index: {e}"));
        return exit::FAILED;
    }
    if failed > 0 && done == 0 {
        return exit::FAILED;
    }
    exit::OK
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_strips_think_and_fences() {
        assert_eq!(clean("<think>hmm\n# no</think>\n```markdown\n# Docs\n> s\n```\n"), "# Docs\n> s");
        assert_eq!(clean("Sure! Here it is:\n```\n\n# A\n```"), "# A");
        assert_eq!(clean("no heading"), "no heading");
        assert_eq!(clean("```python x\n# A"), "# A", "a fence with trailing text stays but the heading wins");
        assert_eq!(clean("<think>unterminated # x"), "# x");
    }

    #[test]
    fn batching_uses_python_sizes() {
        let e = json!({"n": "a", "k": "file"});
        assert_eq!(dumps_spaced(&e), r#"{"n": "a", "k": "file"}"#);
        let es: Vec<Value> = (0..10).map(|i| json!({"n": format!("f{i}"), "k": "file"})).collect();
        let one = py::char_len(&dumps_spaced(&es[0]));
        let b = batches(&es, one * 3);
        assert_eq!(b.iter().map(Vec::len).collect::<Vec<_>>(), [3, 3, 3, 1]);
        assert_eq!(batches(&es[..1], 1).len(), 1, "an oversized entry still gets its own batch");
    }

    #[test]
    fn prompt_lines() {
        let c = json!({"path": "Docs", "private": 1, "summary": "Old", "more": "+3 more: 3 .jpg"});
        let p = build_prompt(&c, &[json!({"n": "é", "keep": "x"})], &json!({"style": "sentence", "flagStatus": false, "notes": "hi"}), Some((1, 2)));
        assert_eq!(
            p,
            "Folder: Docs\nBlurb style: one full sentence each.\nNotes from the user: hi\nThis folder is private: describe entries from their names only.\nPrevious summary: Old\n(Part 1 of 2 of this folder's entries.)\nEntries as JSON (n = name, k = kind; keep = existing blurb, reuse it unchanged):\n[{\"n\":\"é\",\"keep\":\"x\"}]\nAlso: +3 more: 3 .jpg — mention this in Notes."
        );
        let q = build_prompt(&json!({"path": "A"}), &[], &json!({}), None);
        assert!(q.contains("short fragments") && q.contains("Mark each project"));
    }

    struct Scripted(Vec<String>, usize);
    impl Backend for Scripted {
        fn ping(&mut self) -> Result<(), String> {
            Ok(())
        }
        fn chat(&mut self, _: &str, _: &str) -> Result<String, ChatError> {
            self.1 += 1;
            Ok(self.0.get(self.1 - 1).cloned().unwrap_or_default())
        }
        fn model(&self) -> String {
            "m".into()
        }
    }

    #[test]
    fn retry_once_then_keep_verbatim() {
        let root = std::env::temp_dir().join(format!("dg-local-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Docs/sub")).unwrap();
        std::fs::write(root.join("Docs/a.txt"), "x").unwrap();
        std::fs::write(root.join("Docs/b.txt"), "x").unwrap();
        let c = json!({"path": "Docs", "entries": [{"n": "a.txt", "keep": "kept as is"}, {"n": "b.txt", "k": "file"}, {"n": "sub", "k": "folder"}]});
        let mut be = Scripted(
            vec![
                "I can't do that".into(),
                "<think>x</think>```\n# Docs\n> Papers.\nTags: A, b\n\n## Contents\n- **a.txt** — changed!\n- **b.txt** — second file\n- **sub/** — a folder\n\n## Notes\nn1\n```".into(),
            ],
            0,
        );
        let d = describe(&mut be, "ollama", &c, &json!({})).unwrap();
        assert_eq!(be.1, 2, "one retry");
        write_description(&root, &c, &d).unwrap();
        let out = std::fs::read_to_string(root.join("Docs/description.md")).unwrap();
        assert_eq!(
            out,
            "# Docs\n> Papers.\nTags: a, b\n\n## Contents\n- **a.txt** — kept as is\n- **b.txt** — second file\n- **sub/** — a folder\n\n## Notes\nn1\n"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
