//! 3D Postcard perspective projection (layer L0, P-044).
//!
//! Projects 2D stage-space planar geometry ("postcards" lying at `Z = 0` in their rest frame)
//! through a 3D Euler rotation `(rx, ry, rz)` about a pivot `(px, py, 0)`, a depth offset `tz`
//! along the camera's optical axis, and a pinhole camera placed at `(vx, vy, -focal)` looking
//! down `+Z` toward the stage plane `Z = 0`.
//!
//! Straight lines stay straight under perspective homography unless clamped against the camera's
//! near plane (`Z_cam >= NEAR_Z`), while cubic and quadratic Bézier segments are adaptively
//! subdivided until their cubic control-point image matches the exact rational perspective curve
//! within `tol` pixels.

use crate::{Affine, Path, PathCmd, Rect, Vec2};

/// Closest camera-space depth allowed (`Z_cam >= NEAR_Z` in stage pixels) so points behind or
/// grazing the lens never divide by zero or explode to infinity.
pub const NEAR_Z: f32 = 10.0;

/// Minimum and maximum field-of-view angles in degrees.
pub const MIN_FOV_DEG: f32 = 10.0;
pub const MAX_FOV_DEG: f32 = 150.0;
pub const DEFAULT_FOV_DEG: f32 = 55.0;

/// Adaptive subdivision bounds (matching `envelope`).
const MAX_DEPTH: u32 = 9;
pub const MAX_PROJECTED_COMMANDS: usize = 200_000;

/// Default subdivision resolution (`12 × 12` cells) for perspective-foreshortened bitmap meshes.
pub const BITMAP_MESH_CELLS: u32 = 12;

/// Precomputed 3D postcard projector in stage space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Projector3D {
    /// Centre of 3D rotation in stage space (`Z = 0`).
    pub pivot: Vec2,
    /// Vanishing point (optical axis intersection with stage `Z = 0`).
    pub vanishing: Vec2,
    /// Pinhole focal length in stage pixels (`0.5 * stage_diag / tan(fov / 2)`).
    pub focal: f32,
    /// Depth translation along `+Z` (into the screen; negative comes toward the camera).
    pub tz: f32,
    /// First two columns of the 3×3 rotation matrix `R = Rz(rz) · Ry(ry) · Rx(rx)` (since local
    /// postcard points have `dz = 0`).
    r00: f64,
    r01: f64,
    r10: f64,
    r11: f64,
    r20: f64,
    r21: f64,
}

/// Rest-space vertices, stage-space projected vertices, and triangle vertex indices of a
/// perspective-projected bitmap warp grid.
pub type ProjectedGrid = (Vec<Vec2>, Vec<Vec2>, Vec<[u32; 3]>);

impl Projector3D {
    /// Builds a pinhole projector for a stage of `stage_w × stage_h` pixels.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        rx_deg: f32,
        ry_deg: f32,
        rz_deg: f32,
        tz: f32,
        fov_deg: f32,
        pivot: Vec2,
        vanishing: Option<Vec2>,
        stage_w: f32,
        stage_h: f32,
    ) -> Self {
        let clean = |v: f32, d: f32| if v.is_finite() { v } else { d };
        let sw = clean(stage_w, 1280.0).max(1.0);
        let sh = clean(stage_h, 720.0).max(1.0);
        let diag = sw.hypot(sh).max(1.0);
        let fov = clean(fov_deg, DEFAULT_FOV_DEG).clamp(MIN_FOV_DEG, MAX_FOV_DEG);
        let half_tan = (f64::from(fov).to_radians() * 0.5).tan().max(1e-4);
        let focal = ((0.5 * f64::from(diag)) / half_tan).clamp(f64::from(NEAR_Z), 1.0e6) as f32;

        let pivot = if pivot.is_finite() { pivot } else { Vec2::new(sw * 0.5, sh * 0.5) };
        let vanishing = vanishing
            .filter(|v| v.is_finite())
            .unwrap_or_else(|| Vec2::new(sw * 0.5, sh * 0.5));
        let tz = clean(tz, 0.0).clamp(-100_000.0, 100_000.0);

        let rx = f64::from(clean(rx_deg, 0.0).clamp(-100_000.0, 100_000.0)).to_radians();
        let ry = f64::from(clean(ry_deg, 0.0).clamp(-100_000.0, 100_000.0)).to_radians();
        let rz = f64::from(clean(rz_deg, 0.0).clamp(-100_000.0, 100_000.0)).to_radians();

        let (sx, cx) = rx.sin_cos();
        let (sy, cy) = ry.sin_cos();
        let (sz, cz) = rz.sin_cos();

        // For a planar point (dx, dy, 0):
        // Rx: (dx, dy * cx, dy * sx)
        // Ry: (dx * cy + dy * sx * sy, dy * cx, -dx * sy + dy * sx * cy)
        // Rz:
        let m00 = cy;
        let m01 = sx * sy;
        let m10 = 0.0;
        let m11 = cx;
        let r00 = cz * m00 - sz * m10;
        let r01 = cz * m01 - sz * m11;
        let r10 = sz * m00 + cz * m10;
        let r11 = sz * m01 + cz * m11;
        let r20 = -sy;
        let r21 = sx * cy;

        Self { pivot, vanishing, focal, tz, r00, r01, r10, r11, r20, r21 }
    }

    /// Computes focal length in stage pixels for `fov_deg` and `stage_w × stage_h`.
    pub fn focal_length(fov_deg: f32, stage_w: f32, stage_h: f32) -> f32 {
        let sw = if stage_w.is_finite() { stage_w.max(1.0) } else { 1280.0 };
        let sh = if stage_h.is_finite() { stage_h.max(1.0) } else { 720.0 };
        let fov = if fov_deg.is_finite() { fov_deg.clamp(MIN_FOV_DEG, MAX_FOV_DEG) } else { DEFAULT_FOV_DEG };
        let half_tan = (f64::from(fov).to_radians() * 0.5).tan().max(1e-4);
        ((0.5 * f64::from(sw.hypot(sh).max(1.0))) / half_tan).clamp(f64::from(NEAR_Z), 1.0e6) as f32
    }

    /// Projects a stage-space point `p` (lying at `Z = 0` on the postcard) onto the stage plane,
    /// returning `(projected_point, perspective_scale)` where `perspective_scale = focal / z_cam`.
    pub fn project_with_scale(&self, p: Vec2) -> (Vec2, f32) {
        if !p.is_finite() {
            return (self.pivot, 1.0);
        }
        let dx = f64::from(p.x) - f64::from(self.pivot.x);
        let dy = f64::from(p.y) - f64::from(self.pivot.y);
        let wx = f64::from(self.pivot.x) + self.r00 * dx + self.r01 * dy;
        let wy = f64::from(self.pivot.y) + self.r10 * dx + self.r11 * dy;
        let wz = f64::from(self.tz) + self.r20 * dx + self.r21 * dy;

        let focal = f64::from(self.focal);
        let z_cam = (focal + wz).max(f64::from(NEAR_Z));
        let k = focal / z_cam;
        let vx = f64::from(self.vanishing.x);
        let vy = f64::from(self.vanishing.y);
        let sx = (vx + (wx - vx) * k).clamp(-1.0e7, 1.0e7) as f32;
        let sy = (vy + (wy - vy) * k).clamp(-1.0e7, 1.0e7) as f32;
        (Vec2::new(sx, sy), (k as f32).clamp(1e-4, 100.0))
    }

    /// Projects a stage-space point `p` onto the stage plane.
    pub fn project(&self, p: Vec2) -> Vec2 {
        self.project_with_scale(p).0
    }

    /// Local perspective scale factor (`focal / z_cam`) at stage point `p`.
    pub fn scale_at(&self, p: Vec2) -> f32 {
        self.project_with_scale(p).1
    }

    /// Projects `path` (given in local space with `local_to_stage` affine transform) into stage
    /// space through the 3D pinhole homography, adaptively subdividing curves (and lines that
    /// cross the near-plane clamp) within `tol` stage pixels.
    pub fn project_path(&self, path: &Path, local_to_stage: &Affine, tol: f32) -> Path {
        if !local_to_stage.is_finite() {
            return Path::new();
        }
        let tol = if tol.is_finite() { tol.max(0.02) } else { 0.15 };
        let map = |p: Vec2| self.project(local_to_stage.apply(p));
        let line_cubic = |a: Vec2, b: Vec2| [a, a.lerp(b, 1.0 / 3.0), a.lerp(b, 2.0 / 3.0), b];

        let mut out = Path { cmds: Vec::with_capacity(path.cmds.len()) };
        let (mut start, mut cur) = (Vec2::ZERO, Vec2::ZERO);
        for cmd in &path.cmds {
            match *cmd {
                PathCmd::MoveTo(p) => {
                    out.move_to(map(p));
                    start = p;
                    cur = p;
                }
                PathCmd::LineTo(p) => {
                    self.emit_segment(&mut out, &line_cubic(cur, p), true, local_to_stage, tol, 0);
                    cur = p;
                }
                PathCmd::QuadTo(q, p) => {
                    let c = [cur, cur.lerp(q, 2.0 / 3.0), p.lerp(q, 2.0 / 3.0), p];
                    self.emit_segment(&mut out, &c, false, local_to_stage, tol, 0);
                    cur = p;
                }
                PathCmd::CubicTo(a, b, p) => {
                    self.emit_segment(&mut out, &[cur, a, b, p], false, local_to_stage, tol, 0);
                    cur = p;
                }
                PathCmd::Close => {
                    if cur != start && !self.stays_straight(&line_cubic(cur, start), local_to_stage, tol) {
                        self.emit_segment(&mut out, &line_cubic(cur, start), true, local_to_stage, tol, 0);
                    }
                    out.close();
                    cur = start;
                }
            }
            if out.cmds.len() >= MAX_PROJECTED_COMMANDS {
                break;
            }
        }
        out
    }

    /// Generates a perspective-projected grid `(rest, shown, tris)` of `cells × cells` quads
    /// (`2 * cells * cells` alternating-diagonal triangles) over `bounds` in local space:
    /// `rest` stays in local space while `shown` is in stage space (`project(local_to_stage · rest)`).
    pub fn project_grid(&self, bounds: &Rect, local_to_stage: &Affine, cells: u32) -> Option<ProjectedGrid> {
        if bounds.is_empty() || !bounds.min.is_finite() || !bounds.max.is_finite() || !local_to_stage.is_finite() {
            return None;
        }
        let n = cells.clamp(2, 48);
        let vcount = ((n + 1) * (n + 1)) as usize;
        let mut rest = Vec::with_capacity(vcount);
        let mut shown = Vec::with_capacity(vcount);
        let (w, h) = (bounds.width(), bounds.height());
        for j in 0..=n {
            let y = if j == n { bounds.max.y } else { bounds.min.y + h * (j as f32 / n as f32) };
            for i in 0..=n {
                let x = if i == n { bounds.max.x } else { bounds.min.x + w * (i as f32 / n as f32) };
                let p = Vec2::new(x, y);
                rest.push(p);
                shown.push(self.project(local_to_stage.apply(p)));
            }
        }
        let mut tris = Vec::with_capacity((2 * n * n) as usize);
        let vidx = |i: u32, j: u32| j * (n + 1) + i;
        for j in 0..n {
            for i in 0..n {
                let (a, b, c, d) = (vidx(i, j), vidx(i + 1, j), vidx(i, j + 1), vidx(i + 1, j + 1));
                if (i + j).is_multiple_of(2) {
                    tris.push([a, b, d]);
                    tris.push([a, d, c]);
                } else {
                    tris.push([a, b, c]);
                    tris.push([b, d, c]);
                }
            }
        }
        Some((rest, shown, tris))
    }

    fn stays_straight(&self, c: &[Vec2; 4], local_to_stage: &Affine, tol: f32) -> bool {
        let a = self.project(local_to_stage.apply(c[0]));
        let b = self.project(local_to_stage.apply(c[3]));
        // Under pure perspective without near-plane clamping, a straight line maps to a straight
        // line (though non-uniformly parameterized in t), so distance to the segment AB is 0.
        [0.25f32, 0.5, 0.75]
            .iter()
            .all(|&t| crate::segment_distance(self.project(local_to_stage.apply(cubic_at(c, t))), a, b) <= tol)
    }

    fn emit_segment(&self, out: &mut Path, c: &[Vec2; 4], straight: bool, local_to_stage: &Affine, tol: f32, depth: u32) {
        let map = |p: Vec2| self.project(local_to_stage.apply(p));
        let m = [map(c[0]), map(c[1]), map(c[2]), map(c[3])];
        if straight && self.stays_straight(c, local_to_stage, tol) {
            out.line_to(m[3]);
            return;
        }
        let err = [0.25f32, 0.5, 0.75]
            .iter()
            .map(|&t| map(cubic_at(c, t)).distance(cubic_at(&m, t)))
            .fold(0.0f32, f32::max);
        if err <= tol || depth >= MAX_DEPTH || out.cmds.len() >= MAX_PROJECTED_COMMANDS {
            out.cubic_to(m[1], m[2], m[3]);
            return;
        }
        let (left, right) = split_cubic(c);
        self.emit_segment(out, &left, straight, local_to_stage, tol, depth + 1);
        self.emit_segment(out, &right, straight, local_to_stage, tol, depth + 1);
    }
}

fn cubic_at(c: &[Vec2; 4], t: f32) -> Vec2 {
    let u = 1.0 - t;
    c[0] * (u * u * u) + c[1] * (3.0 * u * u * t) + c[2] * (3.0 * u * t * t) + c[3] * (t * t * t)
}

fn split_cubic(c: &[Vec2; 4]) -> ([Vec2; 4], [Vec2; 4]) {
    let (ab, bc, cd) = (c[0].lerp(c[1], 0.5), c[1].lerp(c[2], 0.5), c[2].lerp(c[3], 0.5));
    let (abc, bcd) = (ab.lerp(bc, 0.5), bc.lerp(cd, 0.5));
    let mid = abc.lerp(bcd, 0.5);
    ([c[0], ab, abc, mid], [mid, bcd, cd, c[3]])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_projection_preserves_points_and_lines() {
        let p = Projector3D::new(0.0, 0.0, 0.0, 0.0, 55.0, Vec2::new(200.0, 150.0), None, 800.0, 600.0);
        for q in [Vec2::new(0.0, 0.0), Vec2::new(200.0, 150.0), Vec2::new(640.0, 480.0)] {
            assert!(p.project(q).distance(q) < 1e-4, "{q:?} -> {:?}", p.project(q));
        }
        let rect = Path::rect(50.0, 60.0, 120.0, 80.0);
        let out = p.project_path(&rect, &Affine::IDENTITY, 0.05);
        assert_eq!(out.cmds.len(), rect.cmds.len());
    }

    #[test]
    fn yaw_rotation_foreshortens_far_edge_and_enlarges_near_edge() {
        // Pivot at stage centre (400, 300); positive ry pushes +X into +Z (farther) and -X into -Z (nearer)?
        // Check: r20 = -sin(ry). For ry = +45°, dx > 0 has wz = -dx*sin(45°) < 0 (closer to camera -> taller!),
        // while dx < 0 has wz > 0 (farther from camera -> shorter!).
        let p = Projector3D::new(0.0, 45.0, 0.0, 0.0, 55.0, Vec2::new(400.0, 300.0), None, 800.0, 600.0);
        let left_top = p.project(Vec2::new(300.0, 200.0));
        let left_bot = p.project(Vec2::new(300.0, 400.0));
        let right_top = p.project(Vec2::new(500.0, 200.0));
        let right_bot = p.project(Vec2::new(500.0, 400.0));
        let left_h = (left_bot.y - left_top.y).abs();
        let right_h = (right_bot.y - right_top.y).abs();
        assert!(right_h > 200.0 && left_h < 200.0, "foreshortening: left_h={left_h}, right_h={right_h}");
        // Horizontal width narrows under 45° yaw.
        assert!((right_top.x - left_top.x).abs() < 200.0);
    }

    #[test]
    fn near_plane_clamp_prevents_singularity_or_nan() {
        let p = Projector3D::new(85.0, -85.0, 30.0, -50_000.0, 120.0, Vec2::new(100.0, 100.0), None, 800.0, 600.0);
        let circle = Path::ellipse(200.0, 200.0, 200.0, 200.0);
        let projected = p.project_path(&circle, &Affine::IDENTITY, 0.25);
        assert!(!projected.is_empty());
        assert!(projected.bounds().min.is_finite() && projected.bounds().max.is_finite());
    }
}
