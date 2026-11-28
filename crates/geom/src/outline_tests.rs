//! Outlines of strokes and offsets of fills (ADR 0109), against areas known in closed form and
//! against the distance to the path.

use super::*;
use std::f32::consts::PI;

/// The area an even-odd path of loops that don't cross fills: each loop's area, counted
/// negative when it lies inside an odd number of the others (a hole).
fn area(path: &Path) -> f32 {
    let loops = path.flatten(&Affine::IDENTITY, 0.002);
    let signed = |pts: &[Vec2]| {
        let n = pts.len();
        (0..n).map(|i| pts[i].cross(pts[(i + 1) % n])).sum::<f32>() * 0.5
    };
    let inside = |p: Vec2, pts: &[Vec2]| {
        let n = pts.len();
        let mut odd = false;
        for i in 0..n {
            let (a, b) = (pts[i], pts[(i + 1) % n]);
            if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x) {
                odd = !odd;
            }
        }
        odd
    };
    let mut total = 0.0;
    for (i, l) in loops.iter().enumerate() {
        if l.points.len() < 3 {
            continue;
        }
        // A point just inside the loop's first edge.
        let (a, b) = (l.points[0], l.points[1]);
        let n = (b - a).normalized().perp() * 0.01;
        let mid = a.lerp(b, 0.5);
        let probe = if inside(mid + n, &l.points) { mid + n } else { mid - n };
        let depth = loops.iter().enumerate().filter(|(j, o)| *j != i && inside(probe, &o.points)).count();
        total += signed(&l.points).abs() * if depth % 2 == 0 { 1.0 } else { -1.0 };
    }
    total
}

fn loops(path: &Path) -> usize {
    path.cmds.iter().filter(|c| matches!(c, PathCmd::MoveTo(_))).count()
}

fn shape(hw: f32, cap: LineCap, join: LineJoin) -> StrokeShape<'static> {
    StrokeShape { cap, join, ..StrokeShape::round(hw) }
}

fn close(a: f32, b: f32, tol: f32) -> bool {
    (a - b).abs() <= tol
}

fn polyline(pts: &[(f32, f32)]) -> Path {
    let mut p = Path::new();
    for (i, &(x, y)) in pts.iter().enumerate() {
        if i == 0 {
            p.move_to(Vec2::new(x, y));
        } else {
            p.line_to(Vec2::new(x, y));
        }
    }
    p
}

#[test]
fn a_straight_line_with_butt_caps_is_exactly_its_rectangle() {
    let line = Path::line(Vec2::new(10.0, 20.0), Vec2::new(110.0, 20.0));
    let out = stroke_outline(&line, &shape(4.0, LineCap::Butt, LineJoin::Round)).unwrap();
    assert_eq!(loops(&out), 1);
    assert_eq!(out.cmds.len(), 6, "four corners and a close: {}", out.to_svg());
    assert_eq!(out.bounds(), Rect::new(10.0, 16.0, 110.0, 24.0));
    assert!(close(area(&out), 800.0, 1e-2));
}

#[test]
fn caps_add_what_they_should() {
    let line = Path::line(Vec2::new(0.0, 0.0), Vec2::new(100.0, 0.0));
    let round = stroke_outline(&line, &shape(5.0, LineCap::Round, LineJoin::Round)).unwrap();
    assert!(close(area(&round), 1000.0 + PI * 25.0, 0.05), "{}", area(&round));
    assert_eq!(round.bounds(), Rect::new(-5.0, -5.0, 105.0, 5.0));
    // Two straight sides and two half circles of two curves each.
    assert_eq!(round.cmds.iter().filter(|c| matches!(c, PathCmd::CubicTo(..))).count(), 4, "{}", round.to_svg());
    let square = stroke_outline(&line, &shape(5.0, LineCap::Square, LineJoin::Round)).unwrap();
    assert!(close(area(&square), 1100.0, 1e-2));
    assert_eq!(square.cmds.len(), 6, "{}", square.to_svg());
}

#[test]
fn corners_take_their_join() {
    // A right angle, 10 wide, butt ends: the two arms cover 1975; the corner adds its join.
    let bend = polyline(&[(0.0, 0.0), (100.0, 0.0), (100.0, 100.0)]);
    let of = |join, limit| area(&stroke_outline(&bend, &StrokeShape { miter_limit: limit, ..shape(5.0, LineCap::Butt, join) }).unwrap());
    assert!(close(of(LineJoin::Bevel, 4.0), 1975.0 + 12.5, 0.02), "{}", of(LineJoin::Bevel, 4.0));
    assert!(close(of(LineJoin::Miter, 4.0), 1975.0 + 25.0, 0.02), "{}", of(LineJoin::Miter, 4.0));
    assert!(close(of(LineJoin::Round, 4.0), 1975.0 + PI * 25.0 / 4.0, 0.05), "{}", of(LineJoin::Round, 4.0));
    // A right angle's miter is √2 half-widths long: a limit of 1.2 bevels it.
    assert!(close(of(LineJoin::Miter, 1.2), 1975.0 + 12.5, 0.02));
    // A mitered rectangle's outline is two rectangles.
    let rect = stroke_outline(&Path::rect(0.0, 0.0, 100.0, 50.0), &shape(5.0, LineCap::Butt, LineJoin::Miter)).unwrap();
    assert_eq!(loops(&rect), 2);
    assert!(close(area(&rect), 110.0 * 60.0 - 90.0 * 40.0, 0.02), "{}", area(&rect));
    assert_eq!(rect.bounds(), Rect::new(-5.0, -5.0, 105.0, 55.0));
}

#[test]
fn a_circle_s_stroke_is_a_ring_of_curves() {
    let ring = stroke_outline(&Path::ellipse(100.0, 100.0, 50.0, 50.0), &StrokeShape::round(5.0)).unwrap();
    assert_eq!(loops(&ring), 2);
    assert!(close(area(&ring), PI * (55.0f32.powi(2) - 45.0f32.powi(2)), 1.0), "{}", area(&ring));
    assert!(ring.cmds.len() < 80, "curves, not flattened lines: {} commands", ring.cmds.len());
    assert!(ring.cmds.iter().filter(|c| matches!(c, PathCmd::LineTo(_))).count() <= 2, "{}", ring.to_svg());
}

/// Whether `p` is inside an even-odd path (finely flattened: `Path::contains` is for picking).
fn covers(path: &Path, p: Vec2) -> bool {
    let mut odd = false;
    for l in path.flatten(&Affine::IDENTITY, 0.001) {
        let n = l.points.len();
        for i in 0..n {
            let (a, b) = (l.points[i], l.points[(i + 1) % n]);
            if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x) {
                odd = !odd;
            }
        }
    }
    odd
}

/// Every probe point is inside the outline exactly when it is within the half-width of the
/// path (round caps and joins), but for points within `slack` of the edge.
fn matches_distance(path: &Path, hw: f32, out: &Path, slack: f32) {
    matches_distance_every(path, hw, out, slack, 1.0);
}

fn matches_distance_every(path: &Path, hw: f32, out: &Path, slack: f32, step: f32) {
    let b = path.bounds().inflate(hw + 2.0);
    let lines = path.flatten(&Affine::IDENTITY, 0.001);
    let distance = |p: Vec2| lines.iter().flat_map(|l| l.points.windows(2)).map(|w| crate::segment_distance(p, w[0], w[1])).fold(f32::INFINITY, f32::min);
    let (mut checked, mut y) = (0, b.min.y);
    while y < b.max.y {
        let mut x = b.min.x;
        while x < b.max.x {
            let p = Vec2::new(x, y);
            let d = distance(p);
            if (d - hw).abs() > slack {
                checked += 1;
                assert_eq!(covers(out, p), d < hw, "at {p:?}, {d} from the path (half-width {hw})");
            }
            x += 0.73 * step;
        }
        y += 0.61 * step;
    }
    assert!(checked > 200);
}

#[test]
fn curves_tighter_than_the_stroke_is_wide_still_cover_what_the_stroke_covers() {
    // A hairpin, an S and a loop that crosses itself, each stroked wider than it turns.
    for (d, hw) in [
        ("M10 10C60 10 60 22 10 22", 9.0),
        ("M0 50C40 0 60 100 100 50C140 0 160 0 200 50", 14.0),
        ("M20 80C120 -40 -20 -40 80 80", 6.0),
        ("M0 0Q50 80 100 0Q150 -80 200 0", 20.0),
        ("M50 50L50 50.5L90 50L50 49", 4.0),
    ] {
        let path = Path::from_svg(d).unwrap();
        let out = stroke_outline(&path, &StrokeShape::round(hw)).unwrap();
        assert!(!out.is_empty(), "{d}");
        matches_distance(&path, hw, &out, 0.08);
    }
}

#[test]
fn dashes_are_pieces_of_their_own_and_dots_are_caps() {
    let line = Path::line(Vec2::new(0.0, 0.0), Vec2::new(100.0, 0.0));
    let dashed = StrokeShape { dash: Some((&[10.0, 10.0], 0.0)), ..shape(2.0, LineCap::Butt, LineJoin::Round) };
    let out = stroke_outline(&line, &dashed).unwrap();
    assert_eq!(loops(&out), 5);
    assert!(close(area(&out), 5.0 * 10.0 * 4.0, 0.01), "{}", area(&out));
    // Half a period in: a 5 px dash, four whole ones, and a last 5 px.
    let shifted = stroke_outline(&line, &StrokeShape { dash: Some((&[10.0, 10.0], 5.0)), ..dashed }).unwrap();
    assert_eq!(loops(&shifted), 6);
    assert!(close(area(&shifted), 200.0, 0.01));
    // Dots: discs every 10 units, from the start to the end.
    let dots = stroke_outline(&line, &StrokeShape { dash: Some((&[0.0, 10.0], 0.0)), ..StrokeShape::round(2.0) }).unwrap();
    assert_eq!(loops(&dots), 11);
    assert!(close(area(&dots), 11.0 * PI * 4.0, 0.3), "{}", area(&dots));
    // A dash that goes round a corner keeps the corner's join.
    let bend = polyline(&[(0.0, 0.0), (50.0, 0.0), (50.0, 50.0)]);
    let round_corner = stroke_outline(&bend, &StrokeShape { dash: Some((&[40.0, 5.0, 20.0, 100.0], 0.0)), ..shape(2.0, LineCap::Butt, LineJoin::Miter) }).unwrap();
    assert_eq!(loops(&round_corner), 2);
    assert!(close(area(&round_corner), 40.0 * 4.0 + 20.0 * 4.0, 0.01), "{}", area(&round_corner));
    // Dashes follow curves by their length: a circle of radius 50 holds 31 whole periods of 10.
    let circle = Path::ellipse(0.0, 0.0, 50.0, 50.0);
    let on_circle = stroke_outline(&circle, &StrokeShape { dash: Some((&[5.0, 5.0], 0.0)), ..shape(1.0, LineCap::Butt, LineJoin::Round) }).unwrap();
    assert_eq!(loops(&on_circle), 31, "the 32nd, cut short where the circle starts, touches the first: one fill");
    assert!(close(area(&on_circle), 2.0 * (31.0 * 5.0 + (2.0 * PI * 50.0 - 310.0)), 0.5), "31 dashes and 4.16 of the 32nd, 2 wide: {}", area(&on_circle));
}

#[test]
fn a_width_function_shapes_each_side() {
    let line = Path::line(Vec2::new(0.0, 0.0), Vec2::new(100.0, 0.0));
    // Full width at the start, none at the end: a triangle, and half a disc at the wide end.
    let taper = |t: f32| (1.0 - t, 1.0 - t);
    let out = stroke_outline(&line, &StrokeShape { widths: Some(&taper), ..StrokeShape::round(5.0) }).unwrap();
    assert!(close(area(&out), 500.0 + PI * 25.0 / 2.0, 0.1), "{}", area(&out));
    assert_eq!(loops(&out), 1);
    // One-sided: only the left of the direction of travel (up, on a y-down screen).
    let left = |_: f32| (1.0, 0.0);
    let one = stroke_outline(&line, &StrokeShape { widths: Some(&left), ..shape(5.0, LineCap::Butt, LineJoin::Round) }).unwrap();
    assert_eq!(one.bounds(), Rect::new(0.0, -5.0, 100.0, 0.0));
    // Mirrored: the other side.
    let flipped = stroke_outline(&line, &StrokeShape { widths: Some(&left), flip: true, ..shape(5.0, LineCap::Butt, LineJoin::Round) }).unwrap();
    assert_eq!(flipped.bounds(), Rect::new(0.0, 0.0, 100.0, 5.0));
    // A smooth bulge on a curve: wider in the middle, to the tolerance.
    let bulge = |t: f32| {
        let k = 0.2 + 0.8 * (PI * t).sin();
        (k, k)
    };
    let curve = Path::from_svg("M0 0C40 -40 80 40 120 0").unwrap();
    let out = stroke_outline(&curve, &StrokeShape { widths: Some(&bulge), ..StrokeShape::round(6.0) }).unwrap();
    assert_eq!(loops(&out), 1);
    let mid = Vec2::new(60.0, 0.0);
    assert!(covers(&out, mid + Vec2::new(-3.5, -4.0)));
    assert!(!covers(&out, Vec2::new(2.0, 4.0)), "thin at the ends");
}

#[test]
fn rectangles_and_circles_expand_and_inset_by_the_book() {
    let rect = Path::rect(100.0, 100.0, 80.0, 40.0);
    let grown = offset(&rect, FillRule::NonZero, 10.0).unwrap();
    assert!(close(area(&grown), 80.0 * 40.0 + 2.0 * 10.0 * 120.0 + PI * 100.0, 0.3), "{}", area(&grown));
    assert_eq!(grown.bounds(), Rect::new(90.0, 90.0, 190.0, 150.0));
    assert_eq!(loops(&grown), 1);
    // Four sides and four quarter circles.
    assert_eq!(grown.cmds.iter().filter(|c| matches!(c, PathCmd::CubicTo(..))).count(), 4, "{}", grown.to_svg());
    let shrunk = offset(&rect, FillRule::NonZero, -10.0).unwrap();
    assert!(close(area(&shrunk), 60.0 * 20.0, 0.01), "{}", area(&shrunk));
    assert_eq!(shrunk.bounds(), Rect::new(110.0, 110.0, 170.0, 130.0));
    assert_eq!(shrunk.cmds.len(), 6, "corners stay corners: {}", shrunk.to_svg());
    // Past half its height nothing is left.
    assert!(offset(&rect, FillRule::NonZero, -20.5).unwrap().is_empty());
    assert_eq!(offset(&rect, FillRule::NonZero, 0.0).unwrap().bounds(), rect.bounds());

    let circle = Path::ellipse(0.0, 0.0, 50.0, 50.0);
    for d in [-30.0f32, -5.0, 0.5, 5.0, 40.0] {
        let out = offset(&circle, FillRule::NonZero, d).unwrap();
        let r = 50.0 + d;
        assert!(close(area(&out), PI * r * r, PI * r * 0.12), "offset {d}: {} for {}", area(&out), PI * r * r);
        assert_eq!(loops(&out), 1);
    }
}

#[test]
fn holes_shrink_when_the_fill_grows_and_overlapping_loops_are_one_region() {
    // A square with a square hole (even-odd).
    let mut ring = Path::rect(0.0, 0.0, 100.0, 100.0);
    ring.cmds.extend(Path::rect(30.0, 30.0, 40.0, 40.0).cmds);
    let grown = offset(&ring, FillRule::EvenOdd, 5.0).unwrap();
    assert!(close(area(&grown), (10_000.0 + 2000.0 + PI * 25.0) - 900.0, 0.3), "{}", area(&grown));
    let shrunk = offset(&ring, FillRule::EvenOdd, -5.0).unwrap();
    assert!(close(area(&shrunk), 8100.0 - (2500.0 - (100.0 - PI * 25.0)), 0.3), "{}", area(&shrunk));
    // The hole closes.
    assert_eq!(loops(&offset(&ring, FillRule::EvenOdd, 21.0).unwrap()), 1);
    // Two overlapping squares (non-zero) are one region: the edges inside it don't count.
    let mut two = Path::rect(0.0, 0.0, 60.0, 60.0);
    two.cmds.extend(Path::rect(40.0, 0.0, 60.0, 60.0).cmds);
    let shrunk = offset(&two, FillRule::NonZero, -10.0).unwrap();
    assert!(close(area(&shrunk), 80.0 * 40.0, 0.05), "{}", area(&shrunk));
    // A neck narrower than twice the inset parts the fill in two.
    let dumbbell = Path::from_svg("M0 0L40 0L40 15L60 15L60 0L100 0L100 40L60 40L60 25L40 25L40 40L0 40Z").unwrap();
    assert_eq!(loops(&offset(&dumbbell, FillRule::NonZero, -6.0).unwrap()), 2);
}

#[test]
fn soften_makes_the_core_and_its_bands() {
    let rect = Path::rect(0.0, 0.0, 100.0, 60.0);
    let out = soften(&rect, FillRule::NonZero, 12.0, 4, false).unwrap();
    assert_eq!(out.len(), 5);
    assert!(close(area(&out[0]), 6000.0, 0.01));
    let grown = |d: f32| 6000.0 + 2.0 * d * 160.0 + PI * d * d;
    for (i, band) in out.iter().enumerate().skip(1) {
        let (d0, d1) = (3.0 * (i as f32 - 1.0), 3.0 * i as f32);
        assert!(close(area(band), grown(d1) - grown(d0), 0.5), "band {i}: {}", area(band));
        assert_eq!(loops(band), 2, "a ring");
    }
    let inset = soften(&rect, FillRule::NonZero, 12.0, 3, true).unwrap();
    assert_eq!(inset.len(), 4);
    assert!(close(area(&inset[0]), 76.0 * 36.0, 0.05));
    let total: f32 = inset.iter().map(area).sum();
    assert!(close(total, 6000.0, 0.5), "core and bands make up the fill: {total}");
    assert_eq!(inset[3].bounds(), rect.bounds());
    // Thinner than the edge: no core, and the bands that are left.
    let thin = soften(&Path::rect(0.0, 0.0, 100.0, 10.0), FillRule::NonZero, 12.0, 3, true).unwrap();
    assert!(thin[0].is_empty() && thin[1].is_empty() && !thin[2].is_empty() && !thin[3].is_empty());
    assert!(soften(&rect, FillRule::NonZero, 0.0, 4, false).is_none());
    assert!(soften(&rect, FillRule::NonZero, 5.0, 0, false).is_none());
    assert!(soften(&rect, FillRule::NonZero, 5.0, MAX_SOFTEN_STEPS + 1, false).is_none());
}

#[test]
fn thin_strokes_keep_their_shape() {
    // Far thinner than the planar map's snap distance.
    let line = polyline(&[(0.0, 0.0), (30.0, 0.0), (30.0, 20.0)]);
    let out = stroke_outline(&line, &shape(0.05, LineCap::Butt, LineJoin::Miter)).unwrap();
    assert!(close(area(&out), 50.0 * 0.1, 1e-3), "{}", area(&out));
    let grown = offset(&Path::rect(0.0, 0.0, 10.0, 10.0), FillRule::NonZero, 0.05).unwrap();
    assert!(close(area(&grown), 100.0 + 4.0 * 0.5 + PI * 0.0025, 2e-3), "{}", area(&grown));
}

#[test]
fn long_and_busy_strokes_are_outlined_in_parts() {
    // 400 corners: far more ribbons than one planar map takes.
    let mut zigzag = Path::new();
    zigzag.move_to(Vec2::ZERO);
    for i in 1..=400 {
        zigzag.line_to(Vec2::new(i as f32 * 10.0, if i % 2 == 0 { 0.0 } else { 30.0 }));
    }
    let out = stroke_outline(&zigzag, &StrokeShape::round(2.0)).unwrap();
    assert_eq!(loops(&out), 1);
    matches_distance_every(&zigzag, 2.0, &out, 0.05, 5.0);
    // 2,000 dots.
    let line = Path::line(Vec2::ZERO, Vec2::new(10_000.0, 0.0));
    let dots = stroke_outline(&line, &StrokeShape { dash: Some((&[0.0, 5.0], 0.0)), ..StrokeShape::round(1.0) }).unwrap();
    assert_eq!(loops(&dots), 2001);
}

#[test]
fn hostile_input_never_panics() {
    let nan = Path { cmds: vec![PathCmd::MoveTo(Vec2::ZERO), PathCmd::LineTo(Vec2::new(f32::NAN, 1.0))] };
    assert!(stroke_outline(&nan, &StrokeShape::round(2.0)).is_none());
    let far = Path::line(Vec2::new(-1e30, 0.0), Vec2::new(1e30, 1e30));
    let _ = stroke_outline(&far, &StrokeShape::round(2.0));
    let _ = offset(&Path::rect(-1e20, -1e20, 2e20, 2e20), FillRule::NonZero, 1e10);
    for hw in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        assert!(stroke_outline(&Path::rect(0.0, 0.0, 10.0, 10.0), &StrokeShape::round(hw)).is_none_or(|p| p.is_empty()));
    }
    let empty = Path::new();
    assert!(stroke_outline(&empty, &StrokeShape::round(2.0)).unwrap().is_empty());
    assert!(offset(&empty, FillRule::NonZero, 5.0).unwrap().is_empty());
    assert!(offset(&Path::rect(0.0, 0.0, 10.0, 10.0), FillRule::NonZero, f32::NAN).is_none());
    // A point: a dot with round caps, nothing otherwise.
    let dot = Path { cmds: vec![PathCmd::MoveTo(Vec2::new(5.0, 5.0)), PathCmd::LineTo(Vec2::new(5.0, 5.0))] };
    assert!(close(area(&stroke_outline(&dot, &StrokeShape::round(3.0)).unwrap()), PI * 9.0, 0.05));
    assert!(stroke_outline(&dot, &shape(3.0, LineCap::Butt, LineJoin::Miter)).unwrap().is_empty());
    // Endless dashes are declined, not attempted.
    let line = Path::line(Vec2::ZERO, Vec2::new(1e6, 0.0));
    assert!(stroke_outline(&line, &StrokeShape { dash: Some((&[0.001, 0.001], 0.0)), ..StrokeShape::round(1.0) }).is_none());
    let silly = |t: f32| (f32::NAN, t * f32::INFINITY);
    let _ = stroke_outline(&Path::rect(0.0, 0.0, 10.0, 10.0), &StrokeShape { widths: Some(&silly), ..StrokeShape::round(1.0) });
    // Handles on their anchors (no direction at the ends).
    let stub = Path::from_svg("M0 0C0 0 50 50 50 50").unwrap();
    assert!(close(area(&stroke_outline(&stub, &shape(1.0, LineCap::Butt, LineJoin::Round)).unwrap()), 2.0 * 50.0 * 2f32.sqrt(), 0.05));
    // Curve clean-up entry points stay finite and never panic on hostile paths.
    for p in [&nan, &far, &empty, &dot, &stub] {
        for v in [-1.0f32, 0.0, 0.5, 5.0, f32::NAN, f32::INFINITY] {
            let _ = crate::optimize(p, v);
            let _ = crate::smooth(p, v);
            let _ = crate::straighten(p, v);
        }
    }
}
