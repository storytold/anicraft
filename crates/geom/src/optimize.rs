//! Shape optimization, smoothing, straightening, 4-corner projective homography distortion,
//! and arc-length spine warping for vector art brushes (layer L0).

use crate::walk::CurveWalk;
use crate::{Affine, Path, PathCmd, Rect, Vec2, fit_curve, segment_distance};

/// Maximum commands a deformed path (distort or art-brush warp) may emit.
pub const MAX_WARPED_COMMANDS: usize = 200_000;
/// Maximum recursive subdivision depth for nonlinear cubic deformation.
const MAX_SUBDIV_DEPTH: u32 = 9;

#[derive(Clone, Copy, Debug)]
struct Knot {
    anchor: Vec2,
    cin: Vec2,
    cout: Vec2,
}

#[derive(Clone, Debug)]
struct Subpath {
    knots: Vec<Knot>,
    closed: bool,
}

fn cubic_at(c: &[Vec2; 4], t: f32) -> Vec2 {
    let u = 1.0 - t;
    c[0] * (u * u * u) + c[1] * (3.0 * u * u * t) + c[2] * (3.0 * u * t * t) + c[3] * (t * t * t)
}

fn split_cubic(c: &[Vec2; 4]) -> ([Vec2; 4], [Vec2; 4]) {
    let (ab, bc, cd) = (c[0].lerp(c[1], 0.5), c[1].lerp(c[2], 0.5), c[2].lerp(c[3], 0.5));
    let (abc, bcd) = (ab.lerp(bc, 0.5), bc.lerp(cd, 0.5));
    let m = abc.lerp(bcd, 0.5);
    ([c[0], ab, abc, m], [m, bcd, cd, c[3]])
}

/// Counts anchors on `path`, omitting a duplicate closing anchor that lands on its subpath's
/// `MoveTo`.
pub fn count_anchors(path: &Path) -> usize {
    let mut count = 0usize;
    let mut start: Option<Vec2> = None;
    let mut last_seg_end: Option<Vec2> = None;
    for (i, cmd) in path.cmds.iter().enumerate() {
        match *cmd {
            PathCmd::MoveTo(p) => {
                start = Some(p);
                last_seg_end = None;
                count += 1;
            }
            PathCmd::LineTo(p) | PathCmd::QuadTo(_, p) | PathCmd::CubicTo(_, _, p) => {
                let next_is_close = matches!(path.cmds.get(i + 1), Some(PathCmd::Close));
                if !(next_is_close && start.is_some_and(|s| s.distance(p) <= 1e-3)) {
                    count += 1;
                }
                last_seg_end = Some(p);
            }
            PathCmd::Close => {
                let _ = last_seg_end.take();
            }
        }
    }
    count
}

fn parse_subpaths(path: &Path) -> Vec<Subpath> {
    let mut out = Vec::new();
    let mut cur_knots: Vec<Knot> = Vec::new();
    for cmd in &path.cmds {
        match *cmd {
            PathCmd::MoveTo(p) => {
                if !cur_knots.is_empty() {
                    out.push(Subpath { knots: std::mem::take(&mut cur_knots), closed: false });
                }
                cur_knots.push(Knot { anchor: p, cin: p, cout: p });
            }
            PathCmd::LineTo(p) => {
                cur_knots.push(Knot { anchor: p, cin: p, cout: p });
            }
            PathCmd::QuadTo(q, p) => {
                if let Some(prev) = cur_knots.last_mut() {
                    let prev_a = prev.anchor;
                    prev.cout = prev_a.lerp(q, 2.0 / 3.0);
                    cur_knots.push(Knot { anchor: p, cin: p.lerp(q, 2.0 / 3.0), cout: p });
                } else {
                    cur_knots.push(Knot { anchor: p, cin: p, cout: p });
                }
            }
            PathCmd::CubicTo(c1, c2, p) => {
                if let Some(prev) = cur_knots.last_mut() {
                    prev.cout = c1;
                    cur_knots.push(Knot { anchor: p, cin: c2, cout: p });
                } else {
                    cur_knots.push(Knot { anchor: p, cin: p, cout: p });
                }
            }
            PathCmd::Close => {
                if cur_knots.len() >= 2
                    && let (Some(first), Some(last)) = (cur_knots.first(), cur_knots.last())
                    && first.anchor.distance(last.anchor) <= 1e-3
                {
                    let last_cin = last.cin;
                    cur_knots.pop();
                    if let Some(f) = cur_knots.first_mut() {
                        f.cin = last_cin;
                    }
                }
                if !cur_knots.is_empty() {
                    out.push(Subpath { knots: std::mem::take(&mut cur_knots), closed: true });
                }
            }
        }
    }
    if !cur_knots.is_empty() {
        out.push(Subpath { knots: cur_knots, closed: false });
    }
    out
}

fn emit_subpaths(subs: &[Subpath]) -> Path {
    let mut out = Path::new();
    let emit_seg = |out: &mut Path, a: &Knot, b: &Knot| {
        if a.cout.distance(a.anchor) <= 1e-4 && b.cin.distance(b.anchor) <= 1e-4 {
            out.line_to(b.anchor);
        } else {
            out.cubic_to(a.cout, b.cin, b.anchor);
        }
    };
    for sub in subs {
        let Some(first) = sub.knots.first() else { continue };
        out.move_to(first.anchor);
        for pair in sub.knots.windows(2) {
            if let [a, b] = pair {
                emit_seg(&mut out, a, b);
            }
        }
        if sub.closed {
            if let Some(last) = sub.knots.last()
                && sub.knots.len() >= 2
                && (last.cout.distance(last.anchor) > 1e-4 || first.cin.distance(first.anchor) > 1e-4)
            {
                out.cubic_to(last.cout, first.cin, first.anchor);
            }
            out.close();
        }
    }
    out
}

/// Smooths a path's anchors and tangent handles according to `strength` in `0.0..=100.0`.
/// Open endpoints stay pinned; interior anchors and closed loops relax via Taubin/Laplacian
/// smoothing while corners blend toward smooth Catmull-Rom cubic tangents.
pub fn smooth_path(path: &Path, strength: f32) -> Path {
    let alpha = if strength.is_finite() { (strength / 100.0).clamp(0.0, 1.0) } else { 0.5 };
    if alpha <= 1e-5 || path.is_empty() {
        return path.clone();
    }
    let mut subs = parse_subpaths(path);
    for sub in &mut subs {
        let n = sub.knots.len();
        if n < 2 {
            continue;
        }
        // Taubin two-step relaxation keeps closed shapes from collapsing inward while smoothing jagged corners.
        let passes = if alpha > 0.6 { 2 } else { 1 };
        let lambda = 0.42 * alpha;
        let mu = -0.44 * alpha;
        if n >= 3 {
            for _ in 0..passes {
                for step in [lambda, mu] {
                    let prev_anchors: Vec<Vec2> = sub.knots.iter().map(|k| k.anchor).collect();
                    for i in 0..n {
                        if !sub.closed && (i == 0 || i + 1 == n) {
                            continue;
                        }
                        let im = if i == 0 { n - 1 } else { i - 1 };
                        let ip = (i + 1) % n;
                        let (Some(&pm), Some(&p0), Some(&pp)) = (prev_anchors.get(im), prev_anchors.get(i), prev_anchors.get(ip)) else { continue };
                        let avg = (pm + pp) * 0.5;
                        let delta = (avg - p0) * step;
                        if let Some(k) = sub.knots.get_mut(i) {
                            k.anchor = p0 + delta;
                            k.cin = k.cin + delta;
                            k.cout = k.cout + delta;
                        }
                    }
                }
            }
        }
        // Compute Catmull-Rom smooth handles and blend by `alpha`.
        let anchors: Vec<Vec2> = sub.knots.iter().map(|k| k.anchor).collect();
        for i in 0..n {
            let Some(&p0) = anchors.get(i) else { continue };
            if !sub.closed && (i == 0 || i + 1 == n) {
                if i == 0
                    && let Some(&p1) = anchors.get(1)
                    && let Some(k) = sub.knots.get_mut(0)
                {
                    let target_out = p0.lerp(p1, 1.0 / 3.0);
                    k.cout = k.cout.lerp(target_out, alpha * 0.5);
                } else if i + 1 == n
                    && let Some(&pm) = anchors.get(n - 2)
                    && let Some(k) = sub.knots.get_mut(i)
                {
                    let target_in = p0.lerp(pm, 1.0 / 3.0);
                    k.cin = k.cin.lerp(target_in, alpha * 0.5);
                }
                continue;
            }
            let im = if i == 0 { n - 1 } else { i - 1 };
            let ip = (i + 1) % n;
            let (Some(&pm), Some(&pp)) = (anchors.get(im), anchors.get(ip)) else { continue };
            let dir = (pp - pm).normalized();
            let d_in = p0.distance(pm) / 3.0;
            let d_out = p0.distance(pp) / 3.0;
            let target_in = p0 - dir * d_in;
            let target_out = p0 + dir * d_out;
            if let Some(k) = sub.knots.get_mut(i) {
                k.cin = k.cin.lerp(target_in, alpha);
                k.cout = k.cout.lerp(target_out, alpha);
            }
        }
    }
    emit_subpaths(&subs)
}

/// Collapses nearly-collinear curves into clean `LineTo` segments, merges nearly-collinear
/// consecutive lines, and snaps near-horizontal / near-vertical lines within `tolerance`.
pub fn straighten_path(path: &Path, tolerance: f32) -> Path {
    let tol = if tolerance.is_finite() { tolerance.clamp(0.01, 500.0) } else { 4.0 };
    if path.is_empty() {
        return path.clone();
    }
    let mut subs = parse_subpaths(path);
    for sub in &mut subs {
        let n = sub.knots.len();
        if n < 2 {
            continue;
        }
        // 1. Collapse curves whose control points lie within `tol` of the chord.
        let seg_count = if sub.closed { n } else { n - 1 };
        for i in 0..seg_count {
            let j = (i + 1) % n;
            let (Some(ka), Some(kb)) = (sub.knots.get(i).copied(), sub.knots.get(j).copied()) else { continue };
            let d1 = segment_distance(ka.cout, ka.anchor, kb.anchor);
            let d2 = segment_distance(kb.cin, ka.anchor, kb.anchor);
            if d1 <= tol && d2 <= tol {
                if let Some(a) = sub.knots.get_mut(i) {
                    a.cout = a.anchor;
                }
                if let Some(b) = sub.knots.get_mut(j) {
                    b.cin = b.anchor;
                }
            }
        }
        // 2. Merge consecutive straight segments that are nearly collinear.
        let mut merged: Vec<Knot> = Vec::with_capacity(sub.knots.len());
        for k in &sub.knots {
            while merged.len() >= 2 {
                let len = merged.len();
                let (Some(a), Some(b)) = (merged.get(len - 2).copied(), merged.get(len - 1).copied()) else { break };
                let ab_straight = a.cout.distance(a.anchor) <= 1e-4 && b.cin.distance(b.anchor) <= 1e-4;
                let bc_straight = b.cout.distance(b.anchor) <= 1e-4 && k.cin.distance(k.anchor) <= 1e-4;
                if ab_straight && bc_straight && segment_distance(b.anchor, a.anchor, k.anchor) <= tol * 0.5 {
                    merged.pop();
                } else {
                    break;
                }
            }
            merged.push(*k);
        }
        if !sub.closed || merged.len() >= 3 {
            sub.knots = merged;
        }
        // 3. Snap near-horizontal and near-vertical straight segments.
        let m = sub.knots.len();
        let segs = if sub.closed { m } else { m.saturating_sub(1) };
        for i in 0..segs {
            let j = (i + 1) % m;
            let (Some(ka), Some(kb)) = (sub.knots.get(i).copied(), sub.knots.get(j).copied()) else { continue };
            let straight = ka.cout.distance(ka.anchor) <= 1e-4 && kb.cin.distance(kb.anchor) <= 1e-4;
            if !straight {
                continue;
            }
            let dx = kb.anchor.x - ka.anchor.x;
            let dy = kb.anchor.y - ka.anchor.y;
            if dy.abs() <= tol && dx.abs() > tol * 1.5 {
                let y = if i == 0 && sub.closed { ka.anchor.y } else { (ka.anchor.y + kb.anchor.y) * 0.5 };
                if let Some(a) = sub.knots.get_mut(i) {
                    let old = a.anchor;
                    a.anchor.y = y;
                    a.cin.y += y - old.y;
                    a.cout.y += y - old.y;
                }
                if let Some(b) = sub.knots.get_mut(j) {
                    let old = b.anchor;
                    b.anchor.y = y;
                    b.cin.y += y - old.y;
                    b.cout.y += y - old.y;
                }
            } else if dx.abs() <= tol && dy.abs() > tol * 1.5 {
                let x = if i == 0 && sub.closed { ka.anchor.x } else { (ka.anchor.x + kb.anchor.x) * 0.5 };
                if let Some(a) = sub.knots.get_mut(i) {
                    let old = a.anchor;
                    a.anchor.x = x;
                    a.cin.x += x - old.x;
                    a.cout.x += x - old.x;
                }
                if let Some(b) = sub.knots.get_mut(j) {
                    let old = b.anchor;
                    b.anchor.x = x;
                    b.cin.x += x - old.x;
                    b.cout.x += x - old.x;
                }
            }
        }
    }
    emit_subpaths(&subs)
}

/// Ramer-Douglas-Peucker simplification on an open polyline.
fn rdp(pts: &[Vec2], eps: f32, out: &mut Vec<Vec2>) {
    if pts.len() <= 2 {
        for &p in pts {
            if out.last().is_none_or(|q| q.distance(p) > 1e-5) {
                out.push(p);
            }
        }
        return;
    }
    let (Some(&first), Some(&last)) = (pts.first(), pts.last()) else { return };
    let mut best_d = -1.0f32;
    let mut best_i = 0usize;
    for (i, &p) in pts.iter().enumerate().take(pts.len() - 1).skip(1) {
        let d = segment_distance(p, first, last);
        if d > best_d {
            best_d = d;
            best_i = i;
        }
    }
    if best_d <= eps {
        if out.last().is_none_or(|q| q.distance(first) > 1e-5) {
            out.push(first);
        }
        if out.last().is_none_or(|q| q.distance(last) > 1e-5) {
            out.push(last);
        }
    } else if let (Some(left), Some(right)) = (pts.get(..=best_i), pts.get(best_i..)) {
        rdp(left, eps, out);
        rdp(right, eps, out);
    }
}

/// Optimizes a path by flattening redundant short & collinear segments and refitting minimal
/// cubic Béziers and straight lines within `tolerance` (`0.1..=50.0`).
pub fn optimize_path(path: &Path, tolerance: f32) -> Path {
    let tol = if tolerance.is_finite() { tolerance.clamp(0.1, 50.0) } else { 2.5 };
    if path.is_empty() {
        return path.clone();
    }
    let flat_tol = (tol * 0.15).clamp(0.03, 0.5);
    let polylines = path.flatten(&Affine::IDENTITY, flat_tol);
    let mut out = Path::new();
    for pl in polylines {
        if pl.points.len() < 2 {
            continue;
        }
        // Filter micro-jitter points closer than a fraction of tolerance.
        let min_step = (tol * 0.25).clamp(0.05, 4.0);
        let mut cleaned: Vec<Vec2> = Vec::with_capacity(pl.points.len());
        for &p in &pl.points {
            if !p.is_finite() {
                continue;
            }
            if cleaned.last().is_none_or(|q| q.distance(p) >= min_step) {
                cleaned.push(p);
            }
        }
        if cleaned.len() < 2 {
            if let (Some(&a), Some(&b)) = (pl.points.first(), pl.points.last())
                && a.distance(b) > 1e-4
            {
                out.move_to(a).line_to(b);
                if pl.closed {
                    out.close();
                }
            }
            continue;
        }
        if !pl.closed
            && let Some(&last_orig) = pl.points.last()
            && cleaned.last().is_some_and(|q| q.distance(last_orig) > 1e-5)
        {
            cleaned.push(last_orig);
        }
        let corner_win = (tol * 2.0).clamp(3.0, 24.0);
        let fitted = if pl.closed {
            // If a closed polygon simplifies cleanly to straight edges via RDP within tol * 0.25, keep crisp lines;
            // otherwise fit minimal cubics + straighten nearly-flat cubics.
            let c = fit_curve(&cleaned, tol, corner_win, true);
            straighten_collinear_cubics(&c, tol * 0.2)
        } else {
            let mut simp = Vec::new();
            rdp(&cleaned, tol * 0.2, &mut simp);
            let src = if simp.len() >= 2 { &simp } else { &cleaned };
            let c = fit_curve(src, tol, corner_win, false);
            straighten_collinear_cubics(&c, tol * 0.2)
        };
        if !fitted.is_empty() {
            out.cmds.extend(fitted.cmds);
        }
    }
    if out.is_empty() { path.clone() } else { out }
}

fn straighten_collinear_cubics(path: &Path, eps: f32) -> Path {
    let mut out = Path::new();
    let mut cur = Vec2::ZERO;
    for cmd in &path.cmds {
        match *cmd {
            PathCmd::MoveTo(p) => {
                out.move_to(p);
                cur = p;
            }
            PathCmd::LineTo(p) => {
                out.line_to(p);
                cur = p;
            }
            PathCmd::QuadTo(q, p) => {
                if segment_distance(q, cur, p) <= eps {
                    out.line_to(p);
                } else {
                    out.quad_to(q, p);
                }
                cur = p;
            }
            PathCmd::CubicTo(c1, c2, p) => {
                if segment_distance(c1, cur, p) <= eps && segment_distance(c2, cur, p) <= eps {
                    out.line_to(p);
                } else {
                    out.cubic_to(c1, c2, p);
                }
                cur = p;
            }
            PathCmd::Close => {
                out.close();
            }
        }
    }
    out
}

/// Exact 2D projective homography from a rest rectangle `(TL, TR, BR, BL)` to an arbitrary convex
/// quad `corners = [TL, TR, BR, BL]`.
#[derive(Clone, Copy, Debug)]
pub struct Homography {
    rest_min: (f64, f64),
    rest_size: (f64, f64),
    /// Numerators and perspective denominator over normalized unit square `(u, v) ∈ [0, 1]²`:
    /// `x' = (a*u + b*v + c) / (g*u + h*v + 1)`
    /// `y' = (d*u + e*v + f) / (g*u + h*v + 1)`
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    e: f64,
    f: f64,
    g: f64,
    h: f64,
}

impl Homography {
    /// Computes the projective map from `rest`'s `(TL, TR, BR, BL)` to `corners`.
    /// Returns `None` if `rest` has zero area, any point is non-finite, or `corners` is
    /// degenerate / non-convex (where the horizon line crosses `[0, 1]²`).
    pub fn from_rect_to_quad(rest: &Rect, corners: &[Vec2; 4]) -> Option<Homography> {
        if !corners.iter().all(|p| p.is_finite())
            || !rest.min.is_finite()
            || !rest.max.is_finite()
            || !(rest.width() > 1e-6 && rest.height() > 1e-6)
        {
            return None;
        }
        let p0 = (f64::from(corners[0].x), f64::from(corners[0].y));
        let p1 = (f64::from(corners[1].x), f64::from(corners[1].y));
        let p2 = (f64::from(corners[2].x), f64::from(corners[2].y));
        let p3 = (f64::from(corners[3].x), f64::from(corners[3].y));

        // Verify strict quad convexity (consistent non-zero cross-product sign at all 4 corners).
        let edges = [
            (p1.0 - p0.0, p1.1 - p0.1),
            (p2.0 - p1.0, p2.1 - p1.1),
            (p3.0 - p2.0, p3.1 - p2.1),
            (p0.0 - p3.0, p0.1 - p3.1),
        ];
        let mut sign = 0.0f64;
        for i in 0..4 {
            let e0 = edges[i];
            let e1 = edges[(i + 1) % 4];
            let cross = e0.0 * e1.1 - e0.1 * e1.0;
            if cross.abs() <= 1e-6 {
                return None;
            }
            if sign == 0.0 {
                sign = cross.signum();
            } else if cross * sign <= 0.0 {
                return None;
            }
        }

        let dx1 = p1.0 - p2.0;
        let dy1 = p1.1 - p2.1;
        let dx2 = p3.0 - p2.0;
        let dy2 = p3.1 - p2.1;
        let sx = p0.0 - p1.0 + p2.0 - p3.0;
        let sy = p0.1 - p1.1 + p2.1 - p3.1;

        let (g, h) = if sx.abs() < 1e-12 && sy.abs() < 1e-12 {
            (0.0, 0.0)
        } else {
            let det = dx1 * dy2 - dx2 * dy1;
            if det.abs() <= 1e-9 {
                return None;
            }
            ((sx * dy2 - sy * dx2) / det, (dx1 * sy - dy1 * sx) / det)
        };

        // Ensure denominator `g*u + h*v + 1` stays strictly positive on `[0, 1]²`.
        if 1.0 + g <= 1e-3 || 1.0 + h <= 1e-3 || 1.0 + g + h <= 1e-3 {
            return None;
        }

        let a = p1.0 - p0.0 + g * p1.0;
        let b = p3.0 - p0.0 + h * p3.0;
        let c = p0.0;
        let d = p1.1 - p0.1 + g * p1.1;
        let e = p3.1 - p0.1 + h * p3.1;
        let f = p0.1;

        Some(Homography {
            rest_min: (f64::from(rest.min.x), f64::from(rest.min.y)),
            rest_size: (f64::from(rest.width()), f64::from(rest.height())),
            a,
            b,
            c,
            d,
            e,
            f,
            g,
            h,
        })
    }

    /// Maps a 2D point through the projective homography, clamping denominator away from zero
    /// for off-bounds Bézier control handles.
    pub fn apply(&self, p: Vec2) -> Option<Vec2> {
        let u = (f64::from(p.x) - self.rest_min.0) / self.rest_size.0;
        let v = (f64::from(p.y) - self.rest_min.1) / self.rest_size.1;
        let denom = (self.g * u + self.h * v + 1.0).max(1e-3);
        let x = (self.a * u + self.b * v + self.c) / denom;
        let y = (self.d * u + self.e * v + self.f) / denom;
        let out = Vec2::new(x as f32, y as f32);
        (out.is_finite() && out.x.abs() <= 1e7 && out.y.abs() <= 1e7).then_some(out)
    }

    pub fn is_affine(&self) -> bool {
        self.g.abs() < 1e-9 && self.h.abs() < 1e-9
    }
}

/// Deforms `path` through the 4-corner projective homography from `rest` to `corners`,
/// keeping straight lines straight and adaptively subdividing curves within `tol`.
pub fn distort_path(path: &Path, rest: &Rect, corners: &[Vec2; 4], tol: f32) -> Option<Path> {
    let h = Homography::from_rect_to_quad(rest, corners)?;
    let tol = if tol.is_finite() { tol.clamp(1e-3, 50.0) } else { 0.1 };
    let mut out = Path::new();
    let (mut start, mut cur) = (Vec2::ZERO, Vec2::ZERO);
    for cmd in &path.cmds {
        match *cmd {
            PathCmd::MoveTo(p) => {
                out.move_to(h.apply(p)?);
                start = p;
                cur = p;
            }
            PathCmd::LineTo(p) => {
                // Under any 2D homography with positive denominator, straight lines map to exact straight lines!
                out.line_to(h.apply(p)?);
                cur = p;
            }
            PathCmd::QuadTo(q, p) => {
                let cubic = [cur, cur.lerp(q, 2.0 / 3.0), p.lerp(q, 2.0 / 3.0), p];
                subdiv_mapped(&mut out, &cubic, tol, 0, &|pt| h.apply(pt))?;
                cur = p;
            }
            PathCmd::CubicTo(c1, c2, p) => {
                let cubic = [cur, c1, c2, p];
                subdiv_mapped(&mut out, &cubic, tol, 0, &|pt| h.apply(pt))?;
                cur = p;
            }
            PathCmd::Close => {
                out.close();
                cur = start;
            }
        }
        if out.cmds.len() > MAX_WARPED_COMMANDS {
            return None;
        }
    }
    Some(out)
}

fn subdiv_mapped(
    out: &mut Path,
    c: &[Vec2; 4],
    tol: f32,
    depth: u32,
    map_fn: &impl Fn(Vec2) -> Option<Vec2>,
) -> Option<()> {
    let m = [map_fn(c[0])?, map_fn(c[1])?, map_fn(c[2])?, map_fn(c[3])?];
    let mut max_err = 0.0f32;
    for t in [0.25f32, 0.5, 0.75] {
        let exact = map_fn(cubic_at(c, t))?;
        max_err = max_err.max(exact.distance(cubic_at(&m, t)));
    }
    if max_err <= tol || depth >= MAX_SUBDIV_DEPTH {
        out.cubic_to(m[1], m[2], m[3]);
        return (out.cmds.len() <= MAX_WARPED_COMMANDS).then_some(());
    }
    let (left, right) = split_cubic(c);
    subdiv_mapped(out, &left, tol, depth + 1, map_fn)?;
    subdiv_mapped(out, &right, tol, depth + 1, map_fn)
}

/// Maps a point `p` in `source_bounds` onto the arc-length parameterized `spine`:
/// `u = (p.x - source_bounds.min.x) / source_bounds.width()`
/// `v = p.y - source_bounds.center().y`
/// `out = spine_pos(u * L) + spine_normal(u * L) * (v * width_scale)`
/// Points slightly outside `[0, 1]` (such as cubic control handles) extrapolate smoothly along
/// the end tangents.
pub fn map_point_to_spine(p: Vec2, source_bounds: &Rect, spine: &CurveWalk, width_scale: f32) -> Option<Vec2> {
    let w = source_bounds.width();
    if !(w > 1e-5 && p.is_finite() && width_scale.is_finite()) {
        return None;
    }
    let u = (p.x - source_bounds.min.x) / w;
    let v = (p.y - source_bounds.center().y) * width_scale;
    let total = spine.length();
    let s = u * total;
    let (base, tan) = if s < 0.0 {
        let t0 = spine.tangent(0.0);
        (spine.at(0.0) + t0 * s, t0)
    } else if s > total {
        let t1 = spine.tangent(total);
        (spine.at(total) + t1 * (s - total), t1)
    } else {
        (spine.at(s), spine.tangent(s))
    };
    let normal = tan.perp();
    let out = base + normal * v;
    (out.is_finite() && out.x.abs() <= 1e7 && out.y.abs() <= 1e7).then_some(out)
}

/// Warps a vector `path` from `source_bounds` along `spine` using arc-length parameterization
/// (`CurveWalk`), adaptively subdividing segments (including straight lines along curved spines!)
/// within `tol`.
pub fn warp_along_spine(
    path: &Path,
    source_bounds: &Rect,
    spine: &CurveWalk,
    width_scale: f32,
    tol: f32,
) -> Option<Path> {
    if source_bounds.width() <= 1e-5 || !width_scale.is_finite() {
        return None;
    }
    let tol = if tol.is_finite() { tol.clamp(0.02, 20.0) } else { 0.25 };
    let map_pt = |pt: Vec2| map_point_to_spine(pt, source_bounds, spine, width_scale);
    let line_as_cubic = |a: Vec2, b: Vec2| [a, a.lerp(b, 1.0 / 3.0), a.lerp(b, 2.0 / 3.0), b];

    let mut out = Path::new();
    let (mut start, mut cur) = (Vec2::ZERO, Vec2::ZERO);
    for cmd in &path.cmds {
        match *cmd {
            PathCmd::MoveTo(p) => {
                out.move_to(map_pt(p)?);
                start = p;
                cur = p;
            }
            PathCmd::LineTo(p) => {
                let c = line_as_cubic(cur, p);
                subdiv_spine_line_or_curve(&mut out, &c, true, tol, 0, &map_pt)?;
                cur = p;
            }
            PathCmd::QuadTo(q, p) => {
                let c = [cur, cur.lerp(q, 2.0 / 3.0), p.lerp(q, 2.0 / 3.0), p];
                subdiv_spine_line_or_curve(&mut out, &c, false, tol, 0, &map_pt)?;
                cur = p;
            }
            PathCmd::CubicTo(c1, c2, p) => {
                let c = [cur, c1, c2, p];
                subdiv_spine_line_or_curve(&mut out, &c, false, tol, 0, &map_pt)?;
                cur = p;
            }
            PathCmd::Close => {
                if cur.distance(start) > 1e-4 {
                    let c = line_as_cubic(cur, start);
                    subdiv_spine_line_or_curve(&mut out, &c, true, tol, 0, &map_pt)?;
                }
                out.close();
                cur = start;
            }
        }
        if out.cmds.len() > MAX_WARPED_COMMANDS {
            return None;
        }
    }
    Some(out)
}

fn subdiv_spine_line_or_curve(
    out: &mut Path,
    c: &[Vec2; 4],
    straight_src: bool,
    tol: f32,
    depth: u32,
    map_fn: &impl Fn(Vec2) -> Option<Vec2>,
) -> Option<()> {
    let m = [map_fn(c[0])?, map_fn(c[1])?, map_fn(c[2])?, map_fn(c[3])?];
    if straight_src {
        let mut line_err = 0.0f32;
        for t in [0.25f32, 0.5, 0.75] {
            let exact = map_fn(cubic_at(c, t))?;
            line_err = line_err.max(exact.distance(m[0].lerp(m[3], t)));
        }
        if line_err <= tol {
            out.line_to(m[3]);
            return (out.cmds.len() <= MAX_WARPED_COMMANDS).then_some(());
        }
    }
    let mut max_err = 0.0f32;
    for t in [0.25f32, 0.5, 0.75] {
        let exact = map_fn(cubic_at(c, t))?;
        max_err = max_err.max(exact.distance(cubic_at(&m, t)));
    }
    if max_err <= tol || depth >= MAX_SUBDIV_DEPTH {
        out.cubic_to(m[1], m[2], m[3]);
        return (out.cmds.len() <= MAX_WARPED_COMMANDS).then_some(());
    }
    let (left, right) = split_cubic(c);
    subdiv_spine_line_or_curve(out, &left, false, tol, depth + 1, map_fn)?;
    subdiv_spine_line_or_curve(out, &right, false, tol, depth + 1, map_fn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optimize_reduces_redundant_anchors_on_noisy_polyline() {
        let mut p = Path::new();
        p.move_to(Vec2::new(0.0, 0.0));
        for i in 1..=80 {
            let x = i as f32 * 2.5;
            let y = (x * 0.03).sin() * 30.0;
            p.line_to(Vec2::new(x, y));
        }
        let before = count_anchors(&p);
        let opt = optimize_path(&p, 2.5);
        let after = count_anchors(&opt);
        assert!(before > 60);
        assert!(after < before / 4, "expected strong anchor reduction: {before} -> {after}");
    }

    #[test]
    fn straighten_collapses_shallow_curves_and_snaps_axes() {
        let mut p = Path::new();
        p.move_to(Vec2::new(10.0, 20.0))
            .cubic_to(Vec2::new(40.0, 21.0), Vec2::new(70.0, 19.5), Vec2::new(100.0, 21.5));
        let st = straighten_path(&p, 4.0);
        assert!(st.cmds.iter().all(|c| !matches!(c, PathCmd::CubicTo(..))), "{st:?}");
        let anchors = st.anchors();
        assert_eq!(anchors.len(), 2);
        assert!((anchors[0].1.y - anchors[1].1.y).abs() < 1e-4, "snapped horizontal: {anchors:?}");
    }

    #[test]
    fn homography_maps_corners_exact_and_foreshortens_interior() {
        let rest = Rect::new(0.0, 0.0, 100.0, 100.0);
        let corners = [
            Vec2::new(20.0, 10.0),
            Vec2::new(80.0, 10.0),
            Vec2::new(100.0, 110.0),
            Vec2::new(0.0, 110.0),
        ];
        let h = Homography::from_rect_to_quad(&rest, &corners).expect("valid trapezoid");
        assert!(h.apply(Vec2::new(0.0, 0.0)).expect("tl").distance(corners[0]) < 1e-3);
        assert!(h.apply(Vec2::new(100.0, 0.0)).expect("tr").distance(corners[1]) < 1e-3);
        assert!(h.apply(Vec2::new(100.0, 100.0)).expect("br").distance(corners[2]) < 1e-3);
        assert!(h.apply(Vec2::new(0.0, 100.0)).expect("bl").distance(corners[3]) < 1e-3);

        // Projective intersection of diagonals of this symmetric trapezoid lies closer to the narrower top edge (y < 60).
        let center = h.apply(Vec2::new(50.0, 50.0)).expect("center");
        assert!((center.x - 50.0).abs() < 1e-3);
        assert!(center.y < 55.0, "perspective foreshortening pulls midpoint toward narrow top: {center:?}");

        let circle = Path::ellipse(50.0, 50.0, 40.0, 40.0);
        let warped = distort_path(&circle, &rest, &corners, 0.05).expect("distorted circle");
        assert!(!warped.is_empty());

        // Self-intersecting bowtie quad is rejected cleanly.
        let bowtie = [Vec2::new(0.0, 0.0), Vec2::new(100.0, 100.0), Vec2::new(100.0, 0.0), Vec2::new(0.0, 100.0)];
        assert!(Homography::from_rect_to_quad(&rest, &bowtie).is_none());
    }

    #[test]
    fn warp_along_spine_bends_rect_along_quarter_circle() {
        let src = Path::rect(0.0, -5.0, 100.0, 10.0);
        let bounds = src.bounds();
        let mut spine_path = Path::new();
        let r = 100.0f32;
        let k = 0.552_284_8 * r;
        spine_path.move_to(Vec2::new(0.0, 0.0)).cubic_to(Vec2::new(k, 0.0), Vec2::new(r, r - k), Vec2::new(r, r));
        let walk = CurveWalk::new(&spine_path).expect("walk");
        let warped = warp_along_spine(&src, &bounds, &walk, 1.0, 0.2).expect("warped");
        assert!(warped.cmds.iter().any(|c| matches!(c, PathCmd::CubicTo(..))), "straight edge bent into curves");
        let wb = warped.bounds();
        assert!(wb.width() > 80.0 && wb.height() > 80.0, "bent over the 90° arc: {wb:?}");
    }
}
