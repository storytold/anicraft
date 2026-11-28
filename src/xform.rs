//! Free Transform's box and transformation point (ADR 0111).
//!
//! One selected element shows its own box, turned, scaled and skewed as it is, and its handles
//! act along the box's own axes (#175); several elements, or raw pieces, show the upright box
//! of the selection. The transformation point (the white circle) is what a rotation turns
//! about (Alt turns about the opposite corner), and what Alt scales and skews about. Drag it
//! to move it; double-click it to put it back at the centre. A symbol instance keeps its point in the document
//! (`modify.transformation_point`); anything else has one for the selection while Free
//! Transform is held, kept here (#302, #280, #178, #183).
//!
//! Behaviour learned from Adobe's public help pages on transforming objects and from watching
//! the program; nothing was copied.

use crate::app::{App, Tool, XfKind};
use anicraft_engine::TransformOrigin;
use anicraft_engine::doc::ElementKind;
use anicraft_engine::geom::{Affine, Vec2};
use egui::{CursorIcon, Pos2, pos2};
use serde_json::json;
use std::hash::{Hash, Hasher};

/// How near the transformation point (screen pixels) a press grabs it.
pub const POINT_GRAB: f32 = 7.0;
/// How near one of the box's handles, corners or its centre (screen pixels) the dragged point
/// snaps to it.
pub const POINT_SNAP: f32 = 8.0;

/// The selection's box as Free Transform draws it, and its transformation point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    /// The box's own space to stage space.
    pub m: Affine,
    /// The box in its own space: `[x0, y0, x1, y1]`.
    pub b: [f32; 4],
    /// The transformation point, in stage space.
    pub point: Vec2,
    /// The selection is one symbol instance, which keeps its point in the document.
    pub keeps: bool,
    /// The point is one the artist moved (kept by the instance, or held for the selection).
    pub moved: bool,
    /// The point can be dragged: one point for the whole selection (ADR 0109's default mode).
    pub movable: bool,
}

/// The point held for a selection that can't keep one (raw shapes, groups, several elements),
/// while Free Transform is the tool.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Held {
    /// Which elements it is for.
    selection: u64,
    /// The selection's bounds when it was last placed: a selection that moves carries the
    /// point; one that changes any other way drops it.
    bounds: [f32; 4],
    at: Vec2,
}

/// Free Transform's own state: the held point, and the frame a drag began with.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct State {
    held: Option<Held>,
    /// The frame as it was when the drag in progress began (the drag's whole transform is
    /// measured in it).
    pub drag: Option<Frame>,
}

fn selection_key(app: &App) -> u64 {
    let mut ids: Vec<(u64, u64)> = app.s.selection.iter().map(|(l, e)| (l.0, e.0)).collect();
    ids.sort_unstable();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    ids.hash(&mut h);
    h.finish()
}

fn bounds(app: &App) -> Option<[f32; 4]> {
    let b = app.s.selection_bounds();
    (!b.is_empty()).then_some([b.min.x, b.min.y, b.max.x, b.max.y])
}

/// Once a frame: the held point goes with its selection when that moves, and is dropped when
/// the tool or the selection changes, or the selection is reshaped by anything but a Free
/// Transform drag (which carries it itself, see [`carried`]).
pub fn sync(app: &mut App) {
    let Some(h) = app.xf.held else { return };
    let now = (app.tool == Tool::Transform).then(|| bounds(app)).flatten().filter(|_| selection_key(app) == h.selection);
    app.xf.held = now.and_then(|b| {
        if b == h.bounds {
            return Some(h);
        }
        let same = |a: f32, c: f32| (a - c).abs() <= 1e-2 * (1.0 + a.abs().max(c.abs()));
        let moved = same(b[2] - b[0], h.bounds[2] - h.bounds[0]) && same(b[3] - b[1], h.bounds[3] - h.bounds[1]);
        moved.then(|| Held { bounds: b, at: Vec2::new(h.at.x + b[0] - h.bounds[0], h.at.y + b[1] - h.bounds[1]), ..h })
    });
}

/// A Free Transform change `delta` (stage space) was applied to the selection: the held point
/// goes with the artwork.
pub fn carried(app: &mut App, delta: &Affine) {
    if let (Some(h), Some(b)) = (app.xf.held, bounds(app)) {
        let at = delta.apply(h.at);
        app.xf.held = at.is_finite().then_some(Held { bounds: b, at, ..h });
    }
}

/// The frame of the selection now. `None` with nothing selected.
pub fn frame(app: &App) -> Option<Frame> {
    let (m, r) = app.s.transform_frame()?;
    let (mut point, mut moved) = app.s.transformation_point()?;
    let selected = app.s.selected_elements();
    let keeps = matches!(selected.as_slice(), [(_, e)] if matches!(e.kind, ElementKind::Symbol { .. }));
    if !keeps
        && let Some(h) = app.xf.held.filter(|h| h.selection == selection_key(app))
    {
        (point, moved) = (h.at, true);
    }
    let movable = app.s.style.transform_origin == TransformOrigin::Selection;
    Some(Frame { m, b: [r.min.x, r.min.y, r.max.x, r.max.y], point, keeps, moved, movable })
}

/// The handle codes, clockwise from the top left (as the stage draws them).
pub const HANDLES: [(i8, i8); 8] = [(-1, -1), (0, -1), (1, -1), (1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0)];

fn distance_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let (d, v) = (b - a, p - a);
    let l = d.length_sq();
    let t = if l > 0.0 { (v.dot(d) / l).clamp(0.0, 1.0) } else { 0.0 };
    (a + d * t).distance(p)
}

impl Frame {
    /// An upright box (stage space) with its point at the centre.
    pub fn upright(b: [f32; 4]) -> Frame {
        Frame { m: Affine::IDENTITY, b, point: Vec2::new((b[0] + b[2]) * 0.5, (b[1] + b[3]) * 0.5), keeps: false, moved: false, movable: false }
    }

    /// The stage position of handle `(hx, hy)` (-1, 0 or 1 on each axis; `(0, 0)` is the centre).
    pub fn handle(&self, hx: i8, hy: i8) -> Vec2 {
        let [x0, y0, x1, y1] = self.b;
        let along = |h: i8, lo: f32, hi: f32| lo + (hi - lo) * (f32::from(h) + 1.0) * 0.5;
        self.m.apply(Vec2::new(along(hx, x0, x1), along(hy, y0, y1)))
    }

    /// The box's corners on the stage: top left, top right, bottom right, bottom left (of the
    /// box's own space).
    pub fn corners(&self) -> [Vec2; 4] {
        [self.handle(-1, -1), self.handle(1, -1), self.handle(1, 1), self.handle(-1, 1)]
    }

    /// What dragging from screen point `p` would do. `to_screen` maps the stage to the screen.
    pub fn zone(&self, to_screen: impl Fn(Vec2) -> Pos2, p: Pos2) -> Option<XfKind> {
        use crate::stage::{HANDLE, ROTATE_ZONE, SKEW_ZONE};
        if self.movable && to_screen(self.point).distance(p) <= POINT_GRAB {
            return Some(XfKind::Pivot);
        }
        for (hx, hy) in HANDLES {
            let h = to_screen(self.handle(hx, hy));
            if (p.x - h.x).abs() <= HANDLE && (p.y - h.y).abs() <= HANDLE {
                return Some(XfKind::Scale { hx, hy });
            }
        }
        let [tl, tr, br, bl] = self.corners().map(&to_screen);
        // The line between two handles skews (Animate parity, #174); the handles win at its ends.
        for (a, b, hx, hy) in [(tl, tr, 0, -1), (bl, br, 0, 1), (tl, bl, -1, 0), (tr, br, 1, 0)] {
            if distance_to_segment(p, a, b) <= SKEW_ZONE {
                return Some(XfKind::Skew { hx, hy });
            }
        }
        // Inside the (convex) box: on the same side of all four edges.
        let side = |a: Pos2, b: Pos2| (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
        let s = [side(tl, tr), side(tr, br), side(br, bl), side(bl, tl)];
        let inside = s.iter().all(|v| *v >= 0.0) || s.iter().all(|v| *v <= 0.0);
        if inside {
            return Some(XfKind::Move);
        }
        [tl, tr, br, bl].iter().any(|c| c.distance(p) <= ROTATE_ZONE).then_some(XfKind::Rotate)
    }

    /// What a rotation drag started at stage point `start` turns about: the transformation point,
    /// or with `alt` the corner of the box opposite the one nearest `start`.
    pub fn rotate_pivot(&self, start: Pos2, alt: bool) -> Vec2 {
        if !alt {
            return self.point;
        }
        let [tl, tr, br, bl] = self.corners();
        let s = Vec2::new(start.x, start.y);
        [(tl, br), (tr, bl), (br, tl), (bl, tr)].into_iter().min_by(|(a, _), (b, _)| a.distance(s).total_cmp(&b.distance(s))).map_or(self.point, |(_, opp)| opp)
    }

    /// The whole transform (stage space) of a drag of `kind` from stage point `start` to
    /// `cur`. Scaling and skewing act along the box's own axes; a rotation turns about the
    /// transformation point (Alt turns about the opposite corner), and Alt scales and skews about it.
    pub fn total(&self, kind: XfKind, start: Pos2, cur: Pos2, shift: bool, alt: bool) -> Affine {
        match kind {
            XfKind::Pivot => Affine::IDENTITY,
            XfKind::Move => crate::stage::xf_total(kind, self.b, start, cur, shift, alt),
            // A turn is a turn on the stage, however the box is scaled or skewed.
            XfKind::Rotate => {
                let p = self.rotate_pivot(start, alt);
                crate::stage::xf_total_about(kind, self.b, Some(pos2(p.x, p.y)), start, cur, shift, false)
            }
            XfKind::Scale { .. } | XfKind::Skew { .. } => {
                let Some(inv) = self.m.inverse() else { return Affine::IDENTITY };
                let own = |p: Pos2| {
                    let q = inv.apply(Vec2::new(p.x, p.y));
                    pos2(q.x, q.y)
                };
                let about = inv.apply(self.point);
                let local = crate::stage::xf_total_about(kind, self.b, Some(pos2(about.x, about.y)), own(start), own(cur), shift, alt);
                let m = self.m.mul(&local).mul(&inv);
                if m.is_finite() { m } else { Affine::IDENTITY }
            }
        }
    }

    /// The pointer for `kind`: resize arrows follow the box as it is turned.
    pub fn cursor(&self, kind: XfKind, to_screen: impl Fn(Vec2) -> Pos2, dragging: bool) -> CursorIcon {
        // The screen direction a handle moves in, as one of the four arrow pairs.
        let arrows = |from: Vec2, to: Vec2| {
            let d = to_screen(to) - to_screen(from);
            let turn = (d.y.atan2(d.x).to_degrees().rem_euclid(180.0) / 45.0).round() as i32 % 4;
            match turn {
                0 => CursorIcon::ResizeHorizontal,
                1 => CursorIcon::ResizeNwSe,
                2 => CursorIcon::ResizeVertical,
                _ => CursorIcon::ResizeNeSw,
            }
        };
        match kind {
            XfKind::Pivot if dragging => CursorIcon::Grabbing,
            XfKind::Pivot => CursorIcon::Grab,
            XfKind::Move => CursorIcon::Move,
            // No desktop has a rotate cursor: the stage draws its own (#194).
            XfKind::Rotate => CursorIcon::None,
            XfKind::Scale { hx, hy } => arrows(self.handle(0, 0), self.handle(hx, hy)),
            // An edge slides along itself.
            XfKind::Skew { hy: 0, .. } => arrows(self.handle(0, -1), self.handle(0, 1)),
            XfKind::Skew { .. } => arrows(self.handle(-1, 0), self.handle(1, 0)),
        }
    }

    /// `c` (stage space), or the box's handle, corner or centre within [`POINT_SNAP`] screen
    /// pixels of it: where the dragged transformation point lands (#178).
    pub fn snap_point(&self, c: Vec2, zoom: f32) -> Vec2 {
        let reach = POINT_SNAP / zoom.max(1e-3);
        HANDLES.into_iter().chain([(0, 0)]).map(|(hx, hy)| self.handle(hx, hy)).filter(|h| h.distance(c) <= reach).min_by(|a, b| a.distance(c).total_cmp(&b.distance(c))).unwrap_or(c)
    }
}

/// Dragging the transformation point to stage point `c`: an instance keeps it (a document
/// change; the gesture makes the drag one undo step), anything else has it held here. Alt
/// leaves it unsnapped.
pub fn drag_point(app: &mut App, c: Vec2, zoom: f32, alt: bool) {
    let Some(fr) = frame(app) else { return };
    let at = if alt { c } else { fr.snap_point(c, zoom) };
    if !at.is_finite() {
        return;
    }
    if fr.keeps {
        let _ = app.s.execute("modify.transformation_point", &json!({"x": at.x, "y": at.y}));
    } else if let Some(b) = bounds(app) {
        app.xf.held = Some(Held { selection: selection_key(app), bounds: b, at });
    }
}

/// A double-click at screen point `p`: on the transformation point, it goes back to the
/// centre. Answers whether the click was on it.
pub fn double_click(app: &mut App, to_screen: impl Fn(Vec2) -> Pos2, p: Pos2) -> bool {
    let Some(fr) = frame(app).filter(|f| f.movable && to_screen(f.point).distance(p) <= POINT_GRAB) else { return false };
    if !fr.keeps {
        app.xf.held = None;
    } else if fr.moved {
        app.run("modify.reset_transformation_point", json!({}));
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use anicraft_engine::Session;
    use anicraft_engine::doc::ElementId;

    fn near(a: Vec2, b: Vec2) -> bool {
        a.distance(b) < 1e-2
    }

    fn screen(p: Vec2) -> Pos2 {
        pos2(p.x, p.y)
    }

    /// A 100 × 20 box turned 90° about (150, 110): it stands from (140, 60) to (160, 160).
    fn turned() -> Frame {
        let m = Affine::translate(150.0, 110.0).mul(&Affine::rotate(std::f32::consts::FRAC_PI_2));
        Frame { m, b: [-50.0, -10.0, 50.0, 10.0], point: Vec2::new(150.0, 110.0), keeps: true, moved: false, movable: true }
    }

    #[test]
    fn a_turned_box_has_its_handles_where_the_object_is() {
        let f = turned();
        // The box's own right-middle handle is at the bottom of the standing box.
        assert!(near(f.handle(1, 0), Vec2::new(150.0, 160.0)));
        assert!(near(f.handle(-1, -1), Vec2::new(160.0, 60.0)));
        assert_eq!(f.zone(screen, pos2(150.0, 160.0)), Some(XfKind::Scale { hx: 1, hy: 0 }));
        assert_eq!(f.zone(screen, pos2(160.0, 60.0)), Some(XfKind::Scale { hx: -1, hy: -1 }));
        // The point first, then inside, the edges between handles, just outside a corner, far away.
        assert_eq!(f.zone(screen, pos2(152.0, 111.0)), Some(XfKind::Pivot));
        assert_eq!(f.zone(screen, pos2(150.0, 130.0)), Some(XfKind::Move));
        assert_eq!(f.zone(screen, pos2(160.0, 135.0)), Some(XfKind::Skew { hx: 0, hy: -1 }));
        assert_eq!(f.zone(screen, pos2(172.0, 172.0)), Some(XfKind::Rotate));
        assert_eq!(f.zone(screen, pos2(300.0, 300.0)), None);
        // Where the upright bounds would be but the turned box is not: nothing to grab (#175).
        assert_eq!(f.zone(screen, pos2(190.0, 110.0)), None);
        // A point that can't be dragged (each object about its own) is not grabbed.
        let fixed = Frame { movable: false, point: Vec2::new(150.0, 135.0), ..f };
        assert_eq!(fixed.zone(screen, pos2(151.0, 135.0)), Some(XfKind::Move));
        assert_eq!(Frame { movable: true, ..fixed }.zone(screen, pos2(151.0, 135.0)), Some(XfKind::Pivot));
    }

    #[test]
    fn a_turned_boxs_handles_scale_along_its_own_axes() {
        let f = turned();
        // Dragging the box's right-middle handle (bottom of the standing box) 50 px down
        // makes the box half as long again along its own x axis; the far end stays put.
        let m = f.total(XfKind::Scale { hx: 1, hy: 0 }, pos2(150.0, 160.0), pos2(150.0, 210.0), false, false);
        assert!(near(m.apply(f.handle(-1, 0)), f.handle(-1, 0)), "the opposite side stays");
        assert!(near(m.apply(f.handle(1, 0)), Vec2::new(150.0, 210.0)), "the handle follows the pointer");
        let d = m.mul(&f.m).decompose();
        assert!((d.sx - 1.5).abs() < 1e-4 && (d.sy - 1.0).abs() < 1e-4 && d.skew.abs() < 1e-4, "{d:?}");
        assert!((d.rotation.to_degrees() - 90.0).abs() < 1e-3, "still turned 90°, not skewed");
        // Dragged sideways (across the box's axis) the handle does nothing.
        let m = f.total(XfKind::Scale { hx: 1, hy: 0 }, pos2(150.0, 160.0), pos2(190.0, 160.0), false, false);
        assert!(m.m.iter().zip(Affine::IDENTITY.m).all(|(a, b)| (a - b).abs() < 1e-4), "{m:?}");
        // Alt scales about the transformation point: here the centre, so both ends move.
        let m = f.total(XfKind::Scale { hx: 1, hy: 0 }, pos2(150.0, 160.0), pos2(150.0, 210.0), false, true);
        assert!(near(m.apply(f.handle(-1, 0)), Vec2::new(150.0, 10.0)) && near(m.apply(f.point), f.point));
        // A corner scales both of the box's axes; Shift keeps its proportions.
        let m = f.total(XfKind::Scale { hx: 1, hy: 1 }, pos2(140.0, 160.0), pos2(120.0, 260.0), true, false);
        let d = m.mul(&f.m).decompose();
        assert!((d.sx - d.sy).abs() < 1e-4 && d.sx > 1.5, "{d:?}");
        // An edge slides along itself, in the box's own direction.
        let m = f.total(XfKind::Skew { hx: 0, hy: 1 }, pos2(140.0, 110.0), pos2(140.0, 120.0), false, false);
        assert!(near(m.apply(f.handle(0, 1)), Vec2::new(140.0, 120.0)) && near(m.apply(f.handle(0, -1)), f.handle(0, -1)));
    }

    #[test]
    fn a_rotation_turns_about_the_transformation_point() {
        // The point at the box's own left end, (150, 60) on the stage.
        let f = Frame { point: Vec2::new(150.0, 60.0), ..turned() };
        let m = f.total(XfKind::Rotate, pos2(150.0, 170.0), pos2(40.0, 60.0), false, false);
        assert!(near(m.apply(f.point), f.point), "the point stays put");
        // A quarter turn clockwise on screen: the far end swings from below the point to its left.
        assert!(near(m.apply(Vec2::new(150.0, 160.0)), Vec2::new(50.0, 60.0)), "{:?}", m.apply(Vec2::new(150.0, 160.0)));
        // Shift turns in 15° steps.
        let m = f.total(XfKind::Rotate, pos2(150.0, 170.0), pos2(140.0, 170.0), true, false);
        let turn = m.decompose().rotation.to_degrees();
        assert!((turn / 15.0 - (turn / 15.0).round()).abs() < 1e-3, "{turn}");
        // An upright frame's point is its centre: Free Transform as before.
        let u = Frame::upright([0.0, 0.0, 100.0, 50.0]);
        assert_eq!(u.point, Vec2::new(50.0, 25.0));
        assert_eq!(u.total(XfKind::Rotate, pos2(110.0, 25.0), pos2(50.0, 100.0), true, false), crate::stage::xf_total(XfKind::Rotate, u.b, pos2(110.0, 25.0), pos2(50.0, 100.0), true, false));
        assert_eq!(u.total(XfKind::Scale { hx: 1, hy: 1 }, pos2(100.0, 50.0), pos2(200.0, 100.0), false, false), crate::stage::xf_total(XfKind::Scale { hx: 1, hy: 1 }, u.b, pos2(100.0, 50.0), pos2(200.0, 100.0), false, false));
    }

    #[test]
    fn alt_rotation_turns_about_the_opposite_corner_even_on_a_turned_box() {
        let f = Frame { point: Vec2::new(150.0, 60.0), ..turned() };
        let centre = f.handle(0, 0);
        let [tl, tr, br, bl] = f.corners();
        for (grabbed, opposite) in [(tl, br), (tr, bl), (br, tl), (bl, tr)] {
            let out = grabbed + (grabbed - centre).normalized() * 14.0;
            let start = pos2(out.x, out.y);
            let cur = pos2(out.x + 25.0, out.y + 40.0);
            let m = f.total(XfKind::Rotate, start, cur, false, true);
            assert!(near(m.apply(opposite), opposite), "opposite corner {opposite:?} stayed fixed");
            assert!(m.apply(grabbed).distance(grabbed) > 1.0, "grabbed corner {grabbed:?} moved");
        }
        // Shift + Alt still snaps to 15° about the opposite corner.
        let out = tr + (tr - centre).normalized() * 14.0;
        let m = f.total(XfKind::Rotate, pos2(out.x, out.y), pos2(out.x + 60.0, out.y - 10.0), true, true);
        assert!(near(m.apply(bl), bl), "opposite corner stayed fixed with Shift+Alt");
        let turn = m.decompose().rotation.to_degrees();
        assert!((turn / 15.0 - (turn / 15.0).round()).abs() < 1e-3 && turn.abs() > 1.0, "{turn}");
        // An upright frame matches `xf_total` with Alt too.
        let u = Frame::upright([0.0, 0.0, 100.0, 50.0]);
        assert_eq!(u.total(XfKind::Rotate, pos2(112.0, 62.0), pos2(80.0, 95.0), false, true), crate::stage::xf_total(XfKind::Rotate, u.b, pos2(112.0, 62.0), pos2(80.0, 95.0), false, true));
    }

    #[test]
    fn the_dragged_point_snaps_to_the_boxs_handles_and_centre() {
        let f = turned();
        assert!(near(f.snap_point(Vec2::new(158.0, 62.0), 1.0), f.handle(-1, -1)));
        assert!(near(f.snap_point(Vec2::new(151.0, 108.0), 1.0), Vec2::new(150.0, 110.0)));
        assert_eq!(f.snap_point(Vec2::new(300.0, 300.0), 1.0), Vec2::new(300.0, 300.0), "outside the box it is free (#178)");
        // Zoomed in, the reach is the same on screen, so smaller on the stage.
        assert_eq!(f.snap_point(Vec2::new(158.0, 62.0), 8.0), Vec2::new(158.0, 62.0));
        // The arrows follow the turn: the box's own x axis runs down the screen.
        assert_eq!(f.cursor(XfKind::Scale { hx: 1, hy: 0 }, screen, false), CursorIcon::ResizeVertical);
        assert_eq!(f.cursor(XfKind::Scale { hx: 0, hy: 1 }, screen, false), CursorIcon::ResizeHorizontal);
        assert_eq!(Frame::upright([0.0, 0.0, 10.0, 10.0]).cursor(XfKind::Scale { hx: 1, hy: 1 }, screen, false), CursorIcon::ResizeNwSe);
    }

    fn run(app: &mut App, id: &str, p: serde_json::Value) -> serde_json::Value {
        app.s.execute(id, &p).unwrap_or_else(|e| panic!("{id}: {e}"))
    }

    #[test]
    fn an_instance_keeps_its_point_and_a_drawing_holds_one_while_the_tool_is() {
        let mut app = App::with_context(&egui::Context::default(), Session::new(anicraft_engine::doc::Document::default()), Some(std::path::PathBuf::from("unused.png")));
        app.tool = Tool::Transform;
        // A drawing object: its point is held here, not in the document.
        run(&mut app, "draw.rect", json!({"x": 100, "y": 100, "w": 100, "h": 20, "stroke": null, "object": true}));
        let f = frame(&app).unwrap();
        assert!(!f.keeps && !f.moved && f.movable && near(f.point, Vec2::new(150.0, 110.0)));
        let undo = app.s.can_undo();
        drag_point(&mut app, Vec2::new(101.0, 111.0), 1.0, false);
        let f = frame(&app).unwrap();
        assert!(f.moved && near(f.point, Vec2::new(100.0, 110.0)), "snapped to the left-middle handle: {:?}", f.point);
        assert_eq!(app.s.can_undo(), undo, "no document change");
        // A Free Transform change carries it; so does a nudge (the selection only moved).
        let turn = f.total(XfKind::Rotate, pos2(210.0, 110.0), pos2(100.0, 220.0), false, false);
        let [a, b, c, d, e, g] = turn.m;
        run(&mut app, "modify.transform", json!({"a": a, "b": b, "c": c, "d": d, "e": e, "f": g}));
        carried(&mut app, &turn);
        sync(&mut app);
        assert!(near(frame(&app).unwrap().point, Vec2::new(100.0, 110.0)), "the hinge stays where it is");
        let b = app.s.selection_bounds();
        assert!((b.min.x - 90.0).abs() < 0.1 && (b.max.y - 210.0).abs() < 0.1, "the bar swung down from its left end: {b:?}");
        run(&mut app, "element.translate", json!({"dx": 20, "dy": 5}));
        sync(&mut app);
        assert!(near(frame(&app).unwrap().point, Vec2::new(120.0, 115.0)));
        // Double-click puts it back; another tool, or another selection, drops it.
        assert!(double_click(&mut app, screen, pos2(121.0, 116.0)));
        assert!(!frame(&app).unwrap().moved);
        drag_point(&mut app, Vec2::new(300.0, 300.0), 1.0, false);
        app.tool = Tool::Select;
        sync(&mut app);
        app.tool = Tool::Transform;
        assert!(!frame(&app).unwrap().moved);

        // An instance: the drag is a document change, kept and undone as one step.
        let id = ElementId(run(&mut app, "modify.convert_to_symbol", json!({"name": "Arm"}))["instance"].as_u64().unwrap());
        let f = frame(&app).unwrap();
        assert!(f.keeps && !f.moved);
        app.s.begin_gesture("Move Transformation Point");
        for i in 0..10 {
            drag_point(&mut app, Vec2::new(400.0 + i as f32, 400.0), 1.0, false);
        }
        app.s.end_gesture();
        let f = frame(&app).unwrap();
        assert!(f.moved && near(f.point, Vec2::new(409.0, 400.0)));
        let e = app.s.selected_elements().into_iter().map(|(_, e)| e).find(|e| e.id == id).unwrap();
        assert!(e.pivot().is_some());
        // The turn about it leaves it where it is.
        let turn = f.total(XfKind::Rotate, pos2(500.0, 400.0), pos2(409.0, 500.0), false, false);
        assert!(near(turn.apply(f.point), f.point));
        assert!(double_click(&mut app, screen, pos2(409.0, 400.0)));
        assert!(!frame(&app).unwrap().moved);
        run(&mut app, "edit.undo", json!({}));
        assert!(frame(&app).unwrap().moved, "the reset is one step");
        run(&mut app, "edit.undo", json!({}));
        assert!(!frame(&app).unwrap().moved, "and so is the whole drag");
        // With each object about its own point (ADR 0109) the point is not dragged.
        run(&mut app, "modify.transform_origin", json!({"mode": "center"}));
        assert!(!frame(&app).unwrap().movable);
    }
}
