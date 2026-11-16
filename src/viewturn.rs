//! Turning the stage view. The document stays upright: only the picture on screen turns,
//! about the centre of the canvas, the way a page turns under the hand while the hand
//! itself still slides the page in screen pixels.
//!
//! Pan sits outside the rotation. A turn orbits that pan so the stage point under the
//! canvas centre does not jump, and a hand drag still moves the picture by the same
//! screen pixels the pointer moved. A rotation of 0 leaves every mapping on the upright
//! origin and zoom.

use anicraft_engine::geom::Affine;
use egui::{Pos2, Vec2, pos2};

/// Clockwise degrees, folded into (-180, 180]. A non-finite angle is 0.
pub fn wrap_deg(d: f32) -> f32 {
    if !d.is_finite() {
        return 0.0;
    }
    let mut x = d % 360.0;
    if x <= -180.0 {
        x += 360.0;
    } else if x > 180.0 {
        x -= 360.0;
    }
    x
}

/// Clockwise rotation of a screen vector, in degrees, on a y-down screen.
/// The same sign as [`Affine::rotate`]. Zero returns `v` unchanged.
pub fn spin_vec(v: Vec2, degrees: f32) -> Vec2 {
    if degrees == 0.0 || !degrees.is_finite() {
        return v;
    }
    let (s, c) = degrees.to_radians().sin_cos();
    Vec2::new(c * v.x - s * v.y, s * v.x + c * v.y)
}

/// `straight` is where the point sits with the view upright. Turning moves it about
/// `pivot` after the screen-space `pan` has been taken off, then puts the pan back.
pub fn place(straight: Pos2, pivot: Pos2, pan: Vec2, rotation: f32) -> Pos2 {
    if rotation == 0.0 {
        return straight;
    }
    let d = spin_vec(straight - pivot - pan, rotation);
    pivot + pan + d
}

/// Inverse of [`place`].
pub fn unplace(screen: Pos2, pivot: Pos2, pan: Vec2, rotation: f32) -> Pos2 {
    if rotation == 0.0 {
        return screen;
    }
    let d = spin_vec(screen - pivot - pan, -rotation);
    pivot + pan + d
}

/// Pan after turning the view by `delta` degrees. The stage point under the canvas centre stays.
pub fn orbit_pan(pan: Vec2, delta: f32) -> Vec2 {
    spin_vec(pan, delta)
}

/// Pan after standing the view upright. The stage point under the canvas centre stays.
pub fn reset_pan(pan: Vec2, rotation: f32) -> Vec2 {
    spin_vec(pan, -rotation)
}

/// The stage point under the canvas centre. At 0° this is `size/2 − pan/zoom`.
pub fn center_stage(size: Vec2, pan: Vec2, rotation: f32, zoom: f32) -> Vec2 {
    let z = zoom.max(1e-3);
    size * 0.5 - spin_vec(pan, -rotation) / z
}

/// Pan so `screen` stays over the same stage point when the zoom changes. The angle stays.
pub fn zoom_pan(pivot: Pos2, pan: Vec2, rotation: f32, size: Vec2, zoom: f32, screen: Pos2, new_zoom: f32) -> Vec2 {
    let old = zoom.max(1e-6);
    let content = spin_vec(screen - pivot - pan, -rotation);
    let stage_pt = size * 0.5 + content / old;
    let content_new = (stage_pt - size * 0.5) * new_zoom;
    screen - pivot - spin_vec(content_new, rotation)
}

/// How a vertical drag scales the zoom. `delta_y` is the pointer's y minus the press y, on a
/// y-down screen, so dragging up zooms in. A zero or non-finite drag leaves the zoom alone.
/// The factor is measured from the press, so dragging back to that height restores the zoom.
pub fn zoom_drag_factor(delta_y: f32) -> f32 {
    if !delta_y.is_finite() || delta_y == 0.0 {
        return 1.0;
    }
    (-delta_y * 0.008).exp()
}

/// Pointer angle about `pivot`, in degrees, y-down, 0 along +x.
pub fn pointer_angle(pivot: Pos2, p: Pos2) -> f32 {
    let d = p - pivot;
    d.y.atan2(d.x).to_degrees()
}

/// Distance from `p` to the infinite line through `a` and `b`.
pub fn line_distance(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let len = ab.length();
    if len < 1e-6 {
        return (p - a).length();
    }
    ((p.x - a.x) * ab.y - (p.y - a.y) * ab.x).abs() / len
}

/// Axis-aligned bounds of `rect`'s corners mapped back to the upright screen.
/// At 0° this is `rect` itself.
pub fn unrotated_bounds(rect: egui::Rect, pivot: Pos2, pan: Vec2, rotation: f32) -> egui::Rect {
    if rotation == 0.0 {
        return rect;
    }
    let corners = [rect.min, pos2(rect.max.x, rect.min.y), rect.max, pos2(rect.min.x, rect.max.y)];
    let mut min = Pos2::new(f32::INFINITY, f32::INFINITY);
    let mut max = Pos2::new(f32::NEG_INFINITY, f32::NEG_INFINITY);
    for c in corners {
        let p = unplace(c, pivot, pan, rotation);
        min.x = min.x.min(p.x);
        min.y = min.y.min(p.y);
        max.x = max.x.max(p.x);
        max.y = max.y.max(p.y);
    }
    egui::Rect::from_min_max(min, max)
}

/// Upright `translate(origin) · scale(zoom)`, with the view turn about `pivot` outside the pan.
/// At 0° the turn is left off, so the affine is exactly that translate and scale.
pub fn screen_affine(origin: Pos2, zoom: f32, rotation: f32, pivot: Pos2, pan: Vec2) -> Affine {
    let upright = Affine::translate(origin.x, origin.y).mul(&Affine::scale(zoom, zoom));
    if rotation == 0.0 {
        return upright;
    }
    let c = pivot + pan;
    Affine::translate(c.x, c.y).mul(&Affine::rotate(rotation.to_radians())).mul(&Affine::translate(-c.x, -c.y)).mul(&upright)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anicraft_engine::geom::Vec2 as StageVec;

    fn near(a: Vec2, b: Vec2) -> bool {
        (a - b).length() < 1e-3
    }

    #[test]
    fn spin_matches_the_geometry_affine_and_zero_is_exact() {
        let v = Vec2::new(3.0, -4.0);
        assert_eq!(spin_vec(v, 0.0), v);
        let s = spin_vec(v, 40.0);
        let a = Affine::rotate(40.0_f32.to_radians()).apply_vector(StageVec::new(v.x, v.y));
        assert!((s.x - a.x).abs() < 1e-5 && (s.y - a.y).abs() < 1e-5, "{s:?} vs {a:?}");
    }

    #[test]
    fn a_turn_keeps_the_stage_point_under_the_canvas_centre() {
        let size = Vec2::new(200.0, 100.0);
        let zoom = 2.0;
        let mut pan = Vec2::new(10.0, 20.0);
        let mut rot = 15.0;
        let c0 = center_stage(size, pan, rot, zoom);
        pan = orbit_pan(pan, 40.0);
        rot = wrap_deg(rot + 40.0);
        let c1 = center_stage(size, pan, rot, zoom);
        assert!(near(c0, c1), "{c0:?} vs {c1:?}");
        pan = reset_pan(pan, rot);
        let c2 = center_stage(size, pan, 0.0, zoom);
        assert!(near(c0, c2), "reset {c0:?} vs {c2:?}");
        assert_eq!(wrap_deg(0.0), 0.0);
    }

    #[test]
    fn drag_up_zooms_in_and_down_returns() {
        assert_eq!(zoom_drag_factor(0.0), 1.0);
        assert_eq!(zoom_drag_factor(f32::NAN), 1.0);
        let up = zoom_drag_factor(-125.0);
        assert!((up - 1.0_f32.exp()).abs() < 1e-5, "{up}");
        let down = zoom_drag_factor(125.0);
        assert!(down < 1.0, "{down}");
        assert!((up * down - 1.0).abs() < 1e-5, "{up} * {down}");
    }

    #[test]
    fn zoom_about_at_zero_matches_the_upright_formula() {
        // The upright zoom the shell used before a turn existed.
        let old = |pivot: Pos2, pan: Vec2, size: Vec2, zoom: f32, screen: Pos2, new_zoom: f32| {
            let origin = pivot + pan - size * zoom * 0.5;
            let stage_pt = (screen - origin) / zoom;
            let new_origin = screen - stage_pt * new_zoom;
            new_origin - pivot + size * new_zoom * 0.5
        };
        let pivot = pos2(100.0, 100.0);
        let pan = Vec2::new(10.0, 20.0);
        let size = Vec2::new(200.0, 100.0);
        let screen = pos2(150.0, 80.0);
        let got = zoom_pan(pivot, pan, 0.0, size, 2.0, screen, 4.0);
        let want = old(pivot, pan, size, 2.0, screen, 4.0);
        assert_eq!(got, want, "{got:?} vs {want:?}");
        assert_eq!(got, Vec2::new(-30.0, 60.0));
    }

    #[test]
    fn zoom_about_a_turned_view_keeps_the_point_under_the_cursor() {
        let pivot = pos2(100.0, 80.0);
        let pan = Vec2::new(30.0, -10.0);
        let rot = 25.0;
        let size = Vec2::new(200.0, 100.0);
        let zoom = 2.0;
        let screen = pos2(140.0, 60.0);
        let new_zoom = 5.0;
        let stage = size * 0.5 + spin_vec(screen - pivot - pan, -rot) / zoom;
        let pan2 = zoom_pan(pivot, pan, rot, size, zoom, screen, new_zoom);
        let back = pivot + pan2 + spin_vec((stage - size * 0.5) * new_zoom, rot);
        assert!((back.x - screen.x).abs() < 1e-3 && (back.y - screen.y).abs() < 1e-3, "{back:?}");
    }

    #[test]
    fn screen_mapping_roundtrips_and_zero_stays_on_the_origin() {
        let origin = pos2(-90.0, 20.0);
        let zoom = 2.0;
        let pivot = pos2(100.0, 100.0);
        let pan = Vec2::new(10.0, 20.0);
        let p = StageVec::new(120.0, 30.0);
        let straight = pos2(origin.x + p.x * zoom, origin.y + p.y * zoom);
        assert_eq!(place(straight, pivot, pan, 0.0), straight);
        let turned = place(straight, pivot, pan, 15.0);
        let back = unplace(turned, pivot, pan, 15.0);
        assert!((back.x - straight.x).abs() < 1e-3 && (back.y - straight.y).abs() < 1e-3);
        let aff = screen_affine(origin, zoom, 15.0, pivot, pan);
        let got = aff.apply(p);
        assert!((got.x - turned.x).abs() < 1e-3 && (got.y - turned.y).abs() < 1e-3, "{got:?} vs {turned:?}");
        let upright = screen_affine(origin, zoom, 0.0, pivot, pan);
        let u = upright.apply(p);
        assert_eq!((u.x, u.y), (straight.x, straight.y));
    }
}
