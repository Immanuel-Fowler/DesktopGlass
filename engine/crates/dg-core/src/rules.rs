//! Folder classification: what is a group, a project, a package or a file. Single source of truth for the rules
//! that `Models.swift` (`Layout`) and `inventory.py` each keep a copy of today.

use crate::py;
use std::fs;
use std::path::Path;

pub const LOG_DIR: &str = "_Organize Log";
pub const DESC: &str = "description.md";
/// Desktop/A/B/C can be a described folder; deeper ones are just listed.
pub const MAX_DEPTH: usize = 3;
/// Entries sent to the indexer per folder; the rest are summarized as a count.
pub const MAX_ENTRIES: usize = 40;

pub const IGNORED: &[&str] = &[
    ".DS_Store", "desktop.ini", ".localized", "node_modules", ".venv", "venv", "__pycache__", ".git", "dist",
    "build", ".next", DESC, LOG_DIR, "Icon\r",
];
pub const PACKAGE_EXT: &[&str] = &[
    "app", "band", "bundle", "xcodeproj", "xcworkspace", "photoslibrary", "logicx", "framework", "playground",
    "rtfd", "pages", "numbers", "key",
];
pub const MARKERS: &[&str] = &[
    ".git", "package.json", "pyproject.toml", "requirements.txt", "setup.py", "Package.swift", "Cargo.toml",
    "go.mod", "pom.xml", "build.gradle", "Gemfile", "composer.json", "main.py", "app.py", "index.html", "Podfile",
    "pubspec.yaml", "deno.json", "README.md",
];
pub const MARKER_SUFFIXES: &[&str] = &[".xcodeproj", ".xcworkspace", ".sln", ".csproj"];
pub const TEXT_EXT: &[&str] = &[
    ".md", ".txt", ".py", ".js", ".ts", ".tsx", ".jsx", ".swift", ".json", ".csv", ".html", ".css", ".sh", ".yaml",
    ".yml", ".toml", ".sql", ".rs", ".go", ".java", ".c", ".cpp", ".h", ".rb", ".php", ".ipynb",
];
pub const NOTABLE: &[&str] = &[
    "next", "react", "vue", "svelte", "express", "fastify", "vite", "electron", "@tauri-apps/api", "expo",
    "react-native", "tailwindcss", "typescript", "prisma", "@supabase/supabase-js", "firebase", "three",
    "socket.io", "@anthropic-ai/sdk", "openai", "stripe", "plaid", "mongoose", "drizzle-orm",
    "@modelcontextprotocol/sdk",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Group,
    Project,
    Folder,
    Package,
    File,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Group => "group",
            Kind::Project => "project",
            Kind::Folder => "folder",
            Kind::Package => "package",
            Kind::File => "file",
        }
    }
}

/// Hidden, Office lock files and tool folders are never shown or described.
pub fn visible(name: &str) -> bool {
    !name.starts_with('.') && !name.starts_with("~$") && !IGNORED.contains(&name)
}

/// Every name in a folder (hidden ones included), in directory order; empty on error.
pub fn raw_names(p: &Path) -> Option<Vec<String>> {
    let rd = fs::read_dir(p).ok()?;
    Some(rd.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect())
}

/// Python's `sorted(names, key=str.lower)`: stable, by lower-cased code points.
pub fn sort_lower(names: &mut [String]) {
    let mut keyed: Vec<(String, String)> = names.iter().map(|n| (n.to_lowercase(), n.clone())).collect();
    keyed.sort_by(|a, b| a.0.cmp(&b.0));
    for (slot, (_, n)) in names.iter_mut().zip(keyed) {
        *slot = n;
    }
}

/// Visible names, sorted case-insensitively. Empty if the folder can't be read.
pub fn listdir(p: &Path) -> Vec<String> {
    let mut v: Vec<String> = raw_names(p).unwrap_or_default().into_iter().filter(|n| visible(n)).collect();
    sort_lower(&mut v);
    v
}

/// A real directory (symlinks to folders are not walked).
pub fn is_dir(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok_and(|m| m.is_dir())
}

pub fn is_marker(name: &str) -> bool {
    MARKERS.contains(&name) || MARKER_SUFFIXES.iter().any(|s| name.ends_with(s))
}

pub fn is_project(p: &Path) -> bool {
    raw_names(p).is_some_and(|names| names.iter().any(|n| is_marker(n)))
}

fn is_package_name(name: &str) -> bool {
    let ext = py::suffix(name).to_lowercase();
    let ext = ext.trim_start_matches('.');
    PACKAGE_EXT.contains(&ext)
}

/// `kind(p, depth)`: depth 1 is a top-level Desktop item.
pub fn kind(p: &Path, depth: usize) -> Kind {
    if !is_dir(p) {
        return Kind::File;
    }
    let name = p.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
    if is_package_name(&name) {
        return Kind::Package;
    }
    if is_project(p) {
        return Kind::Project;
    }
    if depth <= MAX_DEPTH {
        Kind::Group
    } else {
        Kind::Folder
    }
}

/// `rel` is one of `paths` or inside one of them ("Games/Switch" is under "Games"; "GamesArchive" is not).
pub fn under<S: AsRef<str>>(rel: &str, paths: &[S]) -> bool {
    paths.iter().any(|p| {
        let p = p.as_ref();
        rel == p || (rel.len() > p.len() && rel.starts_with(p) && rel.as_bytes()[p.len()] == b'/')
    })
}

/// One described folder: its path, Desktop-relative path and depth (1 = top level).
#[derive(Clone, Debug)]
pub struct Container {
    pub path: std::path::PathBuf,
    pub rel: String,
    pub depth: usize,
}

/// Every group folder on the Desktop, depth-first in listing order, skipping `skip` subtrees.
pub fn containers<S: AsRef<str>>(root: &Path, skip: &[S]) -> Vec<Container> {
    let mut out = Vec::new();
    walk(root, "", 1, skip, &mut out);
    out
}

fn walk<S: AsRef<str>>(dir: &Path, prefix: &str, depth: usize, skip: &[S], out: &mut Vec<Container>) {
    for n in listdir(dir) {
        let p = dir.join(&n);
        let rel = if prefix.is_empty() { n } else { format!("{prefix}/{n}") };
        if under(&rel, skip) {
            continue;
        }
        if kind(&p, depth) == Kind::Group {
            out.push(Container { path: p.clone(), rel: rel.clone(), depth });
            walk(&p, &rel, depth + 1, skip, out);
        }
    }
}
