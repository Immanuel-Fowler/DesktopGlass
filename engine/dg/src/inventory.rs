//! `dg inventory DESKTOP scan [--all] | check | commit | status`: same arguments, stdout and exit codes as
//! `inventory.py`.

use dg_core::{assemble, paths, py, scan};
use std::io::Write;
use std::process::ExitCode;

const USAGE: &str = "Compact, token-cheap Desktop inventory for the desktopglass-index skill.

Usage:
  inventory.py DESKTOP scan [--all]   compact JSON of folders whose description.md needs work
  inventory.py DESKTOP check          exit 0 if nothing changed (skip Claude), 1 if work is needed
  inventory.py DESKTOP commit         save the change manifest + rebuild _Organize Log/desktop-index.md
  inventory.py DESKTOP status         one-line status

Standard library only. Reads the Desktop; writes only inside _Organize Log/.
Folder rules mirror the DesktopGlass app (Sources/DesktopGlass/Models.swift).
";

fn out(text: &str) {
    let mut o = std::io::stdout().lock();
    let _ = o.write_all(text.as_bytes());
    let _ = o.write_all(b"\n");
    let _ = o.flush();
}

fn usage() -> ExitCode {
    out(USAGE);
    ExitCode::from(2)
}

/// `args` are what follows `dg inventory` (Python's `sys.argv[1:]`).
pub fn run(args: &[String]) -> ExitCode {
    if args.len() < 2 {
        return usage();
    }
    let root = paths::desktop_root(&args[0]);
    match args[1].as_str() {
        "scan" => {
            let force = args.iter().any(|a| a == "--all");
            out(&py::dumps(&scan::scan(&root, force), false));
            ExitCode::SUCCESS
        }
        "check" => ExitCode::from(scan::needs_work(&scan::scan(&root, false)) as u8),
        "commit" => match assemble::commit(&root) {
            Ok(status) => {
                out(&status);
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("dg inventory commit: {}: {e}", root.display());
                ExitCode::from(1)
            }
        },
        "status" => {
            out(&assemble::status_line(&root));
            ExitCode::SUCCESS
        }
        _ => usage(),
    }
}
