//! A long Brush stroke keeps the window answering (issue #519, P-003).

use crate::app::{App, Tool};
use anicraft_engine::geom::Vec2;
use egui::{Event, Modifiers, PointerButton, Pos2, Rect, vec2};
use serde_json::json;
use std::time::Instant;

/// The app on a document, a few frames in, with its clock and the worst frame so far.
struct Rig {
    ctx: egui::Context,
    app: App,
    t: f64,
    worst: (f64, String),
    log: bool,
}

impl Rig {
    fn new(doc: anicraft_engine::doc::Document) -> Rig {
        let ctx = egui::Context::default();
        let app = App::with_context(&ctx, anicraft_engine::Session::new(doc), Some(std::path::PathBuf::from("unused.png")));
        let mut rig = Rig { ctx, app, t: 0.0, worst: (0.0, String::new()), log: false };
        for _ in 0..3 {
            rig.frame("start", Vec::new());
        }
        rig.worst = (0.0, String::new());
        rig
    }

    fn frame(&mut self, what: &str, events: Vec<Event>) -> f64 {
        self.t += 0.05;
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 940.0))), time: Some(self.t), events, ..Default::default() };
        let t = Instant::now();
        let mut out = self.ctx.run_ui(input, |ui| self.app.frame(ui));
        out.textures_delta.clear();
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        if self.log {
            println!("{ms:9.1} ms  {what}");
        }
        if ms > self.worst.0 {
            self.worst = (ms, what.to_string());
        }
        ms
    }

    /// Where stage point `p` is on screen.
    fn at(&self, p: Vec2) -> Pos2 {
        let app = &self.app;
        let size = vec2(app.s.doc.stage.width as f32, app.s.doc.stage.height as f32);
        let origin = app.stage_cache.canvas.center() + app.view.pan - size * app.view.zoom * 0.5;
        origin + vec2(p.x, p.y) * app.view.zoom
    }

    fn button(pos: Pos2, pressed: bool) -> Event {
        Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }
    }

    fn click(&mut self, what: &str, p: Vec2) {
        let sp = self.at(p);
        self.frame(&format!("{what}: move"), vec![Event::PointerMoved(sp)]);
        self.frame(&format!("{what}: press"), vec![Rig::button(sp, true)]);
        self.frame(&format!("{what}: release"), vec![Rig::button(sp, false)]);
        self.frame(&format!("{what}: after"), Vec::new());
    }

    fn drag(&mut self, what: &str, from: Vec2, to: Vec2) {
        let (a, b) = (self.at(from), self.at(to));
        self.frame(&format!("{what}: move"), vec![Event::PointerMoved(a)]);
        self.frame(&format!("{what}: press"), vec![Rig::button(a, true)]);
        for i in 1..=4 {
            self.frame(&format!("{what}: drag"), vec![Event::PointerMoved(a + (b - a) * (i as f32 / 4.0))]);
        }
        self.frame(&format!("{what}: release"), vec![Rig::button(b, false)]);
        for _ in 0..2 {
            self.frame(&format!("{what}: after"), Vec::new());
        }
    }

    /// A stage point on the ink, and one in a gap inside the drawing's bounds.
    fn ink_and_gap(&self) -> (Vec2, Vec2) {
        let b = self.app.s.display_list().items.iter().fold(anicraft_engine::geom::Rect::EMPTY, |r, i| r.union(&i.path.bounds().transformed(&i.transform)));
        let grid = |hit: bool| {
            (0..4000)
                .map(|i| Vec2::new((i * 37 % 1280) as f32, (i * 53 % 720) as f32))
                .find(|p| b.inflate(-20.0).contains(*p) && (0..9).all(|k| self.app.s.hit_test(*p + Vec2::new((k % 3 - 1) as f32 * 2.0, (k / 3 - 1) as f32 * 2.0), 0.0).is_some() == hit))
                .unwrap_or_default()
        };
        (grid(true), grid(false))
    }
}

/// A fast scribble across the stage, as stage points with pressure: `n` samples.
fn scribble(n: usize) -> Vec<(Vec2, Option<f32>)> {
    (0..n).map(|i| i as f32 * 0.06).map(|t| (Vec2::new(640.0 + 420.0 * (t * 0.11).sin() + 170.0 * (t * 0.83).cos(), 360.0 + 200.0 * (t * 0.07).cos() + 130.0 * (t * 0.83).sin()), Some(0.55 + 0.45 * (t * 0.37).sin()))).collect()
}

impl Rig {
    /// Draws `samples` with the Brush: a press, the samples, and the release.
    fn brush(&mut self, samples: Vec<(Vec2, Option<f32>)>) {
        self.app.tool = Tool::Brush;
        let (first, last) = (samples.first().map_or(Vec2::ZERO, |s| s.0), samples.last().map_or(Vec2::ZERO, |s| s.0));
        let a = self.at(first);
        self.frame("brush: move", vec![Event::PointerMoved(a)]);
        self.frame("brush: press", vec![Rig::button(a, true)]);
        self.frame("brush: drag", vec![Event::PointerMoved(a + vec2(20.0, 12.0))]);
        self.frame("brush: drag", vec![Event::PointerMoved(a + vec2(40.0, 24.0))]);
        assert_eq!(self.app.drag, crate::app::Drag::Brush, "the press began a stroke");
        self.app.brush_samples = samples;
        let b = self.at(last);
        self.frame("brush: release", vec![Rig::button(b, false)]);
    }

    /// Frames until no stroke waits (the trace is on another thread), or a long time has passed.
    fn until_painted(&mut self) {
        let started = Instant::now();
        while !self.app.ink.is_empty() && started.elapsed().as_secs() < 120 {
            std::thread::sleep(std::time::Duration::from_millis(2));
            self.frame("waiting", Vec::new());
        }
    }

    fn shapes(&self) -> usize {
        self.app.s.visible_elements().len()
    }
}

fn blank() -> Rig {
    let mut rig = Rig::new(anicraft_engine::doc::Document::default());
    rig.app.s.style.object_drawing = true;
    rig
}

/// ADR 0129: when the pointer lifts on a long stroke, the frame does not wait for its outline.
/// The stroke stays on the stage as drawn, and goes into the document, as one undo step, once
/// its outline is traced.
#[test]
fn a_long_stroke_is_painted_after_the_pointer_lifts_without_holding_the_frame() {
    let mut rig = blank();
    let undo_before = rig.app.s.history().undo_len();
    rig.brush(scribble(3000));
    assert_eq!(rig.app.ink.len(), 1, "the stroke waits for its outline");
    assert_eq!(rig.app.drag, crate::app::Drag::None);
    // It is still drawn: the stage paints its sweep, piece by piece.
    let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 940.0))), time: Some(rig.t + 0.01), ..Default::default() };
    let xf = crate::pathtools::Xf::flat(rig.at(Vec2::ZERO), rig.app.view.zoom);
    let mut out = rig.ctx.run_ui(input, |ui| crate::ink::draw(&rig.app, ui.painter(), xf));
    out.textures_delta.clear();
    assert!(out.shapes.len() > 100, "{} pieces", out.shapes.len());
    rig.until_painted();
    assert_eq!(rig.shapes(), 1, "painted");
    assert_eq!(rig.app.s.history().undo_len(), undo_before + 1, "one undo step");
    // A short stroke never waits.
    rig.brush(scribble(40));
    assert!(rig.app.ink.is_empty());
    assert_eq!(rig.shapes(), 2);
}

/// Nothing comes between a stroke and its paint: an edit asked for while it waits finds it in
/// the document already, strokes are painted in the order they were drawn, and a key press
/// (undo, say) acts on the stroke, not on what was there before it.
#[test]
fn edits_wait_for_the_strokes_drawn_before_them() {
    let mut rig = blank();
    rig.brush(scribble(3000));
    rig.brush(scribble(60));
    assert_eq!(rig.app.ink.len(), 2, "a short stroke queues behind a waiting one");
    // A command: both strokes are in before it runs, the long one first (under the short one).
    rig.app.run("edit.select_all", json!({}));
    assert!(rig.app.ink.is_empty());
    assert_eq!(rig.app.s.selection.len(), 2);
    let widths: Vec<f32> = rig.app.s.visible_elements().iter().map(|e| rig.app.s.element_bounds(e.id).width()).collect();
    assert!(widths.first() > widths.last(), "drawn first, painted first (the long one is under the short one): {widths:?}");
    // A key: the waiting stroke is painted before the key is handled.
    let mut rig = blank();
    rig.brush(scribble(3000));
    assert_eq!(rig.app.ink.len(), 1);
    rig.frame("a key", vec![Event::Key { key: egui::Key::F13, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
    assert!(rig.app.ink.is_empty());
    assert_eq!(rig.shapes(), 1);
    // Only a few strokes wait at once: one more paints the oldest first.
    let mut rig = blank();
    for _ in 0..10 {
        rig.brush(scribble(450));
        assert!(rig.app.ink.len() <= 8, "{} waiting", rig.app.ink.len());
    }
    rig.until_painted();
    assert_eq!(rig.shapes(), 10, "every stroke was painted");
    // Leaving the document: it takes its strokes with it.
    let mut rig = blank();
    rig.brush(scribble(3000));
    crate::ink::flush(&mut rig.app);
    assert_eq!(rig.shapes(), 1);
}

/// The reporter's steps and every tool's gesture on the reporter's file
/// (`ANICRAFT_STROKE_FILE`), each from a fresh window: the worst frame of each.
#[test]
#[ignore = "a measurement, not a check"]
fn long_stroke_frame_cost() {
    let Ok(file) = std::env::var("ANICRAFT_STROKE_FILE") else { return };
    let doc = anicraft_engine::format::load(std::path::Path::new(&file)).expect("the file loads");
    let log = std::env::var("ANICRAFT_STROKE_LOG").is_ok();
    type Scenario = (&'static str, fn(&mut Rig, Vec2, Vec2));
    let scenarios: [Scenario; 12] = [
        ("move, then click a gap", |r, ink, gap| {
            r.app.tool = Tool::Select;
            r.drag("move", ink, ink + Vec2::new(40.0, 24.0));
            r.click("deselect", gap + Vec2::new(40.0, 24.0));
        }),
        ("click a gap", |r, _, gap| {
            r.app.tool = Tool::Select;
            r.click("gap", gap);
        }),
        ("marquee over a part", |r, _, gap| {
            r.app.tool = Tool::Select;
            r.drag("marquee", gap, gap + Vec2::new(260.0, 180.0));
            r.click("deselect", gap);
        }),
        ("double-click", |r, ink, _| {
            r.app.tool = Tool::Select;
            r.click("first", ink);
            r.click("second", ink);
        }),
        ("select all, Free Transform", |r, ink, _| {
            r.app.tool = Tool::Transform;
            r.app.run("edit.select_all", json!({}));
            r.frame("selected", Vec::new());
            r.drag("move", ink, ink + Vec2::new(30.0, 10.0));
        }),
        ("Subselection click", |r, ink, _| {
            r.app.tool = Tool::Subselect;
            r.click("ink", ink);
            r.frame("shown", Vec::new());
        }),
        ("Paint Bucket in a gap", |r, _, gap| {
            r.app.tool = Tool::PaintBucket;
            r.click("gap", gap);
        }),
        ("Paint Bucket on the ink", |r, ink, _| {
            r.app.tool = Tool::PaintBucket;
            r.click("ink", ink);
        }),
        ("Eraser across", |r, ink, _| {
            r.app.tool = Tool::Eraser;
            r.drag("erase", ink + Vec2::new(-200.0, -80.0), ink + Vec2::new(200.0, 80.0));
        }),
        ("Brush across", |r, ink, _| {
            r.app.tool = Tool::Brush;
            r.drag("paint", ink + Vec2::new(-200.0, 80.0), ink + Vec2::new(200.0, -80.0));
        }),
        ("Pencil across", |r, ink, _| {
            r.app.tool = Tool::Pencil;
            r.drag("draw", ink + Vec2::new(-200.0, 60.0), ink + Vec2::new(200.0, -60.0));
        }),
        ("Rectangle over", |r, ink, _| {
            r.app.tool = Tool::Rect;
            r.drag("draw", ink + Vec2::new(-100.0, -60.0), ink + Vec2::new(100.0, 60.0));
            r.app.tool = Tool::Select;
            r.frame("select tool", Vec::new());
        }),
    ];
    for (name, run) in scenarios {
        let mut rig = Rig::new(doc.clone());
        rig.log = log;
        let (ink, gap) = rig.ink_and_gap();
        let t = Instant::now();
        run(&mut rig, ink, gap);
        println!("{:9.1} ms worst ({}), {:8.1} ms in all, {} elements  <- {name}", rig.worst.0, rig.worst.1, t.elapsed().as_secs_f64() * 1000.0, rig.app.s.visible_elements().len());
    }
}
