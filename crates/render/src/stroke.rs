//! Stroker: polylines → the outline of the stroke as loops that don't overlap, of one width or
//! following a width profile (ADR 0110).
//!
//! A stroke is the union of a trapezoid per segment, a join on the outside of every bend and a
//! cap at every open end. Each run of segments (a subpath, or one dash) is built as its own
//! outline: the left edges forwards, the end's cap, the right edges backwards, the start's cap
//! (a closed run: the left edges as one loop, the right edges as another, the other way round).
//! On the inside of a bend the two edges are cut where they cross; where they are too short to
//! cross (a line wider than its curve is tight) the outline goes through the bend's own point
//! instead, which keeps every part of it wound the same way, so parts that overlap add up and
//! never cancel. The loops of all runs are then united ([`anicraft_geom::polygons::union`]):
//! where a stroke folds over itself, crosses itself, or its dashes' caps overlap, each pixel
//! is still covered once, and the renderers' sum of signed areas is the stroke's true coverage.

use anicraft_doc::width::left_normal;
use anicraft_doc::{Cap, DisplayItem, DisplayList, Join, Stroke, WidthProfile};
use anicraft_geom::{Affine, Path, PathCmd, Polyline, Vec2, polygons};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

#[cfg(test)]
fn signed_area(p: &[Vec2]) -> f32 {
    let n = p.len();
    let mut a = 0.0;
    for i in 0..n {
        let (Some(u), Some(v)) = (p.get(i), p.get((i + 1) % n)) else { continue };
        a += u.cross(*v);
    }
    a * 0.5
}

/// The outline of the stroke of `lines` with half-width `hw` (pixels), round caps and joins:
/// loops that don't overlap (fill them non-zero).
pub fn stroke_polygons(lines: &[Polyline], hw: f32) -> Vec<Vec<Vec2>> {
    styled_polygons(lines, hw, &Outline::ROUND, None, None, false)
}

/// Arc tolerance for joins and caps, in output units.
const ARC_TOL: f32 = 0.1;
/// Most points a variable-width polyline is resampled to.
const MAX_SAMPLES: usize = 8192;
/// Widths below this (output units) draw nothing.
const NO_WIDTH: f32 = 1e-4;
/// Minimum stroke width (output pixels) rasterised at full opacity; thinner strokes are widened
/// to this width and faded proportionally in [`stroke_geometry`].
const MIN_RASTER_WIDTH: f32 = 1.0;

/// The points of an arc round `c` of radius `r`, from angle `a0` through `sweep` radians, both
/// ends included; empty when there is nothing to draw.
fn arc(c: Vec2, a0: f32, sweep: f32, r: f32) -> Vec<Vec2> {
    if !(r > NO_WIDTH && sweep.is_finite() && a0.is_finite()) {
        return Vec::new();
    }
    let step = 2.0 * (1.0 - ARC_TOL / r).clamp(-1.0, 1.0).acos();
    let n = if step > 1e-3 { (sweep.abs() / step).ceil() as usize } else { 64 };
    let n = n.clamp(2, 64);
    (0..=n)
        .map(|i| {
            let a = a0 + sweep * (i as f32 / n as f32);
            Vec2::new(c.x + r * a.cos(), c.y + r * a.sin())
        })
        .collect()
}

/// An arc's points without its two ends.
fn between(pts: Vec<Vec2>) -> Vec<Vec2> {
    pts.get(1..pts.len().saturating_sub(1)).map(<[Vec2]>::to_vec).unwrap_or_default()
}

fn angle(v: Vec2) -> f32 {
    v.y.atan2(v.x)
}

/// Whether the path nearly turns back on itself from direction `u` to `v`. Only then can the
/// short way between the two normals miss the outside of the bend, so only then is the sweep of
/// a round join checked against the gap's middle. On a gentle bend the short way is always
/// right, and the middle's direction (the difference of two nearly equal unit vectors) is too
/// noisy to check against: zoomed far in on a wide line it sent the join the long way round,
/// a whole disc where a sliver belonged.
fn reverses(u: Vec2, v: Vec2) -> bool {
    u.dot(v) < -0.5
}

/// The outline of a variable-width stroke of `lines` (output space), as loops that don't
/// overlap, like [`stroke_polygons`].
///
/// `hw` is the nominal half-width in output units; the profile scales it on each side. The
/// profile's positions are fractions of each subpath's arc length measured in the shape's
/// own space: `to_local` maps output points back there (so the widths stay put whatever the
/// zoom or transform). `flip` swaps the sides, for transforms that mirror.
///
/// Each segment is a trapezoid whose ends are perpendicular to it, with the half-widths of
/// its two end points on each side; the outer side of every bend gets a round join of that
/// side's width, and open ends a round cap across their cross-section. Long segments are subdivided first so the widths follow the profile.
pub fn variable_polygons(lines: &[Polyline], hw: f32, profile: &WidthProfile, to_local: Option<&Affine>, flip: bool) -> Vec<Vec<Vec2>> {
    styled_polygons(lines, hw, &Outline::ROUND, Some(profile), to_local, flip)
}

/// How a stroke's outline is shaped, in output units (the stroker's view of a
/// [`anicraft_doc::StrokeStyle`]).
#[derive(Clone, Copy, Debug)]
pub struct Outline<'a> {
    pub cap: Cap,
    pub join: Join,
    pub miter_limit: f32,
    /// On/off lengths along each subpath (output units, starting with a dash) and how far into
    /// them each subpath starts; `None`: solid.
    pub dash: Option<(&'a [f32], f32)>,
}

impl Outline<'_> {
    /// Round caps and joins, solid: the stroke every shape had before styles.
    pub const ROUND: Outline<'static> = Outline { cap: Cap::Round, join: Join::Round, miter_limit: anicraft_doc::stroke_style::DEFAULT_MITER_LIMIT, dash: None };

    fn is_round(&self) -> bool {
        self.cap == Cap::Round && self.join == Join::Round && self.dash.is_none()
    }
}

/// Most dashes one stroke is cut into; past that, the rest of each subpath draws solid (only
/// reached by very long, very finely dashed lines zoomed far in).
const MAX_DASHES: usize = 200_000;

/// The outline of a stroke with any caps, joins and dashes, uniform (`profile` `None`) or
/// variable: loops that don't overlap, outer boundaries one way round and holes the other, so
/// either fill rule fills them alike and each point of the stroke is covered once however its
/// pieces overlap (see the module notes).
///
/// `hw`, `profile`, `to_local` and `flip` are as in [`variable_polygons`]. Dashes are measured
/// along the flattened lines in output units and restart at every subpath, as in SVG; each
/// dash is an open piece with the stroke's caps, its widths those of its place on the whole
/// subpath. A zero-length dash is a dot: its caps alone. A subpath with no length draws a dot
/// when the stroke is round, solid and of one width.
pub fn styled_polygons(lines: &[Polyline], hw: f32, outline: &Outline, profile: Option<&WidthProfile>, to_local: Option<&Affine>, flip: bool) -> Vec<Vec<Vec2>> {
    let (loops, apart) = piece_loops(lines, hw, outline, profile, to_local, flip);
    if apart { loops } else { polygons::union(loops) }
}

/// The outline of every piece of the stroke (see the module notes), before they are united:
/// all wound the same way, so where they overlap their winding numbers add up. Also whether
/// they are known not to overlap as they are (the outline of a convex closed line, which most
/// drawn shapes' are: see [`stroke_piece`]), so the union can be skipped.
fn piece_loops(lines: &[Polyline], hw: f32, outline: &Outline, profile: Option<&WidthProfile>, to_local: Option<&Affine>, flip: bool) -> (Vec<Vec<Vec2>>, bool) {
    let mut out = Vec::new();
    let mut apart = false;
    if !(hw.is_finite() && hw > 0.0) {
        return (out, apart);
    }
    let dash = outline.dash.filter(|(l, _)| l.len() >= 2 && l.iter().all(|x| x.is_finite() && *x >= 0.0) && l.iter().sum::<f32>() > 1e-6);
    let mut budget = MAX_DASHES;
    for pl in lines {
        let pts = if profile.is_some() { resample(pl) } else { cleaned(pl) };
        let n = pts.len();
        if n < 2 {
            // No length: a round, solid line of one width still draws a dot.
            if outline.is_round()
                && profile.is_none()
                && pl.points.len() >= 2
                && let Some(&at) = pts.first()
            {
                out.extend(dot(at, Vec2::new(1.0, 0.0), (hw, hw), outline.cap));
                apart = false;
            }
            continue;
        }
        let closed = pl.closed && pts.first().zip(pts.last()).is_some_and(|(a, b)| a.distance(*b) < 1e-4);
        // Arc length along the output lines (dashes), and in the shape's space (the profile).
        let cum = cumulative(&pts);
        let widths: Vec<(f32, f32)> = match profile {
            Some(pr) => {
                let local: Vec<Vec2> = match to_local {
                    Some(m) => pts.iter().map(|p| m.apply(*p)).collect(),
                    None => pts.clone(),
                };
                let lc = cumulative(&local);
                let total = lc.last().copied().filter(|d| d.is_finite() && *d > 0.0).unwrap_or(1.0);
                lc.iter().map(|c| side_widths(pr, c / total, hw, flip)).collect()
            }
            None => vec![(hw, hw); n],
        };
        let Some((pattern, offset)) = dash else {
            let first = out.is_empty();
            let ring = stroke_piece(&pts, &widths, closed, outline, &mut out);
            // (One piece only: two rings may overlap one another.)
            apart = first && ring && profile.is_none();
            continue;
        };
        apart = false;
        let total = cum.last().copied().unwrap_or(0.0);
        let mut cut = Cutter { pts: &pts, cum: &cum, widths: &widths, seg: 0 };
        for (a, b) in dash_intervals(pattern, offset, total, &mut budget) {
            if b - a <= 1e-6 {
                // A dot: the caps of a piece with no length.
                let (p, d, w) = cut.at(a);
                out.extend(dot(p, d, w, outline.cap));
                continue;
            }
            let (piece, pw) = cut.piece(a, b);
            stroke_piece(&piece, &pw, false, outline, &mut out);
        }
    }
    (out, apart)
}

/// The half-widths on each side at `t` of the way along, in output units.
fn side_widths(pr: &WidthProfile, t: f32, hw: f32, flip: bool) -> (f32, f32) {
    let (l, r) = pr.at(t);
    if flip { (r * hw, l * hw) } else { (l * hw, r * hw) }
}

fn cumulative(pts: &[Vec2]) -> Vec<f32> {
    let mut cum = Vec::with_capacity(pts.len());
    let mut d = 0.0f32;
    cum.push(0.0);
    for w in pts.windows(2) {
        d += w[0].distance(w[1]);
        cum.push(d);
    }
    cum
}

/// The on stretches `[a, b]` of a dash pattern along a subpath `total` long, in order (a dot
/// is a stretch with `a == b`). Uses up `budget`; when it runs out, the rest of the subpath is
/// one stretch.
fn dash_intervals(pattern: &[f32], offset: f32, total: f32, budget: &mut usize) -> Vec<(f32, f32)> {
    let mut out = Vec::new();
    let period: f32 = pattern.iter().sum();
    if !(total.is_finite() && total > 0.0 && period.is_finite() && period > 0.0) {
        return out;
    }
    // Entry `i` of period `k` starts at `base + k · period + starts[i]`: the subpath begins the
    // offset into the pattern. (Positions are computed, not summed, so they don't drift.)
    let base = -(if offset.is_finite() { offset.rem_euclid(period) } else { 0.0 });
    let mut starts = Vec::with_capacity(pattern.len());
    let mut acc = 0.0f32;
    for &len in pattern {
        starts.push(acc);
        acc += len;
    }
    // Every period holds a dash (entry 0), so the budget bounds the walk.
    'periods: for k in 0u32.. {
        let p0 = base + k as f32 * period;
        for (i, (&len, &st)) in pattern.iter().zip(&starts).enumerate() {
            let s = p0 + st;
            if s > total {
                break 'periods;
            }
            let e = s + len;
            if i % 2 == 0 && (if len > 0.0 { e > 0.0 && s < total } else { s >= 0.0 }) {
                if *budget == 0 {
                    out.push((s.max(0.0), total));
                    break 'periods;
                }
                *budget -= 1;
                out.push((s.max(0.0), e.min(total)));
            }
        }
        if k == u32::MAX {
            break;
        }
    }
    out
}

/// Cuts pieces out of a subpath by arc length, walking forwards.
struct Cutter<'a> {
    pts: &'a [Vec2],
    cum: &'a [f32],
    widths: &'a [(f32, f32)],
    /// The segment the last cut was in.
    seg: usize,
}

impl Cutter<'_> {
    /// The segment index holding arc length `s` (moving forwards from the last one).
    fn find(&mut self, s: f32) -> usize {
        let last = self.pts.len().saturating_sub(2);
        while self.seg < last && self.cum.get(self.seg + 1).is_some_and(|&c| c < s) {
            self.seg += 1;
        }
        self.seg
    }

    /// The point, direction and widths at arc length `s`.
    fn at(&mut self, s: f32) -> (Vec2, Vec2, (f32, f32)) {
        let i = self.find(s);
        let (a, b) = (self.pts.get(i).copied().unwrap_or(Vec2::ZERO), self.pts.get(i + 1).copied().unwrap_or(Vec2::ZERO));
        let (c0, c1) = (self.cum.get(i).copied().unwrap_or(0.0), self.cum.get(i + 1).copied().unwrap_or(0.0));
        let k = if c1 > c0 { ((s - c0) / (c1 - c0)).clamp(0.0, 1.0) } else { 0.0 };
        let (w0, w1) = (self.widths.get(i).copied().unwrap_or((0.0, 0.0)), self.widths.get(i + 1).copied().unwrap_or((0.0, 0.0)));
        let w = (w0.0 + (w1.0 - w0.0) * k, w0.1 + (w1.1 - w0.1) * k);
        (a.lerp(b, k), (b - a).normalized(), w)
    }

    /// The points and widths from arc length `a` to `b` (no repeated points).
    fn piece(&mut self, a: f32, b: f32) -> (Vec<Vec2>, Vec<(f32, f32)>) {
        let (p0, _, w0) = self.at(a);
        let mut pts = vec![p0];
        let mut ws = vec![w0];
        let mut push = |p: Vec2, w: (f32, f32)| {
            if pts.last().is_none_or(|q| q.distance(p) > 1e-6) {
                pts.push(p);
                ws.push(w);
            }
        };
        let mut j = self.seg + 1;
        while let (Some(&c), Some(&p)) = (self.cum.get(j), self.pts.get(j)) {
            if c >= b {
                break;
            }
            if c > a {
                push(p, self.widths.get(j).copied().unwrap_or((0.0, 0.0)));
            }
            j += 1;
        }
        let (p1, _, w1) = self.at(b);
        push(p1, w1);
        (pts, ws)
    }
}

/// What joins the end of one segment's edge to the start of the next one's, on one side.
enum Link {
    /// Both edges end at this point: where they cross on the inside of a bend, or the middle
    /// of the hair's breadth between them on a nearly straight run.
    Meet(Vec2),
    /// The join on the outside of a bend: these points between the two edges' ends (none: a
    /// bevel).
    Join(Vec<Vec2>),
    /// Through the bend's own point: the inside of a bend whose edges don't cross.
    Pivot,
}

/// One side (left or right) of a run of segments: each segment's edge and what links it to
/// the one before.
struct Side {
    /// +1: the left of the direction of travel; −1: the right.
    sign: f32,
    /// Each segment's edge, from its start's cross-section to its end's.
    edges: Vec<(Vec2, Vec2)>,
    /// What comes before segment `i`'s edge (`None`: the start of an open run).
    links: Vec<Option<Link>>,
    /// How far along its edge each segment starts and ends, where its neighbours cut it on
    /// the inside of a bend (0 and 1: uncut).
    cuts: Vec<(f64, f64)>,
}

/// Where segment `a0`–`a1` crosses `b0`–`b1`: how far along each, and the point.
fn crossing(a0: Vec2, a1: Vec2, b0: Vec2, b1: Vec2) -> Option<(f64, f64, Vec2)> {
    let f = |v: Vec2| (f64::from(v.x), f64::from(v.y));
    let ((rx, ry), (qx, qy), (wx, wy)) = (f(a1 - a0), f(b1 - b0), f(b0 - a0));
    let den = rx * qy - ry * qx;
    if !(den.is_finite() && den.abs() > 1e-12) {
        return None;
    }
    let s = (wx * qy - wy * qx) / den;
    let t = (wx * ry - wy * rx) / den;
    if !((0.0..=1.0).contains(&s) && (0.0..=1.0).contains(&t)) {
        return None;
    }
    let x = Vec2::new((f64::from(a0.x) + rx * s) as f32, (f64::from(a0.y) + ry * s) as f32);
    x.is_finite().then_some((s, t, x))
}

/// One open or closed run of points with its half-widths per point, as its outline: one loop
/// when open (with its caps), two when closed (see the module notes). The loops may overlap
/// themselves and one another where the run does; all are wound the same way.
///
/// Returns whether the run is a convex closed line none of whose bends had to go through its
/// own point. Given one width all round, its two loops then can't overlap: the outer one is the
/// line's convex polygon grown, with a join at each corner, and the inner one is cut from
/// that polygon's edges moved inwards, each still there and in its place, so it is convex too,
/// and inside the other.
fn stroke_piece(pts: &[Vec2], widths: &[(f32, f32)], closed: bool, outline: &Outline, out: &mut Vec<Vec<Vec2>>) -> bool {
    let n = pts.len();
    if n < 2 {
        return false;
    }
    let segs = n - 1;
    let w = |i: usize| widths.get(i).copied().unwrap_or((0.0, 0.0));
    let p = |i: usize| pts.get(i).copied().unwrap_or(Vec2::ZERO);
    let dirs: Vec<Vec2> = (0..segs).map(|i| (p(i + 1) - p(i)).normalized()).collect();
    let dir = |i: usize| dirs.get(i).copied().unwrap_or(Vec2::ZERO);
    let side = |sign: f32| {
        let half = |i: usize| if sign > 0.0 { w(i).0 } else { w(i).1 };
        let edges = (0..segs)
            .map(|i| {
                let nrm = left_normal(dir(i)) * sign;
                (p(i) + nrm * half(i), p(i + 1) + nrm * half(i + 1))
            })
            .collect();
        Side { sign, edges, links: (0..segs).map(|_| None).collect(), cuts: vec![(0.0, 1.0); segs] }
    };
    let mut sides = [side(1.0), side(-1.0)];
    // Bends: before segment `after`, at point `at` (a closed run's start, too, where its last
    // segment meets its first).
    let joints = (1..segs).map(|i| (i, i - 1, i)).chain(closed.then_some((0, segs - 1, 0)));
    for (at, before, after) in joints {
        let (u, v) = (dir(before), dir(after));
        let (l, r) = w(at);
        // Nearly straight (most joints of a flattened curve): no join; the outer edges are a
        // hair's breadth apart.
        let straight = u.dot(v) > 0.0 && u.cross(v).abs() * l.max(r) < 0.05;
        // The gap opens on the outside of the bend, around the direction u − v.
        let mid = (u - v).normalized();
        let (n0, n1) = (left_normal(u), left_normal(v));
        let outer = if mid.dot(n0) >= 0.0 { 1.0 } else { -1.0 };
        for sd in &mut sides {
            let radius = if sd.sign > 0.0 { l } else { r };
            let (o0, o1) = (n0 * sd.sign, n1 * sd.sign);
            let (a, b) = (p(at) + o0 * radius, p(at) + o1 * radius);
            let link = if radius <= NO_WIDTH || a.distance(b) < 1e-4 || (straight && sd.sign == outer) {
                Link::Meet(a.lerp(b, 0.5))
            } else if sd.sign == outer {
                Link::Join(join(p(at), u, v, mid, o0, o1, radius, outline))
            } else {
                // The inside of the bend: the edges cross, unless they are too short to.
                let (Some(&(b0, _)), Some(&(_, a1))) = (sd.edges.get(before), sd.edges.get(after)) else { continue };
                match crossing(b0, a, b, a1) {
                    Some((s, t, x)) => {
                        if let Some(c) = sd.cuts.get_mut(before) {
                            c.1 = s;
                        }
                        if let Some(c) = sd.cuts.get_mut(after) {
                            c.0 = t;
                        }
                        Link::Meet(x)
                    }
                    None => Link::Pivot,
                }
            };
            if let Some(slot) = sd.links.get_mut(after) {
                *slot = Some(link);
            }
        }
    }
    for sd in &mut sides {
        // A segment cut past its other cut has no edge left: its bends go through their own
        // points instead (which only gives the edges back, so one pass is enough).
        for i in 0..segs {
            if sd.cuts.get(i).is_some_and(|(from, to)| from > to) {
                for j in [i, (i + 1) % segs] {
                    if let Some(Some(link @ Link::Meet(_))) = sd.links.get_mut(j) {
                        *link = Link::Pivot;
                    }
                }
            }
        }
    }
    let chain = |sd: &Side| {
        let mut c: Vec<Vec2> = Vec::with_capacity(segs + 8);
        let mut push = |q: Vec2| {
            if c.last() != Some(&q) {
                c.push(q);
            }
        };
        for i in 0..segs {
            let before = sd.edges.get(if i == 0 { segs - 1 } else { i - 1 }).map_or(Vec2::ZERO, |e| e.1);
            let start = sd.edges.get(i).map_or(Vec2::ZERO, |e| e.0);
            match sd.links.get(i).and_then(Option::as_ref) {
                None => push(start),
                Some(Link::Meet(x)) => push(*x),
                Some(Link::Join(via)) => {
                    push(before);
                    via.iter().for_each(|q| push(*q));
                    push(start);
                }
                Some(Link::Pivot) => {
                    push(before);
                    push(p(i));
                    push(start);
                }
            }
        }
        if !closed && let Some(e) = sd.edges.last() {
            push(e.1);
        }
        c
    };
    let [left, right] = &sides;
    let (mut lc, mut rc) = (chain(left), chain(right));
    rc.reverse();
    if closed {
        out.push(lc);
        out.push(rc);
        let folded = sides.iter().flat_map(|sd| &sd.links).any(|l| matches!(l, Some(Link::Pivot)));
        return !folded && polygons::is_convex(pts.get(..segs).unwrap_or(pts));
    }
    let (d0, d1) = (dir(0), dir(segs - 1));
    lc.extend(cap(p(n - 1), d1, left_normal(d1), w(n - 1), outline.cap, true));
    lc.extend(rc);
    lc.extend(cap(p(0), -d0, left_normal(d0), w(0), outline.cap, false));
    lc.dedup();
    out.push(lc);
    false
}

/// The points of the join between the two outer edges' ends at a bend at `p` from direction
/// `u` to `v` (unit vectors): `o0` and `o1` are the outer normals before and after, `mid` the
/// direction the gap opens in and `radius` the half-width on that side. The ends themselves
/// are not included; none is a bevel.
#[allow(clippy::too_many_arguments)]
fn join(p: Vec2, u: Vec2, v: Vec2, mid: Vec2, o0: Vec2, o1: Vec2, radius: f32, outline: &Outline) -> Vec<Vec2> {
    match outline.join {
        Join::Round => {
            let (a0, a1, am) = (angle(o0), angle(o1), angle(mid));
            let wrap = |x: f32| (x + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
            let mut sweep = wrap(a1 - a0);
            let to_mid = wrap(am - a0);
            // Go round the side that passes through the gap's middle.
            if reverses(u, v) && (to_mid * sweep < 0.0 || to_mid.abs() > sweep.abs()) {
                sweep -= std::f32::consts::TAU.copysign(sweep);
            }
            between(arc(p, a0, sweep, radius))
        }
        Join::Miter | Join::Bevel => {
            // The outer edges meet at p + (o0 + o1) / (1 + o0·o1) · radius, 1 / cos(θ/2) half-
            // widths out (θ the turn); SVG's miter ratio is that over the half-width.
            let c = 1.0 + o0.dot(o1);
            let limit = outline.miter_limit.clamp(1.0, anicraft_doc::stroke_style::MAX_MITER_LIMIT);
            if outline.join == Join::Miter && c > 1e-6 && c * limit * limit >= 2.0 {
                vec![p + (o0 + o1) * (radius / c)]
            } else {
                Vec::new()
            }
        }
    }
}

/// The points of the cap at end `at` of a piece between its left edge's end and its right
/// edge's (neither included), in that order when `end` (the piece's last point) and from right
/// to left otherwise, as the outline goes. It faces `out` (unit, away from the piece), across
/// the cross-section with left normal `nrm` and half-widths `(left, right)`. None: a butt cap.
fn cap(at: Vec2, out: Vec2, nrm: Vec2, (l, r): (f32, f32), kind: Cap, end: bool) -> Vec<Vec2> {
    let half = (l + r) * 0.5;
    if half <= NO_WIDTH || !out.is_finite() {
        return Vec::new();
    }
    let mut pts = match kind {
        Cap::Butt => Vec::new(),
        // Half a disc across the cross-section (so a one-sided width stays on its side).
        Cap::Round => between(arc(at + nrm * ((l - r) * 0.5), angle(nrm), if end { std::f32::consts::PI } else { -std::f32::consts::PI }, half)),
        // The cross-section carried half its thickness past the end.
        Cap::Square => vec![at + nrm * l + out * half, at - nrm * r + out * half],
    };
    if !end {
        pts.reverse();
    }
    pts
}

/// A piece with no length at `at`, heading along `d`: its two caps, as one loop (none for
/// butt caps).
fn dot(at: Vec2, d: Vec2, (l, r): (f32, f32), kind: Cap) -> Option<Vec<Vec2>> {
    if kind == Cap::Butt || (l + r) * 0.5 <= NO_WIDTH || !(at.is_finite() && d.is_finite()) {
        return None;
    }
    let nrm = left_normal(d);
    let mut ring = vec![at + nrm * l];
    ring.extend(cap(at, d, nrm, (l, r), kind, true));
    ring.push(at - nrm * r);
    ring.extend(cap(at, -d, nrm, (l, r), kind, false));
    Some(ring)
}

/// A polyline's finite points with repeats dropped and the start repeated at the end when it
/// is closed (no subdivision: one width needs none).
fn cleaned(pl: &Polyline) -> Vec<Vec2> {
    let mut out: Vec<Vec2> = Vec::with_capacity(pl.points.len() + 1);
    for &p in pl.points.iter().filter(|p| p.is_finite()) {
        if out.last().is_none_or(|q| q.distance(p) > 1e-6) {
            out.push(p);
        }
    }
    if pl.closed
        && let (Some(&first), Some(&last)) = (out.first(), out.last())
        && first.distance(last) > 1e-6
    {
        out.push(first);
    }
    out
}

/// A polyline's points with long segments subdivided (so widths can follow a profile), the
/// start repeated at the end when it is closed, and repeated points dropped.
fn resample(pl: &Polyline) -> Vec<Vec2> {
    let mut src: Vec<Vec2> = pl.points.iter().copied().filter(|p| p.is_finite()).collect();
    if pl.closed
        && let (Some(&first), Some(&last)) = (src.first(), src.last())
        && first.distance(last) > 1e-6
    {
        src.push(first);
    }
    let len: f32 = src.windows(2).map(|w| w[0].distance(w[1])).sum();
    // About 256 samples along the subpath, but none closer than ¾ px or farther than 8 px.
    let step = (len / 256.0).clamp(0.75, 8.0);
    let mut out: Vec<Vec2> = Vec::with_capacity(src.len().min(MAX_SAMPLES));
    for (i, &b) in src.iter().enumerate() {
        if let Some(&a) = i.checked_sub(1).and_then(|j| src.get(j)) {
            let d = a.distance(b);
            let k = if src.len() < MAX_SAMPLES && d.is_finite() { ((d / step).ceil() as usize).clamp(1, MAX_SAMPLES) } else { 1 };
            for j in 1..k {
                if out.len() >= MAX_SAMPLES {
                    break;
                }
                out.push(a.lerp(b, j as f32 / k as f32));
            }
        }
        if out.last().is_none_or(|q| q.distance(b) > 1e-6) {
            out.push(b);
        }
    }
    out
}

/// The polygons stroke `st` fills along `lines` (its path flattened through `m`), and how much
/// to fade them; `None` when it draws nothing.
///
/// `raster` is what the renderer does for strokes too fine for pixels: a stroke thinner than
/// one output unit keeps that footprint and fades by its width instead (no dropouts), and a
/// dash pattern repeating in less than one output unit draws solid, faded by how much of it is
/// on. Vector outlines leave it off and get the exact geometry. Dash lengths are measured along
/// the output lines, scaled like the width (by the mean scale of `m`), so vector exports, which
/// write `stroke-dasharray` the same way, draw the same dashes.
pub fn stroke_geometry(lines: &[Polyline], st: &Stroke, m: &Affine, raster: bool) -> Option<(Vec<Vec<Vec2>>, f32)> {
    let scale = m.mean_scale();
    let mut pw = st.width * scale;
    if !(pw.is_finite() && pw > 0.0) {
        return None;
    }
    let mut alpha = 1.0f32;
    let widest = pw * st.profile.as_ref().map_or(1.0, WidthProfile::max_factor);
    if !(widest.is_finite() && widest > 0.0) {
        return None;
    }
    if raster && widest < MIN_RASTER_WIDTH {
        alpha *= widest / MIN_RASTER_WIDTH;
        pw *= MIN_RASTER_WIDTH / widest;
    }
    let style = &st.style;
    let cap = style.effective_cap();
    let pattern = style.dash.as_ref().map(|d| {
        let (l, o) = d.pattern(st.width);
        (l.into_iter().map(|x| x * scale).collect::<Vec<f32>>(), o * scale)
    });
    let mut dash = pattern.as_ref().map(|(l, o)| (l.as_slice(), *o));
    if let Some((l, _)) = dash {
        let period: f32 = l.iter().sum();
        if !(period.is_finite() && period > 0.0) {
            dash = None;
        } else if raster && period < 1.0 {
            // Finer than a pixel: solid, as light as the pattern's ink.
            let dashes = l.len().div_ceil(2) as f32;
            let on: f32 = l.iter().step_by(2).sum::<f32>() + if cap == Cap::Butt { 0.0 } else { dashes * pw };
            alpha *= (on / period).clamp(0.0, 1.0);
            dash = None;
        }
    }
    let outline = Outline { cap, join: style.join, miter_limit: style.miter_limit, dash };
    let (to_local, flip) = match &st.profile {
        Some(_) => (m.inverse(), m.determinant() < 0.0),
        None => (None, false),
    };
    let polys = styled_polygons(lines, pw * 0.5, &outline, st.profile.as_ref(), to_local.as_ref(), flip);
    Some((polys, alpha))
}

/// A stroke as a filled outline in `m`'s output space, for vector exports: the same polygons
/// the renderer fills, as closed subpaths (fill them non-zero).
pub fn stroke_outline(path: &Path, m: &Affine, st: &Stroke, tolerance: f32) -> Path {
    let lines = path.flatten(m, tolerance);
    let mut out = Path::new();
    let Some((polys, _)) = stroke_geometry(&lines, st, m, false) else { return out };
    for poly in polys {
        let mut it = poly.into_iter();
        let Some(first) = it.next() else { continue };
        out.move_to(first);
        for q in it {
            out.line_to(q);
        }
        out.close();
    }
    out
}

/// `dl` with every variable-width stroke turned into a filled outline item, drawn right after
/// its shape (same layer, element, clip, group and colour effect), so vector exports show what
/// the renderer shows. Borrowed unchanged when there are none.
pub fn outline_variable_strokes(dl: &DisplayList) -> std::borrow::Cow<'_, DisplayList> {
    let varies = |i: &DisplayItem| i.masks.is_none() && !i.guide && i.outline.is_none() && i.stroke.as_ref().is_some_and(|s| s.profile.is_some());
    if !dl.items.iter().any(varies) {
        return std::borrow::Cow::Borrowed(dl);
    }
    let mut items = Vec::with_capacity(dl.items.len() + 8);
    for it in &dl.items {
        let Some(st) = it.stroke.as_ref().filter(|_| varies(it)) else {
            items.push(it.clone());
            continue;
        };
        if it.fill.is_some() {
            items.push(DisplayItem { stroke: None, ..it.clone() });
        }
        if !it.transform.is_finite() {
            continue;
        }
        let outline = stroke_outline(&it.path, &it.transform, st, crate::TOLERANCE);
        if !outline.is_empty() {
            items.push(DisplayItem { path: std::sync::Arc::new(outline), transform: Affine::IDENTITY, fill: Some(anicraft_doc::Paint::Solid(st.color)), stroke: None, fill_rule: anicraft_geom::FillRule::NonZero, image: None, ..it.clone() });
        }
    }
    std::borrow::Cow::Owned(DisplayList { links: Vec::new(), clip_modes: dl.clip_modes.clone(), items, clip_parents: dl.clip_parents.clone(), groups: dl.groups.clone(), left_out: dl.left_out })
}

/// Runs examined per grid cell when chaining raw stroke runs.
const CHAIN_SCAN: usize = 16;

/// One segment of a raw stroke subpath, keeping its exact curve type.
#[derive(Clone, Copy)]
enum RawSeg {
    Line(Vec2),
    Quad(Vec2, Vec2),
    Cubic(Vec2, Vec2, Vec2),
}

impl RawSeg {
    fn end(self) -> Vec2 {
        match self {
            RawSeg::Line(p) | RawSeg::Quad(_, p) | RawSeg::Cubic(_, _, p) => p,
        }
    }

    fn finite(self) -> bool {
        match self {
            RawSeg::Line(p) => p.is_finite(),
            RawSeg::Quad(c, p) => c.is_finite() && p.is_finite(),
            RawSeg::Cubic(c1, c2, p) => c1.is_finite() && c2.is_finite() && p.is_finite(),
        }
    }

    fn set_end(&mut self, p: Vec2) {
        match self {
            RawSeg::Line(e) | RawSeg::Quad(_, e) | RawSeg::Cubic(_, _, e) => *e = p,
        }
    }

    fn push_to(self, path: &mut Path) {
        match self {
            RawSeg::Line(p) => { path.line_to(p); }
            RawSeg::Quad(c, p) => { path.quad_to(c, p); }
            RawSeg::Cubic(c1, c2, p) => { path.cubic_to(c1, c2, p); }
        }
    }
}

/// One subpath extracted from a raw stroke display item in the bucket's common local space.
struct RawSub {
    start: Vec2,
    segs: Vec<RawSeg>,
    closed: bool,
    item: usize,
}

impl RawSub {
    fn end(&self) -> Vec2 {
        self.segs.last().map_or(self.start, |s| s.end())
    }

    fn reverse(self) -> RawSub {
        let end = self.end();
        let mut rev = Vec::with_capacity(self.segs.len());
        let mut prev = self.start;
        for seg in self.segs {
            let next_prev = seg.end();
            rev.push(match seg {
                RawSeg::Line(_) => RawSeg::Line(prev),
                RawSeg::Quad(c, _) => RawSeg::Quad(c, prev),
                RawSeg::Cubic(c1, c2, _) => RawSeg::Cubic(c2, c1, prev),
            });
            prev = next_prev;
        }
        rev.reverse();
        RawSub { start: end, segs: rev, closed: self.closed, item: self.item }
    }

    fn to_path(&self) -> Path {
        let mut p = Path::new();
        p.move_to(self.start);
        for seg in &self.segs {
            seg.push_to(&mut p);
        }
        if self.closed {
            p.close();
        }
        p
    }
}

fn is_raw_stroke(it: &DisplayItem) -> bool {
    it.merge
        && it.fill.is_none()
        && it.outline.is_none()
        && !it.guide
        && it.warp.is_none()
        && it.transform.is_finite()
        && it.stroke.as_ref().is_some_and(|st| st.width.is_finite() && st.width > 0.0)
}

fn is_joinable_raw_stroke(it: &DisplayItem) -> bool {
    is_raw_stroke(it) && it.stroke.as_ref().is_some_and(|st| st.profile.is_none() && st.style.dash.is_none())
}

type RawRunScope = (anicraft_doc::LayerId, Option<u32>, Option<u32>, Option<u32>);

fn raw_run_scope(it: &DisplayItem) -> RawRunScope {
    (it.layer, it.clip, it.masks, it.group)
}

fn same_raw_run(a: &DisplayItem, b: &DisplayItem) -> bool {
    raw_run_scope(a) == raw_run_scope(b)
}

fn same_raw_style(a: &DisplayItem, b: &DisplayItem) -> bool {
    a.transform.m[..4] == b.transform.m[..4] && a.stroke == b.stroke && a.color == b.color
}

/// Whether `it`'s rasterised stroke paint is fully opaque: painted alpha == 1 after colour
/// transform and effective device width at least [`MIN_RASTER_WIDTH`]. Opaque strokes overlapping
/// away from a corner paint identically whether joined or separate; semi-transparent strokes —
/// including sub-pixel strokes widened to [`MIN_RASTER_WIDTH`] at reduced alpha in
/// [`stroke_geometry`] — darken where separate strokes overlap, so joining them would lose
/// opacity at remote crossings.
fn raw_stroke_opaque(it: &DisplayItem, device_width: f32) -> bool {
    device_width.is_finite()
        && device_width >= MIN_RASTER_WIDTH
        && it.stroke.as_ref().is_some_and(|st| it.color.apply(st.color).a >= 1.0)
}

/// Most segment-pair distance checks for one semi-transparent join candidate; past this budget we
/// conservatively refuse the join rather than risk quadratic work on huge scribbles.
const MAX_OVERLAP_CHECKS: usize = 262_144;

fn flatten_raw_segs(start: Vec2, segs: impl IntoIterator<Item = RawSeg>, base_xf: &Affine, flat_tol: f32) -> Vec<(Vec2, Vec2)> {
    let mut path = Path::new();
    path.move_to(start);
    for seg in segs {
        seg.push_to(&mut path);
    }
    let mut out = Vec::new();
    for pl in path.flatten(base_xf, flat_tol) {
        for w in pl.points.windows(2) {
            if let [a, b] = *w {
                out.push((a, b));
            }
        }
    }
    out
}

/// Appends the sub-segments of `a`–`b` that lie outside the open disk of radius `r` around `c`.
fn clip_seg_outside_disk(a: Vec2, b: Vec2, c: Vec2, r: f32, out: &mut Vec<(Vec2, Vec2)>) {
    let r = f64::from(r);
    let r2 = r * r;
    let (ux, uy) = (f64::from(a.x) - f64::from(c.x), f64::from(a.y) - f64::from(c.y));
    let (dx, dy) = (f64::from(b.x) - f64::from(a.x), f64::from(b.y) - f64::from(a.y));
    let aa = dx * dx + dy * dy;
    let cc = ux * ux + uy * uy - r2;
    if aa <= 1e-18 {
        if cc >= 0.0 {
            out.push((a, b));
        }
        return;
    }
    let bb = ux * dx + uy * dy;
    let disc = bb * bb - aa * cc;
    if disc <= 0.0 {
        out.push((a, b));
        return;
    }
    let root = disc.sqrt();
    let t0 = (-bb - root) / aa;
    let t1 = (-bb + root) / aa;
    if !(t0.is_finite() && t1.is_finite()) || t1 <= 0.0 || t0 >= 1.0 {
        out.push((a, b));
        return;
    }
    let at = |t: f64| Vec2::new((f64::from(a.x) + dx * t) as f32, (f64::from(a.y) + dy * t) as f32);
    if t0 > 0.0 {
        out.push((a, at(t0)));
    }
    if t1 < 1.0 {
        out.push((at(t1), b));
    }
}

fn clip_segs_outside_disks(segs: &[(Vec2, Vec2)], disks: &[Vec2], r: f32) -> Vec<(Vec2, Vec2)> {
    let mut cur: Vec<(Vec2, Vec2)> = segs.to_vec();
    let mut next: Vec<(Vec2, Vec2)> = Vec::with_capacity(cur.len());
    for &c in disks {
        next.clear();
        for &(a, b) in &cur {
            clip_seg_outside_disk(a, b, c, r, &mut next);
        }
        std::mem::swap(&mut cur, &mut next);
    }
    cur
}

fn segs_bounds(segs: &[(Vec2, Vec2)]) -> (Vec2, Vec2) {
    let mut min = Vec2::new(f32::INFINITY, f32::INFINITY);
    let mut max = Vec2::new(f32::NEG_INFINITY, f32::NEG_INFINITY);
    for &(a, b) in segs {
        min.x = min.x.min(a.x).min(b.x);
        min.y = min.y.min(a.y).min(b.y);
        max.x = max.x.max(a.x).max(b.x);
        max.y = max.y.max(a.y).max(b.y);
    }
    (min, max)
}

fn bounds_within(a: (Vec2, Vec2), b: (Vec2, Vec2), limit: f32) -> bool {
    a.1.x + limit > b.0.x && b.1.x + limit > a.0.x && a.1.y + limit > b.0.y && b.1.y + limit > a.0.y
}

/// Whether segments `a0`–`a1` and `b0`–`b1` come strictly closer than `limit`.
fn segs_closer_than(a0: Vec2, a1: Vec2, b0: Vec2, b1: Vec2, limit: f32) -> bool {
    let (ax0, ax1) = (a0.x.min(a1.x), a0.x.max(a1.x));
    let (bx0, bx1) = (b0.x.min(b1.x), b0.x.max(b1.x));
    if ax1 + limit <= bx0 || bx1 + limit <= ax0 {
        return false;
    }
    let (ay0, ay1) = (a0.y.min(a1.y), a0.y.max(a1.y));
    let (by0, by1) = (b0.y.min(b1.y), b0.y.max(b1.y));
    if ay1 + limit <= by0 || by1 + limit <= ay0 {
        return false;
    }
    if crossing(a0, a1, b0, b1).is_some() {
        return true;
    }
    anicraft_geom::segment_distance(a0, b0, b1) < limit
        || anicraft_geom::segment_distance(a1, b0, b1) < limit
        || anicraft_geom::segment_distance(b0, a0, a1) < limit
        || anicraft_geom::segment_distance(b1, a0, a1) < limit
}

/// Whether the flattened stage-space centrelines `a` and `b` come closer than `stage_width`
/// anywhere outside the neighbourhood of the shared endpoint(s) `disks`.
///
/// The neighbourhood radius is `stage_width + SNAP` (rather than `half_width + SNAP`) so plain
/// corners — including 90° L-corners and 60° triangle vertices, whose trimmed rays start
/// `2·R·sin(θ/2)` apart — stay at least `stage_width` apart outside the disk while remote
/// crossings or close approaches farther along the stroke are still caught.
fn raw_remote_overlap(a: &[(Vec2, Vec2)], b: &[(Vec2, Vec2)], stage_width: f32, disks: &[Vec2]) -> bool {
    let radius = stage_width + anicraft_geom::planar::SNAP;
    let ta = clip_segs_outside_disks(a, disks, radius);
    let tb = clip_segs_outside_disks(b, disks, radius);
    if ta.is_empty() || tb.is_empty() {
        return false;
    }
    if !bounds_within(segs_bounds(&ta), segs_bounds(&tb), stage_width) {
        return false;
    }
    const CHUNK: usize = 16;
    let b_chunks: Vec<_> = tb.chunks(CHUNK).map(|c| (c, segs_bounds(c))).collect();
    let mut checks = 0usize;
    for ac in ta.chunks(CHUNK) {
        let ab = segs_bounds(ac);
        for &(bc, bb) in &b_chunks {
            if !bounds_within(ab, bb, stage_width) {
                continue;
            }
            for &(a0, a1) in ac {
                for &(b0, b1) in bc {
                    checks += 1;
                    if checks > MAX_OVERLAP_CHECKS {
                        return true;
                    }
                    if segs_closer_than(a0, a1, b0, b1, stage_width) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Adds stage-space endpoints of every open subpath in `it` to `ends`, indexed by `SNAP`-sized
/// grid cells. Called for every raw stroke in the same drawing scope (even across intervening
/// fills or style changes) so a three-way junction with a stroke anywhere in the scope counts as
/// degree >= 3.
fn collect_item_raw_endpoints(it: &DisplayItem, ends: &mut HashMap<(i64, i64), Vec<Vec2>>) {
    let cell = anicraft_geom::planar::SNAP;
    let mut push_open = |start: Vec2, end: Vec2| {
        if start.is_finite() && end.is_finite() {
            ends.entry(raw_grid_key(start, cell)).or_default().push(start);
            ends.entry(raw_grid_key(end, cell)).or_default().push(end);
        }
    };
    let xf = &it.transform;
    let mut item_ends: Vec<(Vec2, Vec2)> = Vec::new();
    let mut start: Option<Vec2> = None;
    let mut last = xf.apply(Vec2::ZERO);
    let mut has_seg = false;
    let mut finite = true;
    for cmd in &it.path.cmds {
        match *cmd {
            PathCmd::MoveTo(p) => {
                if let Some(s) = start.take() && has_seg {
                    item_ends.push((s, last));
                }
                let q = xf.apply(p);
                if !q.is_finite() {
                    finite = false;
                    break;
                }
                start = Some(q);
                last = q;
                has_seg = false;
            }
            PathCmd::LineTo(p) => {
                let q = xf.apply(p);
                if !q.is_finite() {
                    finite = false;
                    break;
                }
                start.get_or_insert(last);
                last = q;
                has_seg = true;
            }
            PathCmd::QuadTo(c, p) => {
                let (c, q) = (xf.apply(c), xf.apply(p));
                if !(c.is_finite() && q.is_finite()) {
                    finite = false;
                    break;
                }
                start.get_or_insert(last);
                last = q;
                has_seg = true;
            }
            PathCmd::CubicTo(c1, c2, p) => {
                let (c1, c2, q) = (xf.apply(c1), xf.apply(c2), xf.apply(p));
                if !(c1.is_finite() && c2.is_finite() && q.is_finite()) {
                    finite = false;
                    break;
                }
                start.get_or_insert(last);
                last = q;
                has_seg = true;
            }
            PathCmd::Close => {
                if let Some(s) = start.take() {
                    last = s;
                }
                has_seg = false;
            }
        }
    }
    if !finite {
        return;
    }
    if let Some(s) = start && has_seg {
        item_ends.push((s, last));
    }
    for (s, e) in item_ends {
        push_open(s, e);
    }
}

fn extract_raw_subs(path: &Path, into: &Affine, item: usize) -> Option<Vec<RawSub>> {
    let mut out = Vec::new();
    let mut cur: Option<RawSub> = None;
    let mut last = Vec2::ZERO;
    for cmd in &path.cmds {
        match *cmd {
            PathCmd::MoveTo(p) => {
                if let Some(s) = cur.take().filter(|s| !s.segs.is_empty()) {
                    out.push(s);
                }
                let q = into.apply(p);
                if !q.is_finite() {
                    return None;
                }
                cur = Some(RawSub { start: q, segs: Vec::new(), closed: false, item });
                last = q;
            }
            PathCmd::LineTo(p) => {
                let q = into.apply(p);
                let seg = RawSeg::Line(q);
                if !seg.finite() {
                    return None;
                }
                cur.get_or_insert(RawSub { start: last, segs: Vec::new(), closed: false, item }).segs.push(seg);
                last = q;
            }
            PathCmd::QuadTo(c, p) => {
                let (c, q) = (into.apply(c), into.apply(p));
                let seg = RawSeg::Quad(c, q);
                if !seg.finite() {
                    return None;
                }
                cur.get_or_insert(RawSub { start: last, segs: Vec::new(), closed: false, item }).segs.push(seg);
                last = q;
            }
            PathCmd::CubicTo(c1, c2, p) => {
                let (c1, c2, q) = (into.apply(c1), into.apply(c2), into.apply(p));
                let seg = RawSeg::Cubic(c1, c2, q);
                if !seg.finite() {
                    return None;
                }
                cur.get_or_insert(RawSub { start: last, segs: Vec::new(), closed: false, item }).segs.push(seg);
                last = q;
            }
            PathCmd::Close => {
                if let Some(mut s) = cur.take() {
                    if s.start.distance(last) > 1e-6 {
                        s.segs.push(RawSeg::Line(s.start));
                    }
                    s.closed = true;
                    last = s.start;
                    if !s.segs.is_empty() {
                        out.push(s);
                    }
                }
            }
        }
    }
    if let Some(s) = cur.filter(|s| !s.segs.is_empty()) {
        out.push(s);
    }
    Some(out)
}

fn raw_grid_key(v: Vec2, cell: f32) -> (i64, i64) {
    ((v.x / cell).floor() as i64, (v.y / cell).floor() as i64)
}

/// Whether exactly two open endpoints of `ends` lie within `SNAP` (stage units) of `at`: a plain
/// corner where two stroke runs meet with no ambiguity.
fn raw_plain_corner(ends: &HashMap<(i64, i64), Vec<Vec2>>, at: Vec2) -> bool {
    if !at.is_finite() {
        return false;
    }
    let tol = anicraft_geom::planar::SNAP;
    let (kx, ky) = raw_grid_key(at, tol);
    let mut n = 0;
    for dx in -1..=1_i64 {
        for dy in -1..=1_i64 {
            let Some(list) = ends.get(&(kx.saturating_add(dx), ky.saturating_add(dy))) else { continue };
            if list.len() > CHAIN_SCAN {
                return false;
            }
            n += list.iter().filter(|e| e.distance(at) <= tol).count();
        }
    }
    n == 2
}

/// The last remaining open subpath with an endpoint within `SNAP` (stage units) of `at` whose
/// matched endpoint also passes [`raw_plain_corner`]. Returns `(slot_index, matched_start)`.
fn raw_chain_find(
    grid: &mut HashMap<(i64, i64), Vec<usize>>,
    ends: &HashMap<(i64, i64), Vec<Vec2>>,
    slots: &[Option<RawSub>],
    base_xf: &Affine,
    at: Vec2,
) -> Option<(usize, bool)> {
    if !at.is_finite() {
        return None;
    }
    let tol = anicraft_geom::planar::SNAP;
    let (kx, ky) = raw_grid_key(at, tol);
    let mut best: Option<(usize, bool)> = None;
    for dx in -1..=1_i64 {
        for dy in -1..=1_i64 {
            let Some(list) = grid.get_mut(&(kx.saturating_add(dx), ky.saturating_add(dy))) else { continue };
            let mut seen = 0;
            let mut k = list.len();
            while k > 0 && seen < CHAIN_SCAN {
                k -= 1;
                seen += 1;
                let Some(&i) = list.get(k) else { break };
                let Some(s) = slots.get(i).and_then(Option::as_ref) else {
                    if k + 1 == list.len() {
                        list.pop();
                    }
                    continue;
                };
                if best.is_some_and(|(b, _)| b >= i) {
                    break;
                }
                let st = base_xf.apply(s.start);
                let en = base_xf.apply(s.end());
                let matched = if st.distance(at) <= tol {
                    Some((true, st))
                } else if en.distance(at) <= tol {
                    Some((false, en))
                } else {
                    None
                };
                if let Some((matched_start, end_pt)) = matched && raw_plain_corner(ends, end_pt) {
                    best = Some((i, matched_start));
                    break;
                }
            }
        }
    }
    best
}

/// Chains open subpaths of the contiguous compatible run `range` (same style, linear transform and
/// colour transform) at plain degree-2 corners using the drawing scope's stage-space endpoint table
/// `ends`; returns `None` when nothing joins.
fn chain_raw_run(
    dl: &DisplayList,
    range: std::ops::Range<usize>,
    ends: &HashMap<(i64, i64), Vec<Vec2>>,
    view_xf: &Affine,
) -> Option<Vec<(usize, DisplayItem)>> {
    let base = dl.items.get(range.start)?;
    let base_xf = base.transform;
    let inv = base_xf.inverse()?;
    let scale = base_xf.mean_scale();
    if !(scale.is_finite() && scale > 1e-6) {
        return None;
    }
    let tol = anicraft_geom::planar::SNAP;

    let mut unchanged: Vec<usize> = Vec::new();
    let mut closed_extra: Vec<RawSub> = Vec::new();
    let mut open: Vec<RawSub> = Vec::new();
    for idx in range {
        let it = dl.items.get(idx)?;
        let into = inv.mul(&it.transform);
        if !into.is_finite() {
            return None;
        }
        let subs = extract_raw_subs(&it.path, &into, idx)?;
        if !subs.iter().any(|s| !s.closed) {
            unchanged.push(idx);
            continue;
        }
        for s in subs {
            if s.closed { closed_extra.push(s); } else { open.push(s); }
        }
    }
    if open.len() < 2 {
        return None;
    }
    open.reverse(); // popped in drawing order
    let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, s) in open.iter().enumerate() {
        let (a, b) = (raw_grid_key(base_xf.apply(s.start), tol), raw_grid_key(base_xf.apply(s.end()), tol));
        grid.entry(a).or_default().push(i);
        if b != a {
            grid.entry(b).or_default().push(i);
        }
    }
    let stage_width = base.stroke.as_ref().map_or(0.0, |st| st.width * scale);
    let device_scale = view_xf.mul(&base_xf).mean_scale();
    let device_width = base.stroke.as_ref().map_or(0.0, |st| st.width * device_scale);
    let opaque = raw_stroke_opaque(base, device_width);
    let view_scale = view_xf.mean_scale();
    let overlap_width = if device_width < MIN_RASTER_WIDTH && view_scale.is_finite() && view_scale > 1e-6 {
        stage_width.max(MIN_RASTER_WIDTH / view_scale)
    } else {
        stage_width
    };
    let flat_tol = (overlap_width * 0.05).clamp(0.1, 0.5);
    let check_overlap = |cand: &RawSub, matched_start: bool, join_pt: Vec2, other_chain_pt: Vec2, cur_flat: &[(Vec2, Vec2)]| -> Option<Vec<(Vec2, Vec2)>> {
        let nf = flatten_raw_segs(cand.start, cand.segs.iter().copied(), &base_xf, flat_tol);
        let other_end = base_xf.apply(if matched_start { cand.end() } else { cand.start });
        let closes = other_end.distance(other_chain_pt) <= tol
            && raw_plain_corner(ends, other_chain_pt)
            && raw_plain_corner(ends, other_end);
        let overlap = if closes {
            raw_remote_overlap(cur_flat, &nf, overlap_width, &[join_pt, other_chain_pt])
        } else {
            raw_remote_overlap(cur_flat, &nf, overlap_width, &[join_pt])
        };
        (!overlap).then_some(nf)
    };
    let mut slots: Vec<Option<RawSub>> = open.into_iter().map(Some).collect();
    let mut joined_any = false;
    let mut paths: Vec<(usize, Path)> = Vec::new();
    while let Some(cur) = slots.iter_mut().rev().find_map(Option::take) {
        while matches!(slots.last(), Some(None)) {
            slots.pop();
        }
        let mut max_item = cur.item;
        let mut start = cur.start;
        let mut segs: VecDeque<RawSeg> = VecDeque::from(cur.segs);
        let mut closed = false;
        let mut chained = false;
        let mut tail_blocked = false;
        let mut head_blocked = false;
        let mut cur_flat: Option<Vec<(Vec2, Vec2)>> = None;
        loop {
            let tail = segs.back().map_or(start, |g| g.end());
            let start_stage = base_xf.apply(start);
            let tail_stage = base_xf.apply(tail);
            // Only close a loop that actually consumed another subpath; a single nearly-closed
            // open path must stay open and keep its caps even when another pair in the run joins.
            if chained
                && segs.len() > 1
                && tail_stage.distance(start_stage) <= tol
                && raw_plain_corner(ends, start_stage)
                && raw_plain_corner(ends, tail_stage)
            {
                closed = true;
                break;
            }
            if !tail_blocked
                && raw_plain_corner(ends, tail_stage)
                && let Some((j, matched_start)) = raw_chain_find(&mut grid, ends, &slots, &base_xf, tail_stage)
            {
                let mut accepted = true;
                let mut cand_flat = None;
                if !opaque && let Some(cand) = slots.get(j).and_then(Option::as_ref) {
                    let cf = cur_flat.get_or_insert_with(|| flatten_raw_segs(start, segs.iter().copied(), &base_xf, flat_tol));
                    match check_overlap(cand, matched_start, tail_stage, start_stage, cf) {
                        Some(nf) => cand_flat = Some(nf),
                        None => {
                            accepted = false;
                            tail_blocked = true;
                        }
                    }
                }
                if accepted && let Some(next) = slots.get_mut(j).and_then(Option::take) {
                    let next = if matched_start { next } else { next.reverse() };
                    max_item = max_item.max(next.item);
                    segs.extend(next.segs);
                    if let (Some(cf), Some(nf)) = (cur_flat.as_mut(), cand_flat) {
                        cf.extend(nf);
                    }
                    chained = true;
                    joined_any = true;
                    continue;
                }
            }
            if !head_blocked
                && raw_plain_corner(ends, start_stage)
                && let Some((j, matched_start)) = raw_chain_find(&mut grid, ends, &slots, &base_xf, start_stage)
            {
                let mut accepted = true;
                let mut cand_flat = None;
                if !opaque && let Some(cand) = slots.get(j).and_then(Option::as_ref) {
                    let cf = cur_flat.get_or_insert_with(|| flatten_raw_segs(start, segs.iter().copied(), &base_xf, flat_tol));
                    match check_overlap(cand, matched_start, start_stage, tail_stage, cf) {
                        Some(nf) => cand_flat = Some(nf),
                        None => {
                            accepted = false;
                            head_blocked = true;
                        }
                    }
                }
                if accepted && let Some(prev) = slots.get_mut(j).and_then(Option::take) {
                    let prev = if matched_start { prev.reverse() } else { prev };
                    max_item = max_item.max(prev.item);
                    start = prev.start;
                    for g in prev.segs.into_iter().rev() {
                        segs.push_front(g);
                    }
                    if let (Some(cf), Some(nf)) = (cur_flat.as_mut(), cand_flat) {
                        cf.extend(nf);
                    }
                    chained = true;
                    joined_any = true;
                    continue;
                }
            }
            break;
        }
        if closed && let Some(last) = segs.back_mut() {
            last.set_end(start);
        }
        let mut path = Path::new();
        path.move_to(start);
        for seg in segs {
            seg.push_to(&mut path);
        }
        if closed {
            path.close();
        }
        paths.push((max_item, path));
    }
    if !joined_any {
        return None;
    }
    let mut built: Vec<(usize, DisplayItem)> = Vec::with_capacity(paths.len() + closed_extra.len() + unchanged.len());
    for (max_item, path) in paths {
        built.push((max_item, DisplayItem { path: Arc::new(path), transform: base_xf, ..base.clone() }));
    }
    for sub in closed_extra {
        built.push((sub.item, DisplayItem { path: Arc::new(sub.to_path()), transform: base_xf, ..base.clone() }));
    }
    for idx in unchanged {
        if let Some(it) = dl.items.get(idx) {
            built.push((idx, it.clone()));
        }
    }
    Some(built)
}

/// `dl` with contiguous solid, uniform-width raw stroke runs (`DisplayItem::merge`) of the same
/// style that meet at plain degree-2 corners chained into continuous paths, so corner joins
/// (Miter, Round, Bevel) render across the separate stroke elements merge drawing splits at
/// corners (ADR 0031, 0046). Borrowed unchanged when no raw strokes join.
pub fn join_raw_strokes(dl: &DisplayList) -> std::borrow::Cow<'_, DisplayList> {
    join_raw_strokes_in_view(dl, &Affine::IDENTITY)
}

/// [`join_raw_strokes`] evaluated under stage-to-device transform `view_xf`, so strokes whose
/// rasterised device width falls below [`MIN_RASTER_WIDTH`] (and are therefore faded to reduced
/// alpha in [`stroke_geometry`]) use the remote-overlap guard.
pub(crate) fn join_raw_strokes_in_view<'a>(dl: &'a DisplayList, view_xf: &Affine) -> std::borrow::Cow<'a, DisplayList> {
    let compatible_pair = |a: &DisplayItem, b: &DisplayItem| {
        is_joinable_raw_stroke(a) && is_joinable_raw_stroke(b) && same_raw_run(a, b) && same_raw_style(a, b) && a.transform.inverse().is_some()
    };
    let mut active_scopes: HashSet<RawRunScope> = HashSet::new();
    for w in dl.items.windows(2) {
        if let (Some(a), Some(b)) = (w.first(), w.get(1)) && compatible_pair(a, b) {
            active_scopes.insert(raw_run_scope(a));
        }
    }
    if active_scopes.is_empty() {
        return std::borrow::Cow::Borrowed(dl);
    }
    // Count endpoint degrees over every raw stroke in the same drawing scope (even across
    // intervening fills or other items), while still only choosing join candidates within a
    // contiguous raw-stroke run.
    let mut scope_ends: HashMap<RawRunScope, HashMap<(i64, i64), Vec<Vec2>>> = HashMap::with_capacity(active_scopes.len());
    for it in &dl.items {
        if is_raw_stroke(it) {
            let scope = raw_run_scope(it);
            if active_scopes.contains(&scope) {
                collect_item_raw_endpoints(it, scope_ends.entry(scope).or_default());
            }
        }
    }
    let mut replacements = Vec::new();
    let mut i = 0;
    while i < dl.items.len() {
        let Some(first) = dl.items.get(i) else { break };
        if !is_raw_stroke(first) {
            i += 1;
            continue;
        }
        let run_start = i;
        let mut run_end = i + 1;
        while let Some(next) = dl.items.get(run_end) {
            if is_raw_stroke(next) && same_raw_run(first, next) { run_end += 1; } else { break; }
        }
        i = run_end;
        let run_slice = dl.items.get(run_start..run_end).unwrap_or_default();
        if !run_slice.windows(2).any(|w| w.first().zip(w.get(1)).is_some_and(|(a, b)| compatible_pair(a, b))) {
            continue;
        }
        let Some(ends) = scope_ends.get(&raw_run_scope(first)) else { continue };
        let mut j = run_start;
        while j < run_end {
            let Some(head) = dl.items.get(j) else { break };
            if !is_joinable_raw_stroke(head) || head.transform.inverse().is_none() {
                j += 1;
                continue;
            }
            let sub_start = j;
            let mut sub_end = j + 1;
            while sub_end < run_end {
                let Some(next) = dl.items.get(sub_end) else { break };
                if is_joinable_raw_stroke(next) && same_raw_style(head, next) { sub_end += 1; } else { break; }
            }
            j = sub_end;
            if sub_end - sub_start >= 2
                && let Some(chained) = chain_raw_run(dl, sub_start..sub_end, ends, view_xf)
            {
                replacements.push((sub_start..sub_end, chained));
            }
        }
    }
    if replacements.is_empty() {
        return std::borrow::Cow::Borrowed(dl);
    }
    let mut items: Vec<DisplayItem> = Vec::with_capacity(dl.items.len());
    let track_links = !dl.links.is_empty();
    let mut remap: Vec<Option<u32>> = if track_links { vec![None; dl.items.len()] } else { Vec::new() };
    let mut cursor = 0;
    for (range, chained) in replacements {
        for idx in cursor..range.start {
            if let Some(slot) = remap.get_mut(idx) {
                *slot = u32::try_from(items.len()).ok();
            }
            if let Some(it) = dl.items.get(idx) {
                items.push(it.clone());
            }
        }
        let mut emitted: Vec<Vec<DisplayItem>> = vec![Vec::new(); range.len()];
        for (slot_idx, it) in chained {
            if let Some(slot) = emitted.get_mut(slot_idx.saturating_sub(range.start)) {
                slot.push(it);
            }
        }
        for (off, list) in emitted.into_iter().enumerate() {
            if let Some(slot) = remap.get_mut(range.start + off) {
                *slot = u32::try_from(items.len()).ok();
            }
            items.extend(list);
        }
        cursor = range.end;
    }
    for idx in cursor..dl.items.len() {
        if let Some(slot) = remap.get_mut(idx) {
            *slot = u32::try_from(items.len()).ok();
        }
        if let Some(it) = dl.items.get(idx) {
            items.push(it.clone());
        }
    }
    let links = if track_links {
        dl.links.iter().filter_map(|(idx, url)| remap.get(*idx as usize).copied().flatten().map(|n| (n, url.clone()))).collect()
    } else {
        Vec::new()
    };
    std::borrow::Cow::Owned(DisplayList { links, clip_modes: dl.clip_modes.clone(), items, clip_parents: dl.clip_parents.clone(), groups: dl.groups.clone(), left_out: dl.left_out })
}

#[cfg(test)]
#[path = "stroke_style_tests.rs"]
mod style_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raster::{Coverage, IRect};
    use anicraft_geom::FillRule;

    /// The winding number of `loops` round `q`.
    pub(super) fn winding(loops: &[Vec<Vec2>], q: Vec2) -> i32 {
        let mut wn = 0i32;
        for l in loops {
            for (a, b) in l.iter().zip(l.iter().cycle().skip(1)) {
                let side = (*b - *a).cross(q - *a);
                if a.y <= q.y && b.y > q.y && side > 0.0 {
                    wn += 1;
                } else if a.y > q.y && b.y <= q.y && side < 0.0 {
                    wn -= 1;
                }
            }
        }
        wn
    }

    fn area(loops: &[Vec<Vec2>]) -> f32 {
        polygons::area(loops) as f32
    }

    fn line(points: &[(f32, f32)], closed: bool) -> Polyline {
        Polyline { points: points.iter().map(|&(x, y)| Vec2::new(x, y)).collect(), closed }
    }

    #[test]
    fn a_bent_line_is_one_outline() {
        // Right 10, down 10, 4 wide: two 10 × 4 bars sharing a 2 × 2 corner, a quarter disc
        // on the outside of the bend and half a disc at each end.
        let pl = line(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)], false);
        let loops = stroke_polygons(&[pl], 2.0);
        assert_eq!(loops.len(), 1);
        let want = 76.0 + 1.25 * std::f32::consts::PI * 4.0;
        assert!((area(&loops) - want).abs() < 0.03 * want, "{} vs {want}", area(&loops));
        assert!(signed_area(&loops[0]) > 0.0, "outer loops are wound the positive way");
        // The inside of the bend is the corner where the two inner edges cross.
        assert!(loops[0].iter().any(|q| q.distance(Vec2::new(8.0, 2.0)) < 1e-4), "{:?}", loops[0]);
        // A closed line is a ring: an outer loop and a hole the other way round.
        let ring = stroke_polygons(&[line(&[(0.0, 0.0), (20.0, 0.0), (20.0, 20.0), (0.0, 20.0)], true)], 2.0);
        assert_eq!(ring.len(), 2);
        assert!(ring.iter().map(|l| signed_area(l)).fold(1.0, |a, b| a * b) < 0.0);
        assert!(winding(&ring, Vec2::new(10.0, 10.0)) == 0 && winding(&ring, Vec2::new(10.0, 0.5)) == 1);
    }

    #[test]
    fn round_joins_on_gentle_bends_stay_slivers_however_far_in_the_view_is() {
        // A gently curving dashed line, wide, as the stage flattens it at 100 % and at 6400 %:
        // every join is a thin wedge on the outside of its bend, never most of a disc, so the
        // stroke covers the same area (in its own units) at every zoom.
        let path = anicraft_geom::Path::from_svg("M30 150C90 20 130 280 200 150C270 20 300 280 370 150").unwrap();
        let taper = anicraft_doc::ProfilePreset::Bulge.profile().unwrap();
        let mut areas: Vec<[f32; 3]> = Vec::new();
        for zoom in [1.0f32, 24.0, 64.0] {
            let lines = path.flatten(&Affine::scale(zoom, zoom), 0.1);
            let hw = 5.5 * zoom;
            let dash = [18.0 * zoom, 9.0 * zoom];
            let outline = Outline { cap: Cap::Butt, join: Join::Round, miter_limit: 3.0, dash: Some((&dash, 0.0)) };
            areas.push([styled_polygons(&lines, hw, &outline, None, None, false), variable_polygons(&lines, hw, &taper, None, false), styled_polygons(&lines, hw, &outline, Some(&taper), None, false)].map(|loops| area(&loops) / (zoom * zoom)));
        }
        let first = areas[0];
        assert!(first.iter().all(|a| *a > 1000.0), "{first:?}");
        for at in &areas[1..] {
            for (a, b) in first.iter().zip(at) {
                // (A disc here is 95 px²; the flattening at 100 % is worth a few.)
                assert!((a - b).abs() < 12.0, "{areas:?}");
            }
        }
    }

    #[test]
    fn degenerate_inputs() {
        assert!(stroke_polygons(&[], 1.0).is_empty());
        let pl = Polyline { points: vec![Vec2::ZERO, Vec2::ZERO], closed: false };
        assert!(stroke_polygons(std::slice::from_ref(&pl), f32::NAN).is_empty());
        // A line with no length is a dot one width across, no NaN.
        let loops = stroke_polygons(&[pl], 1.0);
        assert!(loops.iter().flatten().all(|p| p.is_finite()));
        assert!((area(&loops) - std::f32::consts::PI).abs() < 0.5, "{}", area(&loops));
        // One point is no line.
        assert!(stroke_polygons(&[Polyline { points: vec![Vec2::ZERO], closed: false }], 1.0).is_empty());
    }

    #[test]
    fn variable_outlines_follow_the_profile() {
        let line = Polyline { points: vec![Vec2::new(0.0, 0.0), Vec2::new(100.0, 0.0)], closed: false };
        let taper = anicraft_doc::ProfilePreset::TaperBoth.profile().unwrap();
        let loops = variable_polygons(std::slice::from_ref(&line), 5.0, &taper, None, false);
        assert_eq!(loops.len(), 1);
        assert!(loops[0].len() > 100, "the line is subdivided so the width can vary: {}", loops[0].len());
        assert!(loops.iter().flatten().all(|q| q.is_finite()));
        // Zero width at both ends: no caps, and the outline stays within the profile.
        let half = |x: f32| loops.iter().flatten().filter(|q| (q.x - x).abs() < 0.2).map(|q| q.y.abs()).fold(0.0f32, f32::max);
        assert!(half(0.0) < 0.2 && half(100.0) < 0.2, "{} {}", half(0.0), half(100.0));
        assert!((half(50.0) - 5.0).abs() < 0.05, "full width in the middle: {}", half(50.0));
        // One side only: the other side stays on the line.
        let one = WidthProfile::new(vec![anicraft_doc::WidthPoint::new(0.5, 2.0, 0.0)]).unwrap();
        let loops = variable_polygons(std::slice::from_ref(&line), 5.0, &one, None, false);
        let ys: Vec<f32> = loops.iter().flatten().map(|q| q.y).collect();
        assert!(ys.iter().all(|&y| y < 1e-3) && ys.iter().any(|&y| (y + 10.0).abs() < 1e-3), "heading +x on a y-down screen, the left is -y");
        // Mirrored transforms swap the sides.
        let flipped = variable_polygons(&[line], 5.0, &one, None, true);
        assert!(flipped.iter().flatten().all(|q| q.y > -1e-3));
        // Degenerate input is safe.
        assert!(variable_polygons(&[], 1.0, &one, None, false).is_empty());
        assert!(variable_polygons(&[Polyline { points: vec![Vec2::ZERO; 3], closed: true }], 1.0, &one, None, false).iter().flatten().all(|q| q.is_finite()));
        assert!(variable_polygons(&[Polyline { points: vec![Vec2::ZERO, Vec2::new(f32::INFINITY, 0.0)], closed: false }], f32::NAN, &one, None, false).is_empty());
    }

    #[test]
    fn a_sharp_bend_gets_a_join_on_its_outer_side() {
        // Right 20, then down 10: the outer side of the corner is up and to the right.
        let pl = Polyline { points: vec![Vec2::new(0.0, 0.0), Vec2::new(20.0, 0.0), Vec2::new(20.0, 10.0)], closed: false };
        let fat = WidthProfile::new(vec![anicraft_doc::WidthPoint::new(0.0, 2.0, 2.0)]).unwrap();
        let loops = variable_polygons(&[pl], 2.0, &fat, None, false);
        // The corner's outer quarter (x > 20, y < 0) is covered by the join, within radius 4.
        let reach = loops.iter().flatten().filter(|q| q.x > 20.0 && q.y < 0.0).map(|q| q.distance(Vec2::new(20.0, 0.0))).fold(0.0f32, f32::max);
        assert!((reach - 4.0).abs() < 0.01, "{reach}");
        assert!(winding(&loops, Vec2::new(22.0, -2.0)) == 1 && winding(&loops, Vec2::new(23.5, -3.5)) == 0);
    }

    /// Lines that overlap themselves every way a drawn line does, on a 400 × 300 stage.
    fn tangles() -> Vec<(&'static str, Vec<Polyline>)> {
        let flat = |d: &str| anicraft_geom::Path::from_svg(d).unwrap().flatten(&Affine::IDENTITY, 0.1);
        vec![
            ("a wide wave", flat("M30 150C90 20 130 280 200 150C270 20 300 280 370 150")),
            ("a hairpin tighter than the line is wide", flat("M60 80C330 80 330 104 60 104")),
            ("a loop that crosses itself", flat("M120 260C330 60 80 60 290 260")),
            ("a knot", flat("M50 50C300 50 50 280 300 280C50 280 300 50 50 250")),
            ("a zigzag", flat("M40 250L120 60L170 240L230 80L260 230L350 50")),
            ("there and back", flat("M60 150L340 150L60 150")),
            ("two lines that cross", flat("M40 40L360 260M40 260L360 40")),
            ("a ring smaller than the line is wide", anicraft_geom::Path::ellipse(200.0, 150.0, 10.0, 10.0).flatten(&Affine::IDENTITY, 0.1)),
            ("a closed outline", flat("M60 50L340 50L340 250L60 250Z")),
        ]
    }

    /// Every way of drawing them: half-width, outline and profile.
    fn styles() -> Vec<(&'static str, f32, Outline<'static>, Option<WidthProfile>)> {
        const DASH: [f32; 2] = [30.0, 14.0];
        const CLOSE: [f32; 2] = [20.0, 8.0];
        const DOTS: [f32; 2] = [0.0, 9.0];
        let o = |cap, join| Outline { cap, join, ..Outline::ROUND };
        vec![
            ("round", 7.0, Outline::ROUND, None),
            ("thin", 0.5, Outline::ROUND, None),
            ("miters, flat ends", 6.0, Outline { miter_limit: 10.0, ..o(Cap::Butt, Join::Miter) }, None),
            ("bevels, square ends", 8.0, o(Cap::Square, Join::Bevel), None),
            ("dashes", 5.0, Outline { dash: Some((&DASH, 3.0)), ..o(Cap::Butt, Join::Round) }, None),
            ("dashes whose round caps overlap", 7.0, Outline { dash: Some((&CLOSE, 0.0)), ..Outline::ROUND }, None),
            ("dots that overlap", 6.0, Outline { dash: Some((&DOTS, 0.0)), ..Outline::ROUND }, None),
            ("a bulge", 9.0, Outline::ROUND, anicraft_doc::ProfilePreset::Bulge.profile()),
            ("a dashed taper", 8.0, Outline { dash: Some((&DASH, 0.0)), ..o(Cap::Square, Join::Miter) }, anicraft_doc::ProfilePreset::TaperEnd.profile()),
        ]
    }

    #[test]
    fn a_stroke_s_loops_never_overlap_and_cover_what_its_pieces_cover() {
        for (what, lines) in tangles() {
            for (how, hw, outline, profile) in styles() {
                let (pieces, _) = piece_loops(&lines, hw, &outline, profile.as_ref(), None, false);
                let loops = styled_polygons(&lines, hw, &outline, profile.as_ref(), None, false);
                assert!(!loops.is_empty() && loops.iter().flatten().all(|q| q.is_finite()), "{what}, {how}");
                let (mut inside, mut wrong, mut deepest) = (0usize, 0usize, 0i32);
                for j in 0..200 {
                    for i in 0..266 {
                        // (Off the whole and half pixels the lines' points are on.)
                        let q = Vec2::new(i as f32 * 1.5003 + 0.1234, j as f32 * 1.5003 + 0.2345);
                        let (raw, w) = (winding(&pieces, q), winding(&loops, q));
                        assert!(raw >= 0, "{what}, {how}: pieces all wind one way, so they never cancel ({raw} at {q:?})");
                        assert!((0..=1).contains(&w), "{what}, {how}: covered {w} times at {q:?}");
                        inside += usize::from(raw > 0);
                        wrong += usize::from((raw > 0) != (w > 0));
                        deepest = deepest.max(raw);
                    }
                }
                assert!(inside > 10, "{what}, {how}: {inside}");
                assert_eq!(wrong, 0, "{what}, {how}: the loops cover what the pieces cover (of {inside} samples inside)");
                if what == "a knot" && how == "round" {
                    assert!(deepest >= 2, "the test does overlap");
                }
            }
        }
    }

    /// The coverage of `loops` over a 400 × 300 surface as the renderers work it out (signed
    /// areas summed along each row), under `rule`.
    fn coverage(loops: &[Vec<Vec2>], rule: FillRule) -> Vec<f32> {
        let (w, h) = (400usize, 300usize);
        let mut cov = Coverage::new(IRect { x0: 0, y0: 0, x1: w as i32, y1: h as i32 });
        for l in loops {
            cov.add_polygon(l);
        }
        let mut out = vec![0.0; w * h];
        for (y, row) in out.chunks_mut(w).enumerate() {
            cov.resolve_row(y, rule, row);
        }
        out
    }

    #[test]
    fn the_renderer_s_sum_is_the_stroke_s_true_coverage() {
        let mut worst_before = 0.0f32;
        for (what, lines) in tangles() {
            for (how, hw, outline, profile) in styles() {
                let loops = styled_polygons(&lines, hw, &outline, profile.as_ref(), None, false);
                let drawn = coverage(&loops, FillRule::NonZero);
                // 1. The whole picture: the pixels add up to the outline's area.
                let (sum, want) = (drawn.iter().map(|c| f64::from(*c)).sum::<f64>(), polygons::area(&loops));
                assert!((sum - want).abs() < 0.02 + want * 1e-5, "{what}, {how}: pixels {sum}, area {want}");
                // 2. Every pixel: loops that don't overlap are filled alike by both rules; any
                //    overlap inside a pixel would show as a difference there.
                let even_odd = coverage(&loops, FillRule::EvenOdd);
                let apart = drawn.iter().zip(&even_odd).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
                assert!(apart < 1.0 / 255.0, "{what}, {how}: {apart}");
                // 3. Against the truth: each pixel sampled 16 × 16 times against the pieces.
                //    (Checked for the plain styles; a sample grid is good to a sixteenth.)
                if how == "round" || how == "dashes whose round caps overlap" {
                    let (pieces, _) = piece_loops(&lines, hw, &outline, profile.as_ref(), None, false);
                    let before = coverage(&pieces, FillRule::NonZero);
                    for y in (0..300).step_by(7) {
                        for x in 0..400 {
                            let c = drawn[y * 400 + x];
                            if c <= 0.0 || c >= 1.0 {
                                continue;
                            }
                            let hits = (0..256).filter(|k| winding(&pieces, Vec2::new(x as f32 + ((k % 16) as f32 + 0.5) / 16.0, y as f32 + ((k / 16) as f32 + 0.5) / 16.0)) != 0).count();
                            let truth = hits as f32 / 256.0;
                            assert!((c - truth).abs() < 0.07, "{what}, {how}: pixel ({x}, {y}) drawn {c}, sampled {truth}");
                            worst_before = worst_before.max(before[y * 400 + x] - truth);
                        }
                    }
                }
            }
        }
        // What this fixes: adding the pieces up as they are over-covers edge pixels by up to
        // about half a pixel.
        assert!(worst_before > 0.25, "{worst_before}");
    }

    /// `path` stroked with `st` in 50 % black on a white 400 × 300 stage: each pixel's ink.
    fn drawn(path: &str, st: Stroke) -> Vec<f32> {
        let item = DisplayItem {
            path: std::sync::Arc::new(Path::from_svg(path).unwrap()),
            transform: Affine::IDENTITY,
            fill: None,
            stroke: Some(st),
            fill_rule: FillRule::NonZero,
            color: anicraft_doc::ColorTransform::IDENTITY,
            layer: anicraft_doc::LayerId(0),
            element: anicraft_doc::ElementId(0),
            outline: None,
            clip: None,
            masks: None,
            guide: false,
            group: None,
            image: None,
            smooth: true,
            warp: None,
            merge: false,
        };
        let dl = DisplayList { left_out: false, links: Vec::new(), clip_modes: Vec::new(), items: vec![item], clip_parents: Vec::new(), groups: Vec::new() };
        let view = crate::View { transform: Affine::IDENTITY, width: 400, height: 300, background: Some(anicraft_doc::Rgba::WHITE) };
        let (pm, _) = crate::render(&dl, &view).unwrap();
        pm.data.iter().map(|p| 1.0 - p[0]).collect()
    }

    #[test]
    fn a_see_through_line_that_crosses_itself_is_no_darker_where_it_does() {
        let half = anicraft_doc::Rgba::new(0.0, 0.0, 0.0, 0.5);
        let dashes = anicraft_doc::StrokeStyle { dash: anicraft_doc::Dash::dashed(20.0, 8.0), ..Default::default() };
        for (what, path, st) in [
            ("a knot", "M50 50C300 50 50 280 300 280C50 280 300 50 50 250", Stroke::new(half, 14.0)),
            ("a hairpin", "M60 80C330 80 330 104 60 104", Stroke::new(half, 30.0)),
            ("dashes whose round caps overlap", "M30 150C90 20 130 280 200 150C270 20 300 280 370 150", Stroke::new(half, 14.0).with_style(dashes)),
        ] {
            let ink = drawn(path, st);
            let (darkest, full) = (ink.iter().copied().fold(0.0f32, f32::max), ink.iter().filter(|v| (**v - 0.5).abs() < 1e-3).count());
            assert!(darkest <= 0.5 + 1e-3, "{what}: {darkest}");
            assert!(full > 1000, "{what}: {full} pixels fully inside");
        }
    }

    fn raw_stroke_item(path: &str, color: anicraft_doc::Rgba, width: f32, join: Join) -> DisplayItem {
        let st = Stroke::new(color, width).with_style(anicraft_doc::StrokeStyle { cap: Cap::Butt, join, ..Default::default() });
        DisplayItem {
            path: Arc::new(Path::from_svg(path).unwrap()),
            transform: Affine::IDENTITY,
            fill: None,
            stroke: Some(st),
            fill_rule: FillRule::NonZero,
            color: anicraft_doc::ColorTransform::IDENTITY,
            layer: anicraft_doc::LayerId(0),
            element: anicraft_doc::ElementId(0),
            outline: None,
            clip: None,
            masks: None,
            guide: false,
            group: None,
            image: None,
            smooth: true,
            warp: None,
            merge: true,
        }
    }

    fn raw_dl(items: Vec<DisplayItem>) -> DisplayList {
        DisplayList { left_out: false, links: Vec::new(), clip_modes: Vec::new(), items, clip_parents: Vec::new(), groups: Vec::new() }
    }

    #[test]
    fn raw_join_counts_branches_across_styles_at_three_way_junction() {
        // Two black strokes and one red stroke meet at (100, 100): degree 3, so the black strokes
        // must not join into a corner there.
        let black = anicraft_doc::Rgba::BLACK;
        let red = anicraft_doc::Rgba::new(1.0, 0.0, 0.0, 1.0);
        let dl = raw_dl(vec![
            raw_stroke_item("M 0 100 L 100 100", black, 10.0, Join::Miter),
            raw_stroke_item("M 100 100 L 100 200", black, 10.0, Join::Miter),
            raw_stroke_item("M 100 100 L 200 100", red, 10.0, Join::Miter),
        ]);
        let joined = join_raw_strokes(&dl);
        assert!(matches!(joined, std::borrow::Cow::Borrowed(_)), "three-way junction with a red branch must leave the black pair unjoined");
        assert_eq!(joined.items.len(), 3);
    }

    #[test]
    fn raw_join_leaves_triangle_open_at_spur_and_closes_plain_triangle() {
        let black = anicraft_doc::Rgba::BLACK;
        let has_close = |dl: &DisplayList| dl.items.iter().any(|it| it.path.cmds.iter().any(|c| matches!(c, PathCmd::Close)));
        // Plain triangle: every vertex has degree 2, so all three sides chain and close.
        let plain = raw_dl(vec![
            raw_stroke_item("M 40 40 L 140 40", black, 12.0, Join::Miter),
            raw_stroke_item("M 140 40 L 90 140", black, 12.0, Join::Miter),
            raw_stroke_item("M 90 140 L 40 40", black, 12.0, Join::Miter),
        ]);
        let joined_plain = join_raw_strokes(&plain);
        assert_eq!(joined_plain.items.len(), 1);
        assert!(has_close(&joined_plain), "triangle without a spur should close");

        // Triangle with a spur at (40, 40): the other two corners chain, but closure at the
        // three-way spur vertex must not happen.
        let spurred = raw_dl(vec![
            raw_stroke_item("M 40 40 L 140 40", black, 12.0, Join::Miter),
            raw_stroke_item("M 140 40 L 90 140", black, 12.0, Join::Miter),
            raw_stroke_item("M 90 140 L 40 40", black, 12.0, Join::Miter),
            raw_stroke_item("M 40 40 L 10 10", black, 12.0, Join::Miter),
        ]);
        let joined_spurred = join_raw_strokes(&spurred);
        assert!(!has_close(&joined_spurred), "triangle with a spur must not close at the three-way vertex");
        assert_eq!(joined_spurred.items.len(), 2, "three sides chain into one open path, spur stays separate");
    }

    #[test]
    fn raw_join_checks_candidate_endpoint_degree_both_directions() {
        // Three endpoints a, b, c on one line: dist(a, b) = 0.15 <= SNAP, dist(b, c) = 0.15 <= SNAP,
        // dist(a, c) = 0.30 > SNAP. Count is 2 at a and c, 3 at b: no pair may join in either
        // drawing order or stroke orientation (covers tail-append and head-prepend).
        let black = anicraft_doc::Rgba::BLACK;
        let a_tail = raw_stroke_item("M 20 100 L 99.85 100", black, 10.0, Join::Miter);
        let a_head = raw_stroke_item("M 99.85 100 L 20 100", black, 10.0, Join::Miter);
        let b_sub = raw_stroke_item("M 100 100 L 100 180", black, 10.0, Join::Miter);
        let c_head = raw_stroke_item("M 100.15 100 L 180 100", black, 10.0, Join::Miter);
        let c_tail = raw_stroke_item("M 180 100 L 100.15 100", black, 10.0, Join::Miter);

        for (label, items) in [
            ("tail-append a-b-c", vec![a_tail.clone(), b_sub.clone(), c_head.clone()]),
            ("tail-append c-b-a", vec![c_tail.clone(), b_sub.clone(), a_head.clone()]),
            ("head-prepend a-b-c", vec![a_head.clone(), b_sub.clone(), c_head.clone()]),
            ("head-prepend c-b-a", vec![c_head.clone(), b_sub.clone(), a_tail.clone()]),
        ] {
            let dl = raw_dl(items);
            let joined = join_raw_strokes(&dl);
            assert!(matches!(joined, std::borrow::Cow::Borrowed(_)), "{label}: must not join into degree-3 endpoint b");
            assert_eq!(joined.items.len(), 3, "{label}");
        }
    }

    #[test]
    fn raw_join_preserves_paint_order_across_intervening_stroke() {
        // Red A and Red C meet at (100, 50), with Blue B between them crossing A at (50, 50).
        // Blue B must stay on top of Red A in CPU, GPU scene, and SVG output.
        let red = anicraft_doc::Rgba::new(1.0, 0.0, 0.0, 1.0);
        let blue = anicraft_doc::Rgba::new(0.0, 0.0, 1.0, 1.0);
        let dl = raw_dl(vec![
            raw_stroke_item("M 10 50 L 100 50", red, 12.0, Join::Miter),
            raw_stroke_item("M 50 10 L 50 90", blue, 12.0, Join::Miter),
            raw_stroke_item("M 100 50 L 100 90", red, 12.0, Join::Miter),
        ]);
        let joined = join_raw_strokes(&dl);
        assert!(matches!(joined, std::borrow::Cow::Borrowed(_)), "must not chain across an intervening stroke of another style");
        // CPU oracle: crossing pixel (50, 50) is blue, not red.
        let view = crate::View { transform: Affine::IDENTITY, width: 120, height: 120, background: Some(anicraft_doc::Rgba::WHITE) };
        let (pm, _) = crate::render(&dl, &view).unwrap();
        let px = pm.pixel(50, 50).unwrap();
        assert!(px[2] > px[0], "CPU crossing must show blue on top of red: {px:?}");
        // GPU scene: blue fill comes after the red fill that covers (50, 50).
        let (scene, _) = crate::prepare_scene(&dl, &view).unwrap();
        let top_at_crossing = scene.fills.iter().rposition(|f| f.bounds.x0 <= 50 && 50 < f.bounds.x1 && f.bounds.y0 <= 50 && 50 < f.bounds.y1).unwrap();
        assert!(matches!(scene.fills[top_at_crossing].shader, crate::paint::Shader::Solid(c) if c[2] > c[0]), "GPU scene must keep blue over red at crossing");
        // SVG export: blue path appears after the horizontal red path.
        let svg = crate::svg::to_svg(&dl, &anicraft_doc::Stage { width: 120, height: 120, fps: 24.0, background: anicraft_doc::Rgba::WHITE });
        let red_a = svg.find("M10 50").unwrap();
        let blue_b = svg.find("M50 10").unwrap();
        assert!(red_a < blue_b, "SVG must paint red A before blue B: {svg}");
    }

    #[test]
    fn raw_join_borrows_when_distant_and_remaps_links_when_joined() {
        let black = anicraft_doc::Rgba::BLACK;
        let distant = raw_dl(vec![
            raw_stroke_item("M 0 0 L 20 0", black, 6.0, Join::Miter),
            raw_stroke_item("M 100 0 L 120 0", black, 6.0, Join::Miter),
        ]);
        assert!(matches!(join_raw_strokes(&distant), std::borrow::Cow::Borrowed(_)));

        let mut linked_item = raw_stroke_item("M 200 200 L 240 200", black, 6.0, Join::Miter);
        linked_item.merge = false;
        let mut with_link = raw_dl(vec![
            raw_stroke_item("M 0 0 L 50 0", black, 6.0, Join::Miter),
            raw_stroke_item("M 50 0 L 50 50", black, 6.0, Join::Miter),
            linked_item,
        ]);
        with_link.links.push((2, Arc::from("https://example.com")));
        let joined = join_raw_strokes(&with_link);
        assert_eq!(joined.items.len(), 2);
        assert_eq!(joined.links.len(), 1);
        assert_eq!(joined.links[0].0, 1, "link index shifts from 2 to 1 after the first two strokes merge");
    }

    #[test]
    fn raw_join_keeps_nearly_closed_open_path_open_alongside_joined_pair() {
        // A single open path whose endpoints sit within SNAP (0.1 <= 0.2) alongside a distant
        // right-angle pair in the same style run: the distant pair joins, while the open path
        // must stay open and keep its caps (its outline matches outlining it alone).
        let black = anicraft_doc::Rgba::BLACK;
        let make = |path: &str| {
            let st = Stroke::new(black, 10.0).with_style(anicraft_doc::StrokeStyle { cap: Cap::Square, join: Join::Miter, ..Default::default() });
            DisplayItem { stroke: Some(st), ..raw_stroke_item(path, black, 10.0, Join::Miter) }
        };
        let nearly_closed = make("M 20 20 L 60 20 L 60 60 L 20 20.1");
        let dl = raw_dl(vec![
            nearly_closed.clone(),
            make("M 200 20 L 240 20"),
            make("M 240 20 L 240 60"),
        ]);
        let joined = join_raw_strokes(&dl);
        assert_eq!(joined.items.len(), 2, "distant right-angle pair joins into one item");
        let kept = &joined.items[0];
        assert!(!kept.path.cmds.iter().any(|c| matches!(c, PathCmd::Close)), "nearly-closed open path must not be closed");
        let st = nearly_closed.stroke.as_ref().unwrap();
        let alone = stroke_outline(&nearly_closed.path, &nearly_closed.transform, st, crate::TOLERANCE);
        let after = stroke_outline(&kept.path, &kept.transform, kept.stroke.as_ref().unwrap(), crate::TOLERANCE);
        assert_eq!(after.cmds, alone.cmds, "unjoined open path must keep the exact same caps and outline");
    }

    #[test]
    fn raw_join_preserves_semi_transparent_overlap_away_from_corner() {
        let half = anicraft_doc::Rgba::new(0.0, 0.0, 0.0, 0.5);
        let black = anicraft_doc::Rgba::BLACK;
        // Two strokes sharing (20, 20) and crossing again at (70, 70).
        let a_path = "M 20 20 L 120 20 L 20 120";
        let b_path = "M 20 20 L 20 70 L 120 70";

        let semi_cross = raw_dl(vec![
            raw_stroke_item(a_path, half, 12.0, Join::Miter),
            raw_stroke_item(b_path, half, 12.0, Join::Miter),
        ]);
        let joined_semi = join_raw_strokes(&semi_cross);
        assert!(matches!(joined_semi, std::borrow::Cow::Borrowed(_)), "semi-transparent strokes that cross away from the corner must stay separate");
        assert_eq!(joined_semi.items.len(), 2);
        // Rendered crossing pixel (70, 70) must be darker than a single-stroke pixel (80, 20).
        let view = crate::View { transform: Affine::IDENTITY, width: 140, height: 140, background: Some(anicraft_doc::Rgba::WHITE) };
        let (pm, _) = crate::render(&semi_cross, &view).unwrap();
        let single = pm.pixel(80, 20).unwrap();
        let crossing_px = pm.pixel(70, 70).unwrap();
        assert!(crossing_px[0] < single[0] - 0.15, "crossing must double-blend darker than a single stroke: cross={crossing_px:?}, single={single:?}");

        // Same crossing pair with opaque stroke colour but semi-transparent ColorTransform also stays separate.
        let mut ct_a = raw_stroke_item(a_path, black, 12.0, Join::Miter);
        let mut ct_b = raw_stroke_item(b_path, black, 12.0, Join::Miter);
        ct_a.color = anicraft_doc::ColorTransform::alpha(0.5);
        ct_b.color = anicraft_doc::ColorTransform::alpha(0.5);
        assert!(matches!(join_raw_strokes(&raw_dl(vec![ct_a, ct_b])), std::borrow::Cow::Borrowed(_)), "semi-transparent ColorTransform must also keep crossing strokes separate");

        // Same crossing pair when fully opaque still joins.
        let opaque_cross = raw_dl(vec![
            raw_stroke_item(a_path, black, 12.0, Join::Miter),
            raw_stroke_item(b_path, black, 12.0, Join::Miter),
        ]);
        let joined_opaque = join_raw_strokes(&opaque_cross);
        assert_eq!(joined_opaque.items.len(), 1, "opaque crossing pair still joins");

        // Semi-transparent L-corner with no remote overlap still joins.
        let semi_l = raw_dl(vec![
            raw_stroke_item("M 20 20 L 100 20", half, 12.0, Join::Miter),
            raw_stroke_item("M 100 20 L 100 100", half, 12.0, Join::Miter),
        ]);
        let joined_l = join_raw_strokes(&semi_l);
        assert_eq!(joined_l.items.len(), 1, "semi-transparent L-corner without remote overlap still joins");
    }

    #[test]
    fn raw_join_counts_third_branch_across_intervening_fill() {
        // Stroke A and stroke B meet at (100, 100), followed by a distant fill-only item, then
        // stroke C meeting the same corner on the same drawing scope: degree is 3, so A and B
        // must not join.
        let black = anicraft_doc::Rgba::BLACK;
        let fill_only = DisplayItem {
            path: Arc::new(Path::from_svg("M 300 300 L 340 300 L 340 340 Z").unwrap()),
            fill: Some(anicraft_doc::Paint::Solid(anicraft_doc::Rgba::new(1.0, 0.0, 0.0, 1.0))),
            stroke: None,
            ..raw_stroke_item("M 0 0 L 1 0", black, 10.0, Join::Miter)
        };
        let dl = raw_dl(vec![
            raw_stroke_item("M 0 100 L 100 100", black, 10.0, Join::Miter),
            raw_stroke_item("M 100 100 L 100 200", black, 10.0, Join::Miter),
            fill_only.clone(),
            raw_stroke_item("M 100 100 L 200 100", black, 10.0, Join::Miter),
        ]);
        let joined = join_raw_strokes(&dl);
        assert!(matches!(joined, std::borrow::Cow::Borrowed(_)), "third branch after an intervening fill must prevent joining A and B");
        assert_eq!(joined.items.len(), 4);

        // If the third stroke is on a different layer (different drawing scope), A and B still join.
        let mut other_layer_c = raw_stroke_item("M 100 100 L 200 100", black, 10.0, Join::Miter);
        other_layer_c.layer = anicraft_doc::LayerId(1);
        let dl_other = raw_dl(vec![
            raw_stroke_item("M 0 100 L 100 100", black, 10.0, Join::Miter),
            raw_stroke_item("M 100 100 L 100 200", black, 10.0, Join::Miter),
            fill_only,
            other_layer_c,
        ]);
        let joined_other = join_raw_strokes(&dl_other);
        assert_eq!(joined_other.items.len(), 3, "stroke on another layer must not block joining A and B");
    }

    #[test]
    fn raw_join_guards_subpixel_thin_strokes_at_remote_crossings() {
        let black = anicraft_doc::Rgba::BLACK;
        // Two opaque 0.5 px strokes sharing (20.5, 20.5) and crossing at (70.5, 70.5). The
        // rasteriser widens strokes below 1 device pixel to 1 px at reduced alpha (50%), so
        // separate strokes composite to 75% at the crossing whereas joining would paint 50%.
        let a_path = "M 20.5 20.5 L 120.5 20.5 L 120.5 70.5 L 20.5 70.5";
        let b_path = "M 20.5 20.5 L 20.5 120.5 L 70.5 120.5 L 70.5 40.5";
        let thin_cross = raw_dl(vec![
            raw_stroke_item(a_path, black, 0.5, Join::Miter),
            raw_stroke_item(b_path, black, 0.5, Join::Miter),
        ]);
        let joined = join_raw_strokes(&thin_cross);
        assert!(matches!(joined, std::borrow::Cow::Borrowed(_)), "thin crossing strokes must stay separate");
        assert_eq!(joined.items.len(), 2);

        let view = crate::View { transform: Affine::IDENTITY, width: 140, height: 140, background: Some(anicraft_doc::Rgba::WHITE) };
        let (pm, _) = crate::render(&thin_cross, &view).unwrap();
        let single = pm.pixel(80, 20).unwrap();
        let crossing_px = pm.pixel(70, 70).unwrap();
        assert!(crossing_px[0] < single[0] - 0.15, "thin crossing must double-blend darker than a single stroke: cross={crossing_px:?}, single={single:?}");
        let (scene, _) = crate::prepare_scene(&thin_cross, &view).unwrap();
        let covers_crossing = scene.fills.iter().filter(|f| f.bounds.x0 <= 70 && 70 < f.bounds.x1 && f.bounds.y0 <= 70 && 70 < f.bounds.y1).count();
        assert_eq!(covers_crossing, 2, "GPU scene must keep separate fills over the crossing");

        // A thin 0.5 px L-shape with no remote overlap still joins.
        let thin_l = raw_dl(vec![
            raw_stroke_item("M 20.5 20.5 L 100.5 20.5", black, 0.5, Join::Miter),
            raw_stroke_item("M 100.5 20.5 L 100.5 100.5", black, 0.5, Join::Miter),
        ]);
        let joined_l = join_raw_strokes(&thin_l);
        assert_eq!(joined_l.items.len(), 1, "thin L-corner without remote overlap still joins");

        // Parallel 0.5 px legs 0.7 px apart away from the corner overlap in their widened 1 px
        // raster footprint, so they must also stay separate.
        let thin_near = raw_dl(vec![
            raw_stroke_item("M 20.5 20.5 L 80.5 20.5", black, 0.5, Join::Miter),
            raw_stroke_item("M 20.5 20.5 L 20.5 21.2 L 80.5 21.2", black, 0.5, Join::Miter),
        ]);
        assert!(matches!(join_raw_strokes(&thin_near), std::borrow::Cow::Borrowed(_)), "thin legs within the widened 1 px footprint must stay separate");

        // Zoomed-out view: 2 px stage strokes drawn at 0.25x have effective device width 0.5 px,
        // so a crossing pair stays separate while an L-corner still joins.
        let zoom_out = Affine::scale(0.25, 0.25);
        let zoom_view = crate::View { transform: zoom_out, width: 64, height: 64, background: Some(anicraft_doc::Rgba::WHITE) };
        let zoomed_cross = raw_dl(vec![
            raw_stroke_item("M 42 42 L 202 42 L 202 122 L 42 122", black, 2.0, Join::Miter),
            raw_stroke_item("M 42 42 L 42 202 L 122 202 L 122 82", black, 2.0, Join::Miter),
        ]);
        assert!(matches!(join_raw_strokes_in_view(&zoomed_cross, &zoom_out), std::borrow::Cow::Borrowed(_)), "zoomed-out sub-pixel crossing must stay separate");
        let (zpm, _) = crate::render(&zoomed_cross, &zoom_view).unwrap();
        let zsingle = zpm.pixel(30, 10).unwrap();
        let zcross = zpm.pixel(30, 30).unwrap();
        assert!(zcross[0] < zsingle[0] - 0.15, "zoomed-out crossing must double-blend darker: cross={zcross:?}, single={zsingle:?}");

        let zoomed_l = raw_dl(vec![
            raw_stroke_item("M 42 42 L 202 42", black, 2.0, Join::Miter),
            raw_stroke_item("M 202 42 L 202 202", black, 2.0, Join::Miter),
        ]);
        assert_eq!(join_raw_strokes_in_view(&zoomed_l, &zoom_out).items.len(), 1, "zoomed-out thin L-corner without remote overlap still joins");
    }

    #[test]
    fn raw_join_keeps_dashed_and_dotted_strokes_separate_while_joining_solid() {
        let black = anicraft_doc::Rgba::BLACK;
        let with_dash = |path: &str, dash: Option<anicraft_doc::Dash>| {
            let st = Stroke::new(black, 6.0).with_style(anicraft_doc::StrokeStyle { cap: Cap::Butt, join: Join::Miter, dash, ..Default::default() });
            DisplayItem { stroke: Some(st), ..raw_stroke_item(path, black, 6.0, Join::Miter) }
        };
        // 48-unit horizontal leg + vertical leg with 20-on / 8-off butt dashes: 32.5 units down
        // the vertical leg has phase 4.5 (painted) when separate, vs 24.5 (gap) if joined.
        let dash = anicraft_doc::Dash::dashed(20.0, 8.0);
        let dashed = raw_dl(vec![
            with_dash("M 20 20 L 68 20", dash.clone()),
            with_dash("M 68 20 L 68 80", dash.clone()),
        ]);
        let joined_dashed = join_raw_strokes(&dashed);
        assert!(matches!(joined_dashed, std::borrow::Cow::Borrowed(_)), "dashed strokes meeting at a corner must stay separate");
        assert_eq!(joined_dashed.items.len(), 2);
        let view = crate::View { transform: Affine::IDENTITY, width: 100, height: 100, background: Some(anicraft_doc::Rgba::WHITE) };
        let (pm, _) = crate::render(&dashed, &view).unwrap();
        let vertical_dash_px = pm.pixel(68, 52).unwrap();
        assert!(vertical_dash_px[0] < 0.2, "second leg must keep its own dash phase (painted at y=52.5): {vertical_dash_px:?}");

        // Dotted strokes also stay separate.
        let dots = anicraft_doc::Dash::dotted(6.0);
        let dotted = raw_dl(vec![
            with_dash("M 20 20 L 68 20", dots.clone()),
            with_dash("M 68 20 L 68 80", dots),
        ]);
        let joined_dotted = join_raw_strokes(&dotted);
        assert!(matches!(joined_dotted, std::borrow::Cow::Borrowed(_)), "dotted strokes meeting at a corner must stay separate");
        assert_eq!(joined_dotted.items.len(), 2);

        // Solid strokes on the same corner still join.
        let solid = raw_dl(vec![
            with_dash("M 20 20 L 68 20", None),
            with_dash("M 68 20 L 68 80", None),
        ]);
        let joined_solid = join_raw_strokes(&solid);
        assert_eq!(joined_solid.items.len(), 1, "solid strokes meeting at a corner still join");

        // A dashed third branch at a solid corner still counts toward endpoint degree.
        let solid_with_dashed_spur = raw_dl(vec![
            with_dash("M 20 20 L 68 20", None),
            with_dash("M 68 20 L 68 80", None),
            with_dash("M 68 20 L 96 20", dash),
        ]);
        assert!(matches!(join_raw_strokes(&solid_with_dashed_spur), std::borrow::Cow::Borrowed(_)), "dashed branch at a three-way junction must still keep solid legs unjoined");
    }
}
