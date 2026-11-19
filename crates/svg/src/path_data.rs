//! SVG path data (the `d` attribute) as an AniCraft [`Path`].
//!
//! craft-svg reads the data: every command in absolute and relative form (M L H V C S Q T A Z),
//! implicit repeats, packed numbers and arc flags, with the S/T reflections resolved and
//! quadratic segments preserved. This module only maps its segments onto [`Path`], turning
//! elliptical arcs into cubics with [`anicraft_geom::svg::arc_to`]. Never panics; malformed,
//! non-finite and oversized path data are errors.

use anicraft_geom::svg::{MAX_PATH_CMDS, arc_to};
use anicraft_geom::{Path, PathCmd, Vec2};
use craft_svg::path_data::{Point, QuadSegment, parse_path_with_quads};

fn v(p: Point) -> Vec2 {
    Vec2::new(p.x as f32, p.y as f32)
}

/// Parses SVG path data into a [`Path`]: icon outlines, `path.d` commands and SVG import.
pub fn parse_path_data(d: &str) -> Result<Path, String> {
    // The shared geometry parser permits an implicit origin; SVG and command input must
    // begin with moveto rather than manufacturing an anchor for malformed drawing commands.
    if let Some(first) = d.bytes().find(|byte| !matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
        && !matches!(first, b'M' | b'm')
    {
        return Err("path data must start with moveto".into());
    }
    let subpaths = parse_path_with_quads(d).map_err(|e| e.to_string())?;
    let mut path = Path::new();
    for sub in subpaths {
        path.move_to(v(sub.start));
        let mut cur = sub.start;
        for seg in &sub.segs {
            match *seg {
                QuadSegment::Line(p) => {
                    path.line_to(v(p));
                    cur = p;
                }
                QuadSegment::Quad(q, p) => {
                    path.quad_to(v(q), v(p));
                    cur = p;
                }
                QuadSegment::Cubic(c1, c2, p) => {
                    path.cubic_to(v(c1), v(c2), v(p));
                    cur = p;
                }
                QuadSegment::Arc { rx, ry, rot, large, sweep, end } => {
                    arc_to(&mut path, [cur.x, cur.y], rx, ry, rot, large, sweep, [end.x, end.y]);
                    cur = end;
                }
            }
            if path.cmds.len() > MAX_PATH_CMDS {
                return Err(format!("path has more than {MAX_PATH_CMDS} commands"));
            }
        }
        if sub.closed {
            path.close();
        }
    }
    if !path.cmds.iter().all(finite) {
        return Err("path data has a number too large to draw".into());
    }
    Ok(path)
}

/// Whether every point of `cmd` fits an `f32` (a number past it would draw nothing).
fn finite(cmd: &PathCmd) -> bool {
    match cmd {
        PathCmd::MoveTo(p) | PathCmd::LineTo(p) => p.is_finite(),
        PathCmd::QuadTo(c, p) => c.is_finite() && p.is_finite(),
        PathCmd::CubicTo(a, b, p) => a.is_finite() && b.is_finite() && p.is_finite(),
        PathCmd::Close => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ends(p: &Path) -> Vec<Vec2> {
        p.anchors().into_iter().map(|(_, a)| a).collect()
    }

    #[test]
    fn nonempty_paths_require_an_initial_moveto() {
        for data in ["L10 10", "H10", "V10", "C0 0 1 1 2 2", "Q1 1 2 2", "Z", "  l10 10"] {
            assert!(parse_path_data(data).is_err(), "accepted drawing command without moveto: {data}");
        }
        for data in ["", " \t\r\n", "m1 2 l3 4", " M0 0 L1 1"] {
            assert!(parse_path_data(data).is_ok(), "rejected valid path: {data:?}");
        }
    }

    #[test]
    fn malformed_tokens_do_not_import_a_truncated_path() {
        for data in ["@", "M0 0 L1 1 @", "M0 0 L1 1 1e", "M0 0 L1 1 1e9999"] {
            assert!(parse_path_data(data).is_err(), "accepted malformed path: {data}");
        }
        assert!(parse_path_data("M0 0 L1 1 ").is_ok());
    }

    #[test]
    fn relative_and_shorthand_commands() {
        let p = parse_path_data("m10 10 h5 v5 H10 z m 1,1 l2-2").unwrap();
        assert_eq!(
            ends(&p),
            vec![
                Vec2::new(10.0, 10.0),
                Vec2::new(15.0, 10.0),
                Vec2::new(15.0, 15.0),
                Vec2::new(10.0, 15.0),
                Vec2::new(11.0, 11.0),
                Vec2::new(13.0, 9.0)
            ]
        );
        assert!(matches!(p.cmds[4], PathCmd::Close));
        // Implicit lineto after moveto, packed negative numbers.
        let q = parse_path_data("M0 0 10 0-5.5.5").unwrap();
        assert_eq!(ends(&q), vec![Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0), Vec2::new(-5.5, 0.5)]);
        // S reflects the previous control point.
        let s = parse_path_data("M0 0 C 0 10 10 10 10 0 S 20 -10 20 0").unwrap();
        assert!(matches!(s.cmds[2], PathCmd::CubicTo(c1, _, _) if (c1 - Vec2::new(10.0, -10.0)).length() < 1e-5));
        // Q and T become cubics through the same curve.
        let t = parse_path_data("M0 0 Q 5 10 10 0 T 20 0").unwrap();
        assert_eq!(ends(&t).last().copied(), Some(Vec2::new(20.0, 0.0)));
    }

    #[test]
    fn arcs_land_on_the_ellipse() {
        for (sweep, mid_y) in [("1", -5.0), ("0", 5.0)] {
            let p = parse_path_data(&format!("M0 0 A5 5 0 0 {sweep} 10 0")).unwrap();
            let lines = p.flatten(&anicraft_geom::Affine::IDENTITY, 0.01);
            let pts = &lines[0].points;
            assert!(pts.iter().all(|q| (q.distance(Vec2::new(5.0, 0.0)) - 5.0).abs() < 0.05), "on the circle");
            assert!(pts.iter().any(|q| (q.y - mid_y).abs() < 0.05), "through the right side ({sweep})");
            assert_eq!(pts.last().copied(), Some(Vec2::new(10.0, 0.0)));
        }
        // Packed flags as icon sets write them, and radii too small get scaled up.
        let p = parse_path_data("M2 2a1 1 0 011 1a.1.1 0 0 0 4 0").unwrap();
        assert_eq!(ends(&p).last().copied(), Some(Vec2::new(7.0, 3.0)));
        // A real icon outline (rounded square corner arcs).
        let lock = parse_path_data("M7 11V7a5 5 0 0 1 10 0v4").unwrap();
        assert_eq!(ends(&lock).last().copied(), Some(Vec2::new(17.0, 11.0)));
    }

    #[test]
    fn bad_input_is_an_error() {
        assert!(parse_path_data("").unwrap().is_empty());
        for bad in ["10 10", "M", "M 1", "M0 0 A1 1 0 2 0 3 3", "M0 0 Z 4 4", "M0 0 L1e999 0", "M0 0 L1e300 0", "M0 0 X1 1"] {
            assert!(parse_path_data(bad).is_err(), "{bad}");
        }
    }
}
