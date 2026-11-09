//! A temporary home for installer tests: nothing here ever reads or writes the real home, and agent CLIs are only
//! found in the temp `bin/` folder (no well-known folders, no app bundles).
#![allow(dead_code)]

use dg_install::Ctx;
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

pub struct Home {
    pub root: PathBuf,
}

impl Home {
    pub fn new() -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("dg-install-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Home { root: dg_core::paths::resolve(&dir) }
    }
    pub fn file(&self, rel: &str, text: &str) -> PathBuf {
        let p = self.root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, text).unwrap();
        p
    }
    pub fn dir(&self, rel: &str) -> PathBuf {
        let p = self.root.join(rel);
        fs::create_dir_all(&p).unwrap();
        p
    }
    pub fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.root.join(rel)).unwrap()
    }
    pub fn json(&self, rel: &str) -> Value {
        serde_json::from_str(&self.read(rel)).unwrap()
    }
    pub fn exists(&self, rel: &str) -> bool {
        self.root.join(rel).exists()
    }
    /// Paths for this home, with CLIs looked up only in `<home>/bin` and apps in `<home>/Applications`.
    pub fn ctx(&self) -> Ctx {
        let mut c = Ctx::for_home(&self.root);
        c.search = vec![self.root.join("bin")];
        c.codex_bundles.clear();
        c.applications = self.root.join("Applications");
        c
    }
    /// A fake executable in `<home>/bin`.
    pub fn tool(&self, name: &str, script: &str) -> PathBuf {
        let p = self.file(&format!("bin/{name}"), &format!("#!/bin/sh\n{script}\n"));
        let mut perm = fs::metadata(&p).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perm, 0o755);
        fs::set_permissions(&p, perm).unwrap();
        p
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
