//! Document tabs (ADR 0112): each open document keeps its own undo history, selection,
//! playhead and view; artwork pastes across them with its symbols; closing with unsaved changes
//! asks; autosave covers every open document.

use super::*;
use egui::{Event, Key, Modifiers, RawInput};

fn app() -> (egui::Context, App) {
    let ctx = egui::Context::default();
    let session = Session::new(anicraft_engine::doc::sample::demo());
    let app = App::with_context(&ctx, session, Some(PathBuf::from("unused.png")));
    (ctx, app)
}

fn input(modifiers: Modifiers, events: Vec<Event>) -> RawInput {
    let events = std::iter::once(Event::ModifiersChanged(modifiers)).chain(events).collect();
    RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1600.0, 940.0))), events, ..Default::default() }
}

fn frame(ctx: &egui::Context, app: &mut App, input: RawInput) -> egui::FullOutput {
    let mut out = ctx.run_ui(input, |ui| app.frame(ui));
    out.textures_delta.clear();
    out
}

fn settle(ctx: &egui::Context, app: &mut App) {
    for _ in 0..3 {
        frame(ctx, app, input(Modifiers::NONE, vec![]));
    }
}

fn key(k: Key, m: Modifiers) -> RawInput {
    input(m, vec![Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: m }])
}

/// Ctrl as egui-winit reports it on Windows and Linux (Ctrl is the command key there).
fn ctrl() -> Modifiers {
    Modifiers { ctrl: true, command: true, ..Modifiers::NONE }
}

fn names(app: &App) -> Vec<String> {
    infos(app).into_iter().map(|t| t.name).collect()
}

/// The sample plus two new documents; the third is shown.
fn three(app: &mut App) {
    app.tabs.keep_first();
    app.perform("file.new", json!({"width": 640, "height": 480}));
    app.perform("file.new", json!({"width": 800, "height": 600}));
    assert_eq!(app.tabs.len(), 3);
}

fn click(ctx: &egui::Context, app: &mut App, at: egui::Pos2, button: egui::PointerButton) {
    frame(ctx, app, input(Modifiers::NONE, vec![Event::PointerMoved(at)]));
    for pressed in [true, false] {
        frame(ctx, app, input(Modifiers::NONE, vec![Event::PointerButton { pos: at, button, pressed, modifiers: Modifiers::NONE }]));
    }
}

fn tab_rect(app: &App, i: usize) -> Rect {
    app.tabs.rects[i].1
}

#[test]
fn the_first_document_opened_takes_the_samples_place_and_the_next_gets_a_tab() {
    let (_, mut app) = app();
    assert_eq!(app.tabs.len(), 1);
    app.request("file.new", json!({"width": 640, "height": 480}));
    assert_eq!(app.tabs.len(), 1, "the untouched sample gives way");
    assert_eq!(app.s.doc.stage.width, 640);
    app.request("file.new", json!({"width": 800, "height": 600}));
    assert_eq!((app.tabs.len(), app.tabs.active()), (2, 1), "a document of the user's stays; the new one is shown");
    assert_eq!(names(&app), ["Untitled", "Untitled 2"]);
    // Over unsaved work nothing is asked: the work stays open in its tab.
    app.run("draw.rect", json!({"x": 0, "y": 0, "w": 10, "h": 10}));
    app.request("file.new", json!({}));
    assert!(app.confirm.is_none());
    assert_eq!(app.tabs.len(), 3);
    assert!(infos(&app)[1].dirty);
    // A changed sample is the user's work too.
    let (_, mut app) = self::app();
    app.run("draw.rect", json!({"x": 0, "y": 0, "w": 10, "h": 10}));
    app.request("file.new", json!({}));
    assert_eq!(app.tabs.len(), 2);
}

#[test]
fn switching_keeps_each_documents_undo_selection_playhead_and_view() {
    let (ctx, mut app) = app();
    three(&mut app);
    // The third document: a rectangle, selected; frame 0; zoomed in.
    app.run("draw.rect", json!({"x": 10, "y": 10, "w": 50, "h": 50}));
    settle(&ctx, &mut app);
    app.view.fit = false;
    app.view.zoom = 3.0;
    app.view.pan = vec2(40.0, -20.0);
    app.me_vscroll = 12.0;
    app.run("edit.select_all", json!({}));
    let (third_state, third_steps, third_sel) = (app.s.state, app.s.history_len(), app.s.selection.len());
    assert!(third_steps == 1 && third_sel > 0);
    // The first (the sample): playhead moved, another zoom.
    assert!(activate(&mut app, 0));
    assert_eq!(app.s.doc.library.len(), 2, "the sample's Library shows");
    assert_eq!((app.s.history_len(), app.s.selection.len()), (0, 0), "its own history and selection");
    app.run("timeline.goto", json!({"frame": 7}));
    app.run("edit.select_all", json!({}));
    app.run("element.translate", json!({"dx": 5, "dy": 0}));
    app.view.fit = false;
    app.view.zoom = 0.5;
    let first_sel = app.s.selection.len();
    // Back: everything as it was left, and undo acts on that document only.
    assert!(activate(&mut app, 2));
    assert_eq!((app.s.state, app.s.history_len(), app.s.selection.len(), app.s.frame), (third_state, third_steps, third_sel, 0));
    assert_eq!((app.view.zoom, app.view.pan, app.me_vscroll), (3.0, vec2(40.0, -20.0), 12.0));
    app.invoke("edit.undo");
    assert_eq!(app.s.elements_on(app.s.layer).len(), 0);
    assert!(activate(&mut app, 0));
    assert_eq!((app.s.frame, app.s.selection.len(), app.s.history_len(), app.view.zoom), (7, first_sel, 1, 0.5));
    // The untouched second.
    assert!(activate(&mut app, 1));
    assert_eq!((app.s.doc.stage.width, app.s.history_len(), app.view.fit), (640, 0, true));
}

#[test]
fn history_picks_and_save_as_command_stay_with_their_document() {
    let (_ctx, mut app) = app();
    app.macros.test_folder = Some(std::env::temp_dir().join(format!("anicraft-tabs-picks-{}", std::process::id())));
    three(&mut app);
    // The second document has as long a history as the third, so the same numbers name steps
    // in both.
    assert!(activate(&mut app, 1));
    for x in [10, 80] {
        app.run("draw.rect", json!({"x": x, "y": 10, "w": 50, "h": 50}));
    }
    assert!(activate(&mut app, 2));
    for x in [10, 80] {
        app.run("draw.ellipse", json!({"x": x, "y": 10, "w": 50, "h": 50}));
    }
    crate::macros_ui::pick(&mut app, 0, false, false);
    crate::macros_ui::pick(&mut app, 1, true, false);
    crate::macros_ui::open_save(&mut app);
    assert!(app.macros.save.is_some() && crate::macros_ui::picked(&mut app).len() == 2);
    // Shown another document, nothing is picked there and nothing waits to be saved from it.
    assert!(activate(&mut app, 1));
    assert!(app.macros.save.is_none(), "Save As Command listed the other document's steps");
    assert!(crate::macros_ui::picked(&mut app).is_empty(), "the other document's picks");
}

#[test]
fn a_copy_in_one_document_pastes_into_another_with_its_symbols() {
    let (_, mut app) = app();
    app.tabs.keep_first();
    // The sample's ball (a symbol instance) is copied…
    app.run("layer.select", json!({"index": 0}));
    app.run("edit.select_all", json!({}));
    app.invoke("edit.copy");
    let copied = app.s.selection.len();
    assert!(copied > 0);
    // …and pasted into a new document, which has a symbol of its own under that name.
    app.perform("file.new", json!({}));
    app.run("draw.rect", json!({"x": 0, "y": 0, "w": 10, "h": 10}));
    app.run("modify.convert_to_symbol", json!({"name": "Ball"}));
    app.invoke("edit.paste");
    let lib: Vec<&str> = app.s.doc.library.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(lib.first(), Some(&"Ball"), "{lib:?}");
    assert!(lib.contains(&"Ball 2"), "the copied symbol came along, renamed: {lib:?}");
    let uses = app.s.doc.library_uses();
    let came = app.s.doc.library.iter().find(|s| s.name == "Ball 2").unwrap().id;
    assert!(uses.get(&LibraryItem::Symbol(came)).is_some_and(|n| *n >= 1), "and the pasted instance places it");
    // The sample is as it was.
    assert!(activate(&mut app, 0));
    assert_eq!(app.s.doc.library.len(), 2);
    assert!(!app.s.is_dirty());
}

#[test]
fn closing_a_tab_with_unsaved_changes_asks_and_a_saved_one_just_closes() {
    let (ctx, mut app) = app();
    three(&mut app);
    // The second is untouched: it closes at once.
    let second = infos(&app)[1].id;
    close(&mut app, vec![second]);
    settle(&ctx, &mut app);
    assert_eq!(app.tabs.len(), 2);
    assert!(!app.tabs.asking());
    // The shown one has changes: the question, and Cancel keeps it.
    app.run("draw.rect", json!({"x": 0, "y": 0, "w": 10, "h": 10}));
    close_active(&mut app);
    settle(&ctx, &mut app);
    assert!(app.tabs.asking() && app.modal_open(), "unsaved changes ask first (ADR 0004)");
    assert_eq!(asked_about(&app), ["Untitled 3"]);
    answer(&mut app, Answer::Cancel);
    settle(&ctx, &mut app);
    assert_eq!(app.tabs.len(), 2);
    assert!(app.s.is_dirty(), "the work is still there");
    // Don't Save closes it; the document next to it shows.
    close_active(&mut app);
    settle(&ctx, &mut app);
    answer(&mut app, Answer::Discard);
    settle(&ctx, &mut app);
    assert_eq!(app.tabs.len(), 1);
    assert_eq!(app.s.doc.library.len(), 2, "the sample is shown");
    assert!(app.tabs.closing.is_none());
}

#[test]
fn save_in_the_question_saves_then_closes_and_a_cancelled_save_as_keeps_the_tab() {
    let (ctx, mut app) = app();
    three(&mut app);
    app.run("draw.rect", json!({"x": 0, "y": 0, "w": 10, "h": 10}));
    close_active(&mut app);
    settle(&ctx, &mut app);
    answer(&mut app, Answer::Save);
    settle(&ctx, &mut app);
    assert!(app.prompt.as_ref().is_some_and(|p| p.cmd == "file.save_as"), "a document without a file asks for a name");
    // Cancelled: nothing closes.
    app.prompt = None;
    settle(&ctx, &mut app);
    assert_eq!(app.tabs.len(), 3);
    assert!(app.tabs.closing.is_none() && app.s.is_dirty());
    // Saved: the tab closes.
    let dir = std::env::temp_dir().join(format!("anicraft-tabs-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("third.anicraft");
    close_active(&mut app);
    settle(&ctx, &mut app);
    answer(&mut app, Answer::Save);
    settle(&ctx, &mut app);
    let p = app.prompt.take().unwrap();
    app.request(p.cmd, json!({ p.key: file.display().to_string() }));
    settle(&ctx, &mut app);
    assert!(file.exists());
    assert_eq!(app.tabs.len(), 2);
    // Opening it again shows a new tab; opening it a second time shows that tab.
    app.request("file.open", json!({"path": file.display().to_string()}));
    assert_eq!((app.tabs.len(), app.tabs.active()), (3, 2));
    assert!(activate(&mut app, 0));
    app.request("file.open", json!({"path": file.display().to_string()}));
    assert_eq!((app.tabs.len(), app.tabs.active()), (3, 2), "already open: its tab is shown");
    // A file that can't be opened leaves no tab behind.
    app.request("file.open", json!({"path": dir.join("missing.anicraft").display().to_string()}));
    assert_eq!((app.tabs.len(), app.tabs.active()), (3, 2));
    assert!(app.status.as_ref().is_some_and(|s| s.error));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_close_waits_while_the_system_save_panel_is_up() {
    let (ctx, mut app) = app();
    three(&mut app);
    app.run("draw.rect", json!({"x": 0, "y": 0, "w": 10, "h": 10}));
    close_active(&mut app);
    settle(&ctx, &mut app);
    answer(&mut app, Answer::Save);
    settle(&ctx, &mut app);
    // The system panel instead of the built-in prompt: it answers frames later.
    let p = app.prompt.take().unwrap();
    app.file_request = Some((p.cmd, p.key, String::new(), String::new()));
    settle(&ctx, &mut app);
    assert!(app.tabs.closing.is_some(), "the close waits for the panel's answer");
    app.file_request = None;
    settle(&ctx, &mut app);
    assert!(app.tabs.closing.is_none() && app.s.is_dirty(), "cancelled: the tab stays");
    assert_eq!(app.tabs.len(), 3);
}

#[test]
fn quitting_asks_once_for_every_unsaved_document() {
    let (ctx, mut app) = app();
    three(&mut app);
    app.run("draw.rect", json!({"x": 0, "y": 0, "w": 10, "h": 10}));
    assert!(activate(&mut app, 0));
    app.run("layer.new", json!({}));
    quit(&mut app);
    settle(&ctx, &mut app);
    assert!(app.tabs.asking());
    assert_eq!(asked_about(&app), ["Untitled", "Untitled 3"], "one question names both");
    assert!(!app.allow_close);
    // Cancel: the window stays, with everything open.
    answer(&mut app, Answer::Cancel);
    settle(&ctx, &mut app);
    assert!(!app.allow_close && app.tabs.len() == 3);
    // Don't Save: the window may go.
    quit(&mut app);
    settle(&ctx, &mut app);
    answer(&mut app, Answer::Discard);
    settle(&ctx, &mut app);
    assert!(app.allow_close);
    // Save All with a document that needs a name: asked, and cancelling stops the quit.
    let (ctx, mut app) = self::app();
    three(&mut app);
    app.run("draw.rect", json!({"x": 0, "y": 0, "w": 10, "h": 10}));
    quit(&mut app);
    settle(&ctx, &mut app);
    answer(&mut app, Answer::Save);
    settle(&ctx, &mut app);
    assert!(app.prompt.is_some());
    app.prompt = None;
    settle(&ctx, &mut app);
    assert!(!app.allow_close && app.tabs.closing.is_none());
}

#[test]
fn closing_the_last_document_leaves_an_empty_one() {
    let (ctx, mut app) = app();
    app.tabs.keep_first();
    close_all(&mut app);
    settle(&ctx, &mut app);
    assert_eq!(app.tabs.len(), 1);
    assert!(app.s.doc.library.is_empty() && !app.s.is_dirty(), "a blank page");
    // Which gives way to the next document opened.
    app.request("file.new_sample", json!({}));
    assert_eq!(app.tabs.len(), 1);
    assert_eq!(app.s.doc.library.len(), 2);
}

#[test]
fn tabs_are_clicked_closed_with_the_middle_button_and_switched_with_the_keys() {
    let (ctx, mut app) = app();
    three(&mut app);
    settle(&ctx, &mut app);
    assert_eq!(app.tabs.rects.len(), 3);
    // A click shows the document.
    let at = tab_rect(&app, 0).left_center() + vec2(30.0, 0.0);
    click(&ctx, &mut app, at, egui::PointerButton::Primary);
    assert_eq!(app.tabs.active(), 0);
    // Ctrl+Tab and Ctrl+Shift+Tab go round; Ctrl+PageDown and Ctrl+PageUp too.
    frame(&ctx, &mut app, key(Key::Tab, ctrl()));
    assert_eq!(app.tabs.active(), 1);
    frame(&ctx, &mut app, key(Key::Tab, Modifiers { shift: true, ..ctrl() }));
    frame(&ctx, &mut app, key(Key::Tab, Modifiers { shift: true, ..ctrl() }));
    assert_eq!(app.tabs.active(), 2, "round the end");
    frame(&ctx, &mut app, key(Key::PageDown, ctrl()));
    assert_eq!(app.tabs.active(), 0);
    frame(&ctx, &mut app, key(Key::PageUp, ctrl()));
    assert_eq!(app.tabs.active(), 2);
    // The middle button closes a tab (this one has no changes).
    settle(&ctx, &mut app);
    let at = tab_rect(&app, 1).left_center() + vec2(30.0, 0.0);
    click(&ctx, &mut app, at, egui::PointerButton::Middle);
    settle(&ctx, &mut app);
    assert_eq!(app.tabs.len(), 2);
    // Ctrl+W closes the shown one; its × does too.
    frame(&ctx, &mut app, key(Key::W, ctrl()));
    settle(&ctx, &mut app);
    assert_eq!(app.tabs.len(), 1);
    app.perform("file.new", json!({}));
    settle(&ctx, &mut app);
    let at = pos2(tab_rect(&app, 1).right() - PAD * 0.5 - CLOSE_W * 0.5, tab_rect(&app, 1).center().y);
    click(&ctx, &mut app, at, egui::PointerButton::Primary);
    settle(&ctx, &mut app);
    assert_eq!(app.tabs.len(), 1);
}

#[test]
fn a_dragged_tab_changes_places_and_the_shown_document_stays_shown() {
    let (ctx, mut app) = app();
    three(&mut app);
    settle(&ctx, &mut app);
    let ids: Vec<u64> = infos(&app).iter().map(|t| t.id).collect();
    let from = tab_rect(&app, 2).left_center() + vec2(30.0, 0.0);
    let to = tab_rect(&app, 0).left_center() + vec2(10.0, 0.0);
    frame(&ctx, &mut app, input(Modifiers::NONE, vec![Event::PointerMoved(from)]));
    frame(&ctx, &mut app, input(Modifiers::NONE, vec![Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }]));
    for step in 1..=12 {
        let at = from + (to - from) * (step as f32 / 12.0);
        frame(&ctx, &mut app, input(Modifiers::NONE, vec![Event::PointerMoved(at)]));
    }
    frame(&ctx, &mut app, input(Modifiers::NONE, vec![Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }]));
    settle(&ctx, &mut app);
    let now: Vec<u64> = infos(&app).iter().map(|t| t.id).collect();
    assert_eq!(now, [ids[2], ids[0], ids[1]], "the third tab is first");
    assert_eq!(app.tabs.active(), 0, "and still the one shown");
    assert_eq!(app.s.doc.stage.width, 800);
}

#[test]
fn the_tabs_are_a_tab_list_for_screen_readers() {
    use egui::accesskit::Role;
    let (ctx, mut app) = app();
    ctx.enable_accesskit();
    three(&mut app);
    app.run("draw.rect", json!({"x": 0, "y": 0, "w": 10, "h": 10}));
    settle(&ctx, &mut app);
    let update = frame(&ctx, &mut app, input(Modifiers::NONE, vec![])).platform_output.accesskit_update.unwrap();
    let list = update.nodes.iter().find(|(_, n)| n.role() == Role::TabList).expect("a tab list");
    assert_eq!(list.1.label(), Some("Open documents"));
    let tabs: Vec<_> = update.nodes.iter().filter(|(_, n)| n.role() == Role::Tab).collect();
    assert_eq!(tabs.len(), 3);
    assert!(tabs.iter().all(|(id, _)| list.1.children().contains(id)), "the tabs are the list's");
    let mut said: Vec<(&str, Option<bool>)> = tabs.iter().filter_map(|(_, n)| Some((n.label()?, n.is_selected()))).collect();
    said.sort();
    assert_eq!(said, [("Untitled", Some(false)), ("Untitled 2", Some(false)), ("Untitled 3, unsaved changes", Some(true))]);
    // Each has a named button to close it.
    assert!(update.nodes.iter().any(|(_, n)| n.role() == Role::Button && n.label() == Some("Close Untitled 2")));
}

#[test]
fn a_document_that_is_not_shown_keeps_no_pictures() {
    let (ctx, mut app) = app();
    app.tabs.keep_first();
    settle(&ctx, &mut app);
    let _ = app.s.display_list();
    app.perform("file.new", json!({}));
    assert_eq!(app.stage_cache.tile_count(), 0, "the tiles of the document put aside are gone");
    assert!(app.stage_cache.stats().is_none_or(|(n, _, _)| n == 0), "and the render cache's pictures");
    assert!(app.stage_cache.shown_state.is_none());
}

/// In a recovery folder of this test's own.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn autosave_and_recovery_cover_every_open_document() {
    use anicraft_engine::recovery;
    let dir = std::env::temp_dir().join(format!("anicraft-tabs-recovery-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let wait = |saver_ids: &[String]| {
        for _ in 0..200 {
            if saver_ids.iter().all(|id| recovery::document_path(&dir, id).exists()) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the copies were not written");
    };
    crate::platform::set_recovery_dir_for_tests(Some(dir.clone()));
    {
        let (ctx, mut app) = app();
        three(&mut app);
        let ids = recovery_sessions(&app);
        assert_eq!(ids.len(), 3, "each open document is its own recovery session");
        assert_eq!(ids.iter().collect::<HashSet<_>>().len(), 3);
        // Unsaved work in the third and the first; the second is untouched.
        app.run("draw.rect", json!({"x": 0, "y": 0, "w": 10, "h": 10}));
        assert!(activate(&mut app, 0), "putting a document aside copies its unsaved work");
        app.run("layer.new", json!({}));
        settle(&ctx, &mut app);
        // The window goes without a decision (a crash, as far as the copies can tell).
        finish_all(&mut app, false);
        // (The shown document's session comes first, then the others in tab order.)
        wait(&[ids[0].clone(), ids[1].clone()]);
        assert!(!recovery::document_path(&dir, &ids[2]).exists(), "nothing to recover for the untouched one");
    }
    // The next start finds both and restores each into a tab of its own.
    let (ctx, mut app) = app();
    assert_eq!(app.recovery.found.len(), 2);
    let found: Vec<String> = app.recovery.found.iter().map(|e| e.session.clone()).collect();
    for id in &found {
        app.request("file.recover", json!({"dir": dir.display().to_string(), "session": id}));
    }
    settle(&ctx, &mut app);
    assert_eq!(app.tabs.len(), 2, "the first takes the sample's place, the second gets a tab");
    assert!(infos(&app).iter().all(|t| t.dirty), "restored work is unsaved work");
    assert!(app.recovery.found.is_empty());
    // A clean end removes this session's copies.
    app.allow_close = true;
    finish_all(&mut app, true);
    crate::platform::set_recovery_dir_for_tests(None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "a measurement: cargo test --bin anicraft switching_cost -- --ignored --nocapture"]
fn switching_cost() {
    let (ctx, mut app) = app();
    app.tabs.keep_first();
    app.perform("file.new_sample", json!({}));
    app.perform("file.new_sample", json!({}));
    settle(&ctx, &mut app);
    let mut times = Vec::new();
    for i in 0..300 {
        let t = std::time::Instant::now();
        assert!(activate(&mut app, i % 3));
        times.push(t.elapsed().as_secs_f64() * 1e6);
    }
    times.sort_by(f64::total_cmp);
    eprintln!("switch: median {:.1} µs, p95 {:.1} µs, max {:.1} µs", times[150], times[285], times[299]);
}
