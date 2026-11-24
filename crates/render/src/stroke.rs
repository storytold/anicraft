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
use anicraft_geom::{Affine, Path, Polyline, Vec2, polygons};

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
    if raster && widest < 1.0 {
        alpha *= widest;
        pw /= widest;
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
            let paint = st.paint.clone().unwrap_or(anicraft_doc::Paint::Solid(st.color)).transformed(&it.transform);
            let keep_img = matches!(paint, anicraft_doc::Paint::Bitmap { .. });
            items.push(DisplayItem {
                path: std::sync::Arc::new(outline),
                transform: Affine::IDENTITY,
                fill: Some(paint),
                stroke: None,
                fill_rule: anicraft_geom::FillRule::NonZero,
                image: if keep_img { it.image.clone() } else { None },
                ..it.clone()
            });
        }
    }
    std::borrow::Cow::Owned(DisplayList { links: Vec::new(), clip_modes: dl.clip_modes.clone(), items, clip_parents: dl.clip_parents.clone(), groups: dl.groups.clone(), left_out: dl.left_out })
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
}
