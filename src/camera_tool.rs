//! The Camera tool and the camera frame on the stage (ADR 0027).
//!
//! While editing, the stage is in stage space and the camera shows as its frame: an outline in
//! the camera layer's colour of what the movie will show, with the centre marked. With the
//! Camera tool, a drag pans the camera, the round handle above the frame turns it, and the
//! wheel zooms it; each drag (or burst of wheel turns) is one undo step. With View › Preview
//! Through Camera the stage shows the movie's picture instead: there a drag moves the picture
//! under the pointer (the camera the other way), as panning a view does, and the Selection
//! tool picks and drags artwork where the movie shows it (ADR 0082): the press picks through
//! the camera (`element.pick {view: camera}`), and each move is mapped back to the stage
//! through the inverse of the picked layer's view (camera and depth), so the artwork follows
//! the pointer on the picture.

use crate::app::{App, Drag, Tool};
use crate::theme::{T, to_color32};
use anicraft_engine::doc::Camera;
use anicraft_engine::geom::{Affine, Vec2};
use egui::{Color32, CursorIcon, Pos2, Rect, Stroke, StrokeKind, pos2, vec2};
use serde_json::json;

/// How far the rotate handle sits outside the frame, and its grab radius (screen points).
const KNOB_OFFSET: f32 = 26.0;
const KNOB_GRAB: f32 = 12.0;
const PIVOT_GRAB: f32 = 8.0;
const CORNER_GRAB: f32 = 10.0;
const EDGE_GRAB: f32 = 6.0;
const ROTATE_ZONE: f32 = 22.0;
/// How long after the last wheel turn a zoom burst ends (seconds).
const WHEEL_BURST: f64 = 0.45;

/// The stage on screen: `screen = origin + stage · zoom`; `preview` when it shows the movie.
#[derive(Clone, Copy)]
pub struct Geo {
    pub origin: Pos2,
    pub zoom: f32,
    #[allow(dead_code)]
    pub stage_rect: Rect,
    pub preview: bool,
}

impl Geo {
    fn to_screen(self, p: Vec2) -> Pos2 {
        pos2(self.origin.x + p.x * self.zoom, self.origin.y + p.y * self.zoom)
    }
}

/// The scene's camera at the playhead (the rest camera on a blank camera keyframe).
pub fn camera(app: &App) -> Option<Camera> {
    app.s.timeline().camera_state_at(app.s.frame, &app.s.doc.stage)
}

/// Whether the stage renders through the scene's active visible camera (automatic whenever a
/// visible camera layer exists).
pub fn previewing(app: &App) -> bool {
    app.s.timeline().camera_layer().is_some_and(|l| l.visible)
}

/// Picking the Camera tool adds a camera when the scene has none (as Animate does), once per
/// pick: undoing the add leaves the tool in hand without adding it again.
pub fn on_tool_change(app: &mut App) {
    if app.tool == Tool::Camera && app.last_tool != Tool::Camera && app.s.timeline().camera_layer().is_none() && app.run("camera.add", json!({})).is_some() {
        app.status_ok(tr!("Added a camera. Drag on the stage to pan it, turn it with the round handle, zoom with the wheel. {undo} to undo.", undo = app.undo_key()));
    }
    app.last_tool = app.tool;
}

/// Interactive zone on the camera frame when the Camera tool is active.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zone {
    /// Centre pivot point (`cam.center`): shows the drag/grab cursor.
    Pivot,
    /// Top-left corner (`corner = 0`): shows the Truck In/Out cursor and zooms on drag.
    Truck,
    /// Other corner handles (`1` top-right, `2` bottom-right, `3` bottom-left): scale/zoom on drag.
    Scale(usize),
    /// Just outside a corner or on the top rotate handle: shows the rotation cursor.
    Rotate,
    /// Camera frame edge or interior (outside the pivot dot): shows the move cursor.
    Move,
}

/// The keyframe edited by Camera tool handles, wheel, Properties, and arrow nudges: resolves
/// authoritatively from `app.camera_key_sel` (by keyframe frame identity) when valid keyframes are
/// selected, falling back to `app.s.frame` when no keyframe is explicitly selected.
pub fn active_target_frame(app: &App) -> u32 {
    if app.s.editing.is_empty()
        && let Some(cam_layer) = app.s.timeline().camera_layer()
    {
        let mut sel = app.camera_key_sel.iter().copied().filter(|f| cam_layer.keyframe_starting_at(*f).is_some());
        if let Some(first_sel) = sel.next() {
            if sel.next().is_none() {
                return first_sel;
            }
            if cam_layer.keyframe_starting_at(app.s.frame).is_some() && app.camera_key_sel.contains(&app.s.frame) {
                return app.s.frame;
            }
            return app
                .camera_key_sel
                .iter()
                .copied()
                .filter(|f| cam_layer.keyframe_starting_at(*f).is_some())
                .max()
                .unwrap_or(app.s.frame);
        }
    }
    app.s.frame
}

/// The active target keyframe's camera transform (falling back to the playhead camera).
pub fn active_camera(app: &App) -> Option<Camera> {
    camera_at_frame(app, active_target_frame(app)).or_else(|| camera(app))
}

fn camera_at_frame(app: &App, frame: u32) -> Option<Camera> {
    app.s.timeline().camera_state_at(frame, &app.s.doc.stage)
}

fn stage_to_screen(app: &App, g: &Geo, p: Vec2) -> Pos2 {
    let q = if g.preview
        && let Some(c) = camera(app)
    {
        c.stage_view(&app.s.doc.stage).apply(p)
    } else {
        p
    };
    g.to_screen(q)
}

fn frame_corners(cam: &Camera, app: &App, g: &Geo) -> [Pos2; 4] {
    cam.frame(&app.s.doc.stage).map(|p| stage_to_screen(app, g, p))
}

fn dist_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let (d, v) = (b - a, p - a);
    let l = d.length_sq();
    let t = if l > 0.0 { (v.dot(d) / l).clamp(0.0, 1.0) } else { 0.0 };
    (a + d * t).distance(p)
}

/// The camera's centre, rotate handle and the middle of the frame's top edge, on screen.
fn handles(cam: &Camera, app: &App, g: &Geo) -> (Pos2, Pos2, Pos2) {
    let f = frame_corners(cam, app, g);
    let centre = stage_to_screen(app, g, cam.center);
    let top = pos2((f[0].x + f[1].x) * 0.5, (f[0].y + f[1].y) * 0.5);
    let up = top - centre;
    let dir = if up.length() > 1e-3 { up / up.length() } else { vec2(0.0, -1.0) };
    (centre, top + dir * KNOB_OFFSET, top)
}

/// Hit-tests `cam`'s handles, edges, interior, and outside-corner rotate zones.
fn zone_of_cam(cam: &Camera, app: &App, g: &Geo, p: Pos2) -> Option<Zone> {
    let (centre, knob, _) = handles(cam, app, g);
    let f = frame_corners(cam, app, g);
    let mut best: Option<(f32, Zone)> = None;
    let mut consider = |dist: f32, radius: f32, zone: Zone| {
        if dist <= radius && best.is_none_or(|(bd, _)| dist < bd) {
            best = Some((dist, zone));
        }
    };
    consider(knob.distance(p), KNOB_GRAB, Zone::Rotate);
    consider(centre.distance(p), PIVOT_GRAB, Zone::Pivot);
    consider(f[0].distance(p), CORNER_GRAB, Zone::Truck);
    for (i, corner) in f.iter().enumerate().skip(1) {
        consider(corner.distance(p), CORNER_GRAB, Zone::Scale(i));
    }
    if let Some((_, zone)) = best {
        return Some(zone);
    }
    for (a, b) in [(f[0], f[1]), (f[1], f[2]), (f[2], f[3]), (f[3], f[0])] {
        if dist_to_segment(p, a, b) <= EDGE_GRAB {
            return Some(Zone::Move);
        }
    }
    let side = |a: Pos2, b: Pos2| (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
    let s = [side(f[0], f[1]), side(f[1], f[2]), side(f[2], f[3]), side(f[3], f[0])];
    let inside = s.iter().all(|v| *v >= 0.0) || s.iter().all(|v| *v <= 0.0);
    if inside {
        return Some(Zone::Move);
    }
    f.iter().any(|c| c.distance(p) <= ROTATE_ZONE).then_some(Zone::Rotate)
}

/// Resolves the single authoritative active camera keyframe `(frame, cam, zone)` under screen point `p`.
fn hit_target(app: &App, g: &Geo, p: Pos2) -> Option<(u32, Camera, Zone)> {
    let frame = active_target_frame(app);
    let cam = camera_at_frame(app, frame)?;
    zone_of_cam(&cam, app, g, p).map(|z| (frame, cam, z))
}

/// Which interactive zone of the active camera frame lies under screen point `p`.
#[cfg(test)]
pub fn zone(app: &App, g: &Geo, p: Pos2) -> Option<Zone> {
    hit_target(app, g, p).map(|(_, _, z)| z)
}

fn scale_cursor(d: egui::Vec2) -> CursorIcon {
    let turn = (d.y.atan2(d.x).to_degrees().rem_euclid(180.0) / 45.0).round() as i32 % 4;
    match turn {
        0 => CursorIcon::ResizeHorizontal,
        1 => CursorIcon::ResizeNwSe,
        2 => CursorIcon::ResizeVertical,
        _ => CursorIcon::ResizeNeSw,
    }
}

/// Cursor icon for the Camera tool at screen point `p`.
pub fn cursor(app: &App, g: &Geo, p: Pos2) -> CursorIcon {
    match hit_target(app, g, p) {
        Some((_, _, Zone::Pivot)) => CursorIcon::Grab,
        Some((_, _, Zone::Move)) => CursorIcon::Move,
        Some((_, cam, Zone::Scale(i))) => {
            let (centre, _, _) = handles(&cam, app, g);
            let f = frame_corners(&cam, app, g);
            scale_cursor(f[i] - centre)
        }
        // Custom vector cursors (Truck In/Out and Rotate) are painted on the tooltip layer.
        Some((_, _, Zone::Truck | Zone::Rotate)) => CursorIcon::None,
        None => CursorIcon::Grab,
    }
}

/// Paints the Camera tool's custom cursor (Truck In/Out or Rotate) at `at`.
pub fn draw_cursor(app: &App, g: &Geo, painter: &egui::Painter, at: Pos2) {
    let (cam, is_truck) = match app.drag {
        Drag::CameraZoom { frame, .. } => (camera_at_frame(app, frame).or_else(|| camera(app)), true),
        Drag::CameraRotate { frame, .. } => (camera_at_frame(app, frame).or_else(|| camera(app)), false),
        _ => match hit_target(app, g, at) {
            Some((_, c, z)) => (Some(c), z == Zone::Truck),
            None => (active_camera(app), false),
        },
    };
    let Some(cam) = cam else { return };
    let (centre, _, _) = handles(&cam, app, g);
    if is_truck {
        let dir = frame_corners(&cam, app, g)[0] - centre;
        truck_cursor(painter, at, dir);
    } else {
        crate::stage::rotate_cursor(painter, at, at - centre);
    }
}

/// Diagonal double-arrow Truck In/Out cursor at `at` along `outward` (dark with a light edge).
fn truck_cursor(painter: &egui::Painter, at: Pos2, outward: egui::Vec2) {
    let u = if outward.length() > 1e-3 { outward.normalized() } else { vec2(-std::f32::consts::FRAC_1_SQRT_2, -std::f32::consts::FRAC_1_SQRT_2) };
    let n = vec2(-u.y, u.x);
    let shaft = [at - u * 6.5, at + u * 6.5];
    for (width, colour) in [(4.0, Color32::WHITE), (1.6, Color32::BLACK)] {
        painter.line_segment(shaft, Stroke::new(width, colour));
    }
    for tip_dir in [u, -u] {
        let pts = vec![at + tip_dir * 10.0, at + tip_dir * 5.0 + n * 4.0, at + tip_dir * 5.0 - n * 4.0];
        painter.add(egui::Shape::convex_polygon(pts, Color32::BLACK, Stroke::new(1.0, Color32::WHITE)));
    }
    painter.circle(at, 2.5, Color32::WHITE, Stroke::new(1.5, Color32::BLACK));
}

/// A Camera tool drag starting at screen point `press`.
pub fn drag_start(app: &mut App, g: &Geo, press: Pos2) -> Drag {
    let hit = hit_target(app, g, press);
    let (frame, cam, zone) = match hit {
        Some((f, c, z)) => (f, c, Some(z)),
        None => {
            let target = active_target_frame(app);
            let Some(c) = camera_at_frame(app, target).or_else(|| camera(app)) else { return Drag::None };
            (target, c, None)
        }
    };
    let (centre, _, _) = handles(&cam, app, g);
    match zone {
        Some(Zone::Rotate) => {
            // Undo labels stay English in the history and are translated where they show.
            app.s.begin_gesture(tr_noop!("Rotate Camera"));
            let d = press - centre;
            Drag::CameraRotate { frame, last: d.y.atan2(d.x) }
        }
        Some(Zone::Truck | Zone::Scale(_)) => {
            app.s.begin_gesture(tr_noop!("Zoom Camera"));
            Drag::CameraZoom { frame, last_dist: (press - centre).length().max(4.0) }
        }
        Some(Zone::Pivot | Zone::Move) | None => {
            app.s.begin_gesture(tr_noop!("Move Camera"));
            Drag::CameraPan { frame, last: press }
        }
    }
}

/// Continues a Camera tool drag to screen point `cur`.
pub fn drag_to(app: &mut App, g: &Geo, drag: &mut Drag, cur: Pos2) {
    match drag {
        Drag::CameraPan { frame, last } => {
            let Some(cam) = camera_at_frame(app, *frame).or_else(|| camera(app)) else { return };
            let d = (cur - *last) / g.zoom.max(1e-3);
            if d.length_sq() <= 0.0 {
                return;
            }
            *last = cur;
            // Previewing at the playhead: the picture follows the pointer, so the camera moves
            // the opposite way (in the playhead camera's orientation & zoom). Dragging a
            // separate keyframe's overlay moves that box forward in the same view space.
            let v = if g.preview {
                let view_cam = camera(app).unwrap_or(cam);
                let sign = if *frame == app.s.frame { -1.0 } else { 1.0 };
                Affine::rotate(view_cam.rotation).apply_vector(Vec2::new(d.x, d.y)) * (sign / view_cam.zoom)
            } else {
                Vec2::new(d.x, d.y)
            };
            app.run("camera.set", json!({"frame": *frame, "dx": v.x, "dy": v.y}));
        }
        Drag::CameraRotate { frame, last } => {
            let Some(cam) = camera_at_frame(app, *frame).or_else(|| camera(app)) else { return };
            let (centre, _, _) = handles(&cam, app, g);
            let d = cur - centre;
            if d.length() < 4.0 {
                return;
            }
            let a = d.y.atan2(d.x);
            let mut turn = a - *last;
            // The short way round, so crossing ±180° doesn't spin the camera.
            turn = (turn + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
            *last = a;
            let sign = if g.preview && *frame == app.s.frame { -1.0 } else { 1.0 };
            let deg = sign * turn.to_degrees();
            if deg.abs() > 1e-4 {
                app.run("camera.set", json!({"frame": *frame, "rotate_by": deg}));
            }
        }
        Drag::CameraZoom { frame, last_dist } => {
            let Some(cam) = camera_at_frame(app, *frame).or_else(|| camera(app)) else { return };
            let (centre, _, _) = handles(&cam, app, g);
            let dist = (cur - centre).length().max(4.0);
            let factor = (*last_dist / dist).clamp(0.5, 2.0);
            *last_dist = dist;
            if (factor - 1.0).abs() > 1e-4 {
                app.run("camera.set", json!({"frame": *frame, "zoom_by": factor}));
            }
        }
        _ => {}
    }
}

/// One wheel turn with the Camera tool: zooms the camera; turns close together are one step.
pub fn wheel(app: &mut App, scroll_y: f32, now: f64) {
    if !matches!(app.drag, Drag::None | Drag::CameraWheel { .. }) || active_camera(app).is_none() || scroll_y == 0.0 {
        return;
    }
    if !matches!(app.drag, Drag::CameraWheel { .. }) {
        app.s.begin_gesture(tr_noop!("Zoom Camera"));
    }
    app.drag = Drag::CameraWheel { until: now + WHEEL_BURST };
    let by = (scroll_y * 0.0025).clamp(-0.5, 0.5).exp();
    let frame = active_target_frame(app);
    app.run("camera.set", json!({"frame": frame, "zoom_by": by}));
}

/// Ends a wheel burst once the wheel has been still for a moment (the gesture then closes).
pub fn tick(app: &mut App, ctx: &egui::Context, now: f64) {
    if let Drag::CameraWheel { until } = app.drag {
        if now >= until {
            app.drag = Drag::None;
        } else {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64((until - now).max(0.01)));
        }
    }
}

/// Drops any selected camera keyframe frames that no longer exist on the scene's camera layer.
pub fn sync_key_selection(app: &mut App) {
    if !app.s.editing.is_empty() {
        app.camera_key_sel.clear();
        return;
    }
    let Some(cam_layer) = app.s.timeline().camera_layer() else {
        app.camera_key_sel.clear();
        return;
    };
    app.camera_key_sel.retain(|f| cam_layer.keyframe_starting_at(*f).is_some());
}

/// The single `(frame, camera)` pair drawn as the Stage camera editing overlay, when active.
pub fn active_overlay_target(app: &App) -> Option<(u32, Camera)> {
    if app.tool != Tool::Camera || !app.s.editing.is_empty() {
        return None;
    }
    app.s.timeline().camera_layer()?;
    let target_frame = active_target_frame(app);
    camera_at_frame(app, target_frame).map(|cam| (target_frame, cam))
}

/// Draws the single active camera bounding box, pivot marker, and interactive handles only while
/// the Camera tool is active on the main timeline.
pub fn draw(app: &App, painter: &egui::Painter, g: &Geo, hot: Option<Pos2>) {
    let Some((target_frame, cam)) = active_overlay_target(app) else { return };
    let Some(layer) = app.s.timeline().camera_layer() else { return };
    let hovered = hot.filter(|_| app.drag == Drag::None).and_then(|p| hit_target(app, g, p)).filter(|(f, _, _)| *f == target_frame).map(|(_, _, z)| z);
    let color = to_color32(layer.color);
    let color = if layer.visible { color } else { color.gamma_multiply(0.45) };
    let f = frame_corners(&cam, app, g);
    let stroke = Stroke::new(if hovered == Some(Zone::Move) { 2.5 } else { 2.0 }, color);
    painter.add(egui::Shape::closed_line(f.to_vec(), stroke));
    // The centre, as a small cross.
    let c = stage_to_screen(app, g, cam.center);
    painter.line_segment([c - vec2(6.0, 0.0), c + vec2(6.0, 0.0)], stroke);
    painter.line_segment([c - vec2(0.0, 6.0), c + vec2(0.0, 6.0)], stroke);
    draw_frame_handles(painter, &cam, app, g, color, 4.0, hovered);
}

/// Exposes the active camera bounding box as a keyboard-focusable accessible node on the Stage
/// with its keyframe identity, transform state, and visible focus ring.
pub fn a11y_control(app: &App, ui: &mut egui::Ui, g: &Geo, canvas: Rect) {
    let Some((target_frame, cam)) = active_overlay_target(app) else { return };
    let f = frame_corners(&cam, app, g);
    let rect = Rect::from_points(&f).intersect(canvas);
    if rect.width() < 1.0 || rect.height() < 1.0 {
        return;
    }
    let id = egui::Id::unique("stage_camera_box");
    let resp = ui.interact(rect, id, egui::Sense::focusable_noninteractive());
    let label = tr!(
        "Camera frame {frame} · x {x}, y {y} · zoom {zoom}% · rotation {rot}° · Arrows move, +/− zoom, Alt+Arrows rotate",
        frame = target_frame + 1,
        x = format!("{:.0}", cam.center.x),
        y = format!("{:.0}", cam.center.y),
        zoom = format!("{:.0}", cam.zoom * 100.0),
        rot = format!("{:.0}", cam.rotation.to_degrees()),
    );
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, true, &label));
    ui.ctx().accesskit_node_builder(resp.id, |node| node.set_selected(true));
    if resp.has_focus() {
        ui.memory_mut(|m| m.set_focus_lock_filter(id, egui::EventFilter { horizontal_arrows: true, vertical_arrows: true, tab: false, escape: false }));
        ui.painter().add(egui::Shape::closed_line(f.to_vec(), Stroke::new(2.0, T.accent)));
    }
}

fn draw_frame_handles(painter: &egui::Painter, cam: &Camera, app: &App, g: &Geo, color: Color32, pivot_r: f32, hot: Option<Zone>) {
    let (centre, knob, edge) = handles(cam, app, g);
    let f = frame_corners(cam, app, g);
    for (i, corner) in f.iter().enumerate() {
        let is_hot = hot == Some(if i == 0 { Zone::Truck } else { Zone::Scale(i) });
        let sz = if is_hot { 9.0 } else { 7.0 };
        let r = Rect::from_center_size(*corner, vec2(sz, sz));
        painter.rect_filled(r, 0.0, Color32::WHITE);
        painter.rect_stroke(r, 0.0, Stroke::new(1.5, if is_hot { T.accent } else { color }), StrokeKind::Outside);
    }
    let pivot_hot = hot == Some(Zone::Pivot);
    painter.circle(centre, if pivot_hot { pivot_r + 1.5 } else { pivot_r }, Color32::WHITE, Stroke::new(1.5, if pivot_hot { T.accent } else { color }));
    painter.line_segment([edge, knob], Stroke::new(1.5, color));
    let knob_hot = hot == Some(Zone::Rotate);
    painter.circle(knob, if knob_hot { 7.0 } else { 5.5 }, Color32::WHITE, Stroke::new(1.5, if knob_hot { T.accent } else { color }));
}

/// A Selection tool press on the camera preview at picture point `at`: picks what the movie
/// shows there and, on artwork, starts moving the selection (one undo step).
pub fn select_press(app: &mut App, at: Vec2, add: bool) -> Drag {
    app.bone_sel = None;
    let hit = app.run("element.pick", json!({"x": at.x, "y": at.y, "add": add, "view": "camera"}));
    if hit.is_none_or(|v| v["element"].is_null()) || app.s.selection.is_empty() {
        return Drag::None;
    }
    app.s.begin_gesture(tr_noop!("Move"));
    Drag::CameraMove { last: at }
}

/// Continues a move on the preview to picture point `cur`: the picture's step mapped back to
/// the stage through the active layer's view, so the artwork stays under the pointer.
pub fn move_to(app: &mut App, last: &mut Vec2, cur: Vec2) {
    let d = cur - *last;
    if d.length() <= 0.0 {
        return;
    }
    *last = cur;
    let back = app.s.camera_view(app.s.layer).and_then(|v| v.inverse()).unwrap_or(Affine::IDENTITY);
    let s = back.apply_vector(d);
    if s.x.is_finite() && s.y.is_finite() {
        app.run("element.translate", json!({"dx": s.x, "dy": s.y}));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, Key, Modifiers, PointerButton};

    fn test_app() -> App {
        let ctx = egui::Context::default();
        let session = anicraft_engine::Session::new(anicraft_engine::doc::Document::default());
        App::with_context(&ctx, session, Some(std::path::PathBuf::from("unused.png")))
    }

    fn run_frame(ctx: &egui::Context, app: &mut App, events: Vec<Event>) {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 800.0))),
            events,
            ..Default::default()
        };
        ctx.run_ui(input, |ui| app.frame(ui)).textures_delta.clear();
    }

    fn drag_slider(ctx: &egui::Context, app: &mut App, from: Pos2, to: Pos2) {
        run_frame(ctx, app, vec![Event::PointerMoved(from)]);
        run_frame(ctx, app, vec![Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }]);
        for i in 1..=6 {
            let p = from + (to - from) * (i as f32 / 6.0);
            run_frame(ctx, app, vec![Event::PointerMoved(p)]);
        }
        run_frame(ctx, app, vec![Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }]);
        run_frame(ctx, app, vec![]);
    }

    #[test]
    fn previewing_requires_visible_camera_layer() {
        let mut app = test_app();
        assert!(!previewing(&app), "previewing is false when no camera layer exists");

        // Add camera: stage automatically renders through the visible camera layer without a View menu toggle.
        assert!(app.run("camera.add", json!({})).is_some());
        assert!(previewing(&app), "previewing is active automatically when a visible camera layer exists");

        // Hide camera layer: camera is turned off, so previewing must be false (artwork editing unpaused).
        let cam_id = app.s.timeline().camera_layer().expect("camera layer").id;
        assert!(app.run("layer.toggle_visible", json!({"layer": cam_id.0})).is_some());
        assert!(!app.s.timeline().camera_layer().expect("camera layer").visible);
        assert!(!previewing(&app), "hiding the camera layer turns off camera preview so artwork can be edited");

        // Unhide camera layer: camera rendering returns automatically.
        assert!(app.run("layer.toggle_visible", json!({"layer": cam_id.0})).is_some());
        assert!(app.s.timeline().camera_layer().expect("camera layer").visible);
        assert!(previewing(&app), "unhiding the camera layer restores camera rendering");
    }

    #[test]
    fn camera_layer_defaults_to_blue_frame() {
        let mut app = test_app();
        assert!(app.run("camera.add", json!({})).is_some());
        let c = app.s.timeline().camera_layer().expect("camera layer").color;
        assert!(c.b > c.r && c.b > c.g, "camera frame colour should be blue, got {c:?}");
    }

    #[test]
    fn no_floating_camera_toolbar_and_stage_handles_cover_all_controls() {
        let ctx = egui::Context::default();
        let session = anicraft_engine::Session::new(anicraft_engine::doc::sample::demo());
        let mut app = App::with_context(&ctx, session, Some(std::path::PathBuf::from("unused.png")));
        app.tool = Tool::Camera;
        for _ in 0..3 {
            run_frame(&ctx, &mut app, vec![]);
        }

        // Verify no floating camera toolbar area or zoom/rotate slider accessibility node is rendered.
        assert!(ctx.memory(|m| m.area_rect(egui::Id::unique("camera_controls"))).is_none(), "redundant floating camera toolbar must not render");
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 800.0))), ..Default::default() };
        let mut out = ctx.run_ui(input, |ui| app.frame(ui));
        out.textures_delta.clear();
        if let Some(upd) = out.platform_output.accesskit_update.as_ref() {
            for (_, node) in &upd.nodes {
                let lbl = node.label().unwrap_or("");
                assert!(!matches!(lbl, "Camera zoom mode" | "Camera rotate mode" | "Camera zoom slider" | "Camera rotate slider"), "unexpected floating toolbar control: {lbl}");
            }
        }

        // Reposition, zoom (wheel + corner handles), and rotate still work directly on Stage without the toolbar.
        let cam0 = camera(&app).expect("camera");
        wheel(&mut app, 120.0, 1.0);
        let z_wheel = camera(&app).expect("camera").zoom;
        assert!(z_wheel > cam0.zoom, "wheel zooms camera in: {} -> {z_wheel}", cam0.zoom);

        let canvas = app.stage_cache.canvas;
        let size = vec2(app.s.doc.stage.width as f32, app.s.doc.stage.height as f32);
        let raw = canvas.center() + app.view.pan - size * app.view.zoom * 0.5;
        let origin = pos2(raw.x.round(), raw.y.round());
        let g = Geo { origin, zoom: app.view.zoom, stage_rect: Rect::from_min_size(origin, size * app.view.zoom), preview: previewing(&app) };
        let cam_now = camera(&app).expect("camera");
        let (centre, knob, _) = handles(&cam_now, &app, &g);

        // Reposition via pivot handle drag.
        drag_slider(&ctx, &mut app, centre, centre + vec2(40.0, -25.0));
        let moved = camera(&app).expect("camera");
        assert_ne!(moved.center, cam_now.center, "pivot drag repositions camera");

        // Rotate via top rotate knob drag.
        let r0 = moved.rotation;
        let (_, knob2, _) = handles(&moved, &app, &Geo { origin, zoom: app.view.zoom, stage_rect: Rect::from_min_size(origin, size * app.view.zoom), preview: previewing(&app) });
        drag_slider(&ctx, &mut app, knob2, knob2 + vec2(30.0, 10.0));
        let r1 = camera(&app).expect("camera").rotation;
        assert!((r1 - r0).abs() > 1e-3, "rotate knob turns camera: {r0} -> {r1}");
        let _ = knob;

        // Scale via bottom-right corner handle drag (Zone::Scale(2)).
        let cam3 = camera(&app).expect("camera");
        let g3 = Geo { origin, zoom: app.view.zoom, stage_rect: Rect::from_min_size(origin, size * app.view.zoom), preview: previewing(&app) };
        let f3 = frame_corners(&cam3, &app, &g3);
        let (c3, _, _) = handles(&cam3, &app, &g3);
        let z_before = cam3.zoom;
        drag_slider(&ctx, &mut app, f3[2], f3[2] + (c3 - f3[2]) * 0.25);
        assert!(camera(&app).expect("camera").zoom > z_before, "corner handle scales camera");
    }

    #[test]
    fn camera_frame_hover_zones_switch_cursors_and_drive_truck_and_rotate() {
        let ctx = egui::Context::default();
        let session = anicraft_engine::Session::new(anicraft_engine::doc::sample::demo());
        let mut app = App::with_context(&ctx, session, Some(std::path::PathBuf::from("unused.png")));
        app.tool = Tool::Camera;
        for _ in 0..3 {
            run_frame(&ctx, &mut app, vec![]);
        }
        let cam = camera(&app).expect("camera");
        let canvas = app.stage_cache.canvas;
        let size = vec2(app.s.doc.stage.width as f32, app.s.doc.stage.height as f32);
        let raw = canvas.center() + app.view.pan - size * app.view.zoom * 0.5;
        let origin = pos2(raw.x.round(), raw.y.round());
        let g = Geo { origin, zoom: app.view.zoom, stage_rect: Rect::from_min_size(origin, size * app.view.zoom), preview: previewing(&app) };
        let f = frame_corners(&cam, &app, &g);
        let (centre, _, _) = handles(&cam, &app, &g);

        let hover_cursor = |app: &mut App, p: Pos2| {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 800.0))), events: vec![Event::PointerMoved(p)], ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| app.frame(ui));
            out.textures_delta.clear();
            out.platform_output.cursor_icon
        };

        // Centre pivot point -> drag (Grab) icon.
        assert_eq!(zone(&app, &g, centre), Some(Zone::Pivot));
        assert_eq!(hover_cursor(&mut app, centre), CursorIcon::Grab);

        // Frame edge and frame interior -> Move icon.
        let top_edge = pos2((f[0].x + f[1].x) * 0.5, f[0].y);
        let inside = centre + vec2(30.0, 20.0);
        assert_eq!(zone(&app, &g, top_edge), Some(Zone::Move));
        assert_eq!(hover_cursor(&mut app, top_edge), CursorIcon::Move);
        assert_eq!(zone(&app, &g, inside), Some(Zone::Move));
        assert_eq!(hover_cursor(&mut app, inside), CursorIcon::Move);

        // Top-left corner -> Truck In/Out icon (custom drawn, OS cursor None).
        assert_eq!(zone(&app, &g, f[0]), Some(Zone::Truck));
        assert_eq!(hover_cursor(&mut app, f[0]), CursorIcon::None);

        // Just outside a corner -> rotation icon (custom drawn, OS cursor None).
        let outside_br = f[2] + vec2(13.0, 13.0);
        assert_eq!(zone(&app, &g, outside_br), Some(Zone::Rotate));
        assert_eq!(hover_cursor(&mut app, outside_br), CursorIcon::None);

        // Dragging the top-left corner inward trucks in (zooms in).
        let z0 = camera(&app).expect("camera").zoom;
        let inward = f[0] + (centre - f[0]) * 0.25;
        drag_slider(&ctx, &mut app, f[0], inward);
        let z1 = camera(&app).expect("camera").zoom;
        assert!(z1 > z0, "dragging top-left corner inward trucks in: {z0} -> {z1}");

        // Dragging just outside a corner rotates the camera.
        let cam2 = camera(&app).expect("camera");
        let f2 = frame_corners(&cam2, &app, &g);
        let out_corner = f2[2] + vec2(13.0, 13.0);
        let r0 = cam2.rotation;
        drag_slider(&ctx, &mut app, out_corner, out_corner + vec2(-15.0, 15.0));
        let r1 = camera(&app).expect("camera").rotation;
        assert!((r1 - r0).abs() > 1e-3, "dragging just outside a corner rotates camera: {r0} -> {r1}");
    }

    #[test]
    fn single_active_camera_box_follows_timeline_keyframe_selection() {
        let ctx = egui::Context::default();
        let session = anicraft_engine::Session::new(anicraft_engine::doc::sample::demo());
        let mut app = App::with_context(&ctx, session, Some(std::path::PathBuf::from("unused.png")));
        assert!(app.run("camera.add", json!({})).is_some());
        assert!(app.run("timeline.insert_keyframe", json!({"frame": 12})).is_some());
        assert!(app.run("timeline.insert_keyframe", json!({"frame": 23})).is_some());
        assert!(app.run("camera.set", json!({"frame": 0, "x": 500.0, "y": 300.0, "zoom": 100.0})).is_some());
        assert!(app.run("camera.set", json!({"frame": 12, "x": 640.0, "y": 360.0, "zoom": 130.0, "rotation": 10.0})).is_some());
        assert!(app.run("camera.set", json!({"frame": 23, "x": 780.0, "y": 420.0, "zoom": 160.0})).is_some());
        app.tool = Tool::Camera;

        for _ in 0..3 {
            run_frame(&ctx, &mut app, vec![]);
        }
        let cam_id = app.s.timeline().camera_layer().expect("camera layer").id;
        let cam_color = to_color32(app.s.timeline().camera_layer().expect("camera layer").color);
        let cam_row = app.drop.timeline.as_ref().expect("timeline geo").rows.iter().find(|(_, id)| *id == cam_id).map(|(r, _)| *r).expect("camera row");
        let (_, cells_left, cell_w, scroll) = ctx.data(|d| d.get_temp::<(Rect, f32, f32, f32)>(egui::Id::unique("tl_geometry"))).expect("tl_geometry");
        let cell_at = |f: u32| pos2(cells_left + (f as f32 - scroll + 0.5) * cell_w, cam_row.center().y);

        let click_cell = |app: &mut App, p: Pos2| {
            run_frame(&ctx, app, vec![Event::PointerMoved(p)]);
            run_frame(&ctx, app, vec![Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }]);
            run_frame(&ctx, app, vec![Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }]);
        };

        let key_count = |app: &App| app.s.timeline().camera_layer().expect("cam layer").keyframes.len();
        let initial_keys = key_count(&app);
        let c0 = app.s.timeline().camera_state_at(0, &app.s.doc.stage).unwrap();
        let c12 = app.s.timeline().camera_state_at(12, &app.s.doc.stage).unwrap();
        let c23 = app.s.timeline().camera_state_at(23, &app.s.doc.stage).unwrap();

        // Repeatedly switch between first (0), intermediate (12), and last (23) keyframes in Timeline:
        // only ONE bounding box is displayed each time, matching the selected keyframe without
        // altering stored transforms or creating duplicate keyframes.
        for (target_f, expected_cam) in [(0, c0), (23, c23), (12, c12), (0, c0), (23, c23)] {
            click_cell(&mut app, cell_at(target_f));
            assert_eq!(app.camera_key_sel.iter().copied().collect::<Vec<_>>(), vec![target_f]);
            assert_eq!(active_overlay_target(&app), Some((target_f, expected_cam)));
            assert_eq!(active_target_frame(&app), target_f);

            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 800.0))), ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| app.frame(ui));
            out.textures_delta.clear();
            let canvas = app.stage_cache.canvas;
            let solid_boxes: Vec<_> = out
                .shapes
                .iter()
                .filter(|s| s.clip_rect == canvas)
                .filter(|s| matches!(&s.shape, egui::Shape::Path(p) if p.closed && p.points.len() == 4 && p.stroke.color == egui::epaint::ColorMode::Solid(cam_color)))
                .collect();
            assert_eq!(solid_boxes.len(), 1, "expected exactly 1 bounding box for keyframe {target_f}");
        }
        assert_eq!(key_count(&app), initial_keys, "switching selection must not duplicate keyframes");
        assert_eq!(app.s.timeline().camera_state_at(0, &app.s.doc.stage).unwrap(), c0);
        assert_eq!(app.s.timeline().camera_state_at(12, &app.s.doc.stage).unwrap(), c12);
        assert_eq!(app.s.timeline().camera_state_at(23, &app.s.doc.stage).unwrap(), c23);
    }

    #[test]
    fn phase2_handles_edit_selected_keyframe_preserve_undo_and_never_touch_artwork() {
        let ctx = egui::Context::default();
        let session = anicraft_engine::Session::new(anicraft_engine::doc::sample::demo());
        let mut app = App::with_context(&ctx, session, Some(std::path::PathBuf::from("unused.png")));
        assert!(app.run("camera.add", json!({})).is_some());
        assert!(app.run("timeline.insert_keyframe", json!({"frame": 23})).is_some());
        assert!(app.run("camera.set", json!({"frame": 23, "x": 700.0, "y": 400.0, "zoom": 130.0})).is_some());
        assert!(app.run("timeline.goto", json!({"frame": 23})).is_some());
        // Select artwork on an artwork layer BEFORE picking Camera tool: camera handles must
        // never modify that artwork, even on drag or arrow-key nudge.
        assert!(app.run("layer.select", json!({"index": 1})).is_some());
        assert!(app.run("edit.select_all", json!({})).is_some());
        let art_before = app.s.selection_bounds();
        assert!(!art_before.is_empty());

        app.tool = Tool::Camera;
        app.camera_key_sel.insert(23);
        for _ in 0..3 {
            run_frame(&ctx, &mut app, vec![]);
        }

        let canvas = app.stage_cache.canvas;
        let size = vec2(app.s.doc.stage.width as f32, app.s.doc.stage.height as f32);
        let raw = canvas.center() + app.view.pan - size * app.view.zoom * 0.5;
        let origin = pos2(raw.x.round(), raw.y.round());
        let g = Geo { origin, zoom: app.view.zoom, stage_rect: Rect::from_min_size(origin, size * app.view.zoom), preview: previewing(&app) };
        let cam23 = app.s.timeline().camera_state_at(23, &app.s.doc.stage).expect("cam 23");
        let f23 = frame_corners(&cam23, &app, &g);

        // All 4 corner handles are interactive: Truck on top-left, Scale on the other 3 corners.
        assert_eq!(cursor(&app, &g, f23[0]), CursorIcon::None);
        assert_eq!(cursor(&app, &g, f23[1]), CursorIcon::ResizeNeSw);
        assert_eq!(cursor(&app, &g, f23[2]), CursorIcon::ResizeNwSe);
        assert_eq!(cursor(&app, &g, f23[3]), CursorIcon::ResizeNeSw);

        // Dragging bottom-right Scale handle inward scales/zooms keyframe 23 in one undo step.
        let history0 = app.s.history_len();
        let z0 = cam23.zoom;
        let (centre, _, _) = handles(&cam23, &app, &g);
        drag_slider(&ctx, &mut app, f23[2], f23[2] + (centre - f23[2]) * 0.25);
        let z1 = app.s.timeline().camera_state_at(23, &app.s.doc.stage).expect("cam 23").zoom;
        assert!(z1 > z0, "scale handle zooms selected keyframe 23: {z0} -> {z1}");
        assert_eq!(app.s.history_len(), history0 + 1, "handle drag is one undo step");
        // Keyframe 0 remains untouched.
        assert!((app.s.timeline().camera_state_at(0, &app.s.doc.stage).expect("cam 0").zoom - 1.0).abs() < 1e-4);

        // Undo and Redo work cleanly.
        assert!(app.run("edit.undo", json!({})).is_some());
        assert!((app.s.timeline().camera_state_at(23, &app.s.doc.stage).expect("cam 23").zoom - z0).abs() < 1e-4);
        assert!(app.run("edit.redo", json!({})).is_some());
        assert!((app.s.timeline().camera_state_at(23, &app.s.doc.stage).expect("cam 23").zoom - z1).abs() < 1e-4);

        // Arrow-key nudge while Camera tool is active moves the camera, NOT selected artwork.
        let cam_before_nudge = app.s.timeline().camera_state_at(23, &app.s.doc.stage).expect("cam 23").center;
        run_frame(&ctx, &mut app, vec![Event::Key { key: Key::ArrowRight, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
        run_frame(&ctx, &mut app, vec![Event::Key { key: Key::ArrowRight, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE }]);
        let cam_after_nudge = app.s.timeline().camera_state_at(23, &app.s.doc.stage).expect("cam 23").center;
        assert!(cam_after_nudge.x > cam_before_nudge.x, "arrow key moves camera: {cam_before_nudge:?} -> {cam_after_nudge:?}");
        assert_eq!(app.s.selection_bounds(), art_before, "selected artwork must not be modified while manipulating camera");
    }

    #[test]
    fn phase3_camera_interpolation_scrubbing_and_playback() {
        let ctx = egui::Context::default();
        let session = anicraft_engine::Session::new(anicraft_engine::doc::sample::demo());
        let mut app = App::with_context(&ctx, session, Some(std::path::PathBuf::from("unused.png")));
        assert!(app.run("camera.add", json!({})).is_some());
        assert!(app.run("timeline.insert_keyframe", json!({"frame": 20})).is_some());
        assert!(app.run("camera.set", json!({"frame": 0, "x": 400.0, "y": 260.0, "zoom": 100.0, "rotation": 0.0})).is_some());
        assert!(app.run("camera.set", json!({"frame": 20, "x": 800.0, "y": 460.0, "zoom": 200.0, "rotation": 40.0})).is_some());
        assert!(app.run("timeline.create_tween", json!({"frame": 0})).is_some());

        // Keyed layer depth on an artwork layer interpolates linearly between depth keys.
        let sky = app.s.doc.timeline(0).layers.last().expect("sky layer").id;
        assert!(app.run("layer.set_depth", json!({"layer": sky.0, "depth": 0.0, "key": true, "frame": 0})).is_some());
        assert!(app.run("layer.set_depth", json!({"layer": sky.0, "depth": 1000.0, "key": true, "frame": 20})).is_some());

        // Exact stored transforms are preserved at keyframe positions 0 and 20.
        let c0 = app.s.timeline().camera_state_at(0, &app.s.doc.stage).expect("cam 0");
        let c20 = app.s.timeline().camera_state_at(20, &app.s.doc.stage).expect("cam 20");
        assert!((c0.center.x - 400.0).abs() < 1e-3 && (c0.center.y - 260.0).abs() < 1e-3);
        assert!((c0.zoom - 1.0).abs() < 1e-4 && c0.rotation.abs() < 1e-4);
        assert!((c20.center.x - 800.0).abs() < 1e-3 && (c20.center.y - 460.0).abs() < 1e-3);
        assert!((c20.zoom - 2.0).abs() < 1e-4 && (c20.rotation.to_degrees() - 40.0).abs() < 1e-3);

        // Midpoint frame 10 with linear tween: halfway in position, zoom, rotation, and depth.
        let c10 = app.s.timeline().camera_state_at(10, &app.s.doc.stage).expect("cam 10");
        assert!((c10.center.x - 600.0).abs() < 1e-2 && (c10.center.y - 360.0).abs() < 1e-2, "{c10:?}");
        assert!((c10.zoom - 1.5).abs() < 1e-3, "{c10:?}");
        assert!((c10.rotation.to_degrees() - 20.0).abs() < 1e-2, "{c10:?}");
        let sky_depth_10 = app.s.doc.timeline(0).layer(sky).expect("sky").depth_at(10);
        assert!((sky_depth_10 - 500.0).abs() < 1e-2, "interpolated depth at frame 10: {sky_depth_10}");

        // Existing easing functionality applies to camera tweens (e.g. ease out shifts midpoint ahead).
        let cam_id = app.s.timeline().camera_layer().expect("camera layer").id;
        assert!(app.run("timeline.set_easing", json!({"layer": cam_id.0, "frame": 0, "easing": {"classic": 100}})).is_some());
        let c10_eased = app.s.timeline().camera_state_at(10, &app.s.doc.stage).expect("cam 10 eased");
        assert!(c10_eased.center.x > 600.0 && c10_eased.center.x < 800.0, "ease-out 100 shifts midpoint ahead: {:?}", c10_eased.center);
        assert!(app.run("timeline.set_easing", json!({"layer": cam_id.0, "frame": 0, "easing": "Linear"})).is_some());

        // Camera preview is active by default so Timeline scrubbing updates the preview framing & tiles.
        assert!(previewing(&app));
        let wait_tiles = |app: &mut App, t: &mut f64| {
            for _ in 0..300 {
                *t += 0.016;
                let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 800.0))), time: Some(*t), ..Default::default() };
                ctx.run_ui(input, |ui| app.frame(ui)).textures_delta.clear();
                let (ready, needed) = app.stage_cache.ready;
                if needed > 0 && ready == needed && app.stage_cache.service.as_ref().is_some_and(|s| !s.busy()) {
                    return true;
                }
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            false
        };

        let mut t = 1.0;
        assert!(app.run("timeline.goto", json!({"frame": 0})).is_some());
        assert!(wait_tiles(&mut app, &mut t), "frame 0 camera preview rendered");
        let cam_scrub_0 = camera(&app).expect("cam scrub 0");

        assert!(app.run("timeline.goto", json!({"frame": 10})).is_some());
        assert!(wait_tiles(&mut app, &mut t), "frame 10 camera preview rendered");
        let cam_scrub_10 = camera(&app).expect("cam scrub 10");
        assert!((cam_scrub_10.center.x - 600.0).abs() < 1e-2);
        assert!(cam_scrub_10.zoom > cam_scrub_0.zoom);

        // Playback animates the camera smoothly across frames.
        assert!(app.run("timeline.goto", json!({"frame": 0})).is_some());
        app.toggle_play();
        assert!(app.playing);
        let mut seen_frames = Vec::new();
        let mut seen_zooms = Vec::new();
        for _ in 0..12 {
            t += 1.0 / 24.0;
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 800.0))),
                time: Some(t),
                predicted_dt: 1.0 / 24.0,
                ..Default::default()
            };
            ctx.run_ui(input, |ui| app.frame(ui)).textures_delta.clear();
            seen_frames.push(app.s.frame);
            seen_zooms.push(camera(&app).expect("cam during playback").zoom);
        }
        assert!(seen_frames.last().copied().unwrap_or(0) > 0, "playback advanced frames: {seen_frames:?}");
        assert!(seen_zooms.last().copied().unwrap_or(0.0) > seen_zooms.first().copied().unwrap_or(0.0), "playback animated camera zoom: {seen_zooms:?}");
        app.toggle_play();
        assert!(!app.playing);
    }

    #[test]
    fn single_camera_bounding_box_switches_with_keyframe_and_stays_aligned() {
        let ctx = egui::Context::default();
        let session = anicraft_engine::Session::new(anicraft_engine::doc::sample::demo());
        let mut app = App::with_context(&ctx, session, Some(std::path::PathBuf::from("unused.png")));
        assert!(app.run("camera.add", json!({})).is_some());
        app.tool = Tool::Camera;

        let render_stage_shapes = |app: &mut App| -> Vec<egui::epaint::ClippedShape> {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 800.0))), ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| app.frame(ui));
            out.textures_delta.clear();
            out.shapes
        };

        let closed_boxes_in_color = |shapes: &[egui::epaint::ClippedShape], canvas: Rect, col: Color32| -> Vec<[Pos2; 4]> {
            shapes
                .iter()
                .filter(|s| s.clip_rect == canvas)
                .filter_map(|s| match &s.shape {
                    egui::Shape::Path(p) if p.closed && p.points.len() == 4 && p.stroke.color == egui::epaint::ColorMode::Solid(col) => {
                        Some([p.points[0], p.points[1], p.points[2], p.points[3]])
                    }
                    _ => None,
                })
                .collect()
        };

        let pivots_in_color = |shapes: &[egui::epaint::ClippedShape], canvas: Rect, col: Color32| -> Vec<Pos2> {
            shapes
                .iter()
                .filter(|s| s.clip_rect == canvas)
                .filter_map(|s| match &s.shape {
                    egui::Shape::Circle(c) if c.fill == Color32::WHITE && c.stroke.color == col => Some(c.center),
                    _ => None,
                })
                .collect()
        };

        let expected_corners_and_pivot = |cam: &Camera, app: &App| -> ([Pos2; 4], Pos2) {
            let canvas = app.stage_cache.canvas;
            let size = vec2(app.s.doc.stage.width as f32, app.s.doc.stage.height as f32);
            let raw = canvas.center() + app.view.pan - size * app.view.zoom * 0.5;
            let origin = pos2(raw.x.round(), raw.y.round());
            let g = Geo { origin, zoom: app.view.zoom, stage_rect: Rect::from_min_size(origin, size * app.view.zoom), preview: previewing(app) };
            (frame_corners(cam, app, &g), stage_to_screen(app, &g, cam.center))
        };

        let assert_single_box = |shapes: &[egui::epaint::ClippedShape], canvas: Rect, col: Color32, cam: &Camera, app: &App, label: &str| {
            let (exp, exp_pivot) = expected_corners_and_pivot(cam, app);
            let boxes = closed_boxes_in_color(shapes, canvas, col);
            assert_eq!(boxes.len(), 1, "{label}: expected exactly one bounding box, got {boxes:?}");
            assert!(boxes[0].iter().zip(exp.iter()).all(|(p, q)| p.distance(*q) < 1.5), "{label}: box mismatch {exp:?} vs {:?}", boxes[0]);
            let pivots = pivots_in_color(shapes, canvas, col);
            assert!(pivots.iter().any(|c| c.distance(exp_pivot) < 1.5), "{label}: missing centre pivot at {exp_pivot:?}");
        };

        let cam_color = to_color32(app.s.timeline().camera_layer().expect("camera layer").color);
        for _ in 0..3 {
            run_frame(&ctx, &mut app, vec![]);
        }
        assert!(app.run("timeline.insert_keyframe", json!({"frame": 23})).is_some());
        assert!(app.run("camera.set", json!({"frame": 0, "x": 420.0, "y": 240.0, "zoom": 90.0})).is_some());
        assert!(app.run("camera.set", json!({"frame": 23, "x": 860.0, "y": 460.0, "zoom": 220.0})).is_some());
        let c0 = app.s.timeline().camera_state_at(0, &app.s.doc.stage).unwrap();
        let c23 = app.s.timeline().camera_state_at(23, &app.s.doc.stage).unwrap();

        let cam_id = app.s.timeline().camera_layer().expect("camera layer").id;
        let cam_row = app.drop.timeline.as_ref().expect("timeline geo").rows.iter().find(|(_, id)| *id == cam_id).map(|(r, _)| *r).expect("camera row");
        let (_, cells_left, cell_w, scroll) = ctx.data(|d| d.get_temp::<(Rect, f32, f32, f32)>(egui::Id::unique("tl_geometry"))).expect("tl_geometry");
        let cell_at = |f: u32| pos2(cells_left + (f as f32 - scroll + 0.5) * cell_w, cam_row.center().y);
        let click_cell = |app: &mut App, p: Pos2| {
            run_frame(&ctx, app, vec![Event::PointerMoved(p)]);
            run_frame(&ctx, app, vec![Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }]);
            run_frame(&ctx, app, vec![Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }]);
        };
        let canvas = app.stage_cache.canvas;

        click_cell(&mut app, cell_at(0));
        let shapes_0 = render_stage_shapes(&mut app);
        assert_single_box(&shapes_0, canvas, cam_color, &c0, &app, "first keyframe active box");

        click_cell(&mut app, cell_at(23));
        let shapes_23 = render_stage_shapes(&mut app);
        assert_single_box(&shapes_23, canvas, cam_color, &c23, &app, "last keyframe active box");

        // Pan and zoom Stage view -> active box remains aligned with Geo::to_screen coordinates.
        let (before23, _) = expected_corners_and_pivot(&c23, &app);
        app.view.fit = false;
        app.view.pan += vec2(65.0, -40.0);
        app.view.zoom *= 1.35;
        let shapes_e = render_stage_shapes(&mut app);
        let (after23, _) = expected_corners_and_pivot(&c23, &app);
        assert!(before23[0].distance(after23[0]) > 20.0);
        assert_single_box(&shapes_e, canvas, cam_color, &c23, &app, "active box stays aligned after stage pan/zoom");
    }

    #[test]
    fn camera_overlays_only_render_when_camera_tool_is_active_and_preserve_data() {
        let ctx = egui::Context::default();
        let session = anicraft_engine::Session::new(anicraft_engine::doc::sample::demo());
        let mut app = App::with_context(&ctx, session, Some(std::path::PathBuf::from("unused.png")));
        assert!(app.run("camera.add", json!({})).is_some());
        assert!(app.run("timeline.insert_keyframe", json!({"frame": 23})).is_some());
        assert!(app.run("camera.set", json!({"frame": 0, "x": 500.0, "y": 300.0, "zoom": 100.0})).is_some());
        assert!(app.run("camera.set", json!({"frame": 23, "x": 760.0, "y": 420.0, "zoom": 160.0, "rotation": 12.0})).is_some());
        assert!(app.run("timeline.create_tween", json!({"frame": 0})).is_some());
        assert!(app.run("timeline.goto", json!({"frame": 12})).is_some());
        app.camera_key_sel.insert(0);

        let expected_c0 = app.s.timeline().camera_state_at(0, &app.s.doc.stage).expect("cam 0");
        let expected_c23 = app.s.timeline().camera_state_at(23, &app.s.doc.stage).expect("cam 23");
        let expected_c12 = app.s.timeline().camera_state_at(12, &app.s.doc.stage).expect("cam 12");
        let cam_color = to_color32(app.s.timeline().camera_layer().expect("camera layer").color);

        let render_stage_shapes = |app: &mut App| -> Vec<egui::epaint::ClippedShape> {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 800.0))), ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| app.frame(ui));
            out.textures_delta.clear();
            out.shapes
        };

        let count_camera_overlays = |shapes: &[egui::epaint::ClippedShape], canvas: Rect| -> (usize, usize, usize) {
            let mut boxes = 0;
            let mut pivots_and_knobs = 0;
            let mut handles_rects = 0;
            for s in shapes.iter().filter(|s| s.clip_rect == canvas) {
                match &s.shape {
                    egui::Shape::Path(p)
                        if p.closed && p.points.len() == 4 && p.stroke.color == egui::epaint::ColorMode::Solid(cam_color) =>
                    {
                        boxes += 1;
                    }
                    egui::Shape::Circle(c) if c.fill == Color32::WHITE && c.stroke.color == cam_color => {
                        pivots_and_knobs += 1;
                    }
                    egui::Shape::Rect(r) if r.fill == Color32::TRANSPARENT && r.stroke.color == cam_color && (r.rect.width() - 7.0).abs() < 2.5 => {
                        handles_rects += 1;
                    }
                    _ => {}
                }
            }
            (boxes, pivots_and_knobs, handles_rects)
        };

        // Step 1: Select Camera -> single active bounding box + pivot + rotate knob + 4 corner handles appear.
        app.tool = Tool::Camera;
        for _ in 0..3 {
            run_frame(&ctx, &mut app, vec![]);
        }
        let canvas = app.stage_cache.canvas;
        let shapes_cam1 = render_stage_shapes(&mut app);
        let (b1, p1, h1) = count_camera_overlays(&shapes_cam1, canvas);
        assert_eq!(b1, 1, "Camera active: expected 1 active camera bounding box, got {b1}");
        assert_eq!(p1, 2, "Camera active: expected pivot marker + rotate knob, got {p1}");
        assert_eq!(h1, 4, "Camera active: expected 4 corner handles, got {h1}");

        // Steps 2-4: Select Brush, Eraser, Select, Subselect -> ALL camera overlays vanish immediately.
        for other_tool in [Tool::Brush, Tool::Eraser, Tool::Select, Tool::Subselect] {
            app.tool = other_tool;
            let shapes_other = render_stage_shapes(&mut app);
            assert!(active_overlay_target(&app).is_none(), "{other_tool:?}: active_overlay_target must be None");
            let (b, p, h) = count_camera_overlays(&shapes_other, canvas);
            assert_eq!((b, p, h), (0, 0, 0), "{other_tool:?}: all camera boxes, pivots and handles must hide");
        }

        // Step 5-6: Repeated tool switching preserves keyframes, transforms, timeline state, selection and playback.
        for other_tool in [Tool::Camera, Tool::Transform, Tool::Brush, Tool::Camera, Tool::Select, Tool::Camera] {
            app.tool = other_tool;
            run_frame(&ctx, &mut app, vec![]);
        }
        assert_eq!(app.tool, Tool::Camera);
        assert_eq!(app.s.frame, 12);
        assert_eq!(app.camera_key_sel.iter().copied().collect::<Vec<_>>(), vec![0]);
        assert_eq!(app.s.timeline().camera_state_at(0, &app.s.doc.stage), Some(expected_c0));
        assert_eq!(app.s.timeline().camera_state_at(12, &app.s.doc.stage), Some(expected_c12));
        assert_eq!(app.s.timeline().camera_state_at(23, &app.s.doc.stage), Some(expected_c23));

        let shapes_cam2 = render_stage_shapes(&mut app);
        let (b2, p2, h2) = count_camera_overlays(&shapes_cam2, canvas);
        assert_eq!((b2, p2, h2), (b1, p1, h1), "selecting Camera again restores exact overlays cleanly");
    }

    #[test]
    fn camera_keyframes_affect_stage_artwork_pixels_scrub_playback_and_brush_switch() {
        let ctx = egui::Context::default();
        let mut doc = anicraft_engine::doc::Document::default();
        doc.stage.width = 400;
        doc.stage.height = 300;
        doc.stage.background = anicraft_engine::doc::Rgba::WHITE;
        let session = anicraft_engine::Session::new(doc);
        let mut app = App::with_context(&ctx, session, Some(std::path::PathBuf::from("unused.png")));
        assert!(app.run("draw.rect", json!({"x": 180.0, "y": 130.0, "w": 40.0, "h": 40.0, "fill": "#000000", "stroke": null})).is_some());
        assert!(app.run("timeline.insert_frame", json!({"frame": 20})).is_some());
        assert!(app.run("camera.add", json!({})).is_some());
        assert!(app.run("timeline.insert_keyframe", json!({"frame": 20})).is_some());
        assert!(app.run("camera.set", json!({"frame": 0, "x": 200.0, "y": 150.0, "zoom": 100.0, "rotation": 0.0})).is_some());
        assert!(app.run("camera.set", json!({"frame": 20, "x": 225.0, "y": 120.0, "zoom": 200.0, "rotation": 30.0})).is_some());
        assert!(app.run("timeline.create_tween", json!({"frame": 0})).is_some());
        app.tool = Tool::Camera;

        assert!(previewing(&app), "camera rendering through stage is active");

        let wait_tiles = |app: &mut App, t: &mut f64| {
            for _ in 0..300 {
                *t += 0.016;
                let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 800.0))), time: Some(*t), ..Default::default() };
                ctx.run_ui(input, |ui| app.frame(ui)).textures_delta.clear();
                let (ready, needed) = app.stage_cache.ready;
                if needed > 0 && ready == needed && app.stage_cache.service.as_ref().is_some_and(|s| !s.busy()) {
                    return true;
                }
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            false
        };

        let render_stage_rgba = |app: &App| -> Vec<u8> {
            let job = anicraft_engine::render_service::make_job_for(
                0,
                app.s.state,
                &app.s.doc,
                app.s.doc.scene_start(app.s.scene) + app.s.frame,
                anicraft_engine::render::View::stage(&app.s.doc, 1.0),
                None,
            )
            .through_camera(previewing(app));
            let svc = app.stage_cache.service.as_ref().expect("render service");
            svc.render_now(job).rgba_premul.clone()
        };

        let dark_bounds = |rgba: &[u8]| -> (u32, u32, u32, u32, f32, f32) {
            let (mut min_x, mut min_y, mut max_x, mut max_y) = (u32::MAX, u32::MAX, 0u32, 0u32);
            let (mut sx, mut sy, mut n) = (0u64, 0u64, 0u64);
            for y in 0..300u32 {
                for x in 0..400u32 {
                    let i = ((y * 400 + x) * 4) as usize;
                    if rgba[i] < 64 && rgba[i + 1] < 64 && rgba[i + 2] < 64 {
                        min_x = min_x.min(x);
                        min_y = min_y.min(y);
                        max_x = max_x.max(x);
                        max_y = max_y.max(y);
                        sx += x as u64;
                        sy += y as u64;
                        n += 1;
                    }
                }
            }
            assert!(n > 0, "expected rendered dark artwork pixels on stage");
            (min_x, min_y, max_x - min_x + 1, max_y - min_y + 1, sx as f32 / n as f32, sy as f32 / n as f32)
        };

        // Frame 0: rest camera -> 40x40 square centered at (200, 150).
        let mut t = 1.0;
        assert!(app.run("timeline.goto", json!({"frame": 0})).is_some());
        assert!(wait_tiles(&mut app, &mut t), "stage rendered frame 0 tiles");
        let rgba0 = render_stage_rgba(&app);
        let (_, _, w0, h0, cx0, cy0) = dark_bounds(&rgba0);
        assert!((38..=42).contains(&w0) && (38..=42).contains(&h0), "frame 0 artwork size 40x40: {w0}x{h0}");
        assert!((cx0 - 199.5).abs() < 2.0 && (cy0 - 149.5).abs() < 2.0, "frame 0 center: ({cx0}, {cy0})");

        // Keyframe 20: zoomed 200%, panned to (225, 120), rotated 30 deg -> artwork scales up, shifts, and rotates!
        assert!(app.run("timeline.goto", json!({"frame": 20})).is_some());
        assert!(wait_tiles(&mut app, &mut t), "stage rendered frame 20 tiles");
        let rgba20 = render_stage_rgba(&app);
        let (_, _, w20, h20, cx20, cy20) = dark_bounds(&rgba20);
        let cam20 = camera(&app).expect("cam 20");
        let exp20 = cam20.stage_view(&app.s.doc.stage).apply(Vec2::new(200.0, 150.0));
        assert!(w20 > w0 * 2 && h20 > h0 * 2, "200% + 30deg rotated square spans >2x pixels: {w0}x{h0} -> {w20}x{h20}");
        assert!((cx20 - exp20.x).abs() < 2.0 && (cy20 - exp20.y).abs() < 2.0, "pan/rotate matches evaluated camera transform: ({cx20},{cy20}) vs {exp20:?}");
        assert!(cx20 < cx0 - 10.0 && cy20 > cy0 + 50.0, "pan/rotate shifted visible artwork composition: ({cx0},{cy0}) -> ({cx20},{cy20})");

        // Editing camera keyframe updates Stage artwork immediately without modifying underlying element geometry.
        let el_bounds_before = app.s.doc.timeline(0).layers[1].keyframes[0].elements[0].transform;
        assert!(app.run("camera.set", json!({"frame": 20, "x": 180.0, "y": 150.0, "zoom": 150.0, "rotation": 0.0})).is_some());
        assert!(wait_tiles(&mut app, &mut t), "stage redrew after keyframe edit");
        let rgba20_edited = render_stage_rgba(&app);
        let (_, _, w20_e, _, cx20_e, _) = dark_bounds(&rgba20_edited);
        assert!((58..=62).contains(&w20_e) && cx20_e > 220.0, "keyframe edit moved/scaled stage pixels: w={w20_e}, cx={cx20_e}");
        assert_eq!(app.s.doc.timeline(0).layers[1].keyframes[0].elements[0].transform, el_bounds_before, "underlying artwork coordinates stay untouched");

        // Scrub intermediate frame 10 -> smooth interpolation between frame 0 (40px) and edited frame 20 (60px).
        assert!(app.run("timeline.goto", json!({"frame": 10})).is_some());
        assert!(wait_tiles(&mut app, &mut t), "stage rendered scrubbed frame 10");
        let rgba10_scrub = render_stage_rgba(&app);
        let (_, _, w10, _, cx10, _) = dark_bounds(&rgba10_scrub);
        assert!(w10 > w0 && w10 < w20_e && cx10 > cx0 && cx10 < cx20_e, "midpoint frame 10 interpolated size/pos: w={w10}, cx={cx10}");

        // Playback produces the exact same frame output as scrubbing to the destination frame.
        assert!(app.run("timeline.goto", json!({"frame": 0})).is_some());
        app.toggle_play();
        for _ in 0..10 {
            t += 1.0 / 24.0;
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 800.0))),
                time: Some(t),
                predicted_dt: 1.0 / 24.0,
                ..Default::default()
            };
            ctx.run_ui(input, |ui| app.frame(ui)).textures_delta.clear();
        }
        let played_frame = app.s.frame;
        assert!(played_frame > 0, "playback advanced frame to {played_frame}");
        let rgba_played = render_stage_rgba(&app);
        app.toggle_play();
        assert!(app.run("timeline.goto", json!({"frame": played_frame})).is_some());
        assert_eq!(rgba_played, render_stage_rgba(&app), "playback matches scrubbing frame-for-frame");

        // Switching between Camera and Brush hides overlays while Stage camera rendering stays active & identical.
        app.tool = Tool::Brush;
        assert!(wait_tiles(&mut app, &mut t), "brush tool renders stage");
        assert!(active_overlay_target(&app).is_none(), "Brush hides camera overlays");
        assert!(previewing(&app), "camera rendering stays active under Brush");
        assert_eq!(render_stage_rgba(&app), rgba_played, "camera composition unchanged under Brush");

        // Stage viewport navigation stays separate from camera animation.
        let cam_before_nav = app.s.timeline().camera_state_at(20, &app.s.doc.stage).unwrap();
        app.view.fit = false;
        app.view.pan += vec2(50.0, -30.0);
        app.view.zoom *= 1.5;
        run_frame(&ctx, &mut app, vec![]);
        assert_eq!(app.s.timeline().camera_state_at(20, &app.s.doc.stage).unwrap(), cam_before_nav);
        assert_eq!(render_stage_rgba(&app), rgba_played, "stage pan/zoom does not double-apply or alter camera image");
    }

    #[test]
    fn automatic_camera_rendering_without_view_menu_and_keyframe_selection_activation() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut doc = anicraft_engine::doc::Document::default();
        doc.stage.width = 400;
        doc.stage.height = 300;
        doc.stage.background = anicraft_engine::doc::Rgba::WHITE;
        let mut app = App::with_context(&ctx, anicraft_engine::Session::new(doc), Some(std::path::PathBuf::from("unused.png")));
        assert!(app.run("draw.rect", json!({"x": 180.0, "y": 130.0, "w": 40.0, "h": 40.0, "fill": "#000000", "stroke": null})).is_some());
        let art_layer = app.s.layer;
        assert!(app.run("timeline.insert_frame", json!({"layer": art_layer.0, "frame": 0, "count": 23})).is_some());
        assert!(app.run("camera.add", json!({})).is_some());
        let cam_layer = app.s.timeline().camera_layer().expect("camera layer").id;
        assert!(app.run("timeline.insert_keyframe", json!({"frame": 23})).is_some());
        assert!(app.run("camera.set", json!({"frame": 0, "x": 200.0, "y": 150.0, "zoom": 100.0})).is_some());
        assert!(app.run("camera.set", json!({"frame": 23, "x": 230.0, "y": 150.0, "zoom": 180.0})).is_some());
        assert!(app.run("timeline.create_tween", json!({"frame": 0})).is_some());
        app.show_panel(crate::dock::Panel::Timeline);

        // 1. No "Preview Through Camera" / "View from Camera" option remains in View menu, Properties, or source UI.
        assert!(!include_str!("menus.rs").contains("Preview Through Camera"));
        assert!(!include_str!("menus.rs").contains("View from Camera"));
        assert!(!include_str!("dialogs.rs").contains("Preview Through Camera"));
        assert!(!include_str!("panels.rs").contains("Preview through camera"));
        assert!(!include_str!("timeline.rs").contains("Preview Through Camera"));

        let run_tree = |app: &mut App| {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 800.0))), ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| app.frame(ui));
            out.textures_delta.clear();
            out.shapes
        };

        // 2. Selecting Camera layer activates Camera overlay; selecting another layer hides overlay
        //    while automatic camera rendering continues on the Stage.
        assert!(app.run("layer.select", json!({"layer": cam_layer.0})).is_some());
        assert_eq!(app.tool, Tool::Camera);
        run_tree(&mut app);
        assert!(active_overlay_target(&app).is_some(), "selecting Camera layer enables editing overlay");

        assert!(app.run("layer.select", json!({"layer": art_layer.0})).is_some());
        assert_ne!(app.tool, Tool::Camera, "selecting non-camera layer leaves Camera overlay mode");
        assert!(active_overlay_target(&app).is_none(), "selecting non-camera layer hides overlay");
        assert!(previewing(&app), "camera Stage rendering continues automatically when non-camera layer is active");

        // Switch back to Camera layer & go to frame 12 (between 100% keyframe 0 and 180% keyframe 23).
        assert!(app.run("layer.select", json!({"layer": cam_layer.0})).is_some());
        assert!(app.run("timeline.goto", json!({"frame": 12})).is_some());
        for _ in 0..3 {
            run_tree(&mut app);
        }
        let canvas = app.stage_cache.canvas;
        let size = vec2(app.s.doc.stage.width as f32, app.s.doc.stage.height as f32);
        let raw = canvas.center() + app.view.pan - size * app.view.zoom * 0.5;
        let origin = pos2(raw.x.round(), raw.y.round());
        let g = Geo { origin, zoom: app.view.zoom, stage_rect: Rect::from_min_size(origin, size * app.view.zoom), preview: true };

        // 3. First keyframe selected -> single box uses keyframe 0 and modifies keyframe 0 only.
        app.camera_key_sel.clear();
        app.camera_key_sel.insert(0);
        run_tree(&mut app);
        let c0_before = app.s.timeline().camera_state_at(0, &app.s.doc.stage).unwrap();
        let c23_before = app.s.timeline().camera_state_at(23, &app.s.doc.stage).unwrap();
        let (piv0, _, _) = handles(&c0_before, &app, &g);
        assert!(canvas.contains(piv0), "pivot handle of keyframe 0 sits inside stage canvas");
        drag_slider(&ctx, &mut app, piv0, piv0 + vec2(15.0, 15.0));
        assert_ne!(app.s.timeline().camera_state_at(0, &app.s.doc.stage).unwrap(), c0_before, "selected first keyframe CAN be manipulated");
        assert_eq!(app.s.timeline().camera_state_at(23, &app.s.doc.stage).unwrap(), c23_before, "last keyframe stays untouched while first is selected");

        // 4. Last keyframe selected -> single box switches to keyframe 23 and modifies keyframe 23 only.
        app.camera_key_sel.clear();
        app.camera_key_sel.insert(23);
        run_tree(&mut app);
        let c0_locked = app.s.timeline().camera_state_at(0, &app.s.doc.stage).unwrap();
        let f23_active = frame_corners(&c23_before, &app, &g);
        drag_slider(&ctx, &mut app, f23_active[0], f23_active[0] + vec2(15.0, 15.0));
        assert_eq!(app.s.timeline().camera_state_at(0, &app.s.doc.stage).unwrap(), c0_locked, "first keyframe stays untouched while last is selected");
        let c23_after = app.s.timeline().camera_state_at(23, &app.s.doc.stage).unwrap();
        assert_ne!(c23_after, c23_before, "selected last keyframe CAN be manipulated");

        // 5. Save and reopen preserves keyframes and automatic camera Stage rendering without any View option.
        let saved = anicraft_engine::format::to_json(&app.s.doc).expect("save");
        let reopened = anicraft_engine::format::from_json(&saved).expect("reopen");
        let app2 = App::with_context(&ctx, anicraft_engine::Session::new(reopened), Some(std::path::PathBuf::from("unused.png")));
        assert!(previewing(&app2), "reopened camera document renders through camera automatically");
        assert_eq!(app2.s.timeline().camera_state_at(0, &app2.s.doc.stage).unwrap(), c0_locked);
        assert_eq!(app2.s.timeline().camera_state_at(23, &app2.s.doc.stage).unwrap(), c23_after);
    }

    #[test]
    fn active_keyframe_manipulates_selected_first_or_last_across_all_ops_overlapping_and_separated() {
        let ctx = egui::Context::default();
        let mut app = test_app();
        assert!(app.run("timeline.insert_frame", json!({"count": 23})).is_some());
        assert!(app.run("camera.add", json!({})).is_some());
        assert!(app.run("timeline.goto", json!({"frame": 23})).is_some());
        assert!(app.run("timeline.insert_keyframe", json!({})).is_some());
        assert!(app.run("timeline.goto", json!({"frame": 0})).is_some());
        assert!(app.run("timeline.create_tween", json!({})).is_some());
        app.tool = Tool::Camera;

        let settle = |app: &mut App| {
            for _ in 0..3 {
                run_frame(&ctx, app, vec![]);
            }
        };
        let geo_of = |app: &App| {
            let canvas = app.stage_cache.canvas;
            let size = vec2(app.s.doc.stage.width as f32, app.s.doc.stage.height as f32);
            let raw = canvas.center() + app.view.pan - size * app.view.zoom * 0.5;
            let origin = pos2(raw.x.round(), raw.y.round());
            Geo { origin, zoom: app.view.zoom, stage_rect: Rect::from_min_size(origin, size * app.view.zoom), preview: previewing(app) }
        };
        let cam_at = |app: &App, f: u32| app.s.timeline().camera_state_at(f, &app.s.doc.stage).unwrap();

        settle(&mut app);

        // Phase A: Overlapping identical boxes at frame 0 (green) and frame 23 (red), while playhead stays at 0.
        assert_eq!(cam_at(&app, 0), cam_at(&app, 23), "start & end transforms overlap identically initially");

        // Select ONLY last keyframe (23) -> active target, wheel, move, resize, rotate, and pivot must all mutate 23 and preserve 0.
        app.camera_key_sel.clear();
        app.camera_key_sel.insert(23);
        settle(&mut app);
        assert_eq!(active_target_frame(&app), 23);
        assert_eq!(active_camera(&app).unwrap(), cam_at(&app, 23));

        // 1. Overlapping Pivot drag -> moves keyframe 23 only.
        let c0 = cam_at(&app, 0);
        let c23 = cam_at(&app, 23);
        let g = geo_of(&app);
        let (pivot23, _, _) = handles(&c23, &app, &g);
        assert_eq!(hit_target(&app, &g, pivot23).map(|(f, _, z)| (f, z)), Some((23, Zone::Pivot)));
        drag_slider(&ctx, &mut app, pivot23, pivot23 + vec2(30.0, 20.0));
        assert_eq!(cam_at(&app, 0), c0, "pivot drag with last selected must leave first keyframe untouched");
        assert!(cam_at(&app, 23).center.distance(c23.center) > 5.0, "pivot drag moves last keyframe");

        // Reset 23 back to overlap 0 before testing Move, Resize, Rotate, Wheel independently on identical overlap.
        assert!(app.run("camera.set", json!({"frame": 23, "x": c0.center.x, "y": c0.center.y, "zoom": c0.zoom * 100.0, "rotation": c0.rotation.to_degrees()})).is_some());
        settle(&mut app);

        // 2. Overlapping Edge Move -> moves keyframe 23 only.
        let c23 = cam_at(&app, 23);
        let g = geo_of(&app);
        let (_, _, top_edge23) = handles(&c23, &app, &g);
        assert_eq!(hit_target(&app, &g, top_edge23).map(|(f, _, z)| (f, z)), Some((23, Zone::Move)));
        drag_slider(&ctx, &mut app, top_edge23, top_edge23 + vec2(25.0, 15.0));
        assert_eq!(cam_at(&app, 0), c0, "move drag with last selected must leave first keyframe untouched");
        assert!(cam_at(&app, 23).center.distance(c23.center) > 5.0, "edge move drag pans last keyframe");

        assert!(app.run("camera.set", json!({"frame": 23, "x": c0.center.x, "y": c0.center.y, "zoom": c0.zoom * 100.0, "rotation": c0.rotation.to_degrees()})).is_some());
        settle(&mut app);

        // 3. Overlapping Corner Resize/Zoom -> scales keyframe 23 only.
        let c23 = cam_at(&app, 23);
        let g = geo_of(&app);
        let f23 = frame_corners(&c23, &app, &g);
        assert_eq!(hit_target(&app, &g, f23[1]).map(|(f, _, z)| (f, z)), Some((23, Zone::Scale(1))));
        drag_slider(&ctx, &mut app, f23[1], f23[1] + vec2(-25.0, 15.0));
        assert_eq!(cam_at(&app, 0), c0, "resize drag with last selected must leave first keyframe untouched");
        assert!((cam_at(&app, 23).zoom - c23.zoom).abs() > 0.05, "resize drag changes last keyframe zoom");

        // 4. Wheel zoom with last keyframe selected (while playhead still on 0) -> zooms 23 only.
        let c23_before_wheel = cam_at(&app, 23);
        wheel(&mut app, 120.0, 1.0);
        assert_eq!(cam_at(&app, 0), c0, "wheel zoom with last selected must leave first keyframe untouched");
        assert!(cam_at(&app, 23).zoom > c23_before_wheel.zoom, "wheel zoom updates last keyframe");

        assert!(app.run("camera.set", json!({"frame": 23, "x": c0.center.x, "y": c0.center.y, "zoom": c0.zoom * 100.0, "rotation": c0.rotation.to_degrees()})).is_some());
        settle(&mut app);

        // 5. Overlapping Rotate knob -> rotates keyframe 23 only.
        let c23 = cam_at(&app, 23);
        let g = geo_of(&app);
        let (_, knob23, _) = handles(&c23, &app, &g);
        assert_eq!(hit_target(&app, &g, knob23).map(|(f, _, z)| (f, z)), Some((23, Zone::Rotate)));
        drag_slider(&ctx, &mut app, knob23, knob23 + vec2(35.0, 15.0));
        assert_eq!(cam_at(&app, 0), c0, "rotate drag with last selected must leave first keyframe untouched");
        assert!((cam_at(&app, 23).rotation - c23.rotation).abs() > 0.05, "rotate drag changes last keyframe rotation");

        // Phase B: Separated boxes -> move, resize, rotate, and pivot when ONLY last (23) vs ONLY first (0) is selected.
        assert!(app.run("camera.set", json!({"frame": 0, "x": 380.0, "y": 240.0, "zoom": 115.0, "rotation": -5.0})).is_some());
        assert!(app.run("camera.set", json!({"frame": 23, "x": 860.0, "y": 460.0, "zoom": 160.0, "rotation": 12.0})).is_some());

        // B1. Move playhead to 23 & select 23 -> manipulate 23 only.
        assert!(app.run("timeline.goto", json!({"frame": 23})).is_some());
        app.camera_key_sel.clear();
        app.camera_key_sel.insert(23);
        settle(&mut app);
        assert_eq!(active_target_frame(&app), 23);

        let c0_sep = cam_at(&app, 0);
        let mut cur23 = cam_at(&app, 23);
        for op in [0, 1, 2, 3] {
            let g = geo_of(&app);
            let (piv, knob, top) = handles(&cur23, &app, &g);
            let corners = frame_corners(&cur23, &app, &g);
            let (from, to) = match op {
                0 => (piv, piv + vec2(18.0, -12.0)),
                1 => (top, top + vec2(-16.0, 14.0)),
                2 => (corners[0], corners[0] + vec2(14.0, 10.0)),
                _ => (knob, knob + vec2(28.0, 10.0)),
            };
            drag_slider(&ctx, &mut app, from, to);
            assert_eq!(cam_at(&app, 0), c0_sep, "separated last-selected op {op} must leave first keyframe untouched");
            let next23 = cam_at(&app, 23);
            assert_ne!(next23, cur23, "separated last-selected op {op} must update last keyframe");
            cur23 = next23;
        }

        // B2. Select ONLY first keyframe (0) -> move, resize, rotate, pivot mutate 0 only and leave 23 untouched.
        assert!(app.run("timeline.goto", json!({"frame": 0})).is_some());
        app.camera_key_sel.clear();
        app.camera_key_sel.insert(0);
        settle(&mut app);
        assert_eq!(active_target_frame(&app), 0);

        let c23_locked = cam_at(&app, 23);
        let mut cur0 = cam_at(&app, 0);
        for op in [0, 1, 2, 3] {
            let g = geo_of(&app);
            let (piv, knob, top) = handles(&cur0, &app, &g);
            let corners = frame_corners(&cur0, &app, &g);
            let (from, to) = match op {
                0 => (piv, piv + vec2(18.0, -12.0)),
                1 => (top, top + vec2(-16.0, 14.0)),
                2 => (corners[2], corners[2] + vec2(-16.0, -10.0)),
                _ => (knob, knob + vec2(-28.0, 10.0)),
            };
            drag_slider(&ctx, &mut app, from, to);
            assert_eq!(cam_at(&app, 23), c23_locked, "separated first-selected op {op} must leave last keyframe untouched");
            let next0 = cam_at(&app, 0);
            assert_ne!(next0, cur0, "separated first-selected op {op} must update first keyframe");
            cur0 = next0;
        }

        // 6. Verify interpolation between edited first & last keyframe transforms and playback with hidden overlay.
        let c0_final = cam_at(&app, 0);
        let c23_final = cam_at(&app, 23);
        let c_mid = cam_at(&app, 11);
        let min_x = c0_final.center.x.min(c23_final.center.x);
        let max_x = c0_final.center.x.max(c23_final.center.x);
        assert!(c_mid.center.x > min_x && c_mid.center.x < max_x, "midpoint center X interpolates cleanly between edited endpoints");
        assert!(c_mid.zoom > 0.05 && (c_mid.zoom - c0_final.zoom).abs() > 1e-3 && (c_mid.zoom - c23_final.zoom).abs() > 1e-3, "midpoint zoom interpolates between edited endpoints");

        // 7. Playback advances and animates camera smoothly while editing overlay is hidden (e.g. Select tool).
        app.tool = Tool::Select;
        assert!(active_overlay_target(&app).is_none());
        assert!(previewing(&app));
        app.toggle_play();
        assert!(app.playing);
        let mut t = ctx.input(|i| i.time);
        for _ in 0..8 {
            t += 1.0 / 24.0;
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 800.0))),
                time: Some(t),
                predicted_dt: 1.0 / 24.0,
                ..Default::default()
            };
            ctx.run_ui(input, |ui| app.frame(ui)).textures_delta.clear();
        }
        assert!(app.s.frame > 0, "playback advances while overlay is hidden");
        app.toggle_play();

        // 8. Save & reopen preserves both keyframes independently.
        let saved = anicraft_engine::format::to_json(&app.s.doc).expect("save");
        let reopened = anicraft_engine::format::from_json(&saved).expect("reopen");
        let app2 = App::with_context(&ctx, anicraft_engine::Session::new(reopened), Some(std::path::PathBuf::from("unused.png")));
        assert_eq!(cam_at(&app2, 0), c0_final);
        assert_eq!(cam_at(&app2, 23), c23_final);
    }

    #[test]
    fn camera_stage_keyboard_and_accessibility() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut app = test_app();
        assert!(app.run("timeline.insert_frame", json!({"count": 23})).is_some());
        assert!(app.run("camera.add", json!({})).is_some());
        assert!(app.run("timeline.goto", json!({"frame": 23})).is_some());
        assert!(app.run("timeline.insert_keyframe", json!({})).is_some());
        app.tool = Tool::Camera;
        app.camera_key_sel.clear();
        app.camera_key_sel.insert(23);

        let step = |app: &mut App, events: Vec<Event>| {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 800.0))),
                events,
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| app.frame(ui));
            out.textures_delta.clear();
            out
        };
        for _ in 0..3 {
            step(&mut app, vec![]);
        }

        // Verify AccessKit exposes the active camera bounding box with Role::Button, selected state, and keyframe label.
        let box_id = egui::Id::unique("stage_camera_box");
        ctx.memory_mut(|m| m.request_focus(box_id));
        let out = step(&mut app, vec![]);
        let upd = out.platform_output.accesskit_update.as_ref().expect("accesskit update");
        let cam_node = upd
            .nodes
            .iter()
            .find_map(|(_, n)| n.label().filter(|l| l.starts_with("Camera frame 24")).map(|_| n))
            .expect("accessible camera frame node in AccessKit tree");
        assert_eq!(cam_node.role(), egui::accesskit::Role::Button);
        assert_eq!(cam_node.is_selected(), Some(true));

        // Verify visible focus outline in T.accent is painted on Stage when camera box is focused.
        let canvas = app.stage_cache.canvas;
        let has_focus_ring = out
            .shapes
            .iter()
            .filter(|s| s.clip_rect == canvas)
            .any(|s| matches!(&s.shape, egui::Shape::Path(p) if p.closed && p.stroke.color == egui::epaint::ColorMode::Solid(T.accent)));
        assert!(has_focus_ring, "focused camera bounding box draws accent focus outline");

        // Keyboard move (ArrowRight), zoom (Equals / Minus), and rotate (Alt+ArrowRight) edit ONLY keyframe 23 while camera box is focused.
        let press_key = |app: &mut App, key: Key, modifiers: Modifiers| {
            step(app, vec![Event::ModifiersChanged(modifiers), Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }]);
            step(app, vec![Event::ModifiersChanged(modifiers), Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers }]);
            step(app, vec![Event::ModifiersChanged(Modifiers::NONE)]);
        };
        let c0 = app.s.timeline().camera_state_at(0, &app.s.doc.stage).unwrap();
        let c23_start = app.s.timeline().camera_state_at(23, &app.s.doc.stage).unwrap();

        press_key(&mut app, Key::ArrowRight, Modifiers::NONE);
        let c23_moved = app.s.timeline().camera_state_at(23, &app.s.doc.stage).unwrap();
        assert!(c23_moved.center.x > c23_start.center.x, "ArrowRight pans active camera keyframe");
        assert_eq!(app.s.timeline().camera_state_at(0, &app.s.doc.stage).unwrap(), c0);

        press_key(&mut app, Key::Equals, Modifiers::NONE);
        let c23_zoomed = app.s.timeline().camera_state_at(23, &app.s.doc.stage).unwrap();
        assert!(c23_zoomed.zoom > c23_moved.zoom, "Equals zooms in active camera keyframe");
        assert_eq!(app.s.timeline().camera_state_at(0, &app.s.doc.stage).unwrap(), c0);

        press_key(&mut app, Key::Minus, Modifiers::NONE);
        let c23_unzoomed = app.s.timeline().camera_state_at(23, &app.s.doc.stage).unwrap();
        assert!(c23_unzoomed.zoom < c23_zoomed.zoom, "Minus zooms out active camera keyframe");

        press_key(&mut app, Key::ArrowRight, Modifiers::ALT);
        let c23_rotated = app.s.timeline().camera_state_at(23, &app.s.doc.stage).unwrap();
        assert!(c23_rotated.rotation > c23_unzoomed.rotation, "Alt+ArrowRight rotates active camera keyframe");
        assert_eq!(app.s.timeline().camera_state_at(0, &app.s.doc.stage).unwrap(), c0);

        // Tab moves focus cleanly out of the camera box (no keyboard trap).
        press_key(&mut app, Key::Tab, Modifiers::NONE);
        assert_ne!(ctx.memory(|m| m.focused()), Some(box_id), "Tab moves focus away (no keyboard trap)");
    }

    #[test]
    fn stage_artwork_accessibility_and_wheel_during_drag() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut app = test_app();
        assert!(app.run("draw.rect", json!({"x": 160.0, "y": 110.0, "w": 80.0, "h": 60.0, "fill": "#336699", "stroke": null})).is_some());
        assert!(app.run("camera.add", json!({})).is_some());
        app.tool = Tool::Select;

        let step = |app: &mut App, events: Vec<Event>| {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 800.0))),
                events,
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| app.frame(ui));
            out.textures_delta.clear();
            out
        };
        for _ in 0..3 {
            step(&mut app, vec![]);
        }

        // Focus the Stage via Ctrl+F6 with a visible Camera layer:
        // Stage artwork AccessKit ListBox & keyboard Tab -> Enter selection must still work.
        let ctrl = Modifiers::COMMAND | Modifiers::CTRL;
        for _ in 0..8 {
            if crate::a11y::focused_region(&ctx) == Some(crate::a11y::Region::Stage) {
                break;
            }
            step(&mut app, vec![Event::ModifiersChanged(ctrl), Event::Key { key: Key::F6, physical_key: None, pressed: true, repeat: false, modifiers: ctrl }]);
            step(&mut app, vec![Event::ModifiersChanged(ctrl), Event::Key { key: Key::F6, physical_key: None, pressed: false, repeat: false, modifiers: ctrl }]);
            step(&mut app, vec![Event::ModifiersChanged(Modifiers::NONE)]);
        }
        assert_eq!(crate::a11y::focused_region(&ctx), Some(crate::a11y::Region::Stage), "Ctrl+F6 focuses Stage even with visible Camera layer");
        let out_stage = step(&mut app, vec![]);
        let upd = out_stage.platform_output.accesskit_update.as_ref().expect("accesskit update");
        let has_listbox = upd.nodes.iter().any(|(_, n)| n.role() == egui::accesskit::Role::ListBox && n.label().is_some_and(|l| l.contains("on frame 1")));
        assert!(has_listbox, "Stage artwork AccessKit ListBox must remain exposed when a visible Camera layer exists");

        step(&mut app, vec![Event::Key { key: Key::Tab, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
        step(&mut app, vec![Event::Key { key: Key::Tab, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE }]);
        assert!(app.stage_nav.focused(&ctx).is_some(), "Tab on focused Stage enters artwork element list");
        step(&mut app, vec![Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
        step(&mut app, vec![Event::Key { key: Key::Enter, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE }]);
        assert!(!app.s.selection.is_empty(), "Tab + Enter must select Stage artwork even when Camera layer is visible");

        // Wheel zoom must be ignored during an active pointer drag so it does not clobber app.drag.
        let cam_before = active_camera(&app).expect("camera");
        let canvas_corner = app.stage_cache.canvas.left_top() + vec2(8.0, 8.0);
        app.drag = Drag::CameraPan { frame: 0, last: canvas_corner };
        wheel(&mut app, 120.0, 2.0);
        assert!(matches!(app.drag, Drag::CameraPan { .. }), "wheel must not clobber an active drag");
        assert_eq!(active_camera(&app).expect("camera"), cam_before, "wheel must not mutate camera mid-drag");
        app.drag = Drag::None;
    }
}
