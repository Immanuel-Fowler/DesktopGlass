//! Port of Tests/python/test_agent_guides.py: the instructions block is added once, updated in place, removed
//! cleanly, and never touches the rest of the user's CLAUDE.md / AGENTS.md / GEMINI.md.

mod common;
use common::Home;
use dg_install::clients::{clients, guide_for};
use dg_install::guide::{guide_block, with_guide, without_guide, Guide, BEGIN, END, GUIDE};

#[test]
fn added_to_empty_and_existing_files() {
    assert_eq!(with_guide(""), format!("{GUIDE}\n"));
    let mine = "# My rules\n\nAlways use tabs.\n";
    let out = with_guide(mine);
    assert!(out.starts_with(&format!("{}\n\n{BEGIN}", mine.trim_end_matches('\n'))));
    assert!(out.ends_with(&format!("{END}\n")));
}

#[test]
fn adding_twice_changes_nothing() {
    let once = with_guide("# Mine\n");
    assert_eq!(with_guide(&once), once);
}

#[test]
fn an_old_block_is_replaced_in_place() {
    let old = format!("# Top\n\n{BEGIN}\nold advice\n{END}\n\n# Bottom\nkeep me\n");
    let out = with_guide(&old);
    assert!(!out.contains("old advice"));
    assert!(out.starts_with(&format!("# Top\n\n{BEGIN}")));
    assert!(out.ends_with(&format!("{END}\n\n# Bottom\nkeep me\n")));
}

#[test]
fn removal_keeps_everything_else() {
    let mine = "# My rules\n\nAlways use tabs.\n";
    assert_eq!(without_guide(&with_guide(mine)), mine);
    let middle = format!("# Top\n\n{GUIDE}\n\n# Bottom\n");
    assert_eq!(without_guide(&middle), "# Top\n\n# Bottom\n");
    assert_eq!(without_guide("no block here\n"), "no block here\n");
}

#[test]
fn an_unterminated_marker_is_left_alone() {
    let broken = format!("# Mine\n{BEGIN}\nhalf a block\n");
    assert_eq!(guide_block(&broken), None);
    assert_eq!(without_guide(&broken), broken);
}

#[test]
fn the_global_guide_never_asks_to_open_the_viewer() {
    // Showing output in DesktopGlass is the session-start hook's job, so it only happens in DesktopGlass sessions.
    assert!(!GUIDE.contains("show_artifact"));
    assert!(!GUIDE.contains("dgview"));
    assert!(!GUIDE.contains(" view "));
    assert!(GUIDE.contains("search_desktop"));
}

#[test]
fn the_guide_text_is_the_python_installers() {
    assert_eq!(
        GUIDE,
        "<!-- desktopglass:start -->\n## DesktopGlass\n\nThis Mac runs DesktopGlass, which keeps an organized index of \
         ~/Desktop available through the `desktopglass` MCP\nserver. To find something on the Desktop, use \
         `desktop_overview` and `search_desktop` before listing folders.\n<!-- desktopglass:end -->"
    );
}

#[test]
fn install_and_uninstall_a_new_file() {
    let h = Home::new();
    let path = h.root.join(".codex/AGENTS.md");
    let g = Guide::new(path.clone());
    assert!(!g.installed());
    g.install().unwrap();
    assert!(g.installed());
    g.uninstall().unwrap();
    assert!(!path.exists(), "a file that only held our block is removed");
}

#[test]
fn uninstall_keeps_the_users_file() {
    let h = Home::new();
    let path = h.file("CLAUDE.md", "# Mine\n");
    let g = Guide::new(path.clone());
    g.install().unwrap();
    g.uninstall().unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "# Mine\n");
}

#[test]
fn every_agent_cli_has_an_instructions_file() {
    let h = Home::new();
    let ctx = h.ctx();
    let ids: Vec<&str> = clients(&ctx).iter().map(|c| c.id).collect();
    for id in ["claude-code", "codex", "gemini", "copilot", "opencode"] {
        assert!(ids.contains(&id), "{id}");
        assert!(guide_for(&ctx, id).is_some(), "{id}");
    }
}
