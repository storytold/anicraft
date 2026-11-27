//! Array stage controls and tile coverage for position-only drags.

use crate::app::App;
use crate::theme::{self, T, Type};
use anicraft_engine::{CLOSED_SWEEP_LIMIT, RadialDistribution, StateId};
use anicraft_engine::doc::{DisplayList, Element, LayerId};
use anicraft_engine::geom::{Affine, Rect as ArtRect, Vec2, segment_distance};
use anicraft_engine::render_service::{FrameJob, frame_key};
use egui::{CursorIcon, Id, Painter, PointerButton, Pos2, Rect, Sense, Stroke, Ui, pos2, vec2};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

const HANDLE_RADIUS: f32 = 6.0;
const HANDLE_HIT: f32 = 12.0;
const BAR_HIT: f32 = 9.0;
const MIN_OFFSET_SWEEP: f32 = 0.1;
const RADIAL_CENTRE_RADIUS: f32 = 5.0;
const RADIAL_START_RADIUS: f32 = 7.0;
const RADIAL_END_RADIUS: f32 = 11.0;
const DRIFT_RADIUS: f32 = 15.0;
const SEAM_START_HIT: f32 = 8.0;
const SEAM_END_HIT: f32 = 26.0;
pub(crate) const RADIAL_FIELD_LIMIT: f32 = 100_000.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Endpoints {
    pub start: [f32; 2],
    pub end: [f32; 2],
}

impl Endpoints {
    pub fn extent(self) -> [f32; 2] {
        [self.end[0] - self.start[0], self.end[1] - self.start[1]]
    }

    pub fn spacing(self, count: u32) -> Option<[f32; 2]> {
        (count > 1).then(|| self.extent().map(|d| d / (count - 1) as f32))
    }
}

/// Axis arrays are ordered X/Y (columns/rows). A one-instance axis retains its
/// spacing for subsequent count edits, even though its two endpoints coincide.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct GridGeometry {
    pub start: [f32; 2],
    pub counts: [u32; 2],
    pub spacing: [f32; 2],
    pub spacing_locked: bool,
}

impl GridGeometry {
    fn intervals(self) -> [f32; 2] {
        self.counts.map(|count| count.saturating_sub(1) as f32)
    }

    pub fn endpoints(self) -> Endpoints {
        let [nx, ny] = self.intervals();
        let [x, y] = self.start;
        Endpoints { start: self.start, end: [x + nx * self.spacing[0], y + ny * self.spacing[1]] }
    }

    pub fn set_counts(&mut self, counts: [u32; 2], preferred: usize) {
        let old = self.intervals();
        self.counts = counts;
        let new = self.intervals();
        for (axis, spacing) in self.spacing.iter_mut().enumerate() {
            if old[axis] != 0.0 && new[axis] != 0.0 && old[axis] != new[axis] {
                *spacing = (*spacing * old[axis]) / new[axis];
            }
        }
        if self.spacing_locked {
            let other = 1 - preferred;
            let axis = if new[preferred] == 0.0 && new[other] != 0.0 { other } else { preferred };
            self.match_spacing(self.spacing[axis].abs());
        }
    }

    pub fn set_spacing(&mut self, axis: usize, spacing: f32) {
        self.spacing[axis] = spacing;
        if self.spacing_locked {
            self.match_spacing(spacing.abs());
        }
    }

    fn match_spacing(&mut self, magnitude: f32) {
        self.spacing = self.spacing.map(|spacing| magnitude.copysign(spacing));
    }

    fn drag_corner(mut self, target: [f32; 2], start: bool) -> Self {
        let ends = self.endpoints();
        let anchor = if start { ends.end } else { ends.start };
        let intervals = self.intervals();
        for (axis, spacing) in self.spacing.iter_mut().enumerate() {
            let steps = intervals[axis];
            if steps > 0.0 {
                let span = if start { anchor[axis] - target[axis] } else { target[axis] - anchor[axis] };
                *spacing = span / steps;
            }
        }
        if self.spacing_locked
            && let Some(magnitude) = self.spacing.into_iter().zip(intervals)
                .filter(|(_, steps)| *steps > 0.0)
                .map(|(spacing, _)| spacing.abs()).reduce(f32::min)
        {
            // Use the nearer axis in spacing units. Both extents reach zero
            // continuously when either axis crosses its anchor.
            self.match_spacing(magnitude);
        }
        for (axis, steps) in intervals.into_iter().enumerate() {
            if steps == 0.0 && (start || intervals == [0.0; 2]) {
                // Both corners share this coordinate on a collapsed count axis.
                self.start[axis] = target[axis];
            } else if start {
                self.start[axis] = anchor[axis] - steps * self.spacing[axis];
            }
        }
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Geometry {
    Linear(Endpoints),
    Radial(RadialDistribution),
    Grid(GridGeometry),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Grip {
    Centre,
    Start,
    End,
    Bar,
    Drift,
}

fn direction(degrees: f32) -> Vec2 {
    let (sin, cos) = degrees.rem_euclid(CLOSED_SWEEP_LIMIT).to_radians().sin_cos();
    Vec2::new(cos, sin)
}

fn heading(v: Vec2) -> f32 {
    v.y.atan2(v.x).to_degrees()
}

fn angle_step(from: f32, to: f32) -> f32 {
    (to - from + 180.0).rem_euclid(CLOSED_SWEEP_LIMIT) - 180.0
}

#[derive(Clone, Copy)]
pub(crate) struct GizmoDrag {
    grip: Grip,
    press: Vec2,
    original: Geometry,
    last_angle: f32,
    winding: f32,
}

impl GizmoDrag {
    fn new(grip: Grip, press: Vec2, original: Geometry) -> Self {
        let (last_angle, winding) = match original {
            Geometry::Linear(_) | Geometry::Grid(_) => (0.0, 0.0),
            Geometry::Radial(g) => (g.start + g.sweep, g.sweep),
        };
        Self { grip, press, original, last_angle, winding }
    }

    fn at(&mut self, pointer: Vec2) -> Option<Geometry> {
        if !pointer.is_finite() { return None; }
        let delta = pointer - self.press;
        if !delta.is_finite() { return None; }
        let shift = |[x, y]: [f32; 2]| [x + delta.x, y + delta.y];
        match self.original {
            Geometry::Linear(mut endpoints) => {
                if self.grip != Grip::End { endpoints.start = shift(endpoints.start); }
                if self.grip != Grip::Start { endpoints.end = shift(endpoints.end); }
                endpoints.start.into_iter().chain(endpoints.end).all(f32::is_finite).then_some(Geometry::Linear(endpoints))
            }
            Geometry::Grid(mut grid) => {
                match self.grip {
                    Grip::Start => grid = grid.drag_corner(shift(grid.start), true),
                    Grip::End => grid = grid.drag_corner(shift(grid.endpoints().end), false),
                    Grip::Bar => grid.start = shift(grid.start),
                    _ => return None,
                }
                grid.start.into_iter().chain(grid.endpoints().end).chain(grid.spacing)
                    .all(f32::is_finite).then_some(Geometry::Grid(grid))
            }
            Geometry::Radial(mut radial) => {
                match self.grip {
                    Grip::Centre | Grip::Bar => radial.centre = shift(radial.centre),
                    Grip::Start => {
                        let v = direction(radial.start) * radial.radius + delta;
                        radial.radius = v.length().min(RADIAL_FIELD_LIMIT);
                        if radial.radius > 1e-4 {
                            radial.start += angle_step(radial.start, heading(v));
                        }
                    }
                    Grip::End if !radial.closed_loop && radial.centre_drift != [0.0, 0.0] => {
                        let target = radial.point(1.0) + delta;
                        let (sweep, radius) = solve_drift_end(radial, target, self.winding)?;
                        radial.sweep = sweep;
                        set_end_radius(&mut radial, radius);
                        // Near zero sweep or the field limits, no choice of offset may
                        // reach the target. Keep the last valid placement in that case.
                        let tolerance = 0.01 + target.length() * 1e-6;
                        if radial.point(1.0).distance(target) > tolerance { return None; }
                        self.winding = sweep;
                    }
                    Grip::End => {
                        let v = direction(radial.start + radial.sweep) * radial.radius_at(1.0) as f32 + delta;
                        let radius = v.length();
                        if radius > 1e-4 {
                            let next = heading(v);
                            self.winding += angle_step(self.last_angle, next);
                            self.last_angle = next;
                        }
                        // Keep tracking the pointer past the seam. Reversing within the
                        // overshoot should not pull End away from Start prematurely.
                        let limit = radial.sweep_limit().min(RADIAL_FIELD_LIMIT);
                        radial.sweep = self.winding.clamp(-limit, limit);
                        if radial.closed_loop {
                            radial.radius = radius.min(RADIAL_FIELD_LIMIT);
                        } else {
                            set_end_radius(&mut radial, radius);
                        }
                    }
                    Grip::Drift => radial.centre_drift = shift(radial.centre_drift)
                        .map(|value| value.clamp(-RADIAL_FIELD_LIMIT, RADIAL_FIELD_LIMIT)),
                }
                radial.centre.into_iter().all(f32::is_finite).then_some(Geometry::Radial(radial))
            }
        }
    }
}

fn solve_drift_end(radial: RadialDistribution, target: Vec2, previous: f32) -> Option<(f32, f32)> {
    let revolution = f64::from(CLOSED_SWEEP_LIMIT);
    let [drift_x, drift_y] = radial.centre_drift.map(f64::from);
    let sample = |sweep: f64| {
        let [cx, cy] = radial.centre_after_turns(sweep.abs() / revolution);
        let (sin, cos) = (f64::from(radial.start) + sweep % revolution).to_radians().sin_cos();
        let x = f64::from(target.x) - cx;
        let y = f64::from(target.y) - cy;
        // End lies on the outward ray when the cross product vanishes and the dot product is nonnegative.
        let cross = cos * y - sin * x;
        let radius = cos * x + sin * y;
        let slope = -radius * std::f64::consts::PI / 180.0
            - sweep.signum() * (cos * drift_y - sin * drift_x) / revolution;
        (cross, radius, slope)
    };
    // Follow the last accepted winding. Limit each solve to its neighbourhood so an
    // ambiguous target cannot suddenly add or remove a revolution.
    let mut sweep = f64::from(previous);
    let limit = f64::from(RADIAL_FIELD_LIMIT);
    let min = (sweep - 120.0).max(-limit);
    let max = (sweep + 120.0).min(limit);
    for _ in 0..12 {
        let (cross, radius, slope) = sample(sweep);
        if cross.abs() <= 1e-5 {
            return (radius >= -1e-5).then_some((sweep as f32, radius.max(0.0) as f32));
        }
        if slope.abs() <= 1e-8 { break; }
        let step = (cross / slope).clamp(-45.0, 45.0);
        sweep = [1.0, 0.5, 0.25, 0.125].into_iter()
            .map(|scale| (sweep - step * scale).clamp(min, max))
            .find(|&candidate| sample(candidate).0.abs() < cross.abs())?;
    }
    None
}

fn set_end_radius(radial: &mut RadialDistribution, radius: f32) {
    // A zero-length sweep has no per-turn rate. Keep the old rate if an inward spiral
    // already reaches the centre and the pointer leaves its end there.
    if radial.sweep.abs() >= MIN_OFFSET_SWEEP && (radius > 1e-4 || radial.radius_at(1.0) > 0.0) {
        let turns = radial.sweep.abs() / CLOSED_SWEEP_LIMIT;
        radial.spiral_offset = ((radius - radial.radius) / turns).clamp(-RADIAL_FIELD_LIMIT, RADIAL_FIELD_LIMIT);
    }
}

struct Handle {
    grip: Grip,
    at: Pos2,
    radius: f32,
}

pub(crate) struct Overlay {
    handles: Vec<Handle>,
    guide: Vec<Pos2>,
    drift_bar: Option<[Pos2; 2]>,
    radial_seam: Option<Pos2>,
    radial: bool,
    coincident_ends: bool,
    grid: bool,
    hot: Option<Grip>,
}

fn as_vec(p: Pos2) -> Vec2 {
    Vec2::new(p.x, p.y)
}

fn registration(element: &Element, projection: Affine, show_custom_pivot: bool) -> Option<Pos2> {
    let local = if show_custom_pivot { element.pivot().unwrap_or(Vec2::ZERO) } else { Vec2::ZERO };
    let p = projection.apply(element.transform.apply(local));
    p.is_finite().then_some(pos2(p.x, p.y))
}

pub(crate) fn paint_registrations(painter: &Painter, projection: Affine, instances: &[(LayerId, Arc<Element>)], show_custom_pivot: bool) {
    let clip = painter.clip_rect().expand(6.0);
    for (_, instance) in instances {
        if let Some(p) = registration(instance, projection, show_custom_pivot).filter(|p| clip.contains(*p)) {
            theme::registration_mark(painter, p);
        }
    }
}

/// While only instance positions change, the surrounding artwork and each copy's footprint
/// are stable. Give a tile a new key only when one of its intersecting instances changes.
#[derive(Clone, Copy)]
pub(crate) struct TileBounds {
    pub generation: StateId,
    relative: Option<ArtRect>,
}

impl TileBounds {
    pub fn new(generation: StateId, list: &DisplayList, instances: &[(LayerId, Arc<Element>)]) -> Self {
        let mut bounds = Self { generation, relative: None };
        // Filters can extend beyond path bounds and a moved mask can change other artwork.
        // Keep the full render path in those scenes.
        if list.groups.iter().any(|g| !g.filters.is_empty()) { return bounds; }
        let positions: HashMap<_, _> = instances.iter().map(|(_, el)| (el.id, el.transform.translation())).collect();
        let mut footprint = ArtRect::EMPTY;
        for item in &list.items {
            let Some(at) = positions.get(&item.element) else { continue };
            if item.masks.is_some() { return bounds; }
            let reach = item.stroke.as_ref().map_or(0.0, anicraft_engine::doc::Stroke::reach);
            let xf = Affine::translate(-at.x, -at.y).mul(&item.transform);
            footprint = footprint.union(&item.path.bounds().inflate(reach + 1.0).transformed(&xf));
        }
        bounds.relative = Some(footprint);
        bounds
    }

    pub fn key(&self, job: &FrameJob, tile: [f32; 4], pixel: f32, dim: bool, instances: &[(LayerId, Arc<Element>)]) -> u64 {
        let Some(footprint) = self.relative else { return job.key };
        let [x0, y0, x1, y1] = tile;
        let tile = ArtRect::new(x0, y0, x1, y1).inflate(2.0 * pixel);
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        ("array_tile", frame_key(self.generation, job.frame, &job.view, None), job.raster, dim).hash(&mut hash);
        if !footprint.is_empty() {
            for (_, el) in instances {
                let at = el.transform.translation();
                let bounds = ArtRect { min: footprint.min + at, max: footprint.max + at };
                if !tile.intersect(&bounds).is_empty() {
                    (el.id, at.x.to_bits(), at.y.to_bits()).hash(&mut hash);
                }
            }
        }
        hash.finish()
    }
}

impl Overlay {
    fn new(geometry: Geometry, to_screen: Affine) -> Option<Self> {
        let project = |point| {
            let p = to_screen.apply(point);
            p.is_finite().then_some(pos2(p.x, p.y))
        };
        let mut overlay = Self { handles: Vec::with_capacity(4), guide: Vec::new(), drift_bar: None,
            radial_seam: None, radial: false, coincident_ends: false, grid: false, hot: None };
        match geometry {
            Geometry::Linear(ends) => {
                let [sx, sy] = ends.start;
                let [ex, ey] = ends.end;
                let (start, end) = (project(Vec2::new(sx, sy))?, project(Vec2::new(ex, ey))?);
                overlay.guide.extend([start, end]);
                overlay.endpoint_handles(start, end);
            }
            Geometry::Grid(grid) => {
                let Endpoints { start: [sx, sy], end: [ex, ey] } = grid.endpoints();
                let start = project(Vec2::new(sx, sy))?;
                let end = project(Vec2::new(ex, ey))?;
                let horizontal = project(Vec2::new(ex, sy))?;
                let vertical = project(Vec2::new(sx, ey))?;
                overlay.guide.extend([start, horizontal, end, vertical, start]);
                overlay.grid = sx != ex && sy != ey;
                overlay.endpoint_handles(start, end);
            }
            Geometry::Radial(radial) => {
                overlay.radial = true;
                let [x, y] = radial.centre;
                let centre = project(Vec2::new(x, y))?;
                let start = project(radial.point(0.0))?;
                let end = project(radial.point(1.0))?;
                overlay.radial_seam = (start.distance_sq(end) <= 4.0).then_some(start);
                overlay.handles.extend([
                    Handle { grip: Grip::End, at: end, radius: RADIAL_END_RADIUS },
                    Handle { grip: Grip::Start, at: start, radius: RADIAL_START_RADIUS },
                    Handle { grip: Grip::Centre, at: centre, radius: RADIAL_CENTRE_RADIUS },
                ]);
                if !radial.closed_loop {
                    let [x, y] = radial.centre_after_turns(1.0);
                    let at = project(Vec2::new(x as f32, y as f32))?;
                    overlay.drift_bar = Some([centre, at]);
                    overlay.handles.push(Handle { grip: Grip::Drift, at, radius: DRIFT_RADIUS });
                }
                // Sample the actual distribution curve with bounded screen-space work.
                let turn = radial.sweep.abs().min(radial.sweep_limit());
                let radius = f64::from(radial.radius).max(radial.radius_at(1.0));
                let [cx, cy] = radial.centre_after_turns(f64::from(turn) / f64::from(CLOSED_SWEEP_LIMIT));
                let drift_length = (cx - f64::from(x)).hypot(cy - f64::from(y));
                let span = (f64::from(turn.to_radians()) * radius + (radial.radius_at(1.0) - f64::from(radial.radius)).abs() + drift_length)
                    * f64::from(to_screen.mean_scale());
                let segments = (span / 12.0).max(f64::from(turn) / 12.0).ceil().clamp(1.0, 2048.0) as u32;
                overlay.guide = (0..=segments)
                    .filter_map(|i| project(radial.point(f64::from(i) / f64::from(segments))))
                    .collect();
            }
        }
        Some(overlay)
    }

    fn endpoint_handles(&mut self, start: Pos2, end: Pos2) {
        self.coincident_ends = start.distance_sq(end) < 1.0;
        let end_radius = if self.coincident_ends { HANDLE_RADIUS + 4.0 } else { HANDLE_RADIUS };
        self.handles.extend([
            Handle { grip: Grip::End, at: end, radius: end_radius },
            Handle { grip: Grip::Start, at: start, radius: HANDLE_RADIUS },
        ]);
    }

    fn hit(&self, p: Pos2) -> Option<Grip> {
        if self.coincident_ends
            && let Some(start) = self.handles.iter().find(|h| h.grip == Grip::Start)
        {
            let distance = p.distance(start.at);
            return (distance <= HANDLE_HIT).then_some(if distance < HANDLE_RADIUS { Grip::Start } else { Grip::End });
        }
        if let Some(seam) = self.radial_seam {
            let distance = p.distance(seam);
            if distance <= SEAM_END_HIT {
                // A centre or drift ring can still be selected if it also overlaps the seam.
                for h in &self.handles {
                    let at = p.distance(h.at);
                    if (h.grip == Grip::Centre && at <= h.radius)
                        || (h.grip == Grip::Drift && (at - h.radius).abs() <= 1.5 && distance > SEAM_START_HIT)
                    {
                        return Some(h.grip);
                    }
                }
                return Some(if distance <= SEAM_START_HIT { Grip::Start } else { Grip::End });
            }
        }
        self.handles.iter().rev().filter_map(|handle| {
            let distance = p.distance(handle.at);
            let score = if self.radial && matches!(handle.grip, Grip::Start | Grip::End | Grip::Drift) {
                (distance - handle.radius).abs()
            } else {
                distance
            };
            (distance <= HANDLE_HIT.max(handle.radius + 4.0)).then_some((score, handle.grip))
        }).min_by(|(a, _), (b, _)| a.total_cmp(b)).map(|(_, grip)| grip).or_else(|| {
            let on_line = |a: Pos2, b: Pos2| segment_distance(as_vec(p), as_vec(a), as_vec(b)) <= BAR_HIT;
            let guide = self.guide.windows(2).any(|pair| matches!(pair, [a, b] if on_line(*a, *b)));
            let drift = self.drift_bar.is_some_and(|[a, b]| a != b && on_line(a, b));
            let interior = self.grid && {
                let mut sides = self.guide.windows(2).filter_map(|edge| {
                    let &[a, b] = edge else { return None };
                    let along = b - a;
                    let to_pointer = p - a;
                    Some(along.x * to_pointer.y - along.y * to_pointer.x)
                });
                sides.next().is_some_and(|first| sides.all(|side| first.is_sign_positive() == side.is_sign_positive()))
            };
            (guide || drift || interior).then_some(Grip::Bar)
        })
    }

    pub(crate) fn paint(&self, painter: &Painter) {
        let line = if self.hot == Some(Grip::Bar) { T.accent } else { T.selection };
        if self.guide.len() > 1 {
            painter.add(egui::Shape::line(self.guide.clone(), Stroke::new(1.5, line)));
        }
        if let Some([centre, drift]) = self.drift_bar.filter(|[a, b]| a != b) {
            let edge = if self.hot == Some(Grip::Drift) { T.accent } else { line };
            painter.line_segment([centre, drift], Stroke::new(1.0, edge));
        }
        let clip = painter.clip_rect().expand(96.0);
        for h in self.handles.iter().filter(|h| clip.contains(h.at)) {
            // Fixed sides keep labels from jumping as the grips pass each other.
            let (label, side) = match h.grip {
                Grip::Centre => (tr!("Centre"), vec2(-1.0, 0.0)),
                Grip::Start => (tr!("Start"), vec2(0.0, -1.0)),
                Grip::End => (tr!("End"), vec2(0.0, 1.0)),
                Grip::Drift => (tr!("Drift"), vec2(1.0, 0.0)),
                Grip::Bar => continue,
            };
            let galley = painter.layout_no_wrap(label.to_owned(), theme::font(Type::Caption), T.text);
            let padding = vec2(4.0, 2.0);
            let size = galley.size() + 2.0 * padding;
            let offset = size * 0.5 + egui::Vec2::splat(h.radius + 4.0);
            let shift = vec2(side.x * offset.x, side.y * offset.y);
            let rect = Rect::from_center_size(h.at + shift, size);
            painter.rect_filled(rect, T.rc(), T.panel);
            painter.galley(rect.min + padding, galley, T.text);
        }
        for h in self.handles.iter().filter(|h| clip.contains(h.at)) {
            let edge = if self.hot == Some(h.grip) { T.accent } else { T.selection };
            let stroke = Stroke::new(1.5, edge);
            if self.radial && matches!(h.grip, Grip::End | Grip::Drift) {
                painter.circle_stroke(h.at, h.radius, stroke);
            } else {
                let fill = if h.grip == Grip::Start { T.selection } else { T.panel };
                painter.circle(h.at, h.radius, fill, stroke);
            }
        }
    }
}

/// Update before the stage submits tiles, so the picture and handles use the same geometry.
pub(crate) fn input(app: &mut App, ui: &mut Ui, canvas: Rect, to_screen: Option<Affine>) -> Option<Overlay> {
    let (Some(geometry), Some(to_screen)) = (app.array_panel.gizmo_geometry(), to_screen) else {
        app.array_panel.gizmo_drag = None;
        return None;
    };
    let mut overlay = Overlay::new(geometry, to_screen)?;
    if !app.array_panel.gizmo_editable()
        || app.modal_open()
        || app.palette.is_some()
        || egui::Popup::is_any_open(ui.ctx())
        || app.dock_view.drag.is_some()
    {
        app.array_panel.gizmo_drag = None;
        return Some(overlay);
    }
    let to_stage = to_screen.inverse()?;
    // egui hits rectangles; resolve the actual circles and segment at the original press.
    let response = ui.interact(canvas, Id::unique("array_stage_gizmo"), Sense::drag());
    if response.drag_started_by(PointerButton::Primary)
        && let Some(press) = ui.input(|i| i.pointer.press_origin())
        && let Some(grip) = overlay.hit(press)
    {
        app.array_panel.gizmo_drag = Some(GizmoDrag::new(grip, to_stage.apply(as_vec(press)), geometry));
    }
    if let Some(mut drag) = app.array_panel.gizmo_drag.take() {
        if let Some(p) = response.interact_pointer_pos()
            && (response.dragged_by(PointerButton::Primary) || response.drag_stopped_by(PointerButton::Primary))
            && let Some(moved) = drag.at(to_stage.apply(as_vec(p)))
            && moved != geometry
        {
            let gesture = crate::array_panel::EditGesture::Pointer(ui.ctx().viewport_id(), response.id);
            app.array_panel.set_gizmo_geometry(&mut app.s, moved, Some(gesture));
            overlay = Overlay::new(moved, to_screen)?;
            ui.ctx().request_repaint();
        }
        if ui.input(|i| i.pointer.primary_down()) {
            app.array_panel.gizmo_drag = Some(drag);
            overlay.hot = Some(drag.grip);
            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
            return Some(overlay);
        }
    }
    overlay.hot = response.hover_pos().and_then(|p| overlay.hit(p));
    if overlay.hot.is_some() {
        ui.ctx().set_cursor_icon(CursorIcon::Grab);
    }
    Some(overlay)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coincident_endpoints_can_be_grabbed_independently() {
        let p = [90.0, 60.0];
        let overlay = Overlay::new(Geometry::Linear(Endpoints { start: p, end: p }), Affine::IDENTITY).unwrap();
        assert_eq!(overlay.hit(pos2(90.0, 60.0)), Some(Grip::Start));
        assert_eq!(overlay.hit(pos2(98.0, 60.0)), Some(Grip::End));
        assert_eq!(overlay.hit(pos2(110.0, 60.0)), None);
    }

    #[test]
    fn grid_corner_drags_cross_independently_and_remain_uniform_through_zero() {
        let grid = GridGeometry { start: [10.0, 10.0], counts: [4, 3], spacing: [20.0; 2], spacing_locked: true };
        let mut drag = GizmoDrag::new(Grip::End, Vec2::new(70.0, 50.0), Geometry::Grid(grid));
        for (point, spacing) in [
            ([70.0, 42.0], [16.0, 16.0]),
            ([13.0, 42.0], [1.0, 1.0]),
            ([10.0, 42.0], [0.0, 0.0]),
            ([7.0, 42.0], [-1.0, 1.0]),
            ([-50.0, 42.0], [-16.0, 16.0]),
            ([-50.0, 6.0], [-2.0, -2.0]),
            ([25.0, 6.0], [2.0, -2.0]),
        ] {
            let Some(Geometry::Grid(moved)) = drag.at(Vec2::new(point[0], point[1])) else { panic!("grid drag") };
            assert_eq!(moved.start, grid.start);
            assert_eq!(moved.spacing, spacing);
            assert_eq!(moved.endpoints().extent(), [spacing[0] * 3.0, spacing[1] * 2.0]);
        }
    }

    #[test]
    fn grid_collapsed_geometry_preserves_spacing_and_handles_remain_reachable() {
        let mut grid = GridGeometry { start: [40.0, 60.0], counts: [3, 3], spacing: [-18.0, 24.0], spacing_locked: false };
        let original = grid;
        grid.set_counts([1, 1], 0);
        assert_eq!(grid.spacing, original.spacing);
        let overlay = Overlay::new(Geometry::Grid(grid), Affine::IDENTITY).unwrap();
        assert_eq!(overlay.hit(pos2(40.0, 60.0)), Some(Grip::Start));
        assert_eq!(overlay.hit(pos2(49.0, 60.0)), Some(Grip::End));
        let moved = grid.drag_corner([55.0, 70.0], false);
        assert_eq!(moved.endpoints(), Endpoints { start: [55.0, 70.0], end: [55.0, 70.0] });
        assert_eq!(moved.spacing, grid.spacing);
        grid.set_counts([3, 1], 0);
        let row = Overlay::new(Geometry::Grid(grid), Affine::IDENTITY).unwrap();
        assert_eq!(row.hit(pos2(22.0, 60.0)), Some(Grip::Bar));
        assert_eq!(row.hit(pos2(40.0, 60.0)), Some(Grip::Start));
        assert_eq!(row.hit(pos2(4.0, 60.0)), Some(Grip::End));
        grid.set_counts([1, 3], 0);
        let column = Overlay::new(Geometry::Grid(grid), Affine::IDENTITY).unwrap();
        assert_eq!(column.hit(pos2(40.0, 84.0)), Some(Grip::Bar));
        grid.set_counts([3, 3], 0);
        assert_eq!(grid, original);
    }

    fn radial(sweep: f32) -> RadialDistribution {
        RadialDistribution { centre: [0.0, 0.0], radius: 80.0, start: 0.0, sweep,
            closed_loop: false, spiral_offset: 0.0, centre_drift: [0.0; 2], rotate: false }
    }

    #[test]
    fn radial_end_drag_preserves_winding_in_each_direction_over_multiple_turns() {
        for sign in [-1.0, 1.0] {
            let initial = radial(sign * 350.0);
            let mut drag = GizmoDrag::new(Grip::End, initial.point(1.0), Geometry::Radial(initial));
            for angle in [355.0, 359.0, 361.0, 400.0, 490.0, 580.0, 670.0, 719.0, 721.0, 810.0, 900.0, 990.0, 1082.0, 1078.0] {
                let angle = sign * angle;
                let Some(Geometry::Radial(moved)) = drag.at(direction(angle) * 80.0) else { panic!("expected radial edit") };
                assert!((moved.sweep - angle).abs() < 0.01, "{} vs {angle}", moved.sweep);
                assert!((moved.radius_at(1.0) - 80.0).abs() < 0.01);
            }
        }
    }

    #[test]
    fn closed_end_stays_at_start_when_pointer_reverses_beyond_either_seam() {
        for sign in [-1.0, 1.0] {
            let initial = RadialDistribution { closed_loop: true, ..radial(sign * 350.0) };
            let mut drag = GizmoDrag::new(Grip::End, initial.point(1.0), Geometry::Radial(initial));
            for angle in [355.0, 360.0, 361.0, 385.0, 370.0, 361.0, 360.0, 359.0, 350.0] {
                let pointer = direction(sign * angle) * initial.radius;
                let Some(Geometry::Radial(moved)) = drag.at(pointer) else { panic!("expected radial edit") };
                if angle >= CLOSED_SWEEP_LIMIT {
                    assert_eq!(moved.sweep, sign * CLOSED_SWEEP_LIMIT);
                    assert_eq!(moved.point(0.0), moved.point(1.0));
                } else {
                    assert!((moved.sweep - sign * angle).abs() < 0.01);
                }
            }
        }
    }

    #[test]
    fn radial_coincidences_keep_separate_targets_including_zero_drift() {
        for (radius, sweep, closed_loop) in [(0.0, 360.0, true), (80.0, -360.0, true), (80.0, 0.0, false), (0.0, 0.0, false)] {
            let shape = RadialDistribution { radius, closed_loop, ..radial(sweep) };
            let overlay = Overlay::new(Geometry::Radial(shape), Affine::IDENTITY).unwrap();
            assert_eq!(overlay.handles.len(), if closed_loop { 3 } else { 4 });
            assert_eq!(overlay.guide.first(), overlay.guide.last());
            for handle in &overlay.handles {
                let target = match handle.grip {
                    Grip::Start | Grip::End | Grip::Drift => handle.at + vec2(handle.radius, 0.0),
                    _ => handle.at,
                };
                assert_eq!(overlay.hit(target), Some(handle.grip));
            }
        }
    }

    #[test]
    fn full_turn_end_has_a_broad_annulus_around_the_start_target() {
        for sweep in [-360.0, 360.0] {
            let radial = RadialDistribution { closed_loop: true, start: -15.0, ..radial(sweep) };
            let overlay = Overlay::new(Geometry::Radial(radial), Affine::IDENTITY).unwrap();
            let seam = overlay.radial_seam.unwrap();
            for direction in [vec2(1.0, 0.0), vec2(0.0, -1.0), vec2(-0.6, 0.8)] {
                for distance in [0.0, 3.0, 7.0] {
                    assert_eq!(overlay.hit(seam + direction * distance), Some(Grip::Start));
                }
                for distance in [9.0, 12.0, 18.0, 25.0] {
                    assert_eq!(overlay.hit(seam + direction * distance), Some(Grip::End));
                }
                assert_ne!(overlay.hit(seam + direction * 27.0), Some(Grip::End));
            }
        }
    }

    #[test]
    fn radial_grips_keep_their_positions_as_end_moves_past_start() {
        let zoom = 0.36;
        let shape = RadialDistribution { radius: 420.0, start: -8.8, ..radial(0.0) };
        for angle in 350..=370 {
            let shape = RadialDistribution { sweep: angle as f32, ..shape };
            let overlay = Overlay::new(Geometry::Radial(shape), Affine::scale(zoom, zoom)).unwrap();
            let grip = |which| overlay.handles.iter().find(|handle| handle.grip == which).unwrap().at;
            let end = shape.point(1.0) * zoom;
            assert_eq!(grip(Grip::End), pos2(end.x, end.y));
            let start = shape.point(0.0) * zoom;
            assert_eq!(grip(Grip::Start), pos2(start.x, start.y));
            assert_eq!(grip(Grip::Drift), grip(Grip::Centre));
        }
    }

    #[test]
    fn drifting_end_follows_pointer_over_multiple_turns_in_both_directions() {
        for sign in [-1.0, 1.0] {
            let initial = RadialDistribution { centre: [720.0, -100.0], centre_drift: [76.0, -55.0], start: -20.0,
                spiral_offset: -7.0, ..radial(sign * 350.0) };
            let mut drag = GizmoDrag::new(Grip::End, initial.point(1.0), Geometry::Radial(initial));
            for angle in [355.0, 359.0, 361.0, 402.0, 449.0, 499.0, 558.0, 617.0, 677.0, 719.0, 723.0,
                782.0, 842.0, 899.0, 950.0, 1010.0, 1079.0, 1082.0, 1075.0] {
                let target = RadialDistribution { sweep: sign * angle, spiral_offset: angle / 65.0 - 12.0, ..initial };
                let pointer = target.point(1.0);
                let Some(Geometry::Radial(moved)) = drag.at(pointer) else { panic!("no placement at {angle} with sign {sign}") };
                assert!((moved.sweep - target.sweep).abs() < 0.01, "{} vs {}", moved.sweep, target.sweep);
                assert!((moved.spiral_offset - target.spiral_offset).abs() < 0.01);
                assert!(moved.point(1.0).distance(pointer) < 0.001);
                assert_eq!(moved.centre_drift, initial.centre_drift);
                assert_eq!(moved.point(0.0), initial.point(0.0));
            }
        }
    }

    #[test]
    fn grid_stagger_tiles_invalidate_at_old_and_new_positions_outside_the_gizmo() {
        use anicraft_engine::render::View;
        use anicraft_engine::render_service::{make_job, render_job};
        use serde_json::json;
        let mut s = anicraft_engine::Session::default();
        s.execute("draw.rect", &json!({"x": 0, "y": 0, "w": 20, "h": 20, "fill": "#ff0000", "stroke": null})).unwrap();
        s.execute("modify.convert_to_symbol", &json!({"name": "Source"})).unwrap();
        let mut p = json!({"mode": "grid", "rows": 2, "columns": 2, "start_x": 40, "start_y": 25,
            "spacing_x": 20, "spacing_y": 20, "stagger": {"rows": {"enabled": true, "x": 300}}});
        s.preview_array(&p).unwrap();
        let generation = s.array_translation_generation().unwrap();
        let footprint = TileBounds::new(generation, &s.display_list(), &s.selected_elements());
        let tiles = |session: &anicraft_engine::Session| [-300.0, 0.0, 300.0].map(|left| {
            let view = View { width: 100, height: 70, transform: Affine::translate(-left, 0.0), background: None };
            let mut job = make_job(session.state, &session.doc, 0, view, None);
            job.key = footprint.key(&job, [left, 0.0, left + 100.0, 70.0], 1.0, false, &session.selected_elements());
            job
        });
        let old = tiles(&s);
        p["stagger"]["rows"]["x"] = json!(-300);
        s.preview_array(&p).unwrap();
        let new = tiles(&s);
        assert_eq!(s.array_translation_generation(), Some(generation));
        assert_eq!(old[1].key, new[1].key);
        assert_eq!(render_job(&old[1]).rgba_premul, render_job(&new[1]).rgba_premul);
        for i in [0, 2] {
            assert_ne!(old[i].key, new[i].key);
            assert_ne!(render_job(&old[i]).rgba_premul, render_job(&new[i]).rgba_premul);
        }
    }

    #[test]
    fn linear_tiles_follow_the_copies_and_clear_the_area_they_left() {
        use anicraft_engine::render::View;
        use anicraft_engine::render_service::{make_job, render_job};
        use serde_json::json;
        let mut s = anicraft_engine::Session::default();
        s.execute("draw.rect", &json!({"x": 0, "y": 0, "w": 20, "h": 20, "fill": "#ff0000", "stroke": null})).unwrap();
        s.execute("modify.convert_to_symbol", &json!({"name": "Source"})).unwrap();
        let mut p = json!({"mode": "linear", "count": 2, "start_x": 50, "start_y": 25, "dx": 20, "dy": 0});
        s.preview_array(&p).unwrap();
        let generation = s.array_translation_generation().unwrap();
        let footprint = TileBounds::new(generation, &s.display_list(), &s.selected_elements());
        let tile = |session: &anicraft_engine::Session, left: f32| {
            let view = View { width: 100, height: 60, transform: Affine::translate(-left, 0.0), background: None };
            let mut job = make_job(session.state, &session.doc, 0, view, None);
            job.key = footprint.key(&job, [left, 0.0, left + 100.0, 60.0], 1.0, false, &session.selected_elements());
            job
        };
        let old = [tile(&s, 0.0), tile(&s, 300.0), tile(&s, 1300.0)];
        p["start_x"] = json!(1350);
        s.preview_array(&p).unwrap();
        let new = [tile(&s, 0.0), tile(&s, 300.0), tile(&s, 1300.0)];
        assert_eq!(s.array_translation_generation(), Some(generation));
        assert_ne!(old[0].key, new[0].key);
        assert_eq!(old[1].key, new[1].key);
        assert_ne!(old[2].key, new[2].key);
        for i in [0, 2] {
            assert_ne!(render_job(&old[i]).rgba_premul, render_job(&new[i]).rgba_premul);
        }
        assert_eq!(render_job(&old[1]).rgba_premul, render_job(&new[1]).rgba_premul);
    }
}
