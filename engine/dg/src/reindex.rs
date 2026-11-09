//! `dg reindex` · `dg ask` · `dg login` (Phase 2). Thin wrappers over the dg-index crate, which keeps
//! reindex.sh / ask.sh / login.sh's inputs, output lines and exit codes.
use dg_index::{Mode, Options};
use std::process::ExitCode;

fn code(c: i32) -> ExitCode {
    ExitCode::from((c & 0xff) as u8)
}

fn options() -> Options {
    Options::from_env(&|k| std::env::var(k).ok())
}

/// `dg reindex [--auth-only | --permissions] [flags]`; environment as reindex.sh (see `dg reindex --help`).
pub fn reindex(args: &[String]) -> ExitCode {
    let mut o = options();
    match o.apply_args(args) {
        Ok(mode) => code(dg_index::reindex(&o, mode)),
        Err(e) => {
            if !e.is_empty() {
                eprintln!("dg reindex: {e}");
            }
            eprintln!("{}", dg_index::USAGE);
            ExitCode::from(2)
        }
    }
}

/// `dg ask [harness] [model]` (positional, like ask.sh); flags such as `--desktop` also work.
pub fn ask(args: &[String]) -> ExitCode {
    let mut o = options();
    o.harness = "claude".into();
    o.model.clear();
    match split(args, &mut o) {
        Ok(pos) => {
            if let Some(h) = pos.first() {
                o.harness = h.clone();
            }
            if let Some(m) = pos.get(1) {
                o.model = m.clone();
            }
            code(dg_index::ask(&o))
        }
        Err(e) => usage("ask [harness] [model]", &e),
    }
}

/// `dg login [harness]` (login.sh).
pub fn login(args: &[String]) -> ExitCode {
    let mut o = options();
    o.harness = "claude".into();
    match split(args, &mut o) {
        Ok(pos) => {
            if let Some(h) = pos.first() {
                o.harness = h.clone();
            }
            code(dg_index::login(&o))
        }
        Err(e) => usage("login [harness]", &e),
    }
}

/// Positional arguments out; `--flag` ones applied to `o`.
fn split(args: &[String], o: &mut Options) -> Result<Vec<String>, String> {
    let (flags, pos): (Vec<&String>, Vec<&String>) = {
        let mut flags = vec![];
        let mut pos = vec![];
        let mut it = args.iter().peekable();
        while let Some(a) = it.next() {
            if a.starts_with("--") {
                flags.push(a);
                let takes_value = matches!(a.as_str(), "--desktop" | "--skill-dir" | "--model" | "--harness");
                if takes_value {
                    if let Some(v) = it.next() {
                        flags.push(v);
                    }
                }
            } else {
                pos.push(a);
            }
        }
        (flags, pos)
    };
    let flags: Vec<String> = flags.into_iter().cloned().collect();
    match o.apply_args(&flags)? {
        Mode::Run => Ok(pos.into_iter().cloned().collect()),
        _ => Err("unexpected flag".into()),
    }
}

fn usage(what: &str, e: &str) -> ExitCode {
    if !e.is_empty() {
        eprintln!("dg: {e}");
    }
    eprintln!("Usage: dg {what}");
    ExitCode::from(2)
}
