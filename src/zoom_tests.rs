//! The zoom edit-bar field (#43): an editable percentage with a separate presets arrow,
//! driven frame by frame like the retime tests.

use crate::app::App;
use egui::{Event, Modifiers, PointerButton, Pos2, Rect, vec2};

fn frame(ctx: &egui::Context, app: &mut App, events: Vec<Event>, modifiers: Modifiers) {
    sized_frame(ctx, app, vec2(1600.0, 940.0), events, modifiers);
}

fn sized_frame(ctx: &egui::Context, app: &mut App, size: egui::Vec2, events: Vec<Event>, modifiers: Modifiers) {
    let mut all = vec![Event::ModifiersChanged(modifiers)];
    all.extend(events);
    let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)), events: all, ..Default::default() };
    ctx.run_ui(input, |ui| app.frame(ui)).textures_delta.clear();
}

fn settle(ctx: &egui::Context, app: &mut App) {
    for _ in 0..3 {
        frame(ctx, app, Vec::new(), Modifiers::NONE);
    }
}

fn button(pos: Pos2, pressed: bool, modifiers: Modifiers) -> Event {
    Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers }
}

fn click(ctx: &egui::Context, app: &mut App, at: Pos2, modifiers: Modifiers) {
    frame(ctx, app, vec![Event::PointerMoved(at)], modifiers);
    frame(ctx, app, vec![button(at, true, modifiers)], modifiers);
    frame(ctx, app, vec![button(at, false, modifiers)], modifiers);
    settle(ctx, app);
}

fn key(ctx: &egui::Context, app: &mut App, key: egui::Key, modifiers: Modifiers) {
    frame(ctx, app, vec![Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }], modifiers);
    frame(ctx, app, vec![Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers }], modifiers);
    settle(ctx, app);
}

fn type_text(ctx: &egui::Context, app: &mut App, s: &str) {
    frame(ctx, app, vec![Event::Text(s.to_owned())], Modifiers::NONE);
    settle(ctx, app);
}

fn select_all(ctx: &egui::Context, app: &mut App) {
    frame(ctx, app, vec![Event::Key { key: egui::Key::A, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::CTRL }], Modifiers::CTRL);
    frame(ctx, app, vec![Event::Key { key: egui::Key::A, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::CTRL }], Modifiers::CTRL);
    settle(ctx, app);
}

fn clear_field(ctx: &egui::Context, app: &mut App) {
    select_all(ctx, app);
    // Selection or not (Select All is ⌘A on macOS, so Ctrl+A selects nothing there), this
    // leaves the field empty: the seed is 3-4 digits, deleted on both sides of the cursor.
    for key in [egui::Key::Backspace, egui::Key::Delete] {
        for _ in 0..8 {
            frame(ctx, app, vec![Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }], Modifiers::NONE);
            frame(ctx, app, vec![Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE }], Modifiers::NONE);
        }
    }
    settle(ctx, app);
}

fn zoom_rects(ctx: &egui::Context) -> (Rect, Rect) {
    let field = ctx.data(|d| d.get_temp::<Rect>(egui::Id::unique("zoom_field"))).expect("the zoom field drew");
    let arrow = ctx.data(|d| d.get_temp::<Rect>(egui::Id::unique("zoom_arrow"))).expect("the zoom arrow drew");
    (field, arrow)
}

#[test]
fn zoom_field_types_a_value_and_rejects_garbage() {
    let ctx = egui::Context::default();
    let session = anicraft_engine::Session::new(anicraft_engine::doc::sample::demo());
    let mut app = App::with_context(&ctx, session, Some(std::path::PathBuf::from("unused.png")));
    app.view.fit = false;
    app.view.zoom = 1.0;
    settle(&ctx, &mut app);
    app.view.fit = false;
    app.view.zoom = 1.0;
    settle(&ctx, &mut app);

    // Clicking the text focuses it without changing the zoom (no presets menu from the text).
    let (field, arrow) = zoom_rects(&ctx);
    let before = app.view.zoom;
    click(&ctx, &mut app, field.center(), Modifiers::NONE);
    assert!((app.view.zoom - before).abs() < 1e-6, "focusing the field leaves zoom alone");
    // Clicking the arrow alone does not change the zoom either (it only opens the presets).
    click(&ctx, &mut app, arrow.center(), Modifiers::NONE);
    assert!((app.view.zoom - before).abs() < 1e-6, "the presets arrow alone zooms nothing");
    key(&ctx, &mut app, egui::Key::Escape, Modifiers::NONE);

    // Type a custom value and press Enter: the stage zooms there.
    let (field, _) = zoom_rects(&ctx);
    click(&ctx, &mut app, field.center(), Modifiers::NONE);
    clear_field(&ctx, &mut app);
    type_text(&ctx, &mut app, "150");
    key(&ctx, &mut app, egui::Key::Enter, Modifiers::NONE);
    assert!((app.view.zoom - 1.5).abs() < 0.01, "typed 150% zooms to 1.5, got {}", app.view.zoom);

    // Garbage reverts to the current zoom without changing anything.
    let (field, _) = zoom_rects(&ctx);
    click(&ctx, &mut app, field.center(), Modifiers::NONE);
    clear_field(&ctx, &mut app);
    type_text(&ctx, &mut app, "abc");
    key(&ctx, &mut app, egui::Key::Enter, Modifiers::NONE);
    assert!((app.view.zoom - 1.5).abs() < 0.01, "invalid text keeps zoom at 1.5, got {}", app.view.zoom);
}

#[test]
fn stage_zoom_measures_device_pixels_at_non_unit_display_scale() {
    let ctx = egui::Context::default();
    let session = anicraft_engine::Session::new(anicraft_engine::doc::sample::demo());
    let mut app = App::with_context(&ctx, session, Some(std::path::PathBuf::from("unused.png")));
    settle(&ctx, &mut app);
    app.view.fit = false;
    ctx.set_pixels_per_point(1.6);
    settle(&ctx, &mut app);
    let scale = ctx.pixels_per_point();
    assert!((scale - 1.0).abs() > 0.1, "simulate a scaled display");

    key(&ctx, &mut app, egui::Key::Num1, Modifiers::COMMAND);
    assert!((app.view.zoom * scale - 1.0).abs() < 0.01, "100% maps one stage pixel to one device pixel");

    let centre_px = |app: &App| (app.stage_cache.canvas.center().to_vec2() + app.view.pan) * ctx.pixels_per_point();
    let previous_centre = centre_px(&app);
    ctx.set_pixels_per_point(2.0);
    // The same window in device pixels takes fewer points on the new display.
    for _ in 0..3 {
        sized_frame(&ctx, &mut app, vec2(1280.0, 752.0), Vec::new(), Modifiers::NONE);
    }
    let scale = ctx.pixels_per_point();
    assert!((scale - 2.0).abs() < 0.01, "simulate changing to another display scale");
    assert!((app.view.zoom * scale - 1.0).abs() < 0.01, "a chosen 100% remains 100% after the display scale changes");
    let current_centre = centre_px(&app);
    assert!((current_centre - previous_centre).length() < 1.0, "the stage centre is stable in device pixels: {previous_centre:?} → {current_centre:?}");

    settle(&ctx, &mut app);
    let (field, _) = zoom_rects(&ctx);
    click(&ctx, &mut app, field.center(), Modifiers::NONE);
    clear_field(&ctx, &mut app);
    type_text(&ctx, &mut app, "200");
    key(&ctx, &mut app, egui::Key::Enter, Modifiers::NONE);
    assert!((app.view.zoom * scale - 2.0).abs() < 0.01, "typed 200% maps one stage pixel to two device pixels");
}

#[test]
fn fit_in_window_shows_the_whole_large_stage_below_the_manual_zoom_limit() {
    let ctx = egui::Context::default();
    let mut doc = anicraft_engine::doc::Document::default();
    doc.stage.width = 16000;
    doc.stage.height = 100;
    let session = anicraft_engine::Session::new(doc);
    let mut app = App::with_context(&ctx, session, Some(std::path::PathBuf::from("unused.png")));
    ctx.set_pixels_per_point(0.8);
    let size = vec2(800.0, 800.0);
    for _ in 0..3 {
        sized_frame(&ctx, &mut app, size, Vec::new(), Modifiers::NONE);
    }
    let scale = ctx.pixels_per_point();
    let stage = app.s.doc.stage.width as f32;
    let canvas = app.stage_cache.canvas.width();
    assert!((scale - 0.8).abs() < 0.01 && canvas * scale < stage * 0.05, "this canvas needs less than 5%: {canvas} pt, stage {stage} px, scale {scale}");
    let width = stage * app.view.zoom;
    assert!(width <= canvas && width > canvas * 0.5, "stage width {width} fits the canvas {canvas}");

    let fitted = app.view.zoom;
    let modifiers = Modifiers::COMMAND;
    for pressed in [true, false] {
        let event = Event::Key { key: egui::Key::Minus, physical_key: None, pressed, repeat: false, modifiers };
        sized_frame(&ctx, &mut app, size, vec![event], modifiers);
    }
    assert_eq!(app.view.zoom, fitted, "Zoom Out from Fit does not jump up to the manual minimum");
}
