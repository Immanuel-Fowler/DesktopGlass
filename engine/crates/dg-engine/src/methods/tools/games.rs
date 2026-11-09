//! Games (dg-core `games`, ported from GameLibrary/GameFinder/GameCatalog.swift).
//!
//! UI link methods:
//! - `games/scan {root}` → `[{path, target, title, platform, platform_id, detected_by, modified}]` (the library)
//! - `games/search {root}` → `{games: [{path, title, platform, platform_id, source}], status}` (Find Games)
//! - `games/add_shortcuts {root, games: [{path, title}]}` → `{summary, added: [path], failed: [title]}`
//! - `games/history {lastPlayed: {path: seconds}, sort: "recent"|"name"}` → `{}`: play history and the library's
//!   sort order stay UI state (UserDefaults, written when a game launches); the UI pushes them here at start-up and
//!   after each change so `games.list` can answer without a round trip.
//!
//! Control method answered here: `games.list {query?, platform?, limit?}` (same result as AppControl's). The Games
//! folder is the `gamesFolder` setting, else a top-level Desktop folder named Games / Gaming / Video Games
//! (`DesktopStore.isGamesFolder`). `games.play` stays in the UI (it launches an app).

use super::{required, str_param};
use crate::Engine;
use dg_core::games::{self as g, FoundGame, GamePlatform};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct History {
    last_played: HashMap<String, f64>,
    recent_first: bool,
}

static HISTORY: Mutex<Option<History>> = Mutex::new(None);

pub fn handle(engine: &Arc<Engine>, method: &str, p: &Value) -> Option<Result<Value, String>> {
    Some(match method {
        "games/scan" => required(p, "root").map(|r| g::find_games_json(Path::new(r))),
        "games/search" => required(p, "root").map(|r| g::search_this_mac_json(Path::new(r))),
        "games/add_shortcuts" => add_shortcuts(p),
        "games/history" => {
            let mut h = History::default();
            for (k, v) in p.get("lastPlayed").and_then(|v| v.as_object()).into_iter().flatten() {
                if let Some(t) = v.as_f64() {
                    h.last_played.insert(k.clone(), t);
                }
            }
            h.recent_first = str_param(p, "sort") == Some("recent");
            *HISTORY.lock().unwrap() = Some(h);
            Ok(json!({}))
        }
        "games.list" => list(engine, p, &dg_platform::desktop()),
        _ => return None,
    })
}

fn add_shortcuts(p: &Value) -> Result<Value, String> {
    let root = PathBuf::from(required(p, "root")?);
    let games: Vec<FoundGame> = p
        .get("games")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|v| {
            Some(FoundGame {
                path: PathBuf::from(v.get("path")?.as_str()?),
                title: v.get("title")?.as_str()?.to_string(),
                platform: GamePlatform::Mac,
                source: v.get("source").and_then(|s| s.as_str()).unwrap_or("").to_string(),
            })
        })
        .collect();
    // One at a time, so the UI knows which ones failed (it keeps those in its list).
    let mut added = vec![];
    let mut failed = vec![];
    for game in &games {
        let (_, written) = g::add_shortcuts(std::slice::from_ref(game), &root);
        if written.is_empty() {
            failed.push(game.title.clone());
        }
        added.extend(written);
    }
    let n = added.len();
    let folder = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let summary = format!("Added {n} shortcut{} to {folder}", if n == 1 { "" } else { "s" })
        + &if failed.is_empty() { String::new() } else { format!(" · couldn't add: {}", failed.join(", ")) };
    Ok(json!({"summary": summary, "added": added, "failed": failed}))
}

/// `DesktopStore.isGamesFolder` over the Desktop's top-level folders.
fn games_folder(engine: &Arc<Engine>, desktop: &Path) -> Option<PathBuf> {
    let chosen = engine.settings.string("gamesFolder").unwrap_or_default();
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(desktop)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            p.is_dir() && !name.starts_with('.') && !g::PACKAGE_EXTENSIONS.contains(&ext(p).as_str())
        })
        .collect();
    dirs.sort_by(|a, b| g::localized_standard_compare(&name(a), &name(b)));
    dirs.into_iter().find(|d| {
        if !chosen.is_empty() {
            name(d) == chosen
        } else {
            ["games", "gaming", "video games"].contains(&name(d).to_lowercase().as_str())
        }
    })
}

fn name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn ext(p: &Path) -> String {
    p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

fn list(engine: &Arc<Engine>, p: &Value, desktop: &Path) -> Result<Value, String> {
    let root = games_folder(engine, desktop).ok_or(
        "There's no Games folder on the Desktop (a folder named Games, or one chosen with “Use as Games Folder”)",
    )?;
    let q = str_param(p, "query").unwrap_or("").to_lowercase();
    let platform = str_param(p, "platform").unwrap_or("").to_lowercase();
    let limit = p.get("limit").and_then(|l| l.as_u64()).unwrap_or(200) as usize;
    let guard = HISTORY.lock().unwrap();
    let empty = History::default();
    let history = guard.as_ref().unwrap_or(&empty);
    let played = |path: &Path| history.last_played.get(&*path.to_string_lossy()).copied();

    let mut games = g::find_games(&root);
    if history.recent_first {
        // GameLibrary.applySort(.recent): most recently played first, then by title.
        games.sort_by(|a, b| {
            let (x, y) = (played(&a.path).unwrap_or(f64::MIN), played(&b.path).unwrap_or(f64::MIN));
            y.partial_cmp(&x).unwrap_or(std::cmp::Ordering::Equal).then_with(|| g::localized_standard_compare(&a.title, &b.title))
        });
    }
    let out: Vec<Value> = games
        .iter()
        .filter(|e| {
            (q.is_empty() || e.title.to_lowercase().contains(&q))
                && (platform.is_empty()
                    || e.platform.id() == platform
                    || e.platform.name().to_lowercase().contains(&platform))
        })
        .take(limit)
        .map(|e| {
            let mut o = json!({"title": e.title, "platform": e.platform.name(), "platform_id": e.platform.id(),
                               "path": e.path.to_string_lossy()});
            if let Some(t) = played(&e.path) {
                o["last_played"] = json!(iso8601(t));
            }
            o
        })
        .collect();
    Ok(Value::Array(out))
}

/// `ISO8601DateFormatter().string(from:)`: `2026-11-06T16:00:00Z`.
fn iso8601(secs: f64) -> String {
    let s = secs.floor() as i64;
    let (days, rem) = (s.div_euclid(86_400), s.rem_euclid(86_400));
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{engine, temp_dir};
    use super::*;

    fn app(dir: &Path, name: &str) -> PathBuf {
        let a = dir.join(name);
        std::fs::create_dir_all(a.join("Contents")).unwrap();
        a
    }

    #[test]
    fn iso_dates() {
        assert_eq!(iso8601(0.0), "1970-01-01T00:00:00Z");
        assert_eq!(iso8601(1_793_980_800.0), "2026-11-06T16:00:00Z");
        assert_eq!(iso8601(951_782_400.5), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn games_list_answers_from_the_engine() {
        let dir = temp_dir("games");
        let e = engine(&dir);
        let desktop = dir.join("Desktop");
        let err = list(&e, &json!({}), &desktop).unwrap_err();
        assert!(err.starts_with("There's no Games folder"));

        let games = desktop.join("Games");
        std::fs::create_dir_all(&games).unwrap();
        app(&games, "Zeta.app");
        app(&games.join("Indie"), "Alpha [v1.2].app");
        std::fs::create_dir_all(games.join("notes")).unwrap();

        let r = list(&e, &json!({}), &desktop).unwrap();
        let titles: Vec<&str> = r.as_array().unwrap().iter().map(|g| g["title"].as_str().unwrap()).collect();
        assert_eq!(titles, ["Alpha", "Zeta"]);
        assert_eq!(r[0]["platform"], "Mac");
        assert_eq!(r[0]["platform_id"], "mac");
        assert!(r[0].get("last_played").is_none());

        // Play history pushed by the UI: recent-first order and last_played.
        let zeta = r[1]["path"].as_str().unwrap().to_string();
        handle(&e, "games/history", &json!({"lastPlayed": {zeta.clone(): 1_793_980_800.0}, "sort": "recent"}))
            .unwrap()
            .unwrap();
        let r = list(&e, &json!({}), &desktop).unwrap();
        assert_eq!(r[0]["title"], "Zeta");
        assert_eq!(r[0]["last_played"], "2026-11-06T16:00:00Z");
        let r = list(&e, &json!({"query": "alp", "platform": "mac", "limit": 5}), &desktop).unwrap();
        assert_eq!(r.as_array().unwrap().len(), 1);
        let r = list(&e, &json!({"platform": "switch"}), &desktop).unwrap();
        assert_eq!(r, json!([]));
        handle(&e, "games/history", &json!({})).unwrap().unwrap();

        // A chosen Games folder wins over the default names.
        std::fs::create_dir_all(desktop.join("Play")).unwrap();
        e.settings.set(json!({"gamesFolder": "Play"}).as_object().unwrap()).unwrap();
        assert_eq!(list(&e, &json!({}), &desktop).unwrap(), json!([]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_and_shortcuts() {
        let dir = temp_dir("games-sc");
        let e = engine(&dir);
        let games = dir.join("Games");
        std::fs::create_dir_all(&games).unwrap();
        let elsewhere = app(&dir.join("Apps"), "Puzzle.app");
        let r = handle(&e, "games/add_shortcuts", &json!({"root": games,
            "games": [{"path": elsewhere, "title": "Puzzle"}, {"path": dir.join("missing.app"), "title": "Gone"}]}))
            .unwrap()
            .unwrap();
        // Writing a shortcut to a missing app fails on macOS (no bookmark); elsewhere it's a symlink.
        if cfg!(target_os = "macos") {
            assert_eq!(r["summary"], "Added 1 shortcut to Games · couldn't add: Gone");
            assert_eq!(r["failed"], json!(["Gone"]));
        }
        let scanned = handle(&e, "games/scan", &json!({"root": games})).unwrap().unwrap();
        let puzzle = scanned.as_array().unwrap().iter().find(|g| g["title"] == "Puzzle").expect("shortcut scanned");
        assert_eq!(puzzle["detected_by"], "shortcut");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
