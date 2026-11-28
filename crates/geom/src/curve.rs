//! Curve clean-up on Bézier paths: `optimize`, `smooth` and `straighten`.
//!
//! Pure functions on [`Path`] in the path's own units. Every function keeps open endpoints fixed,
//! keeps closed subpaths closed, rejects non-finite coordinates, caps work on hostile paths and
//! never panics.

use crate::edit::{merge_cubics, simplify};
use crate::path::segment_distance;
use crate::polygons;
use crate::walk::cubic_point;
use crate::{Affine, Path, PathCmd, Rect, Vec2};

/// Paths with more commands than this are left as they are so hostile input stays bounded.
const MAX_INPUT_CMDS: usize = 100_000;
/// Most flattened samples fed into the curve fitter per subpath.
const MAX_SUBPATH_POINTS: usize = 16_384;
/// Most segments smoothed or straightened in one subpath.
const MAX_SUBPATH_SEGS: usize = 16_384;
/// Points closer than this are treated as the same anchor when closing a subpath.
const LINK_EPS: f32 = 1e-4;
/// Distance below which a cubic's handles count as lying on its chord.
const FLAT_EPS: f32 = 1e-3;
/// Minimum enclosed area for a closed subpath to count as enclosing visible fill (smaller
/// originals are already degenerate, so area-collapse checks skip them).
const AREA_EPS: f64 = 1e-6;
/// Minimum fraction of a closed subpath's enclosed area that a smoothed candidate must keep;
/// candidates below this have collapsed to near-collinear curves (even a triangle smoothed at
/// strength 1.0 keeps more than 1/16 of its area).
const MIN_SMOOTH_AREA_RATIO: f64 = 0.01;

#[derive(Clone, Copy)]
enum Seg {
    Line(Vec2, Vec2),
    Cubic([Vec2; 4]),
}

impl Seg {
    fn to_cubic(self) -> [Vec2; 4] {
        match self {
            Seg::Line(a, b) => [a, a.lerp(b, 1.0 / 3.0), a.lerp(b, 2.0 / 3.0), b],
            Seg::Cubic(c) => c,
        }
    }

    fn from_cubic(c: [Vec2; 4]) -> Seg {
        if cubic_is_flat(&c) { Seg::Line(c[0], c[3]) } else { Seg::Cubic(c) }
    }
}

struct Subpath {
    start: Vec2,
    segs: Vec<Seg>,
    closed: bool,
}

impl Subpath {
    fn to_path(&self) -> Path {
        let mut p = Path::new();
        self.emit(&mut p);
        p
    }

    fn emit(&self, out: &mut Path) {
        if !self.start.is_finite() {
            return;
        }
        out.move_to(self.start);
        let n = self.segs.len();
        for (i, &seg) in self.segs.iter().enumerate() {
            // Omit a redundant closing line when `Close` draws it.
            if self.closed && n > 1 && i + 1 == n && matches!(seg, Seg::Line(_, end) if end.distance(self.start) <= LINK_EPS) {
                continue;
            }
            match seg {
                Seg::Line(_, p) if p.is_finite() => {
                    out.line_to(p);
                }
                Seg::Cubic([_, c1, c2, p]) if c1.is_finite() && c2.is_finite() && p.is_finite() => {
                    out.cubic_to(c1, c2, p);
                }
                _ => {}
            }
        }
        if self.closed {
            out.close();
        }
    }

    fn bounds_diag(&self) -> f32 {
        let mut r = Rect::EMPTY;
        r.include(self.start);
        for &seg in &self.segs {
            match seg {
                Seg::Line(_, p) => r.include(p),
                Seg::Cubic([_, c1, c2, p]) => {
                    r.include(c1);
                    r.include(c2);
                    r.include(p);
                }
            }
        }
        if r.min.is_finite() && r.max.is_finite() && r.min.x <= r.max.x && r.min.y <= r.max.y { r.width().hypot(r.height()) } else { 0.0 }
    }

    fn cubic_count(&self) -> usize {
        self.segs.iter().filter(|s| matches!(s, Seg::Cubic(_))).count()
    }

    /// Covered fill area of this closed subpath (untangled with [`polygons::union`] so
    /// opposite-winding lobes add rather than cancel).
    fn enclosed_area(&self) -> f64 {
        let diag = self.bounds_diag();
        if !diag.is_finite() || diag <= LINK_EPS {
            return 0.0;
        }
        // Scale flattening tolerance with subpath size so huge coordinates stay bounded and small
        // shapes keep accuracy (`Path::flatten` falls back to 0.1 for tolerances <= 1e-4).
        let flat_tol = (diag * 0.002).max(1e-3);
        let loops: Vec<Vec<Vec2>> = self.to_path().flatten(&Affine::IDENTITY, flat_tol).into_iter().map(|pl| pl.points).collect();
        if loops.is_empty() {
            return 0.0;
        }
        let signed = polygons::area(&loops);
        let united = polygons::union(loops);
        // Fall back to signed area if `union` yields no loops, so truly degenerate/collinear paths
        // still measure zero while a non-degenerate shape is not falsely rejected.
        let area = if united.is_empty() { signed } else { polygons::area(&united) };
        if area.is_finite() { area } else { 0.0 }
    }
}

/// Splits `path` into finite subpaths; non-finite commands are dropped so the result never holds
/// NaN or infinity.
fn extract_subpaths(path: &Path) -> Vec<Subpath> {
    let mut out = Vec::new();
    let mut cur_sub: Option<Subpath> = None;
    let mut cur = Vec2::ZERO;
    let mut start = Vec2::ZERO;
    let flush = |cur_sub: &mut Option<Subpath>, out: &mut Vec<Subpath>| {
        if let Some(sub) = cur_sub.take() {
            out.push(sub);
        }
    };
    for &c in &path.cmds {
        match c {
            PathCmd::MoveTo(p) => {
                flush(&mut cur_sub, &mut out);
                if p.is_finite() {
                    start = p;
                    cur = p;
                    cur_sub = Some(Subpath { start: p, segs: Vec::new(), closed: false });
                }
            }
            PathCmd::LineTo(p) => {
                if !p.is_finite() {
                    continue;
                }
                let sub = cur_sub.get_or_insert_with(|| {
                    start = cur;
                    Subpath { start: cur, segs: Vec::new(), closed: false }
                });
                sub.segs.push(Seg::Line(cur, p));
                cur = p;
            }
            PathCmd::QuadTo(q, p) => {
                if !(q.is_finite() && p.is_finite()) {
                    continue;
                }
                let sub = cur_sub.get_or_insert_with(|| {
                    start = cur;
                    Subpath { start: cur, segs: Vec::new(), closed: false }
                });
                let c1 = cur + (q - cur) * (2.0 / 3.0);
                let c2 = p + (q - p) * (2.0 / 3.0);
                if c1.is_finite() && c2.is_finite() {
                    sub.segs.push(Seg::Cubic([cur, c1, c2, p]));
                    cur = p;
                }
            }
            PathCmd::CubicTo(c1, c2, p) => {
                if !(c1.is_finite() && c2.is_finite() && p.is_finite()) {
                    continue;
                }
                let sub = cur_sub.get_or_insert_with(|| {
                    start = cur;
                    Subpath { start: cur, segs: Vec::new(), closed: false }
                });
                sub.segs.push(Seg::Cubic([cur, c1, c2, p]));
                cur = p;
            }
            PathCmd::Close => {
                if let Some(sub) = cur_sub.as_mut() {
                    if cur.distance(start) > LINK_EPS {
                        sub.segs.push(Seg::Line(cur, start));
                    }
                    sub.closed = true;
                    cur = start;
                }
                flush(&mut cur_sub, &mut out);
            }
        }
    }
    flush(&mut cur_sub, &mut out);
    out
}

fn sanitize(path: &Path) -> Path {
    let mut out = Path::new();
    for sub in extract_subpaths(path) {
        sub.emit(&mut out);
    }
    out
}

fn cubic_is_flat(c: &[Vec2; 4]) -> bool {
    let (p0, c1, c2, p3) = (c[0], c[1], c[2], c[3]);
    let chord = p3 - p0;
    let len = chord.length();
    if len <= LINK_EPS {
        return p0.distance(c1) <= FLAT_EPS && p0.distance(c2) <= FLAT_EPS;
    }
    segment_distance(c1, p0, p3) <= FLAT_EPS
        && segment_distance(c2, p0, p3) <= FLAT_EPS
        && (c1 - p0).dot(chord) >= -FLAT_EPS * len
        && (p3 - c2).dot(chord) >= -FLAT_EPS * len
}

/// Largest distance from sampled interior points of `c` to its chord segment `c[0]..c[3]`.
fn cubic_chord_deviation(c: &[Vec2; 4]) -> f32 {
    let mut best = 0.0f32;
    for k in 1..=7 {
        let q = cubic_point(c, k as f32 / 8.0);
        best = best.max(segment_distance(q, c[0], c[3]));
    }
    best
}

/// Largest distance between sampled points of `a` then `b` and the polyline of `merged`.
fn cubic_pair_error(a: &[Vec2; 4], b: &[Vec2; 4], merged: &[Vec2; 4]) -> f32 {
    const M: usize = 16;
    let mut poly = [Vec2::ZERO; M + 1];
    for (i, slot) in poly.iter_mut().enumerate() {
        *slot = cubic_point(merged, i as f32 / M as f32);
    }
    let dist = |pt: Vec2| {
        let mut best = f32::INFINITY;
        for w in poly.windows(2) {
            best = best.min(segment_distance(pt, w[0], w[1]));
        }
        best
    };
    let mut worst = 0.0f32;
    for k in 1..=7 {
        let t = k as f32 / 8.0;
        worst = worst.max(dist(cubic_point(a, t))).max(dist(cubic_point(b, t)));
    }
    worst.max(dist(a[3]))
}

/// Simplifies `path` by refitting each subpath with cubic Béziers within `tolerance` (in the
/// path's units). Sharp corners stay corners, closed subpaths stay closed and open endpoints stay
/// put. A subpath that cannot be represented with fewer segments is left as it is.
pub fn optimize(path: &Path, tolerance: f32) -> Path {
    if path.is_empty() {
        return Path::new();
    }
    if !tolerance.is_finite() || tolerance <= 0.0 || path.cmds.len() > MAX_INPUT_CMDS {
        return sanitize(path);
    }
    let mut out = Path::new();
    for sub in extract_subpaths(path) {
        let fitted = optimize_subpath(&sub, tolerance);
        fitted.emit(&mut out);
    }
    out
}

fn optimize_subpath(sub: &Subpath, tolerance: f32) -> Subpath {
    if sub.segs.is_empty() || (!sub.closed && sub.segs.len() == 1 && matches!(sub.segs[0], Seg::Line(..))) {
        return Subpath { start: sub.start, segs: sub.segs.clone(), closed: sub.closed };
    }
    let has_curves = sub.cubic_count() > 0;
    let flat_tol = (tolerance * 0.1).clamp(0.01, 0.2);
    let temp = sub.to_path();
    let pls = temp.flatten(&Affine::IDENTITY, flat_tol);
    let Some(pl) = pls.into_iter().next() else {
        return Subpath { start: sub.start, segs: sub.segs.clone(), closed: sub.closed };
    };
    let mut pts = pl.points;
    if pts.len() < 2 {
        return Subpath { start: sub.start, segs: sub.segs.clone(), closed: sub.closed };
    }
    // Far past what the fitter takes: thinning that many points first is itself slow (the
    // simplifier is quadratic on a zigzag), so such an outline stays as it is.
    if pts.len() > MAX_SUBPATH_POINTS.saturating_mul(4) {
        return Subpath { start: sub.start, segs: sub.segs.clone(), closed: sub.closed };
    }
    // Cap points fed to the least-squares fitter so hostile paths stay bounded.
    if pts.len() > MAX_SUBPATH_POINTS {
        pts = simplify(&pts, (tolerance * 0.05).max(1e-3));
        if pts.len() > MAX_SUBPATH_POINTS {
            return Subpath { start: sub.start, segs: sub.segs.clone(), closed: sub.closed };
        }
    }
    // Perimeter determines a corner window that won't walk past short polygon edges.
    let mut perim: f64 = pts.windows(2).map(|w| f64::from(w[0].distance(w[1]))).sum();
    if sub.closed
        && let (Some(&a), Some(&b)) = (pts.first(), pts.last())
    {
        perim += f64::from(a.distance(b));
    }
    if !(perim.is_finite() && perim > 1e-4) {
        return Subpath { start: sub.start, segs: Vec::new(), closed: sub.closed };
    }
    let window = (tolerance * 1.5).clamp(0.5, 6.0).min((perim as f32 / 16.0).max(1e-3));
    let fit_tol = if has_curves { (tolerance - flat_tol).max(tolerance * 0.5) } else { tolerance };
    let mut fitted_path = crate::fit_curve(&pts, fit_tol, window, sub.closed);
    if sub.closed && !matches!(fitted_path.cmds.last(), Some(PathCmd::Close)) {
        fitted_path.close();
    }
    let mut fitted_subs = extract_subpaths(&fitted_path);
    let Some(mut cand) = fitted_subs.pop() else {
        return Subpath { start: sub.start, segs: sub.segs.clone(), closed: sub.closed };
    };
    // Turn cubics that lie on their chord back into plain lines.
    for seg in &mut cand.segs {
        if let Seg::Cubic(c) = *seg {
            *seg = Seg::from_cubic(c);
        }
    }
    cand.closed = sub.closed;
    let orig_end = sub.segs.last().map(|seg| match *seg {
        Seg::Line(_, p) | Seg::Cubic([_, _, _, p]) => p,
    });
    let cand_end = cand.segs.last().map(|seg| match *seg {
        Seg::Line(_, p) | Seg::Cubic([_, _, _, p]) => p,
    });
    let ends_kept = sub.closed || (cand.start == sub.start && cand_end == orig_end);
    let orig_segs = sub.segs.len();
    let new_segs = cand.segs.len();
    let simpler = ends_kept && new_segs > 0 && (new_segs < orig_segs || (new_segs == orig_segs && cand.cubic_count() < sub.cubic_count()));
    if simpler { cand } else { Subpath { start: sub.start, segs: sub.segs.clone(), closed: sub.closed } }
}

/// Softens the curves of `path` and evens out small bumps (`strength` in `0.0..=1.0`).
///
/// Moves interior anchors toward their neighbours' midpoint, blends handles toward smooth
/// tangents and merges adjacent segments whose combined curve stays within a small tolerance.
/// Open endpoints stay put and closed subpaths stay closed.
pub fn smooth(path: &Path, strength: f32) -> Path {
    if path.is_empty() {
        return Path::new();
    }
    if !strength.is_finite() || strength <= 0.0 || path.cmds.len() > MAX_INPUT_CMDS {
        return sanitize(path);
    }
    let s = strength.clamp(0.0, 1.0);
    let mut out = Path::new();
    for sub in extract_subpaths(path) {
        let smoothed = smooth_subpath(&sub, s);
        smoothed.emit(&mut out);
    }
    out
}

fn smooth_subpath(sub: &Subpath, s: f32) -> Subpath {
    if sub.segs.is_empty() || sub.segs.len() > MAX_SUBPATH_SEGS {
        return Subpath { start: sub.start, segs: sub.segs.clone(), closed: sub.closed };
    }
    let cubics: Vec<[Vec2; 4]> = sub.segs.iter().map(|seg| seg.to_cubic()).collect();
    if !sub.closed && cubics.len() == 1 {
        let [p0, c1, c2, p3] = cubics[0];
        let pull = 0.15 * s;
        let nc = [p0, c1.lerp(p0.lerp(p3, 1.0 / 3.0), pull), c2.lerp(p0.lerp(p3, 2.0 / 3.0), pull), p3];
        return Subpath { start: p0, segs: vec![Seg::from_cubic(nc)], closed: false };
    }
    if sub.closed && cubics.len() < 2 {
        return Subpath { start: sub.start, segs: sub.segs.clone(), closed: true };
    }
    let m = cubics.len();
    let n = if sub.closed { m } else { m + 1 };
    let mut anchors: Vec<Vec2> = Vec::with_capacity(n);
    for c in &cubics {
        anchors.push(c[0]);
    }
    if !sub.closed {
        anchors.push(cubics[m - 1][3]);
    }
    // 1. Move each interior (or closed, for 3+ anchors) anchor toward the average of its neighbours.
    let w = 0.5 * s;
    let mut moved = anchors.clone();
    if sub.closed {
        if n > 2 {
            for i in 0..n {
                let prev = anchors[(i + n - 1) % n];
                let next = anchors[(i + 1) % n];
                moved[i] = anchors[i].lerp((prev + next) * 0.5, w);
            }
        }
    } else {
        for i in 1..n - 1 {
            let prev = anchors[i - 1];
            let next = anchors[i + 1];
            moved[i] = anchors[i].lerp((prev + next) * 0.5, w);
        }
    }
    // 2. Carry existing handles with their anchors and blend toward smooth collinear tangents.
    let mut next_cubics: Vec<[Vec2; 4]> = Vec::with_capacity(m);
    for i in 0..m {
        let j = if sub.closed { (i + 1) % n } else { i + 1 };
        let (a0, a1) = (moved[i], moved[j]);
        let carried_c1 = cubics[i][1] + (a0 - anchors[i]);
        let carried_c2 = cubics[i][2] + (a1 - anchors[j]);
        let (target_c1, target_c2) = if sub.closed && n == 2 {
            let out0 = cubics[i][1] - anchors[i];
            let in0 = cubics[j][2] - anchors[i];
            let dir0 = (out0 - in0).normalized();
            let tc1 = if dir0 != Vec2::ZERO { a0 + dir0 * out0.length() } else { carried_c1 };
            let in1 = cubics[i][2] - anchors[j];
            let out1 = cubics[j][1] - anchors[j];
            let dir1 = (out1 - in1).normalized();
            let tc2 = if dir1 != Vec2::ZERO { a1 - dir1 * in1.length() } else { carried_c2 };
            (tc1, tc2)
        } else {
            let tc1 = if sub.closed || (i > 0 && i + 1 < n) {
                let prev = moved[(i + n - 1) % n];
                let dir = (a1 - prev).normalized();
                if dir != Vec2::ZERO { a0 + dir * (a0.distance(a1) / 3.0) } else { a0.lerp(a1, 1.0 / 3.0) }
            } else {
                a0.lerp(a1, 1.0 / 3.0)
            };
            let tc2 = if sub.closed || (j > 0 && j + 1 < n) {
                let next = moved[(j + 1) % n];
                let dir = (next - a0).normalized();
                if dir != Vec2::ZERO { a1 - dir * (a1.distance(a0) / 3.0) } else { a0.lerp(a1, 2.0 / 3.0) }
            } else {
                a0.lerp(a1, 2.0 / 3.0)
            };
            (tc1, tc2)
        };
        let c1 = carried_c1.lerp(target_c1, s);
        let c2 = carried_c2.lerp(target_c2, s);
        if a0.is_finite() && c1.is_finite() && c2.is_finite() && a1.is_finite() {
            next_cubics.push([a0, c1, c2, a1]);
        }
    }
    // 3. Merge adjacent smooth segments when a single cubic stays within a gentle tolerance.
    let diag = sub.bounds_diag();
    let merge_tol = (diag * 0.005 * s).clamp(0.25 * s, 3.0 * s);
    let min_segs = if sub.closed { 2 } else { 1 };
    let mut merged: Vec<[Vec2; 4]> = Vec::with_capacity(next_cubics.len());
    let mut remaining = next_cubics.len();
    for seg in next_cubics {
        if remaining > min_segs
            && let Some(prev) = merged.last_mut()
            && let Some(one) = merge_cubics(*prev, seg)
            && cubic_pair_error(prev, &seg, &one) <= merge_tol
        {
            *prev = one;
            remaining -= 1;
            continue;
        }
        merged.push(seg);
    }
    if sub.closed && merged.len() > min_segs {
        let last_idx = merged.len() - 1;
        if let Some(one) = merge_cubics(merged[last_idx], merged[0])
            && cubic_pair_error(&merged[last_idx], &merged[0], &one) <= merge_tol
        {
            merged[0] = one;
            merged.pop();
        }
    }
    let start = merged.first().map_or(sub.start, |c| c[0]);
    let segs = merged.into_iter().map(Seg::from_cubic).collect();
    let cand = Subpath { start, segs, closed: sub.closed };
    if sub.closed {
        let mut collapsed = (cand.bounds_diag() <= LINK_EPS && sub.bounds_diag() > LINK_EPS)
            || (cand.cubic_count() == 0 && cand.segs.len() < 3 && (sub.cubic_count() > 0 || sub.segs.len() >= 3));
        if !collapsed {
            let orig_area = sub.enclosed_area();
            if orig_area > AREA_EPS && cand.enclosed_area() < orig_area * MIN_SMOOTH_AREA_RATIO {
                collapsed = true;
            }
        }
        if collapsed {
            return Subpath { start: sub.start, segs: sub.segs.clone(), closed: true };
        }
    }
    cand
}

/// Straightens `path` (`strength` in `0.0..=1.0`): curved segments whose deviation from their
/// chord is below a threshold become lines; the rest are pulled toward their chord, and nearly
/// collinear line runs collapse.
pub fn straighten(path: &Path, strength: f32) -> Path {
    if path.is_empty() {
        return Path::new();
    }
    if !strength.is_finite() || strength <= 0.0 || path.cmds.len() > MAX_INPUT_CMDS {
        return sanitize(path);
    }
    let s = strength.clamp(0.0, 1.0);
    let mut out = Path::new();
    for sub in extract_subpaths(path) {
        let straight = straighten_subpath(&sub, s);
        straight.emit(&mut out);
    }
    out
}

fn straighten_subpath(sub: &Subpath, s: f32) -> Subpath {
    if sub.segs.is_empty() || sub.segs.len() > MAX_SUBPATH_SEGS {
        return Subpath { start: sub.start, segs: sub.segs.clone(), closed: sub.closed };
    }
    let pull = 0.6 * s;
    let mut segs: Vec<Seg> = Vec::with_capacity(sub.segs.len());
    for &seg in &sub.segs {
        match seg {
            Seg::Line(a, b) => segs.push(Seg::Line(a, b)),
            Seg::Cubic(c @ [p0, c1, c2, p3]) => {
                let chord = p0.distance(p3);
                if chord <= LINK_EPS {
                    segs.push(Seg::from_cubic(c));
                    continue;
                }
                let thresh = (chord * 0.12 * s).max(4.0 * s);
                if cubic_chord_deviation(&c) <= thresh {
                    segs.push(Seg::Line(p0, p3));
                } else {
                    let nc1 = c1.lerp(p0.lerp(p3, 1.0 / 3.0), pull);
                    let nc2 = c2.lerp(p0.lerp(p3, 2.0 / 3.0), pull);
                    segs.push(Seg::from_cubic([p0, nc1, nc2, p3]));
                }
            }
        }
    }
    // Collapse nearly collinear runs of consecutive lines.
    let diag = sub.bounds_diag();
    let line_tol = (diag * 0.04 * s).min(3.0 * s);
    if line_tol > 1e-4 && segs.len() >= 2 {
        let all_lines = segs.iter().all(|seg| matches!(seg, Seg::Line(..)));
        let mut collapsed: Vec<Seg> = Vec::with_capacity(segs.len());
        let mut i = 0;
        while i < segs.len() {
            if !matches!(segs[i], Seg::Line(..)) {
                collapsed.push(segs[i]);
                i += 1;
                continue;
            }
            let start_i = i;
            let mut pts = Vec::new();
            while i < segs.len() {
                let Seg::Line(a, b) = segs[i] else { break };
                if pts.is_empty() {
                    pts.push(a);
                }
                pts.push(b);
                i += 1;
            }
            // Don't collapse a closed polygon to fewer than three sides.
            let simplified = if sub.closed && all_lines && start_i == 0 && i == segs.len() {
                let mut open = pts.clone();
                if open.len() > 2 && open.first().zip(open.last()).is_some_and(|(a, b)| a.distance(*b) <= LINK_EPS) {
                    open.pop();
                }
                let simp = simplify(&open, line_tol);
                if simp.len() >= 3 {
                    let mut closed_pts = simp;
                    closed_pts.push(closed_pts[0]);
                    closed_pts
                } else {
                    pts
                }
            } else if pts.len() >= 3 && pts[0].distance(pts[pts.len() - 1]) > LINK_EPS {
                simplify(&pts, line_tol)
            } else {
                pts
            };
            for w in simplified.windows(2) {
                collapsed.push(Seg::Line(w[0], w[1]));
            }
        }
        segs = collapsed;
    }
    let start = segs.first().map_or(sub.start, |seg| match *seg {
        Seg::Line(a, _) | Seg::Cubic([a, ..]) => a,
    });
    let cand = Subpath { start, segs, closed: sub.closed };
    // A thin closed fill (a leaf of two shallow curves) would lose its area to its chords and
    // vanish from the stage: like Smooth, Straighten leaves such an outline as it is.
    if sub.closed {
        let orig_area = sub.enclosed_area();
        if orig_area > AREA_EPS && cand.enclosed_area() < orig_area * MIN_SMOOTH_AREA_RATIO {
            return Subpath { start: sub.start, segs: sub.segs.clone(), closed: true };
        }
    }
    cand
}

#[cfg(test)]
mod tests {
    use super::*;

    fn max_dist_to_path(pts: &[Vec2], path: &Path) -> f32 {
        let pls = path.flatten(&Affine::IDENTITY, 0.05);
        pts.iter()
            .map(|p| {
                pls.iter()
                    .flat_map(|l| {
                        let n = l.points.len();
                        let count = if l.closed { n } else { n.saturating_sub(1) };
                        (0..count).map(move |i| segment_distance(*p, l.points[i], l.points[(i + 1) % n]))
                    })
                    .fold(f32::INFINITY, f32::min)
            })
            .fold(0.0, f32::max)
    }

    fn all_finite(path: &Path) -> bool {
        path.cmds.iter().all(|c| match *c {
            PathCmd::MoveTo(p) | PathCmd::LineTo(p) => p.is_finite(),
            PathCmd::QuadTo(a, p) => a.is_finite() && p.is_finite(),
            PathCmd::CubicTo(a, b, p) => a.is_finite() && b.is_finite() && p.is_finite(),
            PathCmd::Close => true,
        })
    }

    fn seg_count(path: &Path) -> usize {
        path.cmds.iter().filter(|c| matches!(c, PathCmd::LineTo(_) | PathCmd::QuadTo(..) | PathCmd::CubicTo(..))).count()
    }

    #[test]
    fn optimize_fits_sampled_circle_within_tolerance_with_far_fewer_segments() {
        let r = 60.0f32;
        let samples: Vec<Vec2> = (0..120)
            .map(|i| {
                let a = i as f32 / 120.0 * std::f32::consts::TAU;
                Vec2::new(100.0 + r * a.cos(), 100.0 + r * a.sin())
            })
            .collect();
        let mut circle = Path::new();
        circle.move_to(samples[0]);
        for &p in &samples[1..] {
            circle.line_to(p);
        }
        circle.close();
        let tol = 1.0;
        let out = optimize(&circle, tol);
        assert!(all_finite(&out));
        assert!(matches!(out.cmds.last(), Some(PathCmd::Close)), "closed stays closed");
        assert!(seg_count(&out) < seg_count(&circle) / 10, "far fewer segments: {} vs {}", seg_count(&out), seg_count(&circle));
        let err = max_dist_to_path(&samples, &out);
        assert!(err <= tol + 0.1, "every sample within tolerance: {err}");
    }

    #[test]
    fn optimize_keeps_square_corners_and_closed_loop() {
        // A square sampled with many points along every side still collapses to its 4 corners.
        let mut dense = Path::new();
        dense.move_to(Vec2::new(0.0, 0.0));
        for i in 1..=20 {
            dense.line_to(Vec2::new(i as f32 * 5.0, 0.0));
        }
        for i in 1..=20 {
            dense.line_to(Vec2::new(100.0, i as f32 * 5.0));
        }
        for i in 1..=20 {
            dense.line_to(Vec2::new(100.0 - i as f32 * 5.0, 100.0));
        }
        for i in 1..20 {
            dense.line_to(Vec2::new(0.0, 100.0 - i as f32 * 5.0));
        }
        dense.close();
        let out = optimize(&dense, 1.0);
        assert!(matches!(out.cmds.last(), Some(PathCmd::Close)));
        let anchors: Vec<Vec2> = out.editable_anchors().into_iter().map(|(_, p)| p).collect();
        assert_eq!(anchors.len(), 4, "4 corners kept: {anchors:?}");
        for corner in [Vec2::new(0.0, 0.0), Vec2::new(100.0, 0.0), Vec2::new(100.0, 100.0), Vec2::new(0.0, 100.0)] {
            assert!(anchors.iter().any(|a| a.distance(corner) < 1e-2), "missing corner {corner:?} in {anchors:?}");
        }
        // A plain 4-corner square stays unchanged.
        let plain = Path::rect(10.0, 20.0, 80.0, 60.0);
        let opt_plain = optimize(&plain, 1.0);
        assert_eq!(opt_plain.editable_anchors().len(), 4);
        assert!(matches!(opt_plain.cmds.last(), Some(PathCmd::Close)));
    }

    #[test]
    fn smooth_reduces_zigzag_deviation_and_keeps_endpoints() {
        let mut zig = Path::new();
        zig.move_to(Vec2::new(0.0, 0.0))
            .line_to(Vec2::new(20.0, 20.0))
            .line_to(Vec2::new(40.0, -20.0))
            .line_to(Vec2::new(60.0, 20.0))
            .line_to(Vec2::new(80.0, -20.0))
            .line_to(Vec2::new(100.0, 0.0));
        let dev = |p: &Path| {
            p.flatten(&Affine::IDENTITY, 0.1)
                .into_iter()
                .flat_map(|l| l.points)
                .map(|pt| segment_distance(pt, Vec2::new(0.0, 0.0), Vec2::new(100.0, 0.0)))
                .fold(0.0f32, f32::max)
        };
        let d0 = dev(&zig);
        let once = smooth(&zig, 0.5);
        let twice = smooth(&once, 0.5);
        let (d1, d2) = (dev(&once), dev(&twice));
        assert!(d1 < d0, "first pass reduces deviation: {d1} vs {d0}");
        assert!(d2 < d1, "repeating smooths further: {d2} vs {d1}");
        // Endpoints stay put.
        for p in [&once, &twice] {
            let anchors = p.anchors();
            assert_eq!(anchors.first().map(|a| a.1), Some(Vec2::new(0.0, 0.0)));
            assert_eq!(anchors.last().map(|a| a.1), Some(Vec2::new(100.0, 0.0)));
        }
    }

    #[test]
    fn straighten_turns_a_slightly_bent_segment_into_a_line_and_flattens_arcs() {
        let mut slight = Path::new();
        slight.move_to(Vec2::new(0.0, 0.0)).cubic_to(Vec2::new(30.0, 3.0), Vec2::new(70.0, 3.0), Vec2::new(100.0, 0.0));
        let straight = straighten(&slight, 0.5);
        assert_eq!(straight.cmds, vec![PathCmd::MoveTo(Vec2::new(0.0, 0.0)), PathCmd::LineTo(Vec2::new(100.0, 0.0))]);
        // A deeper arc flattens partway on the first pass and eventually becomes a line.
        let mut arc = Path::new();
        arc.move_to(Vec2::new(0.0, 0.0)).cubic_to(Vec2::new(0.0, 60.0), Vec2::new(100.0, 60.0), Vec2::new(100.0, 0.0));
        let max_y = |p: &Path| p.flatten(&Affine::IDENTITY, 0.1).into_iter().flat_map(|l| l.points).map(|q| q.y).fold(0.0f32, f32::max);
        let p1 = straighten(&arc, 0.5);
        assert!(max_y(&p1) < max_y(&arc), "pulls toward chord: {} vs {}", max_y(&p1), max_y(&arc));
        let mut cur = p1;
        for _ in 0..8 {
            cur = straighten(&cur, 0.5);
        }
        assert!(matches!(cur.cmds.get(1), Some(PathCmd::LineTo(_))), "repeatable until straight: {:?}", cur.cmds);
    }

    #[test]
    fn degenerate_and_hostile_paths_never_panic_or_emit_nan() {
        let empty = Path::new();
        let one = {
            let mut p = Path::new();
            p.move_to(Vec2::new(5.0, 5.0));
            p
        };
        let zero = {
            let mut p = Path::new();
            p.move_to(Vec2::new(5.0, 5.0)).line_to(Vec2::new(5.0, 5.0));
            p
        };
        let nan = Path {
            cmds: vec![
                PathCmd::MoveTo(Vec2::ZERO),
                PathCmd::CubicTo(Vec2::new(f32::NAN, 1.0), Vec2::new(f32::INFINITY, 0.0), Vec2::new(10.0, 0.0)),
                PathCmd::LineTo(Vec2::new(20.0, f32::NEG_INFINITY)),
                PathCmd::Close,
            ],
        };
        let huge = Path::line(Vec2::new(-1e30, 0.0), Vec2::new(1e30, 1e30));
        for p in [&empty, &one, &zero, &nan, &huge] {
            for param in [-1.0, 0.0, 0.5, 100.0, f32::NAN, f32::INFINITY] {
                assert!(all_finite(&optimize(p, param)));
                assert!(all_finite(&smooth(p, param)));
                assert!(all_finite(&straighten(p, param)));
            }
        }
        assert!(optimize(&empty, 1.0).is_empty());
        assert!(smooth(&empty, 0.5).is_empty());
        assert!(straighten(&empty, 0.5).is_empty());
    }

    #[test]
    fn straighten_simplifies_flat_collinear_line_runs() {
        let mut horiz = Path::new();
        horiz.move_to(Vec2::new(0.0, 0.0)).line_to(Vec2::new(50.0, 0.0)).line_to(Vec2::new(100.0, 0.0));
        let out_h = straighten(&horiz, 0.5);
        assert_eq!(seg_count(&out_h), 1, "redundant midpoint removed on horizontal run: {:?}", out_h.cmds);
        assert_eq!(out_h.anchors().first().map(|a| a.1), Some(Vec2::new(0.0, 0.0)));
        assert_eq!(out_h.anchors().last().map(|a| a.1), Some(Vec2::new(100.0, 0.0)));

        let mut vert = Path::new();
        vert.move_to(Vec2::new(10.0, 0.0)).line_to(Vec2::new(10.0, 50.0)).line_to(Vec2::new(10.0, 100.0));
        let out_v = straighten(&vert, 0.5);
        assert_eq!(seg_count(&out_v), 1, "redundant midpoint removed on vertical run: {:?}", out_v.cmds);
    }

    #[test]
    fn optimize_does_not_discard_peaks_above_tolerance_on_long_subpath() {
        let mut zig = Path::new();
        let n = 32_767usize;
        let mut peaks = Vec::new();
        for i in 0..n {
            let y = if i % 2 == 1 { 20.0 } else { 0.0 };
            let pt = Vec2::new(i as f32, y);
            if i == 0 {
                zig.move_to(pt);
            } else {
                zig.line_to(pt);
            }
            if i % 2 == 1 && peaks.len() < 64 {
                peaks.push(pt);
            }
        }
        let tol = 2.05;
        let out = optimize(&zig, tol);
        let err = max_dist_to_path(&peaks, &out);
        assert!(err <= tol + 0.1, "peaks stay within tolerance: {err}");
        assert!(seg_count(&out) > 1, "zigzag did not collapse to a baseline");
    }

    #[test]
    fn optimize_keeps_open_endpoints_fixed_with_tiny_end_segment() {
        let mut p = Path::new();
        let end = Vec2::new(10.00005, 0.0);
        p.move_to(Vec2::new(0.0, 0.0)).line_to(Vec2::new(10.0, 0.0)).line_to(end);
        let out = optimize(&p, 2.05);
        let anchors = out.anchors();
        assert_eq!(anchors.first().map(|a| a.1), Some(Vec2::new(0.0, 0.0)));
        assert_eq!(anchors.last().map(|a| a.1), Some(end), "open endpoint must not move: {:?}", out.cmds);
    }

    #[test]
    fn smooth_does_not_collapse_closed_two_anchor_loop() {
        let mut loop2 = Path::new();
        loop2
            .move_to(Vec2::new(0.0, 0.0))
            .cubic_to(Vec2::new(0.0, 80.0), Vec2::new(80.0, 80.0), Vec2::new(80.0, 0.0))
            .cubic_to(Vec2::new(80.0, -80.0), Vec2::new(0.0, -80.0), Vec2::new(0.0, 0.0))
            .close();
        let out = smooth(&loop2, 1.0);
        assert!(matches!(out.cmds.last(), Some(PathCmd::Close)), "stays closed: {:?}", out.cmds);
        let pls = out.flatten(&Affine::IDENTITY, 0.25);
        let mut b = Rect::EMPTY;
        for pl in &pls {
            for &pt in &pl.points {
                b.include(pt);
            }
        }
        assert!(b.width() > 40.0 && b.height() > 40.0, "two-anchor loop kept its fill area: {b:?}, cmds={:?}", out.cmds);
    }

    #[test]
    fn smooth_does_not_collapse_collinear_anchor_closed_loop() {
        let path_area = |p: &Path| {
            let loops: Vec<Vec<Vec2>> = p.flatten(&Affine::IDENTITY, 0.25).into_iter().map(|pl| pl.points).collect();
            polygons::area(&loops)
        };
        let mut bumps = Path::new();
        bumps
            .move_to(Vec2::new(0.0, 0.0))
            .cubic_to(Vec2::new(0.0, 80.0), Vec2::new(100.0, 80.0), Vec2::new(100.0, 0.0))
            .cubic_to(Vec2::new(100.0, 80.0), Vec2::new(200.0, 80.0), Vec2::new(200.0, 0.0))
            .line_to(Vec2::new(0.0, 0.0))
            .close();
        let orig_area = path_area(&bumps);
        let out = smooth(&bumps, 1.0);
        assert!(matches!(out.cmds.last(), Some(PathCmd::Close)), "stays closed: {:?}", out.cmds);
        let out_area = path_area(&out);
        assert!(
            out_area >= orig_area * 0.5,
            "collinear-anchor closed loop kept its fill area: {out_area} vs {orig_area}, cmds={:?}",
            out.cmds
        );

        let square = Path::rect(0.0, 0.0, 100.0, 100.0);
        let smoothed = smooth(&square, 1.0);
        assert!(matches!(smoothed.cmds.last(), Some(PathCmd::Close)), "closed loop stays closed: {:?}", smoothed.cmds);
        assert_ne!(smoothed, square, "normal closed loop still changes when smoothed");
        assert!(path_area(&smoothed) > 0.0, "smoothed loop still encloses area");
    }

    #[test]
    fn smooth_accepts_closed_path_with_opposite_winding_lobes() {
        let mut path = Path::new();
        path.move_to(Vec2::new(0.0, 0.0))
            .cubic_to(Vec2::new(0.0, 1000.0), Vec2::new(1000.0, 1000.0), Vec2::new(1000.0, 1000.0))
            .line_to(Vec2::new(1000.0, -1000.0))
            .line_to(Vec2::new(0.0, 0.0))
            .line_to(Vec2::new(-1000.0, 1000.0))
            .line_to(Vec2::new(-1000.0, -1000.0))
            .close();
        let covered_area = |p: &Path| {
            let loops: Vec<Vec<Vec2>> = p.flatten(&Affine::IDENTITY, 2.0).into_iter().map(|pl| pl.points).collect();
            polygons::area(&polygons::union(loops))
        };
        let out = smooth(&path, 1.0);
        assert!(all_finite(&out));
        assert!(matches!(out.cmds.last(), Some(PathCmd::Close)), "closed stays closed: {:?}", out.cmds);
        assert_ne!(out, path, "opposite-winding closed path should be smoothed rather than rejected");
        assert!(
            covered_area(&out) > covered_area(&path) * 0.01,
            "smoothed lobes still cover substantial area: {} vs {}",
            covered_area(&out),
            covered_area(&path)
        );
    }

    #[test]
    fn straighten_keeps_visible_zero_chord_cubic_loop() {
        let mut teardrop = Path::new();
        teardrop.move_to(Vec2::new(0.0, 0.0)).cubic_to(Vec2::new(3.0, 0.0), Vec2::new(0.0, 3.0), Vec2::new(0.0, 0.0)).close();
        let out = straighten(&teardrop, 0.5);
        assert!(out.cmds.iter().any(|c| matches!(c, PathCmd::CubicTo(..))), "nondegenerate zero-chord loop kept as cubic: {:?}", out.cmds);
        let pls = out.flatten(&Affine::IDENTITY, 0.05);
        let mut b = Rect::EMPTY;
        for pl in &pls {
            for &pt in &pl.points {
                b.include(pt);
            }
        }
        assert!(b.width() > 0.5 && b.height() > 0.5, "loop stays visible: {b:?}");
    }

    /// A thin leaf, two shallow curves between the same two points, is within Straighten's
    /// reach of its chords on both sides: straightened it would be a line there and back, a
    /// fill with no area, gone from the stage.
    #[test]
    fn straighten_keeps_a_thin_closed_fill() {
        let mut leaf = Path::new();
        leaf.move_to(Vec2::new(0.0, 0.0));
        leaf.cubic_to(Vec2::new(30.0, -8.0), Vec2::new(70.0, -8.0), Vec2::new(100.0, 0.0));
        leaf.cubic_to(Vec2::new(70.0, 8.0), Vec2::new(30.0, 8.0), Vec2::new(0.0, 0.0));
        leaf.close();
        let area = |p: &Path| extract_subpaths(p).iter().map(Subpath::enclosed_area).sum::<f64>();
        let before = area(&leaf);
        assert!(before > 100.0, "the leaf has an area: {before}");
        for strength in [0.25, 0.5, 1.0] {
            let after = area(&straighten(&leaf, strength));
            assert!(after > before * 0.25, "strength {strength}: {before} became {after}");
        }
    }
}
