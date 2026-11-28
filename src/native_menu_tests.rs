//! The macOS menu bar's menus ([`crate::menus::native_tree`]): the in-window bar's commands, and the
//! keys Mac apps use.

use crate::app::App;
use crate::menus::{Act, Row, TOP_MENUS, native_tree};

fn app() -> App {
    let ctx = egui::Context::default();
    let session = anicraft_engine::Session::new(anicraft_engine::doc::sample::demo());
    App::with_context(&ctx, session, Some(std::path::PathBuf::from("unused.png")))
}

fn commands(rows: &[Row], out: &mut Vec<&'static str>) {
    for r in rows {
        match r {
            Row::Item { act: Act::Cmd(id), .. } => out.push(id),
            Row::Sub(_, sub) => commands(sub, out),
            _ => {}
        }
    }
}

/// A command the in-window bar lists under a menu is in the same menu of the macOS bar: the two are
/// built separately, so a new item in one is easily missed in the other.
#[test]
fn every_menu_command_is_in_the_macos_menu_bar() {
    let app = app();
    let tree = native_tree(&app);
    for spec in anicraft_engine::commands() {
        let top = spec.menu.split('/').next().unwrap_or("");
        let Some(i) = TOP_MENUS.iter().position(|t| *t == top) else { continue };
        let mut ids = Vec::new();
        commands(&tree[i].2, &mut ids);
        assert!(ids.contains(&spec.id), "{} ({}) is not in the macOS {top} menu", spec.id, spec.menu);
    }
}

/// On a Mac, Redo is ⇧⌘Z as in every Mac app, and the system's Hide and Minimize keys can't be
/// given to a command (AppKit would take them first).
#[cfg(target_os = "macos")]
#[test]
fn redo_is_shift_cmd_z_and_the_system_keys_are_kept() {
    let app = app();
    assert_eq!(app.keymap.owner("Ctrl+Shift+Z").map(|a| a.id), Some("edit.redo"));
    assert!(app.keymap.keys("edit.redo").iter().any(|k| k == "Ctrl+Y"), "⌘Y still redoes");
    for k in ["Ctrl+H", "Ctrl+Alt+H", "Ctrl+M"] {
        assert!(crate::keymap::reserved(k).is_some(), "{k}");
        assert!(app.keymap.owner(k).is_none(), "{k} is the system's");
    }
}
