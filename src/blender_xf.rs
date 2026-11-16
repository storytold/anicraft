//! Grab, rotate and scale the way Blender does (ADR 0101).
//!
//! G grabs, R rotates, S scales, unless that key already does something else, in which case
//! Shift is held with it. The move is the Selection tool's, and the turn and the size are Free
//! Transform's (`Q`): the same commands, about the selection's centre, one undo step. Type a
//! number for an exact distance, a counterclockwise angle in degrees, or a scale factor.
//! Enter or a click confirms. Esc or a right-click cancels. X or Y locks that axis.

use crate::app::{App, Drag, Tool};
use crate::keymap::{self, Keymap};
use anicraft_engine::geom::Affine;
use egui::{CursorIcon, Key, Modifiers, Pos2, pos2};
use serde_json::json;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Grab,
    Rotate,
    Scale,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Axis {
    X,
    Y,
}

enum Digit {
    N(u8),
    Dot,
    Minus,
    Back,
}

pub struct Live {
    kind: Kind,
    /// Edit Multiple Frames was on when this started, so every marked keyframe moves.
    multi: bool,
    /// Selection bounds when this started: the pivot stays put while the artwork moves.
    bounds: [f32; 4],
    /// Pointer, in stage space, on the first frame. `None` until the stage has reported it.
    origin: Option<Pos2>,
    applied: Affine,
    digits: String,
    axis: Option<Axis>,
    confirm: bool,
    cancel: bool,
}

/// The chord for `key`: the key alone when nothing else owns it, otherwise Shift held with it.
/// `None` when both are already bound.
pub(crate) fn mods_for(map: &Keymap, key: Key) -> Option<Modifiers> {
    let bare = keymap::chord(Modifiers::NONE, key);
    if map.owner(&bare).is_none() {
        return Some(Modifiers::NONE);
    }
    let shift = keymap::chord(Modifiers::SHIFT, key);
    if map.owner(&shift).is_none() {
        return Some(Modifiers::SHIFT);
    }
    None
}

/// Keys. `true` when a transform is underway, so the rest of the shortcut map waits.
pub fn keys(app: &mut App, ctx: &egui::Context, control: bool) -> bool {
    if app.blender.is_some() {
        modal_keys(app, ctx);
        ctx.request_repaint();
        return true;
    }
    if control || ctx.egui_wants_keyboard_input() || egui::Popup::is_any_open(ctx) {
        return false;
    }
    let Some(kind) = pressed(app, ctx) else { return false };
    begin(app, kind);
    false
}

/// Follows the pointer. A click confirms and a right-click cancels, the same as Enter and Esc.
pub fn track(app: &mut App, pointer: Pos2, click: bool, secondary: bool) {
    {
        let Some(op) = app.blender.as_mut() else { return };
        if op.origin.is_none() {
            op.origin = Some(pointer);
        }
        if secondary {
            op.cancel = true;
        } else if click {
            op.confirm = true;
        }
    }
    if app.blender.as_ref().is_some_and(|o| o.cancel) {
        abort(app);
        app.status_ok(tr!("Cancelled."));
        return;
    }
    apply(app, pointer);
    if app.blender.as_ref().is_some_and(|o| o.confirm) {
        confirm(app);
    }
}

/// The cursor while a transform is underway.
pub fn cursor(app: &App) -> Option<CursorIcon> {
    app.blender.as_ref().map(|op| match op.kind {
        Kind::Grab => CursorIcon::Move,
        Kind::Rotate => CursorIcon::Alias,
        Kind::Scale => CursorIcon::ResizeNeSw,
    })
}

fn pressed(app: &App, ctx: &egui::Context) -> Option<Kind> {
    let choices = [(Kind::Grab, Key::G), (Kind::Rotate, Key::R), (Kind::Scale, Key::S)];
    let chords: Vec<(Kind, Modifiers, Key)> = choices.into_iter().filter_map(|(kind, key)| mods_for(&app.keymap, key).map(|m| (kind, m, key))).collect();
    let mut found = None;
    ctx.input_mut(|i| {
        for (kind, mods, key) in chords {
            if i.consume_key(mods, key) {
                found = Some(kind);
            }
        }
    });
    found
}

fn modal_keys(app: &mut App, ctx: &egui::Context) {
    let current = app.blender.as_ref().map(|o| o.kind);
    let mut digit = None;
    let mut axis = None;
    let mut confirm = false;
    let mut cancel = false;
    let mut switch = None;
    ctx.input_mut(|i| {
        if i.consume_key(Modifiers::NONE, Key::Escape) {
            cancel = true;
        }
        if i.consume_key(Modifiers::NONE, Key::Enter) {
            confirm = true;
        }
        if i.consume_key(Modifiers::NONE, Key::Backspace) {
            digit = Some(Digit::Back);
        }
        if i.consume_key(Modifiers::NONE, Key::Minus) {
            digit = Some(Digit::Minus);
        }
        if i.consume_key(Modifiers::NONE, Key::Period) {
            digit = Some(Digit::Dot);
        }
        for (n, key) in [(0, Key::Num0), (1, Key::Num1), (2, Key::Num2), (3, Key::Num3), (4, Key::Num4), (5, Key::Num5), (6, Key::Num6), (7, Key::Num7), (8, Key::Num8), (9, Key::Num9)] {
            if i.consume_key(Modifiers::NONE, key) {
                digit = Some(Digit::N(n));
            }
        }
        if i.consume_key(Modifiers::NONE, Key::X) {
            axis = Some(Axis::X);
        }
        if i.consume_key(Modifiers::NONE, Key::Y) {
            axis = Some(Axis::Y);
        }
        // Inside the transform the letters switch grab, rotate and scale, even when the bare
        // key is a tool the rest of the time.
        if i.consume_key(Modifiers::NONE, Key::G) {
            switch = Some(Kind::Grab);
        }
        if i.consume_key(Modifiers::NONE, Key::R) {
            switch = Some(Kind::Rotate);
        }
        if i.consume_key(Modifiers::NONE, Key::S) {
            switch = Some(Kind::Scale);
        }
    });
    if switch == current {
        switch = None;
    }
    if let Some(op) = app.blender.as_mut() {
        if cancel {
            op.cancel = true;
        }
        if confirm {
            op.confirm = true;
        }
        if let Some(a) = axis {
            op.axis = if op.axis == Some(a) { None } else { Some(a) };
        }
        if let Some(ev) = digit {
            push_digit(&mut op.digits, ev);
        }
    }
    if cancel {
        return;
    }
    if let Some(next) = switch {
        abort(app);
        begin(app, next);
    }
}

/// Starts one. False when there is nothing to transform, or a drag is already open.
fn begin(app: &mut App, kind: Kind) -> bool {
    if crate::camera_tool::previewing(app) {
        app.status_ok(tr!("Turn off camera preview to transform the selection."));
        return false;
    }
    if app.drag != Drag::None || app.s.in_gesture() {
        return false;
    }
    let Some(bounds) = selection_bounds(app) else {
        app.status_ok(tr!("Select something first."));
        return false;
    };
    let multi = crate::onion::editing(app);
    match kind {
        // Grab is a move, which the Selection tool does. Rotate and scale are Free Transform.
        Kind::Grab => {
            if !matches!(app.tool, Tool::Select | Tool::Transform) {
                app.tool = Tool::Select;
            }
        }
        Kind::Rotate | Kind::Scale => app.tool = Tool::Transform,
    }
    app.playing = false;
    app.s.begin_gesture(undo_label(kind));
    app.blender = Some(Live { kind, multi, bounds, origin: None, applied: Affine::IDENTITY, digits: String::new(), axis: None, confirm: false, cancel: false });
    true
}

fn selection_bounds(app: &mut App) -> Option<[f32; 4]> {
    if crate::onion::editing(app) {
        return crate::onion::selection_bounds(app);
    }
    let r = app.s.selection_bounds();
    (!r.is_empty()).then_some([r.min.x, r.min.y, r.max.x, r.max.y])
}

fn undo_label(kind: Kind) -> &'static str {
    match kind {
        Kind::Grab => tr_noop!("Grab"),
        Kind::Rotate => tr_noop!("Rotate"),
        Kind::Scale => tr_noop!("Scale"),
    }
}

fn abort(app: &mut App) {
    app.s.revert_gesture();
    app.s.end_gesture();
    app.blender = None;
}

fn confirm(app: &mut App) {
    let multi = app.blender.as_ref().is_some_and(|o| o.multi);
    if multi {
        crate::onion::drag_end(app);
    } else {
        app.drop_raw();
        app.s.end_gesture();
    }
    app.blender = None;
}

fn apply(app: &mut App, pointer: Pos2) {
    let Some(op) = app.blender.as_ref() else { return };
    let origin = op.origin.unwrap_or(pointer);
    let kind = op.kind;
    let bounds = op.bounds;
    let axis = op.axis;
    let digits = op.digits.clone();
    let applied = op.applied;
    let multi = op.multi;
    let total = total(kind, bounds, origin, pointer, number(&digits), axis);
    let Some(inv) = applied.inverse() else { return };
    let delta = total.mul(&inv);
    let changed = delta.m.iter().zip(Affine::IDENTITY.m).any(|(a, b)| (a - b).abs() > 1e-5);
    let ok = if !changed {
        true
    } else if multi {
        crate::onion::transform_by(app, &delta)
    } else {
        let [a, b, c, d, e, f] = delta.m;
        app.s.execute("modify.transform", &json!({"a": a, "b": b, "c": c, "d": d, "e": e, "f": f})).is_ok()
    };
    if ok {
        if let Some(op) = app.blender.as_mut() {
            op.applied = total;
        }
        say(app, kind, axis, &digits, bounds, origin, pointer);
    } else if let Some(op) = app.blender.as_mut() {
        op.confirm = false;
        app.status_err(tr!("That transform did not apply."));
    }
}

fn say(app: &mut App, kind: Kind, axis: Option<Axis>, digits: &str, bounds: [f32; 4], origin: Pos2, pointer: Pos2) {
    let axis = match axis {
        Some(Axis::X) => "X ",
        Some(Axis::Y) => "Y ",
        None => "",
    };
    let typed = !digits.is_empty() && number(digits).is_some();
    let text = match kind {
        Kind::Grab => {
            let (x, y) = grab_delta(origin, pointer, number(digits), axis_of(axis));
            if typed {
                tr!("Grab {axis}{n}. Enter confirms, Esc cancels.", axis = axis, n = digits)
            } else {
                tr!("Grab {axis}{x}, {y}. Enter confirms, Esc cancels.", axis = axis, x = fmt1(x), y = fmt1(y))
            }
        }
        Kind::Rotate => {
            let n = if typed { digits.to_string() } else { fmt1(rotate_degrees(bounds, origin, pointer)) };
            tr!("Rotate {n}°. Enter confirms, Esc cancels.", n = n)
        }
        Kind::Scale => {
            let n = if typed { digits.to_string() } else { fmt2(scale_factor(bounds, origin, pointer, axis_of(axis))) };
            tr!("Scale {axis}{n}. Enter confirms, Esc cancels.", axis = axis, n = n)
        }
    };
    app.status_ok(text);
}

fn axis_of(label: &str) -> Option<Axis> {
    match label {
        "X " => Some(Axis::X),
        "Y " => Some(Axis::Y),
        _ => None,
    }
}

fn fmt1(n: f32) -> String {
    format!("{n:.1}")
}

fn fmt2(n: f32) -> String {
    format!("{n:.2}")
}

fn push_digit(buf: &mut String, ev: Digit) {
    match ev {
        Digit::Back => {
            buf.pop();
        }
        Digit::Minus => {
            if buf.starts_with('-') {
                buf.remove(0);
            } else {
                buf.insert(0, '-');
            }
        }
        Digit::Dot if buf.contains('.') || buf.len() > 16 => {}
        Digit::Dot => buf.push('.'),
        Digit::N(_) if buf.len() > 16 => {}
        Digit::N(n) => buf.push(char::from(b'0' + n)),
    }
}

fn number(buf: &str) -> Option<f32> {
    let n: f32 = buf.parse().ok()?;
    n.is_finite().then_some(n)
}

/// The whole transform from the start: translation, or a turn or a scale about the centre.
fn total(kind: Kind, bounds: [f32; 4], origin: Pos2, pointer: Pos2, typed: Option<f32>, axis: Option<Axis>) -> Affine {
    let [x0, y0, x1, y1] = bounds;
    let (cx, cy) = ((x0 + x1) * 0.5, (y0 + y1) * 0.5);
    let about = |m: Affine| Affine::translate(cx, cy).mul(&m).mul(&Affine::translate(-cx, -cy));
    match kind {
        Kind::Grab => {
            let (dx, dy) = grab_delta(origin, pointer, typed, axis);
            Affine::translate(dx, dy)
        }
        Kind::Rotate => {
            // Positive degrees are counterclockwise on screen. The stage's y points down, and
            // `Affine::rotate` is clockwise there, so the sign flips.
            let deg = typed.unwrap_or_else(|| rotate_degrees(bounds, origin, pointer));
            about(Affine::rotate(-deg.to_radians()))
        }
        Kind::Scale => {
            let f = clamp_scale(typed.unwrap_or_else(|| scale_factor(bounds, origin, pointer, axis)));
            let (sx, sy) = match axis {
                Some(Axis::X) => (f, 1.0),
                Some(Axis::Y) => (1.0, f),
                None => (f, f),
            };
            about(Affine::scale(sx, sy))
        }
    }
}

fn grab_delta(origin: Pos2, pointer: Pos2, typed: Option<f32>, axis: Option<Axis>) -> (f32, f32) {
    if let Some(n) = typed {
        return match axis {
            Some(Axis::X) => (n, 0.0),
            Some(Axis::Y) => (0.0, n),
            None => {
                let d = pointer - origin;
                let len = d.length();
                if len < 0.5 { (n, 0.0) } else { (d.x / len * n, d.y / len * n) }
            }
        };
    }
    let mut d = pointer - origin;
    match axis {
        Some(Axis::X) => d.y = 0.0,
        Some(Axis::Y) => d.x = 0.0,
        None => {}
    }
    (d.x, d.y)
}

fn rotate_degrees(bounds: [f32; 4], origin: Pos2, pointer: Pos2) -> f32 {
    let [x0, y0, x1, y1] = bounds;
    let (cx, cy) = ((x0 + x1) * 0.5, (y0 + y1) * 0.5);
    let a0 = (origin.y - cy).atan2(origin.x - cx);
    let a1 = (pointer.y - cy).atan2(pointer.x - cx);
    let mut cw = a1 - a0;
    let pi = std::f32::consts::PI;
    cw = (cw + pi).rem_euclid(pi * 2.0) - pi;
    -cw.to_degrees()
}

fn scale_factor(bounds: [f32; 4], origin: Pos2, pointer: Pos2, axis: Option<Axis>) -> f32 {
    let [x0, y0, x1, y1] = bounds;
    let pivot = pos2((x0 + x1) * 0.5, (y0 + y1) * 0.5);
    match axis {
        None => {
            let d0 = (origin - pivot).length().max(8.0);
            (pointer - pivot).length() / d0
        }
        Some(Axis::X) => axis_factor(origin.x, pointer.x, pivot.x),
        Some(Axis::Y) => axis_factor(origin.y, pointer.y, pivot.y),
    }
}

fn axis_factor(start: f32, now: f32, pivot: f32) -> f32 {
    let den = start - pivot;
    if den.abs() < 8.0 { 1.0 + (now - start) / 100.0 } else { (now - pivot) / den }
}

fn clamp_scale(s: f32) -> f32 {
    if !s.is_finite() {
        return 1.0;
    }
    let a = s.abs().clamp(0.01, 1000.0);
    if s < 0.0 { -a } else { a }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use anicraft_engine::Session;
    use anicraft_engine::doc::sample;
    use anicraft_engine::geom::Vec2;
    use std::collections::BTreeMap;

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.05
    }

    #[test]
    fn a_free_key_stays_bare_and_a_taken_key_takes_shift() {
        let map = Keymap::new(&BTreeMap::new());
        assert_eq!(mods_for(&map, Key::G), Some(Modifiers::NONE));
        assert_eq!(mods_for(&map, Key::R), Some(Modifiers::SHIFT));
        assert_eq!(mods_for(&map, Key::S), Some(Modifiers::SHIFT));
    }

    #[test]
    fn typed_rotation_is_counterclockwise_and_typed_grab_is_a_distance() {
        let bounds = [0.0, 0.0, 20.0, 10.0];
        let origin = pos2(20.0, 5.0);
        let still = origin;
        let turn = total(Kind::Rotate, bounds, origin, still, Some(90.0), None);
        let p = turn.apply(Vec2::new(20.0, 5.0));
        assert!(near(p.x, 10.0) && near(p.y, -5.0), "90° counterclockwise, got {p:?}");

        let up = total(Kind::Rotate, bounds, origin, pos2(10.0, -5.0), None, None);
        let q = up.apply(Vec2::new(20.0, 5.0));
        assert!(near(q.x, 10.0) && near(q.y, -5.0), "the pointer above the centre is 90°, got {q:?}");

        let moved = total(Kind::Grab, bounds, origin, origin, Some(15.0), None);
        assert!(near(moved.m[4], 15.0) && near(moved.m[5], 0.0));
        let down = total(Kind::Grab, bounds, origin, origin, Some(15.0), Some(Axis::Y));
        assert!(near(down.m[4], 0.0) && near(down.m[5], 15.0));
        let locked = total(Kind::Grab, bounds, origin, pos2(30.0, 40.0), None, Some(Axis::X));
        assert!(near(locked.m[4], 10.0) && near(locked.m[5], 0.0));

        let grown = total(Kind::Scale, bounds, origin, origin, Some(2.0), None);
        let edge = grown.apply(Vec2::new(20.0, 5.0));
        assert!(near(edge.x, 30.0) && near(edge.y, 5.0), "twice as wide about the centre, got {edge:?}");
    }

    #[test]
    fn minus_toggles_the_sign_and_one_dot_is_kept() {
        let mut s = String::new();
        push_digit(&mut s, Digit::N(4));
        push_digit(&mut s, Digit::N(5));
        push_digit(&mut s, Digit::Minus);
        assert_eq!(s, "-45");
        push_digit(&mut s, Digit::Minus);
        assert_eq!(s, "45");
        push_digit(&mut s, Digit::Dot);
        push_digit(&mut s, Digit::Dot);
        push_digit(&mut s, Digit::N(5));
        assert_eq!(s, "45.5");
        assert!(near(number(&s).unwrap_or(0.0), 45.5));
    }

    fn editor() -> App {
        let ctx = egui::Context::default();
        App::with_context(&ctx, Session::new(sample::demo()), Some(std::path::PathBuf::from("unused.png")))
    }

    #[test]
    fn grab_moves_the_selection_and_a_right_click_puts_it_back() {
        let mut app = editor();
        assert!(app.run("draw.rect", json!({"x": 10.0, "y": 20.0, "w": 100.0, "h": 40.0})).is_some());
        let before = app.s.selection_bounds();
        assert!(begin(&mut app, Kind::Grab));
        assert!(matches!(app.tool, Tool::Select | Tool::Transform));
        track(&mut app, pos2(0.0, 0.0), false, false);
        track(&mut app, pos2(30.0, -8.0), false, false);
        let moved = app.s.selection_bounds();
        assert!(near(moved.min.x, before.min.x + 30.0), "x {} -> {}", before.min.x, moved.min.x);
        assert!(near(moved.min.y, before.min.y - 8.0), "y {} -> {}", before.min.y, moved.min.y);
        let steps = app.s.history_len();
        track(&mut app, pos2(30.0, -8.0), false, true);
        assert!(app.blender.is_none());
        let back = app.s.selection_bounds();
        assert!(near(back.min.x, before.min.x) && near(back.min.y, before.min.y));
        assert_eq!(app.s.history_len(), steps, "a cancelled grab is not an undo step");
    }

    #[test]
    fn a_typed_scale_confirms_as_one_undo_step() {
        let mut app = editor();
        assert!(app.run("draw.rect", json!({"x": 10.0, "y": 20.0, "w": 100.0, "h": 40.0})).is_some());
        let before = app.s.selection_bounds();
        assert!(begin(&mut app, Kind::Scale));
        assert_eq!(app.tool, Tool::Transform);
        let steps = app.s.history_len();
        track(&mut app, pos2(0.0, 0.0), false, false);
        if let Some(op) = app.blender.as_mut() {
            op.digits = "2".into();
        }
        track(&mut app, pos2(0.0, 0.0), true, false);
        assert!(app.blender.is_none());
        let after = app.s.selection_bounds();
        assert!(near(after.width(), before.width() * 2.0), "width {} -> {}", before.width(), after.width());
        assert_eq!(app.s.history_len(), steps + 1, "the scale is one undo step");
        assert_eq!(app.s.undo_label(), Some("Scale"));
    }
}
