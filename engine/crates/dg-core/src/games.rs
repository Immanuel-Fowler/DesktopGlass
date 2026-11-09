//! Port of the non-UI parts of `Games/GameLibrary.swift` (the Games-folder scan, `cleanTitle`), `Games/GameFinder.swift`
//! (searching the Mac for games: apps whose Info.plist says they're games, Steam libraries; and writing shortcuts) and
//! `Games/GameCatalog.swift` (`GamePlatform`). Launching, play history (UserDefaults) and the views stay in Swift.
//!
//! OS-specific pieces live in [`platform`]: Finder-alias detection, resolution and creation (CoreFoundation's bookmark
//! API, the same calls behind Swift's `URL(resolvingAliasFileAt:)` / `writeBookmarkData`), the "hidden" file flag, and
//! reading a binary Info.plist (`plutil`). Symlinks are handled with std everywhere.
//!
//! Approximations (documented differences from Foundation):
//! - `.skipsPackageDescendants`: a directory is a package when its extension is a known bundle extension
//!   ([`PACKAGE_EXTENSIONS`]); Launch Services' full list and the Finder "bundle bit" aren't consulted.
//! - `localizedStandardCompare`: a natural, case-insensitive order (digit runs compared as numbers), not ICU collation.
//! - Paths are compared after `fs::canonicalize` (Swift: `resolvingSymlinksInPath().standardizedFileURL`), which also
//!   resolves `/private` consistently on both sides.

use serde_json::{json, Value};
use std::cmp::Ordering;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// `GamePlatform`: only native Mac games are listed; kept so the JSON keeps `platform` / `platform_id`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GamePlatform {
    Mac,
}

impl GamePlatform {
    /// `GamePlatform.allCases`.
    pub const ALL: &'static [GamePlatform] = &[GamePlatform::Mac];
    /// `rawValue` / `id`.
    pub fn id(&self) -> &'static str {
        "mac"
    }
    /// `name`.
    pub fn name(&self) -> &'static str {
        "Mac"
    }
    /// `symbol` (SF Symbol name).
    pub fn symbol(&self) -> &'static str {
        "macwindow"
    }
}

/// `GameEntry` (without `lastPlayed`, which is UI-side play history).
#[derive(Clone, Debug, PartialEq)]
pub struct GameEntry {
    /// What's in the Games folder (may be a Finder alias / symlink); the card's identity.
    pub path: PathBuf,
    /// The real .app, what gets launched.
    pub target: PathBuf,
    pub platform: GamePlatform,
    pub title: String,
    /// "app" or "shortcut".
    pub detected_by: String,
    /// Modification time of `path` (seconds since 1970), for the library's item list.
    pub modified: Option<f64>,
}

impl GameEntry {
    pub fn to_json(&self) -> Value {
        json!({
            "path": self.path.to_string_lossy(), "target": self.target.to_string_lossy(), "title": self.title,
            "platform": self.platform.name(), "platform_id": self.platform.id(), "detected_by": self.detected_by,
            "modified": self.modified,
        })
    }
}

/// `FoundGame`: a game found on the Mac that isn't in the Games folder yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoundGame {
    pub path: PathBuf,
    pub title: String,
    pub platform: GamePlatform,
    /// "Applications" or "Steam".
    pub source: String,
}

impl FoundGame {
    pub fn to_json(&self) -> Value {
        json!({"path": self.path.to_string_lossy(), "title": self.title, "platform": self.platform.name(),
               "platform_id": self.platform.id(), "source": self.source})
    }
}

/// Directory extensions treated as packages (not entered while scanning).
pub const PACKAGE_EXTENSIONS: &[&str] = &[
    "app", "bundle", "framework", "plugin", "kext", "xpc", "appex", "pkg", "mpkg", "component", "prefpane",
    "qlgenerator", "mdimporter", "saver", "wdgt", "photoslibrary", "rtfd", "xcodeproj", "xcworkspace", "playground",
    "musiclibrary", "photolibrary", "fcpbundle", "logicx", "band", "scptd", "workflow", "docset", "dext", "systemextension",
];

// MARK: - Names

fn file_name(p: &Path) -> String {
    p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

/// `URL.pathExtension`.
fn extension(p: &Path) -> String {
    let n = file_name(p);
    match n.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => ext.to_string(),
        _ => String::new(),
    }
}

/// `url.deletingPathExtension().lastPathComponent`.
fn stem(p: &Path) -> String {
    let n = file_name(p);
    match n.rsplit_once('.') {
        Some((s, _)) if !s.is_empty() => s.to_string(),
        _ => n,
    }
}

/// Regex `\s*\[[^\]]*\]` → "" (ICU `\s`).
fn strip_bracket_tags(s: &str) -> String {
    let ws = |c: char| matches!(c, '\t' | '\n' | '\u{0C}' | '\r' | '\u{2028}' | '\u{2029}')
        || matches!(c as u32, 0x20 | 0xA0 | 0x1680 | 0x2000..=0x200A | 0x202F | 0x205F | 0x3000);
    let t: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < t.len() {
        let mut j = i;
        while j < t.len() && ws(t[j]) {
            j += 1;
        }
        if j < t.len() && t[j] == '[' {
            if let Some(off) = t[j + 1..].iter().position(|&c| c == ']') {
                i = j + 1 + off + 1;
                continue;
            }
        }
        out.push(t[i]);
        i += 1;
    }
    out
}

/// `GameLibrary.cleanTitle(_:)`: "Super Game [v1.2].app" → "Super Game".
pub fn clean_title(path: &Path) -> String {
    let t = strip_bracket_tags(&stem(path)).replace('_', " ");
    let t = t.trim_matches(|c: char| c == '\t' || matches!(c as u32, 0x20 | 0xA0 | 0x1680 | 0x2000..=0x200A | 0x202F | 0x205F | 0x3000));
    if t.is_empty() {
        file_name(path)
    } else {
        t.to_string()
    }
}

/// `GameFinder.cleanName(_:)`: the file name without extension and `[tags]`.
pub fn clean_name(path: &Path) -> String {
    strip_bracket_tags(&stem(path))
}

/// Approximates `localizedStandardCompare`: case-insensitive, digit runs compared numerically.
pub fn localized_standard_compare(a: &str, b: &str) -> Ordering {
    let (x, y): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let (mut i, mut j) = (0, 0);
    while i < x.len() && j < y.len() {
        if x[i].is_ascii_digit() && y[j].is_ascii_digit() {
            let (si, sj) = (i, j);
            while i < x.len() && x[i].is_ascii_digit() {
                i += 1;
            }
            while j < y.len() && y[j].is_ascii_digit() {
                j += 1;
            }
            let na: String = x[si..i].iter().collect::<String>().trim_start_matches('0').to_string();
            let nb: String = y[sj..j].iter().collect::<String>().trim_start_matches('0').to_string();
            let o = na.len().cmp(&nb.len()).then(na.cmp(&nb));
            if o != Ordering::Equal {
                return o;
            }
        } else {
            let o = x[i].to_lowercase().cmp(y[j].to_lowercase());
            if o != Ordering::Equal {
                return o;
            }
            i += 1;
            j += 1;
        }
    }
    (x.len() - i).cmp(&(y.len() - j)).then_with(|| a.cmp(b))
}

// MARK: - Walking (FileManager.enumerator with .skipsHiddenFiles, .skipsPackageDescendants)

fn is_package_dir(p: &Path) -> bool {
    PACKAGE_EXTENSIONS.contains(&extension(p).to_lowercase().as_str())
}

/// Visits everything under `root` down to `max_level` (direct children are level 1), skipping hidden items, not
/// entering packages and not following symlinks. Entries are visited in name order.
fn enumerate(root: &Path, max_level: usize, f: &mut dyn FnMut(&Path)) {
    fn visit(dir: &Path, level: usize, max: usize, f: &mut dyn FnMut(&Path)) {
        if level > max {
            return;
        }
        let Ok(rd) = fs::read_dir(dir) else { return };
        let mut entries: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
        entries.sort();
        for p in entries {
            if file_name(&p).starts_with('.') || platform::is_hidden(&p) {
                continue;
            }
            f(&p);
            let Ok(meta) = fs::symlink_metadata(&p) else { continue };
            if meta.is_dir() && !is_package_dir(&p) {
                visit(&p, level + 1, max, f);
            }
        }
    }
    visit(root, 1, max_level, f);
}

fn real_path(p: &Path) -> PathBuf {
    fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// What a Games-folder item points to: itself, or the resolved target of a symlink / Finder alias
/// (`None` when a shortcut can't be resolved).
fn resolve_shortcut(p: &Path) -> Option<(PathBuf, bool)> {
    let meta = fs::symlink_metadata(p).ok()?;
    if meta.file_type().is_symlink() {
        return fs::canonicalize(p).ok().map(|t| (t, true));
    }
    if meta.is_file() && platform::is_alias_file(p) {
        return platform::resolve_alias(p).map(|t| (t, true));
    }
    Some((p.to_path_buf(), false))
}

// MARK: - The Games folder (GameLibrary)

/// `GameLibrary.findGames(in:)`: Mac apps in the Games folder (to depth 4, not inside packages), including Finder
/// aliases and symlinks to apps; sorted by title.
pub fn find_games(root: &Path) -> Vec<GameEntry> {
    let mut out = vec![];
    enumerate(root, 4, &mut |p| {
        let Some((target, shortcut)) = resolve_shortcut(p) else { return };
        if extension(&target).to_lowercase() != "app" {
            return;
        }
        let modified = fs::symlink_metadata(p)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs_f64());
        out.push(GameEntry {
            path: p.to_path_buf(),
            target,
            platform: GamePlatform::Mac,
            title: clean_title(p),
            detected_by: if shortcut { "shortcut" } else { "app" }.into(),
            modified,
        });
    });
    out.sort_by(|a, b| localized_standard_compare(&a.title, &b.title));
    out
}

/// `find_games` as a JSON array.
pub fn find_games_json(root: &Path) -> Value {
    Value::Array(find_games(root).iter().map(GameEntry::to_json).collect())
}

/// `GameFinder.alreadyInGamesFolder(_:)`: real paths of everything in (or linked from) the Games folder.
pub fn already_in_games_folder(root: &Path) -> HashSet<PathBuf> {
    let mut paths = HashSet::new();
    enumerate(root, 5, &mut |p| {
        let real = match fs::symlink_metadata(p) {
            Ok(m) if m.is_file() && platform::is_alias_file(p) => platform::resolve_alias(p).unwrap_or_else(|| p.to_path_buf()),
            _ => p.to_path_buf(),
        };
        paths.insert(real_path(&real));
    });
    paths
}

// MARK: - Finding games on the Mac (GameFinder)

/// The `"path"` values in a Steam `libraryfolders.vdf` (regex `"path"\s+"([^"]+)"`).
pub fn steam_library_paths(vdf: &str) -> Vec<String> {
    let mut out = vec![];
    let mut rest = vdf;
    while let Some(k) = rest.find("\"path\"") {
        let after = &rest[k + 6..];
        let trimmed = after.trim_start_matches(|c: char| c.is_whitespace());
        let ws_len = after.len() - trimmed.len();
        if ws_len > 0 {
            if let Some(v) = trimmed.strip_prefix('"') {
                if let Some(end) = v.find('"') {
                    if end > 0 {
                        out.push(v[..end].to_string());
                        rest = &v[end + 1..];
                        continue;
                    }
                }
            }
        }
        rest = &rest[k + 1..];
    }
    out
}

/// `LSApplicationCategoryType` from an app's Info.plist (XML natively; binary plists via [`platform`]).
pub fn app_category(app: &Path) -> Option<String> {
    let plist = app.join("Contents/Info.plist");
    let bytes = fs::read(&plist).ok()?;
    if bytes.starts_with(b"bplist") {
        return platform::plist_string(&plist, "LSApplicationCategoryType");
    }
    let text = String::from_utf8_lossy(&bytes);
    xml_plist_string(&text, "LSApplicationCategoryType")
}

/// The `<string>` after `<key>KEY</key>` in an XML plist.
pub fn xml_plist_string(xml: &str, key: &str) -> Option<String> {
    let k = xml.find(&format!("<key>{key}</key>"))?;
    let rest = xml[k + key.len() + 11..].trim_start();
    let v = rest.strip_prefix("<string>")?;
    let end = v.find("</string>")?;
    Some(v[..end].replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&"))
}

fn source_rank(s: &str) -> String {
    match s {
        "Applications" => "0".into(),
        "Steam" => "1".into(),
        other => format!("2{other}"),
    }
}

/// `GameFinder.search(gamesRoot:)` with its inputs explicit: `app_dirs` is normally `/Applications` and
/// `~/Applications` (their non-app subfolders are searched too) and Steam is looked for under `home`. Results already
/// in or linked from `games_root` are dropped. Sorted by source (Applications, Steam), then title.
pub fn search(games_root: &Path, home: &Path, app_dirs: &[PathBuf]) -> Vec<FoundGame> {
    let known = already_in_games_folder(games_root);
    let root_real = real_path(games_root);
    let mut out: Vec<FoundGame> = vec![];
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut add = |path: &Path, source: &str, title: Option<String>| {
        let key = real_path(path);
        if known.contains(&key) || !seen.insert(key.clone()) || key.starts_with(&root_real) && key != root_real {
            return;
        }
        out.push(FoundGame {
            path: path.to_path_buf(),
            title: title.unwrap_or_else(|| clean_name(path)),
            platform: GamePlatform::Mac,
            source: source.into(),
        });
    };
    let sorted_dir = |d: &Path| -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> =
            fs::read_dir(d).map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).collect()).unwrap_or_default();
        v.sort();
        v
    };

    // 1. Mac apps that declare themselves games (LSApplicationCategoryType …games)
    let mut dirs: Vec<PathBuf> = app_dirs.to_vec();
    for d in app_dirs {
        for sub in sorted_dir(d) {
            let n = file_name(&sub);
            if !n.ends_with(".app") && !n.starts_with('.') && sub.is_dir() {
                dirs.push(sub);
            }
        }
    }
    for d in &dirs {
        for app in sorted_dir(d) {
            let n = file_name(&app);
            if !n.ends_with(".app") || n[..n.len() - 4].to_lowercase() == "desktopglass" {
                continue;
            }
            if app_category(&app).unwrap_or_default().to_lowercase().contains("games") {
                add(&app, "Applications", None);
            }
        }
    }

    // 2. Steam libraries (default + any listed in libraryfolders.vdf)
    let steam = home.join("Library/Application Support/Steam");
    let mut libraries = vec![steam.clone()];
    if let Ok(vdf) = fs::read_to_string(steam.join("steamapps/libraryfolders.vdf")) {
        libraries.extend(steam_library_paths(&vdf).into_iter().map(PathBuf::from));
    }
    for lib in libraries {
        for game in sorted_dir(&lib.join("steamapps/common")) {
            if let Some(app) = sorted_dir(&game).into_iter().find(|p| extension(p) == "app") {
                add(&app, "Steam", Some(file_name(&game)));
            }
        }
    }

    out.sort_by(|a, b| {
        if a.source != b.source {
            source_rank(&a.source).cmp(&source_rank(&b.source))
        } else {
            localized_standard_compare(&a.title, &b.title)
        }
    });
    out
}

/// `search` with the real locations: `/Applications`, `~/Applications` and Steam under `$HOME`.
pub fn search_this_mac(games_root: &Path) -> Vec<FoundGame> {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    search(games_root, &home, &[PathBuf::from("/Applications"), home.join("Applications")])
}

/// `search_this_mac` as JSON: `{"games": [...], "status": "..."}` with the Swift status line.
pub fn search_this_mac_json(games_root: &Path) -> Value {
    let found = search_this_mac(games_root);
    json!({"games": found.iter().map(FoundGame::to_json).collect::<Vec<_>>(), "status": search_status(found.len())})
}

/// The status line `GameFinder.scan` shows after a search.
pub fn search_status(count: usize) -> String {
    if count == 0 {
        "No new games found — everything is already in the Games folder.".into()
    } else {
        format!("Found {count} game{} not in the Games folder yet.", if count == 1 { "" } else { "s" })
    }
}

/// The part of `GameFinder.addShortcuts(to:)` that writes files: a Finder alias in `root` for each game, named after
/// its title ("Title", "Title 2", …). Returns the Swift summary line and the shortcuts written.
pub fn add_shortcuts(games: &[FoundGame], root: &Path) -> (String, Vec<PathBuf>) {
    let mut added = vec![];
    let mut failed: Vec<String> = vec![];
    for game in games {
        let mut dest = root.join(&game.title);
        let mut n = 2;
        while fs::symlink_metadata(&dest).is_ok() {
            dest = root.join(format!("{} {n}", game.title));
            n += 1;
        }
        match platform::write_alias(&game.path, &dest) {
            Ok(()) => added.push(dest),
            Err(_) => failed.push(game.title.clone()),
        }
    }
    let count = added.len();
    let summary = format!("Added {count} shortcut{} to {}", if count == 1 { "" } else { "s" }, file_name(root))
        + &if failed.is_empty() { String::new() } else { format!(" · couldn't add: {}", failed.join(", ")) };
    (summary, added)
}

/// OS-specific pieces. On macOS: Finder aliases through CoreFoundation's bookmark API (linked as a system framework,
/// no crate), the `UF_HIDDEN` flag, and `plutil` for binary Info.plists. Elsewhere Finder aliases don't exist: they're
/// never detected, and `write_alias` makes a symlink instead.
pub mod platform {
    use std::path::{Path, PathBuf};

    #[cfg(target_os = "macos")]
    mod cf {
        #![allow(non_upper_case_globals, non_snake_case)]
        use std::ffi::c_void;
        pub type CFTypeRef = *const c_void;
        pub type CFURLRef = *const c_void;
        pub type CFDataRef = *const c_void;
        pub type CFStringRef = *const c_void;
        pub type CFIndex = isize;
        pub type CFOptionFlags = usize;
        pub type Boolean = u8;

        pub const kCFURLBookmarkResolutionWithoutUIMask: CFOptionFlags = 1 << 8;
        pub const kCFURLBookmarkResolutionWithoutMountingMask: CFOptionFlags = 1 << 9;
        pub const kCFURLBookmarkCreationSuitableForBookmarkFile: CFOptionFlags = 1 << 10;

        #[link(name = "CoreFoundation", kind = "framework")]
        extern "C" {
            pub static kCFURLIsAliasFileKey: CFStringRef;
            pub static kCFBooleanTrue: CFTypeRef;
            pub fn CFURLCreateFromFileSystemRepresentation(
                alloc: CFTypeRef,
                buffer: *const u8,
                len: CFIndex,
                is_dir: Boolean,
            ) -> CFURLRef;
            pub fn CFURLCopyResourcePropertyForKey(
                url: CFURLRef,
                key: CFStringRef,
                value: *mut CFTypeRef,
                error: *mut CFTypeRef,
            ) -> Boolean;
            pub fn CFURLCreateBookmarkDataFromFile(alloc: CFTypeRef, url: CFURLRef, error: *mut CFTypeRef) -> CFDataRef;
            pub fn CFURLCreateByResolvingBookmarkData(
                alloc: CFTypeRef,
                bookmark: CFDataRef,
                options: CFOptionFlags,
                relative: CFURLRef,
                props: CFTypeRef,
                is_stale: *mut Boolean,
                error: *mut CFTypeRef,
            ) -> CFURLRef;
            pub fn CFURLGetFileSystemRepresentation(
                url: CFURLRef,
                resolve_against_base: Boolean,
                buffer: *mut u8,
                max_len: CFIndex,
            ) -> Boolean;
            pub fn CFURLCreateBookmarkData(
                alloc: CFTypeRef,
                url: CFURLRef,
                options: CFOptionFlags,
                props: CFTypeRef,
                relative: CFURLRef,
                error: *mut CFTypeRef,
            ) -> CFDataRef;
            pub fn CFURLWriteBookmarkDataToFile(
                bookmark: CFDataRef,
                url: CFURLRef,
                options: CFOptionFlags,
                error: *mut CFTypeRef,
            ) -> Boolean;
            pub fn CFRelease(cf: CFTypeRef);
        }

        /// An owned CF object, released on drop.
        pub struct Owned(pub CFTypeRef);
        impl Drop for Owned {
            fn drop(&mut self) {
                if !self.0.is_null() {
                    unsafe { CFRelease(self.0) }
                }
            }
        }

        pub fn url(p: &std::path::Path, is_dir: bool) -> Option<Owned> {
            use std::os::unix::ffi::OsStrExt;
            let b = p.as_os_str().as_bytes();
            let u = unsafe { CFURLCreateFromFileSystemRepresentation(std::ptr::null(), b.as_ptr(), b.len() as CFIndex, is_dir as u8) };
            if u.is_null() {
                None
            } else {
                Some(Owned(u))
            }
        }

        pub fn path_of(u: CFURLRef) -> Option<std::path::PathBuf> {
            use std::os::unix::ffi::OsStrExt;
            let mut buf = vec![0u8; 4096];
            if unsafe { CFURLGetFileSystemRepresentation(u, 1, buf.as_mut_ptr(), buf.len() as CFIndex) } == 0 {
                return None;
            }
            let n = buf.iter().position(|&c| c == 0)?;
            Some(std::path::PathBuf::from(std::ffi::OsStr::from_bytes(&buf[..n])))
        }
    }

    /// `URLResourceValues.isAliasFile` for a regular file (a Finder alias).
    pub fn is_alias_file(p: &Path) -> bool {
        #[cfg(target_os = "macos")]
        {
            let Some(u) = cf::url(p, false) else { return false };
            let mut value: cf::CFTypeRef = std::ptr::null();
            let ok = unsafe { cf::CFURLCopyResourcePropertyForKey(u.0, cf::kCFURLIsAliasFileKey, &mut value, std::ptr::null_mut()) };
            let v = cf::Owned(value);
            ok != 0 && !v.0.is_null() && v.0 == unsafe { cf::kCFBooleanTrue }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = p;
            false
        }
    }

    /// `URL(resolvingAliasFileAt:options: [.withoutUI, .withoutMounting])` for a Finder alias file.
    pub fn resolve_alias(p: &Path) -> Option<PathBuf> {
        #[cfg(target_os = "macos")]
        {
            let u = cf::url(p, false)?;
            let data = cf::Owned(unsafe { cf::CFURLCreateBookmarkDataFromFile(std::ptr::null(), u.0, std::ptr::null_mut()) });
            if data.0.is_null() {
                return None;
            }
            let opts = cf::kCFURLBookmarkResolutionWithoutUIMask | cf::kCFURLBookmarkResolutionWithoutMountingMask;
            let mut stale = 0u8;
            let r = cf::Owned(unsafe {
                cf::CFURLCreateByResolvingBookmarkData(
                    std::ptr::null(),
                    data.0,
                    opts,
                    std::ptr::null(),
                    std::ptr::null(),
                    &mut stale,
                    std::ptr::null_mut(),
                )
            });
            if r.0.is_null() {
                return None;
            }
            cf::path_of(r.0)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = p;
            None
        }
    }

    /// `bookmarkData(options: .suitableForBookmarkFile)` + `URL.writeBookmarkData(_:to:)`: a Finder alias at `dest`
    /// pointing to `target` (a symlink on other systems).
    pub fn write_alias(target: &Path, dest: &Path) -> std::io::Result<()> {
        #[cfg(target_os = "macos")]
        {
            let fail = || std::io::Error::new(std::io::ErrorKind::Other, "couldn't create a Finder alias");
            let t = cf::url(target, target.is_dir()).ok_or_else(fail)?;
            let data = cf::Owned(unsafe {
                cf::CFURLCreateBookmarkData(
                    std::ptr::null(),
                    t.0,
                    cf::kCFURLBookmarkCreationSuitableForBookmarkFile,
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null_mut(),
                )
            });
            if data.0.is_null() {
                return Err(fail());
            }
            let d = cf::url(dest, false).ok_or_else(fail)?;
            if unsafe { cf::CFURLWriteBookmarkDataToFile(data.0, d.0, 0, std::ptr::null_mut()) } == 0 {
                return Err(fail());
            }
            Ok(())
        }
        #[cfg(not(target_os = "macos"))]
        {
            #[cfg(unix)]
            return std::os::unix::fs::symlink(target, dest);
            #[cfg(windows)]
            return if target.is_dir() {
                std::os::windows::fs::symlink_dir(target, dest)
            } else {
                std::os::windows::fs::symlink_file(target, dest)
            };
        }
    }

    /// The `UF_HIDDEN` flag (what `.skipsHiddenFiles` honours besides dot-names).
    pub fn is_hidden(p: &Path) -> bool {
        #[cfg(target_os = "macos")]
        {
            use std::os::macos::fs::MetadataExt;
            std::fs::symlink_metadata(p).map(|m| m.st_flags() & 0x8000 != 0).unwrap_or(false)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = p;
            false
        }
    }

    /// A string value from a (binary) plist, via `plutil -extract KEY raw`.
    pub fn plist_string(plist: &Path, key: &str) -> Option<String> {
        #[cfg(target_os = "macos")]
        {
            let out = std::process::Command::new("/usr/bin/plutil")
                .args(["-extract", key, "raw", "-o", "-"])
                .arg(plist)
                .stderr(std::process::Stdio::null())
                .output()
                .ok()?;
            if !out.status.success() {
                return None;
            }
            Some(String::from_utf8_lossy(&out.stdout).trim_end_matches('\n').to_string())
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (plist, key);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    //! Port of Tests/DesktopGlassTests/GameLibraryTests.swift, plus GameFinder cases (no Swift tests exist for it).
    use super::*;

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static N: AtomicUsize = AtomicUsize::new(0);
            let p = std::env::temp_dir().join(format!(
                "dg-games-{tag}-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::SeqCst)
            ));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            TempDir(fs::canonicalize(&p).unwrap())
        }
        fn dir(&self, rel: &str) -> PathBuf {
            let p = self.0.join(rel);
            fs::create_dir_all(&p).unwrap();
            p
        }
        fn file(&self, rel: &str, content: &str) -> PathBuf {
            let p = self.0.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, content).unwrap();
            p
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn clean_title_cases() {
        let title = |n: &str| clean_title(&Path::new("/g").join(n));
        assert_eq!(title("Super Game [v1.2].app"), "Super Game");
        assert_eq!(title("my_cool_game.app"), "my cool game");
        assert_eq!(title("[only tags].app"), "[only tags].app");
    }

    #[test]
    fn find_games_lists_only_mac_apps() {
        let dir = TempDir::new("lib");
        dir.dir("Mac Game.app/Contents");
        dir.dir("Indies/Other Game [v2].app/Contents");
        dir.dir("a/b/c/d/e/Too Deep.app/Contents"); // deeper than the scan goes
        dir.file("Zelda.nsp", ""); // console files are no longer games
        dir.file("Setup.exe", "");
        dir.file("disc.iso", "");
        dir.file("Bloodborne/eboot.bin", "");
        dir.file("Bloodborne/sce_sys/param.sfo", "");
        dir.file("description.md", "# Games");
        let outside = TempDir::new("outside");
        let real = outside.dir("Linked.app/Contents").parent().unwrap().to_path_buf();
        std::os::unix::fs::symlink(&real, dir.0.join("Shortcut.app")).unwrap();
        let rom = outside.file("Metroid.gba", "");
        std::os::unix::fs::symlink(&rom, dir.0.join("Metroid.gba")).unwrap();

        let found = find_games(&dir.0);
        let titles: HashSet<String> = found.iter().map(|g| g.title.clone()).collect();
        assert_eq!(titles, ["Mac Game", "Other Game", "Shortcut"].iter().map(|s| s.to_string()).collect());
        assert!(found.iter().all(|g| g.platform == GamePlatform::Mac));
        let link = found.iter().find(|g| g.title == "Shortcut").unwrap();
        assert_eq!(real_path(&link.target), real_path(&real), "shortcuts launch the real app");
        assert_eq!(link.detected_by, "shortcut");
    }

    #[test]
    fn only_mac_platform_remains() {
        assert_eq!(GamePlatform::ALL, &[GamePlatform::Mac]);
    }

    // Extra (not in Swift): Finder aliases written like "Find Games" does are found and resolved.
    #[cfg(target_os = "macos")]
    #[test]
    fn finder_aliases_are_shortcuts() {
        let lib = TempDir::new("alias-lib");
        let outside = TempDir::new("alias-out");
        let app = outside.dir("Aliased Game.app");
        outside.dir("Aliased Game.app/Contents");
        let game = FoundGame { path: app.clone(), title: "Aliased Game".into(), platform: GamePlatform::Mac, source: "Steam".into() };
        let (summary, written) = add_shortcuts(&[game.clone(), game], &lib.0);
        assert_eq!(summary, format!("Added 2 shortcuts to {}", file_name(&lib.0)));
        assert_eq!(written.iter().map(|p| file_name(p)).collect::<Vec<_>>(), ["Aliased Game", "Aliased Game 2"]);
        assert!(platform::is_alias_file(&written[0]));
        assert!(!platform::is_alias_file(&outside.file("plain.txt", "x")));
        let found = find_games(&lib.0);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].detected_by, "shortcut");
        assert_eq!(real_path(&found[0].target), real_path(&app));
        assert!(already_in_games_folder(&lib.0).contains(&real_path(&app)));
    }

    #[test]
    fn steam_libraryfolders_vdf() {
        let vdf = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"/Users/x/Library/Application Support/Steam\"\n\t\t\"label\"\t\t\"\"\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"/Volumes/Games/SteamLibrary\"\n\t}\n}\n";
        assert_eq!(steam_library_paths(vdf), ["/Users/x/Library/Application Support/Steam", "/Volumes/Games/SteamLibrary"]);
        assert!(steam_library_paths("\"path\"\"x\"").is_empty(), "needs whitespace between key and value");
    }

    #[test]
    fn search_finds_game_apps_and_steam_games_not_already_in_the_library() {
        let home = TempDir::new("home");
        let games = TempDir::new("games");
        let apps = home.dir("Applications");
        let plist = |category: &str| {
            format!("<?xml version=\"1.0\"?>\n<plist version=\"1.0\"><dict>\n\t<key>LSApplicationCategoryType</key>\n\t<string>{category}</string>\n</dict></plist>")
        };
        home.file("Applications/Chess [v2].app/Contents/Info.plist", &plist("public.app-category.board-games"));
        home.file("Applications/Utilities/Puzzle.app/Contents/Info.plist", &plist("public.app-category.puzzle-games"));
        home.file("Applications/Notes.app/Contents/Info.plist", &plist("public.app-category.productivity"));
        home.file("Applications/DesktopGlass.app/Contents/Info.plist", &plist("public.app-category.games"));
        home.file("Applications/Linked.app/Contents/Info.plist", &plist("public.app-category.games"));
        std::os::unix::fs::symlink(home.0.join("Applications/Linked.app"), games.0.join("Linked.app")).unwrap();
        // Steam: default library + one from libraryfolders.vdf
        let extra = TempDir::new("steamlib");
        home.file(
            "Library/Application Support/Steam/steamapps/libraryfolders.vdf",
            &format!("\"libraryfolders\" {{ \"1\" {{ \"path\"  \"{}\" }} }}", extra.0.display()),
        );
        home.dir("Library/Application Support/Steam/steamapps/common/Hades/Hades.app");
        extra.dir("steamapps/common/Celeste/Celeste.app");
        extra.dir("steamapps/common/WindowsOnly/bin");

        let found = search(&games.0, &home.0, &[apps]);
        let got: Vec<(String, String)> = found.iter().map(|g| (g.source.clone(), g.title.clone())).collect();
        assert_eq!(
            got,
            [("Applications", "Chess"), ("Applications", "Puzzle"), ("Steam", "Celeste"), ("Steam", "Hades")]
                .iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect::<Vec<_>>()
        );
        assert_eq!(search_status(0), "No new games found — everything is already in the Games folder.");
        assert_eq!(search_status(1), "Found 1 game not in the Games folder yet.");
    }

    #[test]
    fn natural_title_order() {
        let mut v = vec!["game 10", "Game 2", "alpha", "Zeta"];
        v.sort_by(|a, b| localized_standard_compare(a, b));
        assert_eq!(v, ["alpha", "Game 2", "game 10", "Zeta"]);
    }
}
