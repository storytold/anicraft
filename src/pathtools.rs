//! Pen (P), Pencil (Shift+Y) and Subselection (A).
//!
//! The Pen's interaction model comes from PhotoCraft's Pen (`vector_ui.rs`): knots kept as
//! `[anchor, in, out]` while drawing, a press adds a knot, dragging pulls mirrored handles,
//! pressing the first knot closes the path, and the finished path is one command, so one undo
//! step. Added here: Alt breaks the handles, a click on the last knot (or a double-click)
//! finishes an open path, Backspace takes back the last knot, a tiny drag stays a corner, and
//! switching tools keeps the path instead of dropping it.
//!
//! Only view state lives here. Every document change is an engine command (`draw.path`,
//! `draw.freehand`, `path.*`), so agents and the CLI draw and edit exactly the same way.

use crate::app::{App, Drag, Tool};
use crate::theme::T;
use anicraft_engine::cmds_rawsub::{Handle, RawPoints};
use anicraft_engine::doc::{ElementId, ElementKind};
use anicraft_engine::geom::edit::{ANCHOR, C1, C2};
use anicraft_engine::geom::{Affine, Path, PathCmd, Vec2};
use egui::{Color32, Key, Modifiers, Pos2, Rect, Stroke, StrokeKind, pos2, vec2};
use serde_json::json;
use std::sync::Arc;

/// Screen ↔ stage mapping of the current frame.
#[derive(Clone, Copy)]
pub struct Xf {
    pub origin: Pos2,
    pub zoom: f32,
}

impl Xf {
    pub fn to_screen(self, p: Vec2) -> Pos2 {
        pos2(self.origin.x + p.x * self.zoom, self.origin.y + p.y * self.zoom)
    }
    pub fn to_stage(self, p: Pos2) -> Vec2 {
        Vec2::new((p.x - self.origin.x) / self.zoom, (p.y - self.origin.y) / self.zoom)
    }
    /// Stage units per screen point.
    pub fn unit(self) -> f32 {
        1.0 / self.zoom.max(1e-3)
    }
}

/// Pick radius for points and knots, in screen points.
const PICK: f32 = 6.0;
/// Drag threshold in screen points: a press that moved less than this is a click.
const DRAG_MIN: f32 = 2.0;

// ----- Pen ---------------------------------------------------------------------------------

/// What the press that started the current Pen drag took hold of (ADR 0115).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum PenGrip {
    /// A new knot: dragging pulls its mirrored handles (Alt: only the one ahead).
    #[default]
    New,
    /// The last knot: a click takes its handle ahead away (the next segment leaves it as a
    /// corner) or, when it has none, finishes the path; a drag pulls a new handle ahead alone.
    Last { moved: bool },
    /// The end of the path being continued: a drag pulls its handle ahead, a click does nothing.
    Out,
    /// Alt on the last knot: its handles are drawn again. `was` is the knot as it was, drawn
    /// as a ghost until the button is let go.
    Adjust { was: [Vec2; 3] },
}

/// Pen path under construction: knots as `[anchor, in, out]` in stage space.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PenPath {
    pub knots: Vec<[Vec2; 3]>,
    pub dragging: bool,
    pub grip: PenGrip,
    /// The open drawing object this path continues, and whether from its start (ADR 0115):
    /// the first knot is that end, and finishing extends the object instead of adding one.
    pub extends: Option<(ElementId, bool)>,
}

impl PenPath {
    /// SVG path data for `draw.path`. Straight where both handles sit on their anchors.
    pub fn to_svg(&self, closed: bool) -> String {
        let mut d = String::new();
        let Some(first) = self.knots.first() else { return d };
        d.push_str(&format!("M{} {}", first[0].x, first[0].y));
        let seg = |d: &mut String, a: &[Vec2; 3], b: &[Vec2; 3]| {
            if a[2] == a[0] && b[1] == b[0] {
                d.push_str(&format!(" L{} {}", b[0].x, b[0].y));
            } else {
                d.push_str(&format!(" C{} {} {} {} {} {}", a[2].x, a[2].y, b[1].x, b[1].y, b[0].x, b[0].y));
            }
        };
        for w in self.knots.windows(2) {
            seg(&mut d, &w[0], &w[1]);
        }
        if closed {
            if let Some(last) = self.knots.last()
                && (last[2] != last[0] || first[1] != first[0])
            {
                seg(&mut d, last, first);
            }
            d.push_str(" Z");
        }
        d
    }
}

/// The modifier keys the Pen reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PenMods {
    /// Break the handle pair; on the last knot, draw its handles again; on a path, add a point.
    pub alt: bool,
    /// Keep the new segment, or the handle, on a multiple of 45°.
    pub shift: bool,
    /// Ctrl (⌘ on macOS): select the path under the pointer, to show its points.
    pub command: bool,
}

/// What a Pen click here would do: the mark drawn by the pointer, as Animate's Pen cursors
/// tell it (issue #319).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PenHover {
    /// Place a point.
    Place,
    /// Close the path on its first point.
    Close,
    /// Finish the open path (the last point, which has no handle ahead).
    Finish,
    /// Take the handles away: the last point's handle ahead, or a smooth point's pair.
    Corner,
    /// Draw the last point's handles again (Alt).
    Adjust,
    /// Add a point on the path.
    Add,
    /// Delete the point.
    Delete,
    /// Continue the open path from this end.
    Continue,
}

/// `p` moved onto the nearest multiple of 45° around `from` (Shift), at the distance the
/// pointer is along that direction.
pub fn constrain_45(from: Vec2, p: Vec2) -> Vec2 {
    let d = p - from;
    if !d.is_finite() || d.length() < 1e-6 {
        return p;
    }
    let step = std::f32::consts::FRAC_PI_4;
    let a = (d.y.atan2(d.x) / step).round() * step;
    let dir = Vec2::new(a.cos(), a.sin());
    from + dir * (d.x * dir.x + d.y * dir.y)
}

/// Handle vector pulled from anchor `a` to stage point `p` (Shift keeps 45° steps). `None`
/// while the pointer is within [`DRAG_MIN`] screen points of `a` (so a click stays a click).
fn drag_handle(a: Vec2, p: Vec2, zoom: f32, shift: bool) -> Option<Vec2> {
    if a.distance(p) * zoom < DRAG_MIN {
        return None;
    }
    let q = if shift { constrain_45(a, p) } else { p };
    let d = q - a;
    (d.is_finite() && d != Vec2::ZERO).then_some(d)
}

/// A knot `[anchor, in, out]` with mirrored handles at `anchor ∓ handle`.
fn mirrored_knot(a: Vec2, handle: Vec2) -> [Vec2; 3] {
    [a, a - handle, a + handle]
}

/// A point of a path that shows its points, under the Pen.
#[derive(Clone, Copy, Debug, PartialEq)]
enum PenTarget {
    /// An anchor of a selected drawing object: smooth (it has a handle), and the end of an
    /// open path it is, if any (`Some(true)`: the start).
    Anchor { id: ElementId, cmd: usize, smooth: bool, end: Option<bool> },
    /// A shared anchor of the selected raw shapes.
    RawAnchor { at: Vec2, smooth: bool },
    /// The outline of a selected shape, drawing object or raw, between its points.
    Segment,
}

/// Whether `path` is one open run of segments, the kind the Pen can continue.
fn is_open(path: &Path) -> bool {
    path.cmds.len() >= 2 && matches!(path.cmds.first(), Some(PathCmd::MoveTo(_))) && !path.cmds.iter().skip(1).any(|c| matches!(c, PathCmd::MoveTo(_) | PathCmd::Close))
}

/// What of the paths showing their points (the selected shapes on the active layer) is at
/// screen point `at`.
fn pen_target(app: &App, at: Pos2, xf: Xf) -> Option<PenTarget> {
    let near = |q: Vec2| xf.to_screen(q).distance(at) <= PICK;
    let sp = xf.to_stage(at);
    let slop = PICK * xf.unit();
    let shapes = selected_shapes(app);
    for (id, path, m) in &shapes {
        if let Some((cmd, q)) = path.editable_anchors().into_iter().find(|(_, q)| near(m.apply(*q))) {
            let smooth = shown_handles(path, [cmd].into_iter()).iter().any(|&(c, slot, _)| path.point(c, slot).is_some_and(|h| h.distance(q) > 1e-4));
            let end = if !is_open(path) {
                None
            } else if cmd == 0 {
                Some(true)
            } else if cmd + 1 == path.cmds.len() {
                Some(false)
            } else {
                None
            };
            return Some(PenTarget::Anchor { id: *id, cmd, smooth, end });
        }
    }
    let raw = raw_selected(app);
    if !raw.is_empty() {
        let pts = app.s.raw_points(&raw);
        if let Some(q) = pts.anchors.iter().copied().filter(|q| near(*q)).min_by(|a, b| xf.to_screen(*a).distance(at).total_cmp(&xf.to_screen(*b).distance(at))) {
            let smooth = pts.handles.iter().any(|h| h.anchor.distance(q) <= SNAP && h.at.distance(h.anchor) > SNAP);
            return Some(PenTarget::RawAnchor { at: q, smooth });
        }
    }
    if shapes.iter().any(|(_, path, m)| path.nearest_segment(sp, m).is_some_and(|(_, _, d)| d <= slop)) {
        return Some(PenTarget::Segment);
    }
    let on_raw = app.s.visible_elements().iter().filter(|e| raw.contains(&e.id)).take(512).any(|e| match &e.kind {
        ElementKind::Shape(sh) => sh.path.nearest_segment(sp, &e.transform).is_some_and(|(_, _, d)| d <= slop),
        _ => false,
    });
    on_raw.then_some(PenTarget::Segment)
}

/// Where the path being continued ends on its far side (stage space), if it still exists.
fn far_end(app: &App, pen: &PenPath) -> Option<Vec2> {
    let (id, at_start) = pen.extends?;
    let els = app.s.visible_elements();
    let e = els.iter().find(|e| e.id == id)?;
    let ElementKind::Shape(sh) = &e.kind else { return None };
    let q = if at_start { sh.path.point(sh.path.cmds.len().checked_sub(1)?, ANCHOR) } else { sh.path.point(0, ANCHOR) }?;
    Some(e.transform.apply(q))
}

/// What a click at screen point `at` would do (no button down).
pub fn pen_hover(app: &App, at: Pos2, xf: Xf, mods: PenMods) -> PenHover {
    let p = xf.to_stage(at);
    let tol = PICK * xf.unit();
    if let Some(pen) = app.pen.as_ref().filter(|pen| !pen.knots.is_empty()) {
        let n = pen.knots.len();
        if n >= 2 && far_end(app, pen).is_some_and(|q| q.distance(p) <= tol) {
            return PenHover::Close;
        }
        if n >= 2 && pen.extends.is_none() && pen.knots.first().is_some_and(|k| k[0].distance(p) <= tol) {
            return PenHover::Close;
        }
        if let Some(k) = pen.knots.last().filter(|k| k[0].distance(p) <= tol) {
            return match () {
                _ if mods.alt => PenHover::Adjust,
                _ if k[2] != k[0] => PenHover::Corner,
                _ if n >= 2 => PenHover::Finish,
                _ => PenHover::Place,
            };
        }
        return PenHover::Place;
    }
    if mods.command {
        return PenHover::Place;
    }
    match pen_target(app, at, xf) {
        Some(PenTarget::Anchor { end: Some(_), .. }) => PenHover::Continue,
        Some(PenTarget::Anchor { smooth: true, .. } | PenTarget::RawAnchor { smooth: true, .. }) => PenHover::Corner,
        Some(PenTarget::Anchor { .. } | PenTarget::RawAnchor { .. }) => PenHover::Delete,
        Some(PenTarget::Segment) => PenHover::Add,
        None => PenHover::Place,
    }
}

/// A Pen press on a path that shows its points, with no path being drawn (ADR 0115): an end
/// of an open drawing object is picked up to draw on from, a smooth point becomes a corner, a
/// corner is deleted, and the outline between points gets a point. False when the press was
/// on none of them (it starts a new path).
fn pen_press_on_path(app: &mut App, at: Pos2, xf: Xf) -> bool {
    let undo = app.undo_key();
    match pen_target(app, at, xf) {
        Some(PenTarget::Anchor { id, cmd, end: Some(at_start), .. }) => {
            let Some((_, path, m)) = selected_shapes(app).into_iter().find(|(e, _, _)| *e == id) else { return false };
            // The end as a knot: its handle behind is the last segment's, so a handle pulled
            // ahead can be seen against it.
            let (Some(a), behind) = (path.point(cmd, ANCHOR), if at_start { path.point(1, C1) } else { path.point(cmd, C2) }) else { return false };
            let a = m.apply(a);
            let knot = [a, behind.map_or(a, |h| m.apply(h)), a];
            app.pen = Some(PenPath { knots: vec![knot], dragging: true, grip: PenGrip::Out, extends: Some((id, at_start)) });
            app.status_ok(tr!("Continuing the path: click to add points, Enter to finish."));
            true
        }
        Some(PenTarget::Anchor { id, cmd, smooth: true, .. }) => {
            if app.run("path.convert_point", json!({"id": id.0, "cmd": cmd, "smooth": false})).is_some() {
                app.status_ok(tr!("Made a corner point. Click it again to delete it. {undo} to undo.", undo = undo));
            }
            true
        }
        Some(PenTarget::Anchor { id, cmd, .. }) => {
            if app.run("path.delete_points", json!({"id": id.0, "cmds": [cmd]})).is_some() {
                app.status_ok(trn!("Deleted {n} point. {undo} to undo.", "Deleted {n} points. {undo} to undo.", 1, undo = undo));
            }
            true
        }
        Some(PenTarget::RawAnchor { at: q, smooth }) => {
            app.raw_anchors = vec![q];
            if smooth {
                raw_convert(app, false);
                app.status_ok(tr!("Made a corner point. Click it again to delete it. {undo} to undo.", undo = undo));
            } else {
                raw_delete(app);
            }
            app.raw_anchors.clear();
            true
        }
        Some(PenTarget::Segment) => add_point_on_selected(app, at, xf),
        None => false,
    }
}

/// Ctrl-click (⌘ on macOS) with the Pen: the path there is selected, so its points show and
/// the Pen can edit them; over nothing, the selection is dropped.
fn pen_select(app: &mut App, at: Pos2, xf: Xf) {
    let sp = xf.to_stage(at);
    if !raw_show_at(app, sp, false, xf) {
        app.run("element.pick", json!({"x": sp.x, "y": sp.y}));
    }
}

/// The Pen's button went down at screen point `at`.
pub fn pen_press(app: &mut App, at: Pos2, xf: Xf, mods: PenMods) {
    let p = xf.to_stage(at);
    let idle = app.pen.as_ref().is_none_or(|pen| pen.knots.is_empty());
    if idle {
        app.pen = None;
        // Alt-click on a path adds a point to it: the motion path first (ADR 0038), then a
        // selected shape's outline.
        if mods.alt && (crate::motion_path::pen_alt_click(app, at, xf) || add_point_on_selected(app, at, xf)) {
            return;
        }
        if mods.command {
            pen_select(app, at, xf);
            return;
        }
        if !mods.alt && pen_press_on_path(app, at, xf) {
            return;
        }
    }
    let tol = PICK * xf.unit();
    let far = app.pen.as_ref().and_then(|pen| far_end(app, pen));
    let pen = app.pen.get_or_insert_with(PenPath::default);
    let n = pen.knots.len();
    // The other end of the path being continued: a point there, and the path is closed.
    if let Some(q) = far.filter(|q| n >= 2 && q.distance(p) <= tol) {
        pen.knots.push([q; 3]);
        commit_pen(app, true);
        return;
    }
    if n >= 2 && pen.extends.is_none() && pen.knots.first().is_some_and(|k| k[0].distance(p) <= tol) {
        commit_pen(app, true);
        return;
    }
    if let Some(k) = pen.knots.last().filter(|k| k[0].distance(p) <= tol) {
        pen.grip = if mods.alt { PenGrip::Adjust { was: *k } } else { PenGrip::Last { moved: false } };
        pen.dragging = true;
        return;
    }
    // A new knot snaps to points, lines and fill edges (ADR 0045); handles never do. Shift
    // keeps the segment on a multiple of 45° from the knot before, and wins over snapping.
    let prev = pen.knots.last().map(|k| k[0]);
    let p = match prev.filter(|_| mods.shift) {
        Some(from) => {
            app.snap_ring = None;
            constrain_45(from, p)
        }
        None => {
            let q = crate::stage::snap_draw(app, pos2(p.x, p.y), xf.zoom);
            Vec2::new(q.x, q.y)
        }
    };
    let Some(pen) = app.pen.as_mut() else { return };
    pen.knots.push([p; 3]);
    pen.grip = PenGrip::New;
    pen.dragging = true;
}

/// The pointer moved to screen point `at` with the Pen's button down.
pub fn pen_drag(app: &mut App, at: Pos2, xf: Xf, mods: PenMods) {
    let Some(pen) = app.pen.as_mut().filter(|pen| pen.dragging) else { return };
    let grip = pen.grip;
    let Some(k) = pen.knots.last_mut() else { return };
    let a = k[0];
    let p = xf.to_stage(at);
    let handle = drag_handle(a, p, xf.zoom, mods.shift);
    let constrained = if mods.shift { constrain_45(a, p) } else { p };
    match grip {
        PenGrip::New => match handle {
            None => *k = [a; 3],
            Some(d) if mods.alt => k[2] = a + d,
            Some(d) => *k = mirrored_knot(a, d),
        },
        PenGrip::Last { moved: was_moved } => {
            if handle.is_some() || was_moved {
                k[2] = constrained;
                pen.grip = PenGrip::Last { moved: true };
            }
        }
        PenGrip::Out => {
            if let Some(d) = handle {
                k[2] = a + d;
            }
        }
        PenGrip::Adjust { was } => {
            *k = handle.map_or(was, |d| mirrored_knot(a, d));
        }
    }
}

/// The Pen's button was let go.
pub fn pen_release(app: &mut App) {
    let Some(pen) = app.pen.as_mut() else { return };
    if !pen.dragging {
        return;
    }
    pen.dragging = false;
    let grip = std::mem::take(&mut pen.grip);
    if grip != (PenGrip::Last { moved: false }) {
        return;
    }
    // A click on the last knot: its handle ahead goes (the next segment starts as a corner,
    // issue #319); with none to take, the click finishes the path, as a double-click does.
    let n = pen.knots.len();
    match pen.knots.last_mut() {
        Some(k) if k[2] != k[0] => {
            k[2] = k[0];
            app.status_ok(tr!("The next segment starts as a corner. Click the point again, or press Enter, to finish."));
        }
        Some(_) if n >= 2 => commit_pen(app, false),
        _ => {}
    }
}

/// Pointer handling for the Pen. Runs on raw press/release (not egui's drag threshold), so a
/// quick click places a point exactly where it was pressed.
pub fn pen_input(app: &mut App, ui: &egui::Ui, resp: &egui::Response, xf: Xf) {
    let (pressed, down, m) = ui.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_down(), i.modifiers));
    let mods = PenMods { alt: m.alt, shift: m.shift, command: m.command };
    // Off the stage there is nothing to do: a button let go out there is seen when the
    // pointer comes back.
    let Some(pos) = resp.interact_pointer_pos().or_else(|| resp.hover_pos()) else { return };
    if pressed && resp.hovered() {
        pen_press(app, pos, xf, mods);
    } else if down {
        pen_drag(app, pos, xf, mods);
    } else {
        pen_release(app);
    }
}

/// Finishes the Pen path as a shape (one undo step), or as more of the path it continues.
/// Fewer than two knots are dropped.
pub fn commit_pen(app: &mut App, closed: bool) {
    let Some(pen) = app.pen.take() else { return };
    if pen.knots.len() < 2 {
        return;
    }
    match pen.extends {
        // The object stays selected: its points go on showing under the Pen.
        Some((id, at_start)) => {
            app.run("path.extend", json!({"id": id.0, "d": pen.to_svg(false), "end": if at_start { "start" } else { "end" }, "close": closed}));
        }
        None => {
            app.draw("draw.path", json!({"d": pen.to_svg(closed)}));
        }
    }
}

// ----- Pencil ------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PencilMode {
    #[default]
    Smooth,
    Straighten,
    Ink,
}

impl PencilMode {
    /// The English name, translated where it is drawn (`crate::i18n::t`).
    pub fn label(self) -> &'static str {
        match self {
            PencilMode::Smooth => tr_noop!("Smooth"),
            PencilMode::Straighten => tr_noop!("Straighten"),
            PencilMode::Ink => tr_noop!("Ink"),
        }
    }
    /// The English hint, translated where it is drawn (`crate::i18n::t`).
    pub fn hint(self) -> &'static str {
        match self {
            PencilMode::Smooth => tr_noop!("Fits smooth curves to the stroke, keeping sharp corners."),
            PencilMode::Straighten => tr_noop!("Turns the stroke into straight segments."),
            PencilMode::Ink => tr_noop!("Follows the hand closely with little smoothing."),
        }
    }
    fn id(self) -> &'static str {
        match self {
            PencilMode::Smooth => "smooth",
            PencilMode::Straighten => "straighten",
            PencilMode::Ink => "ink",
        }
    }
}

/// Adds a pointer sample to the stroke (closer samples than half a screen point add nothing).
pub fn pencil_sample(app: &mut App, p: Vec2, xf: Xf) {
    if app.pencil.last().is_none_or(|l| l.distance(p) * xf.zoom >= 0.5) && app.pencil.len() < 100_000 {
        app.pencil.push(p);
    }
}

/// Sends the stroke to the engine; smoothing is relative to the screen, so a stroke looks the
/// same at any zoom.
pub fn commit_pencil(app: &mut App, xf: Xf) {
    let pts = std::mem::take(&mut app.pencil);
    if pts.len() < 2 {
        return;
    }
    let points: Vec<[f32; 2]> = pts.iter().map(|p| [p.x, p.y]).collect();
    app.draw("draw.freehand", json!({"points": points, "mode": app.pencil_mode.id(), "tolerance": 1.5 * xf.unit()}));
}

// ----- Subselection --------------------------------------------------------------------------

/// A selected point: element, command, slot.
pub type PointRef = (ElementId, usize, u8);

/// Selected shapes on the active layer with their path and transform, and selected text on a
/// path with the path it follows.
fn selected_shapes(app: &App) -> Vec<(ElementId, Arc<Path>, Affine)> {
    app.s
        .visible_elements()
        .into_iter()
        .filter(|e| app.s.selection.contains(app.s.layer, e.id))
        .filter_map(|e| match &e.kind {
            // Raw shapes show their shared points instead (ADR 0042, below).
            ElementKind::Shape(sh) if sh.merge => None,
            ElementKind::Shape(sh) => Some((e.id, sh.path.clone(), e.transform)),
            // Text on a path shows the points of the path it follows (ADR 0107): the same
            // point commands edit them, and the glyphs follow.
            ElementKind::Text(t) => t.on_path.as_ref().map(|on| (e.id, Arc::new(on.path.clone()), e.transform)),
            ElementKind::Symbol { .. } | ElementKind::Camera | ElementKind::Group { .. } => None,
        })
        .take(64)
        .collect()
}

/// What a Subselection Alt-drag is pulling curve handles out of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PullTarget {
    /// Anchor `cmd` of drawing object (or text on a path) `id`.
    Object { id: ElementId, cmd: usize },
    /// Shared anchor of raw shapes on the active layer.
    Raw,
}

/// Handles shown for the selected anchors: `(cmd, slot, anchor cmd)`. Zero-length handles
/// sitting on their anchor are omitted so they don't shadow the anchor itself.
fn shown_handles(path: &Path, anchors: impl Iterator<Item = usize>) -> Vec<(usize, u8, usize)> {
    let mut v = Vec::new();
    for a in anchors {
        let Some(ap) = path.point(a, ANCHOR) else { continue };
        let (i, o) = path.anchor_segments(a);
        if let Some(i) = i.filter(|&i| matches!(path.cmds.get(i), Some(PathCmd::CubicTo(..))) && path.point(i, C2).is_some_and(|h| h.distance(ap) > 1e-4)) {
            v.push((i, C2, a));
        }
        if let Some(o) = o.filter(|&o| matches!(path.cmds.get(o), Some(PathCmd::CubicTo(..)) | Some(PathCmd::QuadTo(..))) && path.point(o, C1).is_some_and(|h| h.distance(ap) > 1e-4)) {
            v.push((o, C1, a));
        }
    }
    v
}

/// Drops point references that no longer exist (after undo, deselection or a frame change).
pub fn upkeep(app: &mut App) {
    raw_upkeep(app);
    if app.points.is_empty() {
        return;
    }
    let shapes = selected_shapes(app);
    app.points.retain(|(id, cmd, slot)| shapes.iter().any(|(e, p, _)| e == id && p.point(*cmd, *slot).is_some()));
}

/// The point under screen position `at`: shown handles first (they sit on top), then anchors.
fn point_at(app: &App, at: Pos2, xf: Xf) -> Option<PointRef> {
    let near = |q: Vec2, m: &Affine| xf.to_screen(m.apply(q)).distance(at) <= PICK;
    for (id, path, m) in selected_shapes(app) {
        let sel = app.points.iter().filter(|p| p.0 == id && p.2 == ANCHOR).map(|p| p.1);
        for (cmd, slot, _) in shown_handles(&path, sel) {
            if path.point(cmd, slot).is_some_and(|q| near(q, &m)) {
                return Some((id, cmd, slot));
            }
        }
        if let Some((cmd, _)) = path.editable_anchors().into_iter().find(|(_, q)| near(*q, &m)) {
            return Some((id, cmd, ANCHOR));
        }
    }
    None
}

fn anchor_stage_pos(app: &App, id: ElementId, cmd: usize) -> Option<Vec2> {
    selected_shapes(app).into_iter().find(|(e, _, _)| *e == id).and_then(|(_, path, m)| path.point(cmd, ANCHOR).map(|q| m.apply(q)))
}

/// Whether screen point `at` is over an anchor (not a handle) that Subselection + Alt can
/// pull curve handles out of.
pub fn subselect_alt_anchor(app: &App, at: Pos2, xf: Xf) -> bool {
    let pts = raw_shown(app);
    match raw_point_at(app, &pts, at, xf) {
        Some(RawHit::Handle(_)) => return false,
        Some(RawHit::Anchor(_)) => return true,
        None => {}
    }
    if let Some(pt) = point_at(app, at, xf) {
        return pt.2 == ANCHOR;
    }
    let sp = xf.to_stage(at);
    matches!(app.s.raw_grab(sp, 3.0 * xf.unit(), GRAB_POINT * xf.unit()), Some((_, anicraft_engine::cmds_raw::Grab::Point { .. })))
}

/// Press with the Subselection tool: grab a point (Alt on an anchor pulls out curve handles),
/// else a shape (moves it whole, as Animate does when you drag an outline), else start a
/// marquee over points.
pub fn subselect_drag_start(app: &mut App, press: Pos2, shift: bool, alt: bool, xf: Xf) -> Drag {
    let sp = xf.to_stage(press);
    if let Some(d) = raw_drag_start(app, press, shift, alt, xf) {
        return d;
    }
    if let Some(pt) = point_at(app, press, xf) {
        if alt && pt.2 == ANCHOR
            && let Some(anchor) = anchor_stage_pos(app, pt.0, pt.1)
        {
            app.points = vec![pt];
            app.s.begin_gesture(tr_noop!("Convert Anchor Point"));
            return Drag::PullHandle { target: PullTarget::Object { id: pt.0, cmd: pt.1 }, anchor, moved: false };
        }
        if !app.points.contains(&pt) {
            if !shift {
                app.points.clear();
            }
            app.points.push(pt);
        }
        // Undo labels stay English in the history and are translated where they show.
        app.s.begin_gesture(tr_noop!("Move Points"));
        return Drag::Points { last: pos2(sp.x, sp.y) };
    }
    if let Some((layer, el)) = app.s.hit_test(sp, 3.0 * xf.unit()) {
        if !app.s.selection.contains(layer, el) {
            app.run("element.pick", json!({"x": sp.x, "y": sp.y, "add": shift}));
            app.points.clear();
        }
        app.s.begin_gesture(tr_noop!("Move"));
        return Drag::Move { last: pos2(sp.x, sp.y) };
    }
    if !shift {
        app.points.clear();
        app.raw_anchors.clear();
        app.raw_ends.clear();
    }
    // A marquee shows the points of the shapes it touches on the active layer and selects the
    // points inside it; it never cuts (that is the Selection tool's).
    Drag::PointMarquee { start: pos2(sp.x, sp.y), cur: pos2(sp.x, sp.y) }
}

/// Updates an anchor's curve handles while Alt-dragging from it with Subselection. Returns the
/// raw element ids changed (for merging on release), if any.
pub fn pull_handle(app: &mut App, target: PullTarget, anchor: Vec2, moved: &mut bool, cur: Vec2, shift: bool, xf: Xf) -> Option<serde_json::Value> {
    let h = if *moved {
        let q = if shift { constrain_45(anchor, cur) } else { cur };
        let d = q - anchor;
        (d.is_finite() && d != Vec2::ZERO).then_some(d)?
    } else {
        drag_handle(anchor, cur, xf.zoom, shift)?
    };
    *moved = true;
    match target {
        PullTarget::Object { id, cmd } => {
            app.run("path.convert_point", json!({"id": id.0, "cmd": cmd, "smooth": true, "handle": [h.x, h.y]}));
            None
        }
        PullTarget::Raw => {
            let v = app.run("raw.convert_points", json!({"points": [[anchor.x, anchor.y]], "smooth": true, "handle": [h.x, h.y], "merge": false}))?;
            // A point where three or more lines meet has no one curve to pull handles from.
            if v["converted"].as_u64() == Some(0) {
                app.status_ok(tr!("Handles can't be pulled out of a point where more than two lines meet."));
            }
            Some(v["ids"].clone())
        }
    }
}

/// Moves the selected points by a stage-space delta (inside the open gesture). A handle moved
/// alone at a smooth anchor turns the other one with it unless `smooth` is false (Alt).
pub fn move_points(app: &mut App, d: Vec2, smooth: bool) {
    let mut ids: Vec<ElementId> = app.points.iter().map(|p| p.0).collect();
    ids.dedup();
    for id in ids {
        let pts: Vec<[u64; 2]> = app.points.iter().filter(|p| p.0 == id).map(|p| [p.1 as u64, u64::from(p.2)]).collect();
        app.run("path.move_points", json!({"id": id.0, "points": pts, "dx": d.x, "dy": d.y, "smooth": smooth}));
    }
}

/// The Subselection marquee: the shapes it touches on the active layer are selected whole (raw
/// ones with everything connected to them, ADR 0042) and the anchors inside it are selected.
pub fn marquee_points(app: &mut App, a: Pos2, b: Pos2, shift: bool) {
    let r = Rect::from_two_pos(a, b);
    if !shift {
        app.points.clear();
        app.raw_anchors.clear();
        app.raw_ends.clear();
    }
    if let Some(v) = app.run("raw.subselect_rect", json!({"x0": a.x, "y0": a.y, "x1": b.x, "y1": b.y, "add": shift})) {
        for q in positions(&v["anchors"]) {
            if !app.raw_anchors.iter().any(|p| p.distance(q) <= SNAP) {
                app.raw_anchors.push(q);
            }
        }
    }
    for (id, path, m) in selected_shapes(app) {
        for (cmd, q) in path.editable_anchors() {
            let s = m.apply(q);
            if r.contains(pos2(s.x, s.y)) && !app.points.contains(&(id, cmd, ANCHOR)) {
                app.points.push((id, cmd, ANCHOR));
            }
        }
    }
}

/// Click with the Subselection tool: a point toggles into the point selection, a shape is
/// selected (showing its points), empty space clears both.
pub fn subselect_click(app: &mut App, at: Pos2, shift: bool, xf: Xf) {
    if raw_click(app, at, shift, xf) {
        return;
    }
    if let Some(pt) = point_at(app, at, xf) {
        if shift {
            if let Some(i) = app.points.iter().position(|p| *p == pt) {
                app.points.remove(i);
            } else {
                app.points.push(pt);
            }
        } else {
            app.points = vec![pt];
        }
        return;
    }
    let sp = xf.to_stage(at);
    app.run("element.pick", json!({"x": sp.x, "y": sp.y, "add": shift}));
    app.points.clear();
    app.raw_anchors.clear();
    app.raw_ends.clear();
}

/// Double-click on a selected shape's outline adds a point there and selects it.
pub fn subselect_double_click(app: &mut App, at: Pos2, xf: Xf) {
    add_point_on_selected(app, at, xf);
}

/// Adds a point where screen point `at` is on a selected shape's outline and selects it (the
/// Subselection double-click, the Pen's Alt-click). False when it wasn't on one.
fn add_point_on_selected(app: &mut App, at: Pos2, xf: Xf) -> bool {
    let sp = xf.to_stage(at);
    for (id, _, _) in selected_shapes(app) {
        let r = app.s.execute("path.add_point", &json!({"id": id.0, "x": sp.x, "y": sp.y, "slop": PICK * xf.unit()}));
        if let Ok(v) = r {
            app.points = v["cmd"].as_u64().map(|c| vec![(id, c as usize, ANCHOR)]).unwrap_or_default();
            app.motion_anchors.clear();
            app.status_ok(tr!("Added a point. {undo} to undo.", undo = app.undo_key()));
            return true;
        }
    }
    raw_add_point(app, at, xf)
}

/// Adds a point on the raw line or fill edge at screen point `at` (ADR 0045): every raw shape with
/// that edge gets it, and it shows selected with everything connected to it. False when there
/// is no raw edge there.
fn raw_add_point(app: &mut App, at: Pos2, xf: Xf) -> bool {
    let sp = xf.to_stage(at);
    let Ok(v) = app.s.execute("raw.add_point", &json!({"x": sp.x, "y": sp.y, "slop": PICK * xf.unit()})) else { return false };
    let q = Vec2::new(v["x"].as_f64().unwrap_or(0.0) as f32, v["y"].as_f64().unwrap_or(0.0) as f32);
    // Show the shapes on it, as a Subselection click there would.
    if let Some(r) = app.run("raw.subselect", json!({"x": q.x, "y": q.y, "slop": 2.0 * xf.unit()})) {
        app.raw_ends = positions(&r["ends"]);
    }
    app.points.clear();
    app.motion_anchors.clear();
    app.raw_anchors = vec![q];
    app.status_ok(tr!("Added a point. {undo} to undo.", undo = app.undo_key()));
    true
}

/// Anchors in the point selection, grouped per element.
fn selected_anchors(app: &App) -> Vec<(ElementId, Vec<u64>)> {
    let mut out: Vec<(ElementId, Vec<u64>)> = Vec::new();
    for &(id, cmd, slot) in &app.points {
        if slot != ANCHOR {
            continue;
        }
        match out.iter_mut().find(|(e, _)| *e == id) {
            Some((_, v)) => v.push(cmd as u64),
            None => out.push((id, vec![cmd as u64])),
        }
    }
    out
}

pub fn delete_points(app: &mut App) {
    for (id, cmds) in selected_anchors(app) {
        app.run("path.delete_points", json!({"id": id.0, "cmds": cmds}));
    }
    app.points.clear();
}

/// Deletes the selected raw anchors (ADR 0045): the shapes that share each are joined across it.
fn raw_delete(app: &mut App) {
    let pts: Vec<[f32; 2]> = app.raw_anchors.iter().map(|q| [q.x, q.y]).collect();
    let Some(v) = app.run("raw.delete_points", json!({"points": pts})) else { return };
    app.raw_anchors.clear();
    app.raw_ends.clear();
    let (n, kept) = (v["deleted"].as_u64().unwrap_or(0) as usize, v["kept"].as_u64().unwrap_or(0) as usize);
    let undo = app.undo_key();
    app.status_ok(if kept > 0 {
        trn!("Deleted {n} point; kept {kept} where three or more lines meet. {undo} to undo.", "Deleted {n} points; kept {kept} where three or more lines meet. {undo} to undo.", n, kept = kept, undo = undo)
    } else {
        trn!("Deleted {n} point. {undo} to undo.", "Deleted {n} points. {undo} to undo.", n, undo = undo)
    });
}

/// Makes the selected raw anchors smooth or corners, in every shape that has them.
fn raw_convert(app: &mut App, smooth: bool) {
    let pts: Vec<[f32; 2]> = app.raw_anchors.iter().map(|q| [q.x, q.y]).collect();
    app.run("raw.convert_points", json!({"points": pts, "smooth": smooth}));
}

pub fn convert_points(app: &mut App, smooth: bool) {
    for (id, cmds) in selected_anchors(app) {
        for cmd in cmds {
            app.run("path.convert_point", json!({"id": id.0, "cmd": cmd, "smooth": smooth}));
        }
    }
}

/// Right-click menu for selected points (of drawing objects, or raw anchors, ADR 0045); false
/// when no anchor is selected.
pub fn point_menu(app: &mut App, ui: &mut egui::Ui) -> bool {
    let raw = !app.raw_anchors.is_empty();
    if app.tool != Tool::Subselect || (!raw && selected_anchors(app).is_empty()) {
        return false;
    }
    if ui.button(tr!("Make Smooth")).clicked() {
        if raw { raw_convert(app, true) } else { convert_points(app, true) }
        ui.close();
    }
    if ui.button(tr!("Make Corner")).clicked() {
        if raw { raw_convert(app, false) } else { convert_points(app, false) }
        ui.close();
    }
    ui.separator();
    let label = if raw { trn!("Delete Point", "Delete Points", app.raw_anchors.len()) } else { tr!("Delete Point").to_string() };
    if ui.button(label).clicked() {
        if raw { raw_delete(app) } else { delete_points(app) }
        ui.close();
    }
    true
}

// ----- Subselection on raw shapes (ADR 0042) -------------------------------------------------

/// Two positions this close are one shared point.
const SNAP: f32 = anicraft_engine::geom::planar::SNAP;
/// How near (screen pixels) a press takes an anchor of a raw shape that isn't shown yet.
const GRAB_POINT: f32 = 6.0;

fn is_raw(e: &anicraft_engine::doc::Element) -> bool {
    matches!(&e.kind, ElementKind::Shape(sh) if sh.merge)
}

/// The selected raw shapes on the active layer: Subselection shows their points.
fn raw_selected(app: &App) -> Vec<ElementId> {
    app.s.visible_elements().iter().filter(|e| app.s.selection.contains(app.s.layer, e.id) && is_raw(e)).map(|e| e.id).collect()
}

/// The anchors and handles of the selected raw shapes.
pub fn raw_shown(app: &App) -> RawPoints {
    let ids = raw_selected(app);
    if ids.is_empty() { RawPoints::default() } else { app.s.raw_points(&ids) }
}

/// The handles that show: those of the selected anchors' curves and of the clicked edge.
fn visible_handles<'a>(app: &'a App, pts: &'a RawPoints) -> impl Iterator<Item = &'a Handle> + 'a {
    pts.handles.iter().filter(|h| app.raw_anchors.iter().chain(&app.raw_ends).any(|a| a.distance(h.anchor) <= SNAP))
}

fn positions(v: &serde_json::Value) -> Vec<Vec2> {
    v.as_array().into_iter().flatten().filter_map(|p| Some(Vec2::new(p.get(0)?.as_f64()? as f32, p.get(1)?.as_f64()? as f32))).collect()
}

/// What of the shown raw points is under a screen position: a handle (on top), else an anchor.
enum RawHit {
    Handle(Handle),
    Anchor(Vec2),
}

fn raw_point_at(app: &App, pts: &RawPoints, at: Pos2, xf: Xf) -> Option<RawHit> {
    let d = |q: Vec2| xf.to_screen(q).distance(at);
    if let Some(h) = visible_handles(app, pts).filter(|h| d(h.at) <= PICK).min_by(|a, b| d(a.at).total_cmp(&d(b.at))) {
        return Some(RawHit::Handle(*h));
    }
    pts.anchors.iter().copied().filter(|q| d(*q) <= PICK).min_by(|a, b| d(*a).total_cmp(&d(*b))).map(RawHit::Anchor)
}

/// Keeps the selected anchors on the points that are still there (after a drag merged, undo, or
/// a frame change), and drops them with their shapes.
fn raw_upkeep(app: &mut App) {
    if app.raw_anchors.is_empty() && app.raw_ends.is_empty() {
        return;
    }
    // A drag moves them itself.
    if matches!(app.drag, Drag::RawPoint { .. } | Drag::RawHandle { .. } | Drag::PullHandle { .. } | Drag::SnapMove { .. }) {
        return;
    }
    let pts = raw_shown(app);
    let settle = |list: &mut Vec<Vec2>| {
        let mut out: Vec<Vec2> = Vec::with_capacity(list.len());
        for q in list.iter() {
            // A merge may have met it with a point nearby (at most `SNAP` away, ADR 0041).
            if let Some(p) = pts.anchors.iter().copied().filter(|p| p.distance(*q) <= 2.5 * SNAP).min_by(|a, b| a.distance(*q).total_cmp(&b.distance(*q)))
                && !out.contains(&p)
            {
                out.push(p);
            }
        }
        *list = out;
    };
    settle(&mut app.raw_anchors);
    settle(&mut app.raw_ends);
}

/// Shows the connected raw shapes at stage point `sp` (their layer becomes the active one).
/// False when there is no raw shape there.
fn raw_show_at(app: &mut App, sp: Vec2, shift: bool, xf: Xf) -> bool {
    let slop = 3.0 * xf.unit();
    let Some((layer, id)) = app.s.hit_test(sp, slop) else { return false };
    // On the active layer, look first; another layer's shape is for the engine to say.
    if layer == app.s.layer && !app.s.visible_elements().iter().any(|e| e.id == id && is_raw(e)) {
        return false;
    }
    let Some(v) = app.run("raw.subselect", json!({"x": sp.x, "y": sp.y, "add": shift, "slop": slop})) else { return false };
    if v["raw"] != json!(true) {
        return false;
    }
    if !shift {
        app.raw_anchors.clear();
    }
    app.raw_ends = positions(&v["ends"]);
    app.points.clear();
    true
}

/// A press with Subselection on raw shapes: a shown handle bends its curve, a shown anchor moves
/// (with the other selected anchors; Alt pulls out curve handles instead), an anchor of a raw
/// shape not shown yet shows its shapes and takes the point, and a raw line or fill shows its
/// connected shapes and moves all of them. `None` when the press is not on raw artwork.
fn raw_drag_start(app: &mut App, press: Pos2, shift: bool, alt: bool, xf: Xf) -> Option<Drag> {
    let sp = xf.to_stage(press);
    let pts = raw_shown(app);
    let anchor = match raw_point_at(app, &pts, press, xf) {
        Some(RawHit::Handle(h)) => {
            app.s.begin_gesture(tr_noop!("Move Handle"));
            app.raw_touched.clear();
            return Some(Drag::RawHandle { id: h.id, cmd: h.cmd, slot: h.slot, last: pos2(sp.x, sp.y) });
        }
        Some(RawHit::Anchor(q)) => Some(q),
        // An unselected raw shape's corner or end: show its shapes, then take the point.
        None => match app.s.raw_grab(sp, 3.0 * xf.unit(), GRAB_POINT * xf.unit()) {
            Some((_, anicraft_engine::cmds_raw::Grab::Point { at })) if raw_show_at(app, at, false, xf) => Some(at),
            _ => None,
        },
    };
    if let Some(q) = anchor {
        if alt {
            app.raw_anchors = vec![q];
            app.raw_with.clear();
            app.raw_points = serde_json::Value::Null;
            app.raw_touched.clear();
            app.s.begin_gesture(tr_noop!("Convert Anchor Point"));
            return Some(Drag::PullHandle { target: PullTarget::Raw, anchor: q, moved: false });
        }
        if !app.raw_anchors.iter().any(|p| p.distance(q) <= SNAP) {
            if !shift {
                app.raw_anchors.clear();
            }
            app.raw_anchors.push(q);
        }
        app.raw_with = app.raw_anchors.iter().copied().filter(|p| p.distance(q) > SNAP).collect();
        app.raw_points = serde_json::Value::Null;
        app.raw_touched.clear();
        app.s.begin_gesture(tr_noop!("Move Points"));
        return Some(Drag::RawPoint { at: pos2(q.x, q.y), off: pos2(q.x, q.y) - pos2(sp.x, sp.y) });
    }
    // A raw line or fill: everything connected to it moves (Animate's Subselection drag).
    let on_shown = app.s.hit_test(sp, 3.0 * xf.unit()).is_some_and(|(l, id)| l == app.s.layer && app.s.selection.contains(l, id) && app.s.visible_elements().iter().any(|e| e.id == id && is_raw(e)));
    if !on_shown && !raw_show_at(app, sp, shift, xf) {
        return None;
    }
    app.s.begin_gesture(tr_noop!("Move"));
    Some(Drag::SnapMove { start: pos2(sp.x, sp.y), applied: egui::Vec2::ZERO })
}

/// A click with Subselection on raw shapes: an anchor toggles into the selection (Shift adds or
/// removes it), a raw line or fill shows everything connected to it. False when it wasn't on
/// raw artwork.
fn raw_click(app: &mut App, at: Pos2, shift: bool, xf: Xf) -> bool {
    let pts = raw_shown(app);
    match raw_point_at(app, &pts, at, xf) {
        Some(RawHit::Anchor(q)) => {
            match app.raw_anchors.iter().position(|p| p.distance(q) <= SNAP) {
                Some(i) if shift => {
                    app.raw_anchors.remove(i);
                }
                _ if shift => app.raw_anchors.push(q),
                _ => app.raw_anchors = vec![q],
            }
            true
        }
        Some(RawHit::Handle(_)) => true,
        None => raw_show_at(app, xf.to_stage(at), shift, xf),
    }
}

/// The selected raw anchors moved by the arrow keys: every shape sharing them moves too, and
/// they merge where they land (points within 0.2 px meet, ADR 0041). One undo step a press.
fn raw_nudge(app: &mut App, d: Vec2) {
    let Some((&first, rest)) = app.raw_anchors.split_first() else { return };
    let with: Vec<[f32; 2]> = rest.iter().map(|q| [q.x, q.y]).collect();
    let p = json!({"x": first.x, "y": first.y, "to_x": first.x + d.x, "to_y": first.y + d.y, "with": with});
    if app.run("raw.move_point", p).is_some() {
        for q in app.raw_anchors.iter_mut().chain(app.raw_ends.iter_mut()) {
            *q = *q + d;
        }
    }
}

/// While raw points are dragged, the selected anchors follow by `d` (the move this frame), and
/// the clicked edge's ends too when they move (`all`: the whole drawing moves).
pub fn raw_follow(app: &mut App, d: Vec2, all: bool) {
    if d == Vec2::ZERO {
        return;
    }
    let moving = app.raw_anchors.clone();
    for q in app.raw_anchors.iter_mut() {
        *q = *q + d;
    }
    for q in app.raw_ends.iter_mut().filter(|q| all || moving.iter().any(|m| m.distance(**q) <= SNAP)) {
        *q = *q + d;
    }
}

/// The raw shapes' outlines, handles and anchors (hollow squares; selected ones solid).
fn draw_raw(app: &App, painter: &egui::Painter, xf: Xf) {
    let ids = raw_selected(app);
    if ids.is_empty() {
        return;
    }
    let line = Stroke::new(1.0, T.selection);
    let to_px = Affine::translate(xf.origin.x, xf.origin.y).mul(&Affine::scale(xf.zoom, xf.zoom));
    for e in app.s.visible_elements().iter().filter(|e| ids.contains(&e.id)) {
        let ElementKind::Shape(sh) = &e.kind else { continue };
        for pl in sh.path.flatten(&to_px.mul(&e.transform), 0.35) {
            let pts: Vec<Pos2> = pl.points.iter().map(|p| pos2(p.x, p.y)).collect();
            painter.add(if pl.closed { egui::Shape::closed_line(pts, line) } else { egui::Shape::line(pts, line) });
        }
    }
    let pts = app.s.raw_points(&ids);
    for h in visible_handles(app, &pts) {
        let (hs, a) = (xf.to_screen(h.at), xf.to_screen(h.anchor));
        painter.line_segment([a, hs], line);
        painter.circle_filled(hs, 3.0, T.selection);
    }
    for q in pts.anchors.iter().take(20_000) {
        anchor_box(painter, xf.to_screen(*q), app.raw_anchors.iter().any(|p| p.distance(*q) <= SNAP));
    }
}

// ----- keys ----------------------------------------------------------------------------------

/// Keys the path tools own while they are busy, taken before the global shortcuts (so Enter
/// finishes a path instead of starting playback, and Delete removes points, not the shape).
pub fn consume_keys(app: &mut App, ctx: &egui::Context) {
    let take = |k: Key| ctx.input_mut(|i| i.consume_key(Modifiers::NONE, k));
    if app.pen.is_some() {
        if take(Key::Enter) || take(Key::Escape) {
            commit_pen(app, false);
        }
        if (take(Key::Backspace) || take(Key::Delete))
            && let Some(pen) = app.pen.as_mut()
        {
            pen.knots.pop();
            if pen.knots.is_empty() {
                app.pen = None;
            }
        }
    }
    if app.tool == Tool::Subselect && !app.points.is_empty() {
        if take(Key::Delete) || take(Key::Backspace) {
            delete_points(app);
        }
        if take(Key::Escape) {
            app.points.clear();
        }
        if let Some(d) = arrow_nudge(ctx, app.view.zoom) {
            move_points_step(app, d);
        }
    }
    if app.tool == Tool::Subselect && !app.raw_anchors.is_empty() {
        if take(Key::Delete) || take(Key::Backspace) {
            raw_delete(app);
        }
        if take(Key::Escape) {
            app.raw_anchors.clear();
        }
        if let Some(d) = arrow_nudge(ctx, app.view.zoom) {
            raw_nudge(app, d);
        }
    }
}

/// An arrow key press (taken): 1 screen pixel, Shift 10, as Animate's (issue #42 section 4).
fn arrow_nudge(ctx: &egui::Context, zoom: f32) -> Option<Vec2> {
    let mut d = Vec2::ZERO;
    ctx.input_mut(|i| {
        for (k, v) in [(Key::ArrowLeft, Vec2::new(-1.0, 0.0)), (Key::ArrowRight, Vec2::new(1.0, 0.0)), (Key::ArrowUp, Vec2::new(0.0, -1.0)), (Key::ArrowDown, Vec2::new(0.0, 1.0))] {
            // Shift first: a plain chord also matches a press with Shift held.
            for (m, step) in [(Modifiers::SHIFT, 10.0), (Modifiers::NONE, 1.0)] {
                while i.consume_key(m, k) {
                    d = d + v * (step / zoom.max(1e-3));
                }
            }
        }
    });
    (d != Vec2::ZERO).then_some(d)
}

/// Selected points of drawing objects nudged by the arrow keys: one undo step.
fn move_points_step(app: &mut App, d: Vec2) {
    let own = !app.s.in_gesture();
    if own {
        app.s.begin_gesture(tr_noop!("Move Points"));
    }
    move_points(app, d, true);
    if own {
        app.s.end_gesture();
    }
}

// ----- drawing -------------------------------------------------------------------------------

fn bezier(p0: Vec2, c0: Vec2, c1: Vec2, p1: Vec2, t: f32) -> Vec2 {
    let u = 1.0 - t;
    p0 * (u * u * u) + c0 * (3.0 * u * u * t) + c1 * (3.0 * u * t * t) + p1 * (t * t * t)
}

fn anchor_box(painter: &egui::Painter, c: Pos2, filled: bool) {
    let r = Rect::from_center_size(c, vec2(6.0, 6.0));
    painter.rect_filled(r, 0.0, if filled { T.selection } else { Color32::WHITE });
    painter.rect_stroke(r, 0.0, Stroke::new(1.0, T.selection), StrokeKind::Middle);
}

/// The mark by the Pen's pointer for what a click would do: a plus (add a point), a minus
/// (delete it), a caret (make it a corner), a ring (close the path), a slash (continue the
/// path), a square (finish). Drawn in the selection colour over white, so it reads on any
/// artwork.
fn pen_mark(painter: &egui::Painter, c: Pos2, what: PenHover) {
    let r = 4.0;
    let strokes = [Stroke::new(3.5, Color32::WHITE), Stroke::new(1.5, T.selection)];
    let lines: &[[egui::Vec2; 2]] = match what {
        PenHover::Place | PenHover::Adjust => return,
        PenHover::Add => &[[vec2(-r, 0.0), vec2(r, 0.0)], [vec2(0.0, -r), vec2(0.0, r)]],
        PenHover::Delete => &[[vec2(-r, 0.0), vec2(r, 0.0)]],
        PenHover::Corner => &[[vec2(-r, r), vec2(0.0, -r)], [vec2(0.0, -r), vec2(r, r)]],
        PenHover::Continue => &[[vec2(-r, r), vec2(r, -r)]],
        PenHover::Close => {
            for s in strokes {
                painter.circle_stroke(c, r, s);
            }
            return;
        }
        PenHover::Finish => {
            for s in strokes {
                painter.rect_stroke(Rect::from_center_size(c, vec2(2.0 * r - 1.0, 2.0 * r - 1.0)), 0.0, s, StrokeKind::Middle);
            }
            return;
        }
    };
    for s in strokes {
        for [a, b] in lines {
            painter.line_segment([c + *a, c + *b], s);
        }
    }
}

/// Path tool overlays: point editing on selected shapes, the Pen path being drawn and the
/// Pencil stroke in progress.
pub fn draw_overlay(app: &App, painter: &egui::Painter, xf: Xf, hover: Option<Pos2>) {
    let line = Stroke::new(1.0, T.selection);
    // The Pen shows the points of the selected paths too, while it isn't drawing one: it adds,
    // converts and deletes them, and continues an open path from its end (ADR 0115).
    let pen_idle = app.tool == Tool::Pen && app.pen.as_ref().is_none_or(|pen| pen.knots.is_empty());
    if app.tool == Tool::Subselect || pen_idle {
        draw_raw(app, painter, xf);
        for (id, path, m) in selected_shapes(app) {
            let to_px = Affine::translate(xf.origin.x, xf.origin.y).mul(&Affine::scale(xf.zoom, xf.zoom)).mul(&m);
            for pl in path.flatten(&to_px, 0.35) {
                let pts: Vec<Pos2> = pl.points.iter().map(|p| pos2(p.x, p.y)).collect();
                painter.add(if pl.closed { egui::Shape::closed_line(pts, line) } else { egui::Shape::line(pts, line) });
            }
            let sel = app.points.iter().filter(|p| p.0 == id && p.2 == ANCHOR).map(|p| p.1);
            for (cmd, slot, anchor) in shown_handles(&path, sel) {
                if let (Some(h), Some(a)) = (path.point(cmd, slot), path.point(anchor, ANCHOR)) {
                    let (hs, as_) = (xf.to_screen(m.apply(h)), xf.to_screen(m.apply(a)));
                    painter.line_segment([as_, hs], line);
                    let picked = app.points.contains(&(id, cmd, slot));
                    painter.circle(hs, 3.0, if picked { T.selection } else { Color32::WHITE }, line);
                }
            }
            for (cmd, q) in path.editable_anchors().into_iter().take(20_000) {
                anchor_box(painter, xf.to_screen(m.apply(q)), app.points.contains(&(id, cmd, ANCHOR)));
            }
        }
    }
    if let Some(pen) = &app.pen {
        let stroke = Stroke::new(1.5, T.selection);
        let mut pts = Vec::new();
        for w in pen.knots.windows(2) {
            for k in 0..=16 {
                pts.push(xf.to_screen(bezier(w[0][0], w[0][2], w[1][1], w[1][0], k as f32 / 16.0)));
            }
        }
        painter.add(egui::Shape::line(pts, stroke));
        // Alt on the last knot draws its handles again: the segment as it was stays in view
        // until the button is let go (issue #319).
        if let (PenGrip::Adjust { was }, Some(prev)) = (pen.grip, pen.knots.len().checked_sub(2).and_then(|i| pen.knots.get(i))) {
            let ghost: Vec<Pos2> = (0..=16).map(|k| xf.to_screen(bezier(prev[0], prev[2], was[1], was[0], k as f32 / 16.0))).collect();
            painter.add(egui::Shape::line(ghost, Stroke::new(1.0, T.selection.gamma_multiply(0.45))));
        }
        if let (Some(last), Some(h)) = (pen.knots.last(), hover)
            && !pen.dragging
        {
            // The segment a click here would add, as it would curve (Shift: on 45°).
            let shift = painter.ctx().input(|i| i.modifiers.shift);
            let to = if shift { constrain_45(last[0], xf.to_stage(h)) } else { xf.to_stage(h) };
            let ahead: Vec<Pos2> = (0..=16).map(|k| xf.to_screen(bezier(last[0], last[2], to, to, k as f32 / 16.0))).collect();
            painter.add(egui::Shape::dashed_line(&ahead, line, 4.0, 3.0));
            // Closing cue: a ring on the point a click would close the path on.
            let close_on = match pen.extends {
                Some(_) => far_end(app, pen),
                None => pen.knots.first().map(|k| k[0]),
            };
            if pen.knots.len() >= 2
                && let Some(f) = close_on.map(|q| xf.to_screen(q))
                && f.distance(h) <= PICK
            {
                painter.circle_stroke(f, 6.0, Stroke::new(1.5, T.selection));
            }
        }
        for (i, k) in pen.knots.iter().enumerate() {
            let c = xf.to_screen(k[0]);
            let last = i + 1 == pen.knots.len();
            if last && k[2] != k[0] {
                for hnd in [k[1], k[2]] {
                    let hp = xf.to_screen(hnd);
                    painter.line_segment([c, hp], line);
                    painter.circle_filled(hp, 3.0, T.selection);
                }
            }
            anchor_box(painter, c, last);
        }
    }
    // What a click would do, by the pointer (Animate's Pen cursors, issue #319).
    if app.tool == Tool::Pen
        && app.drag == Drag::None
        && let Some(h) = hover
        && app.pen.as_ref().is_none_or(|pen| !pen.dragging)
    {
        let m = painter.ctx().input(|i| i.modifiers);
        let what = pen_hover(app, h, xf, PenMods { alt: m.alt, shift: m.shift, command: m.command });
        pen_mark(painter, h + vec2(13.0, 13.0), what);
    }
    // Alt over an anchor in Subselection shows the convert caret (Alt-drag pulls out handles).
    if app.tool == Tool::Subselect
        && app.drag == Drag::None
        && let Some(h) = hover
        && painter.ctx().input(|i| i.modifiers.alt)
        && subselect_alt_anchor(app, h, xf)
    {
        pen_mark(painter, h + vec2(13.0, 13.0), PenHover::Corner);
    }
    if app.drag == Drag::Pencil && app.pencil.len() > 1 {
        let w = (app.s.style.stroke_width * xf.zoom).max(1.0);
        let pts: Vec<Pos2> = app.pencil.iter().map(|p| xf.to_screen(*p)).collect();
        painter.add(egui::Shape::line(pts, Stroke::new(w, crate::theme::to_color32(app.s.style.stroke))));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn knot(x: f32, y: f32) -> [Vec2; 3] {
        [Vec2::new(x, y); 3]
    }

    #[test]
    fn pen_svg_is_straight_for_corners_and_curved_for_handles() {
        let mut pen = PenPath { knots: vec![knot(0.0, 0.0), knot(100.0, 0.0), knot(100.0, 50.0)], ..Default::default() };
        assert_eq!(pen.to_svg(false), "M0 0 L100 0 L100 50");
        assert_eq!(pen.to_svg(true), "M0 0 L100 0 L100 50 Z");
        pen.knots[1] = [Vec2::new(100.0, 0.0), Vec2::new(80.0, -10.0), Vec2::new(120.0, 10.0)];
        let d = pen.to_svg(false);
        assert!(d.contains(" C0 0 80 -10 100 0") && d.contains(" C120 10 100 50 100 50"), "{d}");
        // The engine takes it.
        let mut s = anicraft_engine::Session::default();
        assert!(s.execute("draw.path", &json!({"d": pen.to_svg(true)})).is_ok());
    }

    /// The Pen (ADR 0115, issue #319) on a headless app, stage and screen coordinates the same.
    mod pen {
        use super::*;

        const XF: Xf = Xf { origin: Pos2::ZERO, zoom: 1.0 };
        const NONE: PenMods = PenMods { alt: false, shift: false, command: false };
        const ALT: PenMods = PenMods { alt: true, shift: false, command: false };
        const SHIFT: PenMods = PenMods { alt: false, shift: true, command: false };
        const CTRL: PenMods = PenMods { alt: false, shift: false, command: true };

        fn app() -> App {
            let ctx = egui::Context::default();
            let session = anicraft_engine::Session::new(anicraft_engine::doc::Document::default());
            let mut app = App::with_context(&ctx, session, Some(std::path::PathBuf::from("unused.png")));
            app.tool = Tool::Pen;
            app.view.zoom = 1.0;
            // Drawing objects: one path is one element (merge drawing splits it at corners).
            app.s.style.object_drawing = true;
            app
        }

        fn v(x: f32, y: f32) -> Vec2 {
            Vec2::new(x, y)
        }

        fn click(app: &mut App, x: f32, y: f32, mods: PenMods) {
            pen_press(app, pos2(x, y), XF, mods);
            pen_release(app);
        }

        fn drag(app: &mut App, from: (f32, f32), to: (f32, f32), mods: PenMods) {
            pen_press(app, pos2(from.0, from.1), XF, mods);
            pen_drag(app, pos2(to.0, to.1), XF, mods);
            pen_release(app);
        }

        fn knots(app: &App) -> Vec<[Vec2; 3]> {
            app.pen.as_ref().map(|p| p.knots.clone()).unwrap_or_default()
        }

        /// The one shape on the stage: its id and its path in stage space.
        fn shape(app: &App) -> (ElementId, Path) {
            let els = app.s.visible_elements();
            assert_eq!(els.len(), 1, "one shape");
            let ElementKind::Shape(sh) = &els[0].kind else { panic!("a shape") };
            (els[0].id, sh.path.transformed(&els[0].transform))
        }

        fn anchors(p: &Path) -> Vec<(f32, f32)> {
            p.editable_anchors().into_iter().map(|(_, q)| (q.x.round(), q.y.round())).collect()
        }

        /// An open drawing object, selected, so the Pen shows its points.
        fn with_object(d: &str) -> App {
            let mut app = app();
            app.run("draw.path", json!({"d": d, "object": true}));
            let (el, _) = shape(&app);
            app.run("element.select", json!({"ids": [el.0]}));
            assert!(app.s.selection.contains(app.s.layer, el));
            app
        }

        #[test]
        fn clicks_place_corners_drags_pull_mirrored_handles_and_alt_breaks_the_pair() {
            let mut app = app();
            click(&mut app, 10.0, 10.0, NONE);
            drag(&mut app, (100.0, 10.0), (130.0, 40.0), NONE);
            assert_eq!(knots(&app), vec![[v(10.0, 10.0); 3], [v(100.0, 10.0), v(70.0, -20.0), v(130.0, 40.0)]]);
            // Alt while dragging a new knot moves only the handle ahead.
            pen_press(&mut app, pos2(200.0, 10.0), XF, NONE);
            pen_drag(&mut app, pos2(220.0, 10.0), XF, NONE);
            pen_drag(&mut app, pos2(200.0, 50.0), XF, ALT);
            pen_release(&mut app);
            assert_eq!(knots(&app)[2], [v(200.0, 10.0), v(180.0, 10.0), v(200.0, 50.0)]);
            // A press that barely moves stays a corner.
            drag(&mut app, (300.0, 10.0), (301.0, 10.5), NONE);
            assert_eq!(knots(&app)[3], [v(300.0, 10.0); 3]);
        }

        #[test]
        fn shift_keeps_segments_and_handles_on_45_degree_steps() {
            let mut app = app();
            click(&mut app, 100.0, 100.0, NONE);
            // Nearly horizontal: the point lands on the horizontal through the knot before.
            click(&mut app, 200.0, 108.0, SHIFT);
            assert_eq!(knots(&app)[1][0], v(200.0, 100.0));
            // Nearly diagonal: on the 45° line, at the pointer's distance along it.
            click(&mut app, 260.0, 150.0, SHIFT);
            let k = knots(&app)[2][0];
            assert!(((k.x - 200.0) - (k.y - 100.0)).abs() < 1e-3 && (k.x - 255.0).abs() < 1e-3, "{k:?}");
            // A handle pulled with Shift is vertical here, and its twin mirrors it.
            pen_press(&mut app, pos2(400.0, 100.0), XF, NONE);
            pen_drag(&mut app, pos2(404.0, 160.0), XF, SHIFT);
            pen_release(&mut app);
            let k = knots(&app)[3];
            assert!((k[2].x - 400.0).abs() < 1e-3 && (k[2].y - 160.0).abs() < 1e-3 && (k[1].y - 40.0).abs() < 1e-3, "{k:?}");
            for (from, p) in [(v(0.0, 0.0), v(10.0, 1.0)), (v(5.0, 5.0), v(-20.0, 31.0)), (v(1.0, 2.0), v(1.0, 2.0)), (v(0.0, 0.0), v(f32::NAN, 1.0))] {
                let q = constrain_45(from, p);
                let d = q - from;
                assert!(!d.is_finite() || d.length() < 1e-6 || (d.x.abs() < 1e-3 || d.y.abs() < 1e-3 || (d.x.abs() - d.y.abs()).abs() < 1e-3), "{from:?} {p:?} -> {q:?}");
            }
        }

        #[test]
        fn a_click_on_the_last_point_breaks_its_handle_then_finishes() {
            // Issue #319, Break Handles: the next segment leaves the point as a sharp corner.
            let mut app = app();
            click(&mut app, 10.0, 100.0, NONE);
            drag(&mut app, (100.0, 100.0), (140.0, 60.0), NONE);
            assert_eq!(pen_hover(&app, pos2(101.0, 99.0), XF, NONE), PenHover::Corner);
            click(&mut app, 101.0, 99.0, NONE);
            assert_eq!(knots(&app)[1], [v(100.0, 100.0), v(60.0, 140.0), v(100.0, 100.0)], "the handle behind stays, the one ahead is gone");
            assert_eq!(pen_hover(&app, pos2(100.0, 100.0), XF, NONE), PenHover::Finish);
            click(&mut app, 200.0, 100.0, NONE);
            // A click on the (corner) last point finishes the open path: one shape.
            click(&mut app, 200.0, 100.0, NONE);
            assert!(app.pen.is_none());
            let (_, p) = shape(&app);
            assert_eq!(anchors(&p), [(10.0, 100.0), (100.0, 100.0), (200.0, 100.0)]);
            assert!(matches!(p.cmds[1], PathCmd::CubicTo(..)) && matches!(p.cmds[2], PathCmd::LineTo(_)), "curved in, straight out: {:?}", p.cmds);
        }

        #[test]
        fn a_drag_from_the_last_point_pulls_a_new_handle_ahead_alone() {
            let mut app = app();
            click(&mut app, 10.0, 100.0, NONE);
            drag(&mut app, (100.0, 100.0), (140.0, 60.0), NONE);
            drag(&mut app, (100.0, 100.0), (100.0, 160.0), NONE);
            assert_eq!(knots(&app)[1], [v(100.0, 100.0), v(60.0, 140.0), v(100.0, 160.0)]);
            assert_eq!(knots(&app).len(), 2, "no point was added, and the path isn't finished");
        }

        #[test]
        fn alt_drag_on_the_last_point_draws_its_segment_again() {
            // Issue #319, Adjust Last Node: the segment as it was shows until the release.
            let mut app = app();
            click(&mut app, 10.0, 100.0, NONE);
            drag(&mut app, (100.0, 100.0), (140.0, 60.0), NONE);
            let was = knots(&app)[1];
            assert_eq!(pen_hover(&app, pos2(100.0, 100.0), XF, ALT), PenHover::Adjust);
            pen_press(&mut app, pos2(100.0, 100.0), XF, ALT);
            assert_eq!(app.pen.as_ref().map(|p| p.grip), Some(PenGrip::Adjust { was }), "the ghost");
            pen_drag(&mut app, pos2(100.0, 150.0), XF, ALT);
            assert_eq!(knots(&app)[1], [v(100.0, 100.0), v(100.0, 50.0), v(100.0, 150.0)]);
            // Back onto the point: nothing changes.
            pen_drag(&mut app, pos2(100.5, 100.0), XF, ALT);
            assert_eq!(knots(&app)[1], was);
            pen_drag(&mut app, pos2(130.0, 100.0), XF, ALT);
            pen_release(&mut app);
            assert_eq!(knots(&app), vec![[v(10.0, 100.0); 3], [v(100.0, 100.0), v(70.0, 100.0), v(130.0, 100.0)]]);
            assert_eq!(app.pen.as_ref().map(|p| (p.grip, p.dragging)), Some((PenGrip::New, false)));
        }

        #[test]
        fn the_first_point_closes_with_a_hint_and_keys_finish_or_step_back() {
            let mut app = app();
            click(&mut app, 10.0, 10.0, NONE);
            click(&mut app, 110.0, 10.0, NONE);
            click(&mut app, 110.0, 90.0, NONE);
            assert_eq!(pen_hover(&app, pos2(12.0, 11.0), XF, NONE), PenHover::Close);
            assert_eq!(pen_hover(&app, pos2(60.0, 60.0), XF, NONE), PenHover::Place);
            click(&mut app, 12.0, 11.0, NONE);
            assert!(app.pen.is_none());
            let (_, p) = shape(&app);
            assert!(matches!(p.cmds.last(), Some(PathCmd::Close)));
            assert_eq!(anchors(&p).len(), 3);
            // Esc finishes an open path; Backspace takes the last point back.
            let ctx = egui::Context::default();
            let key = |app: &mut App, key: Key| {
                let mut input = egui::RawInput::default();
                input.events.push(egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
                let mut out = ctx.run_ui(input, |ui| consume_keys(app, ui.ctx()));
                out.textures_delta.clear();
            };
            click(&mut app, 300.0, 10.0, NONE);
            click(&mut app, 400.0, 10.0, NONE);
            click(&mut app, 400.0, 90.0, NONE);
            key(&mut app, Key::Backspace);
            assert_eq!(knots(&app).len(), 2);
            key(&mut app, Key::Escape);
            assert!(app.pen.is_none());
            assert_eq!(app.s.visible_elements().len(), 2, "the open path is kept");
        }

        #[test]
        fn on_a_selected_path_a_click_converts_then_deletes_a_point_and_adds_one_on_the_outline() {
            // Issue #319, Add/Remove Nodes: caret, then minus, on a point; plus on a segment.
            let mut app = with_object("M0 100 C30 40 70 40 100 100 C130 160 170 160 200 100 L300 100");
            let at = pos2(100.0, 100.0);
            assert_eq!(pen_hover(&app, at, XF, NONE), PenHover::Corner);
            click(&mut app, at.x, at.y, NONE);
            assert!(app.pen.is_none(), "no path was started");
            let (_, p) = shape(&app);
            assert_eq!(anchors(&p).len(), 4);
            assert_eq!(pen_hover(&app, at, XF, NONE), PenHover::Delete, "a corner now: {:?}", p.cmds);
            click(&mut app, at.x, at.y, NONE);
            assert_eq!(anchors(&shape(&app).1), [(0.0, 100.0), (200.0, 100.0), (300.0, 100.0)]);
            // The outline between points takes a point; away from it a click starts a path.
            assert_eq!(pen_hover(&app, pos2(250.0, 101.0), XF, NONE), PenHover::Add);
            click(&mut app, 250.0, 101.0, NONE);
            assert_eq!(anchors(&shape(&app).1), [(0.0, 100.0), (200.0, 100.0), (250.0, 100.0), (300.0, 100.0)]);
            assert!(app.pen.is_none());
            assert_eq!(pen_hover(&app, pos2(250.0, 300.0), XF, NONE), PenHover::Place);
            // Each was one undo step.
            for n in [3, 4, 4] {
                app.run("edit.undo", json!({}));
                assert_eq!(anchors(&shape(&app).1).len(), n);
            }
        }

        #[test]
        fn an_unselected_path_is_drawn_over_and_ctrl_click_selects_it() {
            let mut app = with_object("M0 100 L100 100 L200 100");
            app.run("edit.deselect", json!({}));
            // Its points aren't showing: the Pen starts a path there (a T-junction, say).
            assert_eq!(pen_hover(&app, pos2(100.0, 100.0), XF, NONE), PenHover::Place);
            click(&mut app, 100.0, 100.0, NONE);
            assert_eq!(knots(&app).len(), 1);
            app.pen = None;
            // Ctrl-click selects it, and now the Pen edits it.
            click(&mut app, 150.0, 100.0, CTRL);
            assert!(app.pen.is_none() && app.s.selection.len() == 1);
            assert_eq!(pen_hover(&app, pos2(100.0, 100.0), XF, NONE), PenHover::Delete);
            assert_eq!(pen_hover(&app, pos2(150.0, 100.0), XF, NONE), PenHover::Add);
            click(&mut app, 400.0, 400.0, CTRL);
            assert!(app.s.selection.is_empty(), "Ctrl-click on nothing lets go");
        }

        #[test]
        fn an_open_end_is_continued_and_the_other_end_closes_the_path() {
            let mut app = with_object("M0 100 L100 100 C130 100 150 80 150 50");
            let (id, _) = shape(&app);
            assert_eq!(pen_hover(&app, pos2(150.0, 50.0), XF, NONE), PenHover::Continue);
            assert_eq!(pen_hover(&app, pos2(0.0, 100.0), XF, NONE), PenHover::Continue);
            // Press on the end and pull a handle ahead, then two more points, then finish.
            drag(&mut app, (150.0, 50.0), (150.0, 20.0), NONE);
            let pen = app.pen.clone().expect("continuing");
            assert_eq!(pen.extends, Some((id, false)));
            assert_eq!(pen.knots, vec![[v(150.0, 50.0), v(150.0, 80.0), v(150.0, 20.0)]]);
            click(&mut app, 200.0, 0.0, NONE);
            click(&mut app, 300.0, 0.0, NONE);
            commit_pen(&mut app, false);
            let (same, p) = shape(&app);
            assert_eq!(same, id, "the same object, longer");
            assert_eq!(anchors(&p), [(0.0, 100.0), (100.0, 100.0), (150.0, 50.0), (200.0, 0.0), (300.0, 0.0)]);
            assert!(matches!(p.cmds[3], PathCmd::CubicTo(..)), "the handle pulled from the end curves the join");
            // From the start, round to the other end: the ring shows there and the path closes.
            click(&mut app, 0.0, 100.0, NONE);
            assert_eq!(app.pen.as_ref().and_then(|p| p.extends), Some((id, true)));
            click(&mut app, 0.0, 200.0, NONE);
            assert_eq!(pen_hover(&app, pos2(300.0, 1.0), XF, NONE), PenHover::Close);
            assert_eq!(pen_hover(&app, pos2(0.0, 100.0), XF, NONE), PenHover::Place, "its own first knot doesn't close it");
            click(&mut app, 300.0, 1.0, NONE);
            assert!(app.pen.is_none());
            let (same, p) = shape(&app);
            assert_eq!(same, id);
            assert!(matches!(p.cmds.last(), Some(PathCmd::Close)), "{:?}", p.cmds);
            assert_eq!(anchors(&p).len(), 6);
            // Closed, it has no end to continue: its points convert and delete.
            assert_eq!(pen_hover(&app, pos2(300.0, 0.0), XF, NONE), PenHover::Delete);
            // Two undo steps take both continuations back.
            app.run("edit.undo", json!({}));
            app.run("edit.undo", json!({}));
            assert_eq!(anchors(&shape(&app).1).len(), 3);
        }

        #[test]
        fn the_pen_edits_the_points_of_selected_merged_drawings() {
            let mut app = app();
            app.run("draw.rect", json!({"x": 0, "y": 0, "w": 100, "h": 80, "fill": "#ff0000", "stroke": "#000000", "object": false}));
            app.run("edit.deselect", json!({}));
            assert_eq!(pen_hover(&app, pos2(100.0, 0.0), XF, NONE), PenHover::Place, "not selected: a path starts here");
            click(&mut app, 50.0, 0.0, CTRL);
            assert!(!app.s.selection.is_empty());
            assert_eq!(raw_shown(&app).anchors.len(), 4);
            assert_eq!(pen_hover(&app, pos2(50.0, 0.0), XF, NONE), PenHover::Add);
            click(&mut app, 50.0, 0.0, NONE);
            assert!(app.pen.is_none());
            assert_eq!(raw_shown(&app).anchors.len(), 5);
            // A corner point is deleted: the outline closes up across it.
            assert_eq!(pen_hover(&app, pos2(100.0, 0.0), XF, NONE), PenHover::Delete);
            click(&mut app, 100.0, 0.0, NONE);
            assert_eq!(raw_shown(&app).anchors.len(), 4);
            assert!(app.pen.is_none());
        }
    }

    /// Subselection on a merged drawing (ADR 0042), on a headless app: a raw rectangle with
    /// its outline at the stage origin, stage and screen coordinates the same.
    mod raw {
        use super::*;

        const XF: Xf = Xf { origin: Pos2::ZERO, zoom: 1.0 };

        fn app_and_ctx() -> (App, egui::Context) {
            let ctx = egui::Context::default();
            let session = anicraft_engine::Session::new(anicraft_engine::doc::Document::default());
            let mut app = App::with_context(&ctx, session, Some(std::path::PathBuf::from("unused.png")));
            app.run("draw.rect", json!({"x": 0, "y": 0, "w": 100, "h": 80, "fill": "#ff0000", "stroke": "#000000", "object": false}));
            app.run("edit.deselect", json!({}));
            app.tool = Tool::Subselect;
            app.view.zoom = 1.0;
            (app, ctx)
        }

        fn has(list: &[Vec2], x: f32, y: f32) -> bool {
            list.iter().any(|q| q.distance(Vec2::new(x, y)) < 1e-2)
        }

        #[test]
        fn a_click_shows_the_connected_points_and_selects_anchors() {
            let (mut app, _) = app_and_ctx();
            subselect_click(&mut app, pos2(50.0, 0.0), false, XF);
            assert_eq!(app.s.selection.len(), 5, "the fill and its four sides");
            assert_eq!(raw_shown(&app).anchors.len(), 4);
            assert!(has(&app.raw_ends, 0.0, 0.0) && has(&app.raw_ends, 100.0, 0.0), "{:?}", app.raw_ends);
            subselect_click(&mut app, pos2(100.0, 0.0), false, XF);
            assert_eq!(app.raw_anchors.len(), 1);
            subselect_click(&mut app, pos2(0.0, 0.0), true, XF);
            assert_eq!(app.raw_anchors.len(), 2, "Shift adds");
            subselect_click(&mut app, pos2(0.0, 0.0), true, XF);
            assert_eq!(app.raw_anchors.len(), 1, "and removes");
            subselect_click(&mut app, pos2(500.0, 500.0), false, XF);
            assert!(app.s.selection.is_empty() && app.raw_anchors.is_empty());
        }

        #[test]
        fn arrow_keys_move_the_selected_anchors_with_every_shape_on_them() {
            let (mut app, ctx) = app_and_ctx();
            subselect_click(&mut app, pos2(50.0, 0.0), false, XF);
            subselect_click(&mut app, pos2(100.0, 0.0), false, XF);
            subselect_click(&mut app, pos2(0.0, 0.0), true, XF);
            let mut input = egui::RawInput::default();
            input.events.push(egui::Event::Key { key: Key::ArrowUp, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::SHIFT });
            let mut out = ctx.run_ui(input, |ui| consume_keys(&mut app, ui.ctx()));
            out.textures_delta.clear();
            assert!(has(&app.raw_anchors, 0.0, -10.0) && has(&app.raw_anchors, 100.0, -10.0), "{:?}", app.raw_anchors);
            upkeep(&mut app);
            assert_eq!(app.raw_anchors.len(), 2, "still on the moved points");
            let pts = raw_shown(&app);
            assert!(has(&pts.anchors, 0.0, -10.0) && has(&pts.anchors, 100.0, 80.0) && pts.anchors.len() == 4, "{:?}", pts.anchors);
            assert!(app.s.can_undo());
        }

        #[test]
        fn delete_removes_selected_raw_anchors_and_a_double_click_adds_one() {
            let (mut app, ctx) = app_and_ctx();
            subselect_click(&mut app, pos2(50.0, 0.0), false, XF);
            subselect_click(&mut app, pos2(0.0, 0.0), false, XF);
            assert_eq!(app.raw_anchors.len(), 1);
            let mut input = egui::RawInput::default();
            input.events.push(egui::Event::Key { key: Key::Delete, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
            let mut out = ctx.run_ui(input, |ui| consume_keys(&mut app, ui.ctx()));
            out.textures_delta.clear();
            assert!(app.raw_anchors.is_empty());
            let pts = raw_shown(&app);
            assert_eq!(pts.anchors.len(), 3, "a triangle: {:?}", pts.anchors);
            assert!(!has(&pts.anchors, 0.0, 0.0));
            // A double-click on the right side adds a point there, shared and selected.
            subselect_double_click(&mut app, pos2(101.0, 40.0), XF);
            assert!(has(&app.raw_anchors, 100.0, 40.0), "{:?}", app.raw_anchors);
            assert_eq!(raw_shown(&app).anchors.len(), 4);
            // Make Smooth reaches the engine for raw anchors: the corner at (100, 80) rounds.
            app.raw_anchors = vec![Vec2::new(100.0, 80.0)];
            raw_convert(&mut app, true);
            assert!(!raw_shown(&app).handles.is_empty(), "smooth: handles appear");
        }

        #[test]
        fn a_press_takes_an_anchor_or_moves_everything_connected() {
            let (mut app, _) = app_and_ctx();
            // An anchor of a drawing not shown yet: it shows, and the point is taken.
            assert!(matches!(subselect_drag_start(&mut app, pos2(100.0, 80.0), false, false, XF), Drag::RawPoint { .. }));
            assert_eq!(app.s.selection.len(), 5);
            assert!(has(&app.raw_anchors, 100.0, 80.0));
            app.s.end_gesture();
            // With another anchor selected, Shift: both move.
            assert!(matches!(subselect_drag_start(&mut app, pos2(0.0, 80.0), true, false, XF), Drag::RawPoint { .. }));
            assert_eq!(app.raw_with.len(), 1);
            app.s.end_gesture();
            // Inside the fill: the whole connected drawing moves.
            assert!(matches!(subselect_drag_start(&mut app, pos2(50.0, 40.0), false, false, XF), Drag::SnapMove { .. }));
            app.s.end_gesture();
            // Empty space: a marquee, which selects the anchors inside it.
            assert!(matches!(subselect_drag_start(&mut app, pos2(300.0, 300.0), false, false, XF), Drag::PointMarquee { .. }));
            marquee_points(&mut app, pos2(90.0, -10.0), pos2(120.0, 10.0), false);
            assert_eq!(app.s.selection.len(), 5);
            assert_eq!(app.raw_anchors.len(), 1);
            assert!(has(&app.raw_anchors, 100.0, 0.0));
        }

        #[test]
        fn alt_drag_from_an_anchor_pulls_out_curve_handles_and_alt_click_changes_nothing() {
            // Raw rectangle: Alt-click selects without curving; Alt-drag pulls mirrored handles.
            let (mut app, _) = app_and_ctx();
            subselect_click(&mut app, pos2(50.0, 0.0), false, XF);
            assert!(subselect_alt_anchor(&app, pos2(100.0, 0.0), XF));
            // Alt-click without movement (or a tiny press-release) leaves the shape straight.
            let Drag::PullHandle { target, anchor, mut moved } = subselect_drag_start(&mut app, pos2(100.0, 0.0), false, true, XF) else {
                panic!("expected PullHandle on raw anchor");
            };
            assert!(pull_handle(&mut app, target, anchor, &mut moved, Vec2::new(100.5, 0.5), false, XF).is_none());
            assert!(!moved);
            app.s.end_gesture();
            assert!(raw_shown(&app).handles.is_empty(), "Alt-click without drag must not curve");
            // Alt-drag pulls out mirrored handles live, then merges on release in one undo step.
            let Drag::PullHandle { target, anchor, mut moved } = subselect_drag_start(&mut app, pos2(100.0, 0.0), false, true, XF) else {
                panic!("expected PullHandle");
            };
            let ids = pull_handle(&mut app, target, anchor, &mut moved, Vec2::new(125.0, 15.0), false, XF);
            assert!(moved && ids.is_some());
            let ids = pull_handle(&mut app, target, anchor, &mut moved, Vec2::new(130.0, 20.0), false, XF);
            assert!(ids.is_some());
            if let Some(ids) = ids {
                app.run("raw.merge", json!({"ids": ids}));
            }
            app.s.end_gesture();
            let hs: Vec<_> = raw_shown(&app).handles.into_iter().filter(|h| h.anchor.distance(Vec2::new(100.0, 0.0)) < 0.3).collect();
            assert_eq!(hs.len(), 2, "two handles pulled at corner: {hs:?}");
            assert!(hs.iter().any(|h| h.at.distance(Vec2::new(130.0, 20.0)) < 0.5));
            assert!(hs.iter().any(|h| h.at.distance(Vec2::new(70.0, -20.0)) < 0.5));
            // Hovering a handle with Alt is NOT an anchor pull; dragging it keeps RawHandle (one alone).
            let hpos = pos2(hs[0].at.x, hs[0].at.y);
            assert!(!subselect_alt_anchor(&app, hpos, XF));
            assert!(matches!(subselect_drag_start(&mut app, hpos, false, true, XF), Drag::RawHandle { .. }));
            app.s.end_gesture();
            // One undo takes the whole raw handle pull back.
            app.run("edit.undo", json!({}));
            assert!(raw_shown(&app).handles.is_empty(), "one undo restores the raw rectangle");

            // Drawing object rectangle: Alt-drag pulls mirrored handles; Alt-click changes nothing.
            let ctx = egui::Context::default();
            let session = anicraft_engine::Session::new(anicraft_engine::doc::Document::default());
            let mut obj_app = App::with_context(&ctx, session, Some(std::path::PathBuf::from("unused.png")));
            obj_app.tool = Tool::Subselect;
            obj_app.view.zoom = 1.0;
            obj_app.run("draw.rect", json!({"x": 0, "y": 0, "w": 100, "h": 80, "object": true}));
            let id = obj_app.s.selection.ids().next().unwrap();
            let path_of = |a: &App| -> Path {
                let el = a.s.visible_elements().into_iter().find(|e| e.id == id).unwrap();
                let ElementKind::Shape(sh) = &el.kind else { panic!() };
                sh.path.transformed(&el.transform)
            };
            let before = path_of(&obj_app);
            assert!(subselect_alt_anchor(&obj_app, pos2(100.0, 0.0), XF));
            // Alt-click without movement leaves the object unchanged.
            subselect_click(&mut obj_app, pos2(100.0, 0.0), false, XF);
            let Drag::PullHandle { target, anchor, mut moved } = subselect_drag_start(&mut obj_app, pos2(100.0, 0.0), false, true, XF) else {
                panic!("expected PullHandle on object anchor");
            };
            assert!(pull_handle(&mut obj_app, target, anchor, &mut moved, Vec2::new(100.5, 0.0), false, XF).is_none());
            obj_app.s.end_gesture();
            assert_eq!(path_of(&obj_app), before, "Alt-click without drag changes nothing");
            // Alt-drag pulls mirrored handles live in one undo step.
            let Drag::PullHandle { target, anchor, mut moved } = subselect_drag_start(&mut obj_app, pos2(100.0, 0.0), false, true, XF) else {
                panic!("expected PullHandle");
            };
            pull_handle(&mut obj_app, target, anchor, &mut moved, Vec2::new(115.0, 10.0), false, XF);
            pull_handle(&mut obj_app, target, anchor, &mut moved, Vec2::new(120.0, 30.0), false, XF);
            obj_app.s.end_gesture();
            let after = path_of(&obj_app);
            assert!(after.cmds.iter().filter(|c| matches!(c, PathCmd::CubicTo(..))).count() == 2, "two curved segments: {:?}", after.cmds);
            // Alt on one of those handles moves it alone (Drag::Points, not PullHandle).
            let handle_pos = pos2(120.0, 30.0);
            assert!(!subselect_alt_anchor(&obj_app, handle_pos, XF));
            assert!(matches!(subselect_drag_start(&mut obj_app, handle_pos, false, true, XF), Drag::Points { .. }));
            obj_app.s.end_gesture();
            // One undo restores the whole drag.
            obj_app.run("edit.undo", json!({}));
            assert_eq!(path_of(&obj_app), before, "one undo restores the drawing object");
        }

        #[test]
        fn pull_handle_updates_below_drag_threshold_after_pull_starts() {
            // Drawing object: sub-threshold press stays unchanged, then once pulled past DRAG_MIN,
            // moving back within DRAG_MIN still updates the handle instead of keeping the old one.
            let ctx = egui::Context::default();
            let session = anicraft_engine::Session::new(anicraft_engine::doc::Document::default());
            let mut obj_app = App::with_context(&ctx, session, Some(std::path::PathBuf::from("unused.png")));
            obj_app.tool = Tool::Subselect;
            obj_app.view.zoom = 1.0;
            obj_app.run("draw.rect", json!({"x": 0, "y": 0, "w": 100, "h": 80, "object": true}));
            let id = obj_app.s.selection.ids().next().unwrap();
            let path_of = |a: &App| -> Path {
                let el = a.s.visible_elements().into_iter().find(|e| e.id == id).unwrap();
                let ElementKind::Shape(sh) = &el.kind else { panic!() };
                sh.path.transformed(&el.transform)
            };
            let before = path_of(&obj_app);
            let Drag::PullHandle { target, anchor, mut moved } = subselect_drag_start(&mut obj_app, pos2(100.0, 0.0), false, true, XF) else {
                panic!("expected PullHandle on object anchor");
            };
            // Initial sub-threshold movement leaves geometry unchanged.
            pull_handle(&mut obj_app, target, anchor, &mut moved, anchor + Vec2::new(1.0, 0.0), false, XF);
            assert!(!moved);
            assert_eq!(path_of(&obj_app), before);
            // Pull past threshold creates a long handle.
            pull_handle(&mut obj_app, target, anchor, &mut moved, anchor + Vec2::new(30.0, 20.0), false, XF);
            assert!(moved);
            let long = path_of(&obj_app).point(2, C1).unwrap().distance(anchor);
            assert!(long > 20.0, "expected long handle, got {long}");
            // Moving back to a nonzero sub-threshold offset updates the handle instead of leaving it long.
            pull_handle(&mut obj_app, target, anchor, &mut moved, anchor + Vec2::new(1.0, 0.0), false, XF);
            obj_app.s.end_gesture();
            let short = path_of(&obj_app).point(2, C1).unwrap().distance(anchor);
            assert!(short < DRAG_MIN && short > 0.5, "handle should shrink below threshold: {short} (was {long})");

            // Same for a raw rectangle: moving back within DRAG_MIN updates raw handles too.
            let (mut raw_app, _) = app_and_ctx();
            subselect_click(&mut raw_app, pos2(50.0, 0.0), false, XF);
            let Drag::PullHandle { target, anchor, mut moved } = subselect_drag_start(&mut raw_app, pos2(100.0, 0.0), false, true, XF) else {
                panic!("expected PullHandle on raw anchor");
            };
            assert!(pull_handle(&mut raw_app, target, anchor, &mut moved, anchor + Vec2::new(1.0, 0.0), false, XF).is_none());
            assert!(!moved);
            assert!(pull_handle(&mut raw_app, target, anchor, &mut moved, anchor + Vec2::new(30.0, 20.0), false, XF).is_some());
            assert!(moved);
            let ids = pull_handle(&mut raw_app, target, anchor, &mut moved, anchor + Vec2::new(1.0, 0.0), false, XF);
            assert!(ids.is_some(), "established raw pull must still update below DRAG_MIN");
            if let Some(ids) = ids {
                raw_app.run("raw.merge", json!({"ids": ids}));
            }
            raw_app.s.end_gesture();
            let hs: Vec<_> = raw_shown(&raw_app).handles.into_iter().filter(|h| h.anchor.distance(anchor) < 0.3).collect();
            assert_eq!(hs.len(), 2, "{hs:?}");
            assert!(hs.iter().all(|h| h.at.distance(anchor) < DRAG_MIN && h.at.distance(anchor) > 0.5), "raw handles should reflect sub-threshold update: {hs:?}");
        }
    }

    #[test]
    fn handles_shown_for_a_selected_smooth_anchor() {
        let mut p = Path::new();
        p.move_to(Vec2::new(0.0, 0.0)).line_to(Vec2::new(10.0, 10.0)).line_to(Vec2::new(20.0, 0.0));
        assert!(shown_handles(&p, [1usize].into_iter()).is_empty(), "lines have no handles");
        assert!(p.make_smooth(1));
        let h = shown_handles(&p, [1usize].into_iter());
        assert_eq!(h, vec![(1, C2, 1), (2, C1, 1)]);
    }
}
