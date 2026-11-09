//! End-to-end checks of the `dg` binary: version from app.conf, inventory usage and exit codes, and the
//! Python parity suite (engine/tests/parity/parity.sh) when python3 is available.

use std::path::Path;
use std::process::Command;

const DG: &str = env!("CARGO_BIN_EXE_dg");

#[test]
fn version_comes_from_app_conf() {
    let conf = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../app.conf")).unwrap();
    let version = conf.lines().find_map(|l| l.strip_prefix("VERSION=")).unwrap().trim();
    let out = Command::new(DG).arg("--version").output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), format!("dg {version}"));
}

#[test]
fn inventory_usage_exits_2() {
    for args in [vec![], vec!["/tmp"], vec!["/tmp", "nope"]] {
        let out = Command::new(DG).arg("inventory").args(&args).output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(String::from_utf8_lossy(&out.stdout).contains("DESKTOP scan [--all]"));
    }
}

#[test]
fn check_exit_codes() {
    let dir = std::env::temp_dir().join(format!("dg-cli-check-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Docs")).unwrap();
    std::fs::write(dir.join("Docs/a.txt"), "x").unwrap();
    let code = |cmd: &str| Command::new(DG).arg("inventory").arg(&dir).arg(cmd).output().unwrap().status.code();
    assert_eq!(code("check"), Some(1), "an undescribed folder is work");
    std::fs::write(dir.join("Docs/description.md"), "# Docs\n> d\n\n## Contents\n- **a.txt** — a\n").unwrap();
    assert_eq!(code("commit"), Some(0));
    assert_eq!(code("check"), Some(0), "nothing changed since the commit");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn parity_with_inventory_py() {
    if Command::new("python3").arg("--version").output().is_err() {
        eprintln!("python3 not found; skipping parity");
        return;
    }
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/parity/parity.sh");
    let out = Command::new("bash").arg(&script).arg(DG).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "parity failures:\n{text}");
}
