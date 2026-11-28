//! Modify › Shape (ADR 0109): Convert Lines to Fills against what the renderer drew, Expand Fill
//! and Soften Fill Edges against areas known in closed form, and the colour picker's sample.

use super::*;
use crate::cmds_group::even_odd_area;
use anicraft_doc::Rgba;
use anicraft_render::View;
use std::f64::consts::PI;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    s.execute(id, &p).unwrap_or_else(|e| panic!("{id}: {e}"))
}

/// A white 400 × 300 stage; shapes are drawing objects when `object`, else raw.
fn blank(object: bool) -> Session {
    let mut s = Session::new(anicraft_doc::Document::default());
    run(&mut s, "doc.set_stage", json!({"width": 400, "height": 300, "background": "#ffffff"}));
    run(&mut s, "draw.object_drawing", json!({"on": object}));
    s
}

fn shapes(s: &Session) -> Vec<(ElementId, Shape, Affine)> {
    s.visible_elements()
        .iter()
        .filter_map(|e| match &e.kind {
            ElementKind::Shape(sh) => Some((e.id, sh.clone(), e.transform)),
            _ => None,
        })
        .collect()
}

fn solid(sh: &Shape) -> Option<Rgba> {
    match sh.fill {
        Some(Paint::Solid(c)) => Some(c),
        _ => None,
    }
}

/// The stage as the CPU renderer draws it, as coverage of ink (0 white .. 1 black) per pixel.
fn ink(s: &Session) -> Vec<f32> {
    let (pm, _) = anicraft_render::render(&s.display_list(), &View::stage(&s.doc, 1.0)).unwrap();
    pm.to_rgba8().as_chunks::<4>().0.iter().map(|px| 1.0 - f32::from(px[0]) / 255.0).collect()
}

/// How far two renders are apart: the summed difference in pixels of ink, and the largest
/// difference at one pixel.
fn apart(a: &[f32], b: &[f32]) -> (f32, f32) {
    assert_eq!(a.len(), b.len());
    a.iter().zip(b).map(|(x, y)| (x - y).abs()).fold((0.0, 0.0), |(sum, max), d| (sum + d, max.max(d)))
}

fn area_of(s: &Session, id: ElementId) -> f64 {
    let (_, sh, m) = shapes(s).into_iter().find(|(i, ..)| *i == id).expect("the shape is there");
    even_odd_area(&sh.path, &m)
}

fn total_fill_area(s: &Session) -> f64 {
    shapes(s).iter().filter(|(_, sh, _)| sh.fill.is_some()).map(|(_, sh, m)| even_odd_area(&sh.path, m)).sum()
}

// ----- Convert Lines to Fills ---------------------------------------------------------------------

/// Samples per pixel side of a [`Mask`].
const SS: usize = 8;

/// The stage sampled `SS` × `SS` times per pixel: which sample points something covers.
struct Mask(Vec<bool>);

impl Mask {
    fn new() -> Mask {
        Mask(vec![false; 400 * SS * 300 * SS])
    }

    /// Adds the even-odd inside of `loops` (stage units), by scanlines through the samples.
    fn add(&mut self, loops: &[Vec<Vec2>]) {
        let (w, h) = (400 * SS, 300 * SS);
        let ys = loops.iter().flatten().map(|p| p.y);
        let (y0, y1) = (ys.clone().fold(f32::INFINITY, f32::min), ys.fold(f32::NEG_INFINITY, f32::max));
        let rows = ((y0 * SS as f32).floor().max(0.0) as usize)..((y1 * SS as f32).ceil().max(0.0) as usize + 1).min(h);
        let mut xs: Vec<f32> = Vec::new();
        for row in rows {
            let y = (row as f32 + 0.5) / SS as f32;
            xs.clear();
            for l in loops {
                for (a, b) in l.iter().zip(l.iter().cycle().skip(1)) {
                    if (a.y > y) != (b.y > y) {
                        xs.push(a.x + (y - a.y) / (b.y - a.y) * (b.x - a.x));
                    }
                }
            }
            xs.sort_by(f32::total_cmp);
            for pair in xs.as_chunks::<2>().0 {
                // Samples whose centres lie in [pair[0], pair[1]).
                let from = (pair[0] * SS as f32 - 0.5).ceil().max(0.0) as usize;
                let to = ((pair[1] * SS as f32 - 0.5).ceil().max(0.0) as usize).min(w);
                for cell in self.0[row * w..row * w + w].iter_mut().take(to).skip(from) {
                    *cell = true;
                }
            }
        }
    }

    /// Area in pixels.
    fn area(&self) -> f32 {
        self.0.iter().filter(|b| **b).count() as f32 / (SS * SS) as f32
    }
}

/// What the lines on the stage cover: the outline the renderer fills for each stroke
/// (`anicraft_render::stroke`, loops that don't overlap, ADR 0110), worked out as it does at
/// 2400 % zoom, where its arcs and flattening (0.1 px there) are exact for the purpose: a round
/// cap of a 9 px line is 0.04 px short of a half disc.
fn stroke_coverage(s: &Session) -> Mask {
    let mut mask = Mask::new();
    for (_, sh, m) in shapes(s) {
        let Some(st) = &sh.stroke else { continue };
        let zoom = Affine::scale(24.0, 24.0).mul(&m);
        let lines = sh.path.flatten(&zoom, 0.1);
        let (polys, alpha) = anicraft_render::stroke::stroke_geometry(&lines, st, &zoom, false).expect("the stroke draws");
        assert_eq!(alpha, 1.0);
        let loops: Vec<Vec<Vec2>> = polys.into_iter().map(|poly| poly.into_iter().map(|p| p * (1.0 / 24.0)).collect()).collect();
        mask.add(&loops);
    }
    mask
}

/// What the fills on the stage cover.
fn fill_coverage(s: &Session) -> Mask {
    let mut mask = Mask::new();
    // (Their loops don't overlap, so either fill rule covers the same.)
    for (_, sh, m) in shapes(s).iter().filter(|(_, sh, _)| sh.fill.is_some()) {
        let loops: Vec<Vec<Vec2>> = sh.path.flatten(m, 0.002).into_iter().map(|l| l.points).collect();
        mask.add(&loops);
    }
    mask
}

/// Draws with `draw` (black lines only), converts them, and checks the fills cover what the
/// lines covered: the same area to within a pixel, and the same places.
fn converts_to_the_same_picture(what: &str, object: bool, draw: impl Fn(&mut Session)) {
    let mut s = blank(object);
    draw(&mut s);
    run(&mut s, "edit.select_all", json!({}));
    let before = ink(&s);
    let lines = shapes(&s).iter().filter(|(_, sh, _)| sh.stroke.is_some()).count();
    assert!(lines > 0 && shapes(&s).iter().all(|(_, sh, _)| sh.fill.is_none()), "{what}: lines to convert");
    let stroke = stroke_coverage(&s);
    let v = run(&mut s, "modify.lines_to_fills", json!({}));
    assert_eq!(v["converted"].as_u64(), Some(lines as u64), "{what}: {v}");
    assert!(shapes(&s).iter().all(|(_, sh, _)| sh.stroke.is_none() && sh.fill.is_some()), "{what}: only fills are left");
    let fill = fill_coverage(&s);
    // 1. The area: the same to within a pixel.
    let (was, is) = (stroke.area(), fill.area());
    assert!(was > 20.0, "{what}: the line covered something ({was} px)");
    assert!((was - is).abs() <= 1.0, "{what}: the fills cover {is} px, the lines covered {was} px");
    // 2. The places: samples only one of them covers are a sliver along the edge, a small
    //    share of the area (a missing cap or a dash out of step would be many times this).
    let differ = stroke.0.iter().zip(&fill.0).filter(|(a, b)| a != b).count() as f32 / (SS * SS) as f32;
    assert!(differ <= 0.004 * was + 0.5, "{what}: {differ} px covered by only one of them, of {was} px");
    // 3. The picture: no pixel changes by more than a dash's end may move (the renderer
    //    measures dashes along its flattened line, the fill along the curve), and in all by
    //    no more than its arcs' tolerance (a small dot is a ten-sided polygon at 100 %).
    //    `a_line_is_drawn_as_its_converted_fill_is` holds solid lines to far less.
    //    (A line thinner than a pixel is drawn a pixel wide and fainter, so it never drops out;
    //    its fill is drawn as thin as it is.)
    let (sum, max) = apart(&before, &ink(&s));
    let hairline = shapes(&s).iter().any(|(_, sh, m)| even_odd_area(&sh.path, m) < 0.0) || was < 300.0 && lines == 1 && before.iter().all(|v| *v < 0.9);
    assert!(hairline || max <= 0.65 && sum <= 0.06 * was + 3.0, "{what}: the picture changed by {sum} px in all, {max} at one pixel");
    // One undo step brings the lines back.
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(shapes(&s).iter().filter(|(_, sh, _)| sh.stroke.is_some()).count(), lines, "{what}: undone in one step");
    if std::env::var("SHAPE_NUMBERS").is_ok() {
        eprintln!("{what} (object {object}): lines {was:.2} px, fills {is:.2} px, differ {differ:.2} px, picture {sum:.2} px (worst {max:.2})");
    }
}

#[test]
fn a_converted_line_covers_what_the_line_drew() {
    let line = |extra: Value| {
        move |s: &mut Session| {
            let mut p = json!({"x0": 40.5, "y0": 60.25, "x1": 330.0, "y1": 215.0, "stroke": "#000000", "stroke_width": 9});
            for (k, v) in extra.as_object().unwrap() {
                p[k] = v.clone();
            }
            run(s, "draw.line", p);
        }
    };
    for object in [true, false] {
        converts_to_the_same_picture("round caps", object, line(json!({})));
        converts_to_the_same_picture("flat caps", object, line(json!({"cap": "none"})));
        converts_to_the_same_picture("square caps", object, line(json!({"cap": "square"})));
        converts_to_the_same_picture("dashes", object, line(json!({"dash": [24, 10, 3, 10], "cap": "none"})));
        converts_to_the_same_picture("dashes with round caps and an offset", object, line(json!({"dash": [40, 30], "dash_offset": 7})));
        converts_to_the_same_picture("dots", object, line(json!({"stroke_width": 5, "dot_spacing": 20})));
        converts_to_the_same_picture("a taper", object, line(json!({"profile": "taper_both"})));
        converts_to_the_same_picture("a bulge, dashed", object, line(json!({"profile": "bulge", "dash": [30, 12], "cap": "none"})));
        converts_to_the_same_picture("a one-sided width", object, line(json!({"profile": [[0.0, 1.0, 0.0], [0.5, 0.2, 1.4], [1.0, 1.0, 0.3]]})));
        converts_to_the_same_picture("a hairline", object, line(json!({"stroke_width": 0.6})));
    }
}

#[test]
fn converted_curves_and_corners_cover_what_they_drew() {
    let path = |d: &'static str, extra: Value| {
        move |s: &mut Session| {
            run(s, "draw.path", json!({"d": d, "stroke": "#000000", "stroke_width": 11, "fill": null, "object": true}));
            let mut p = extra.clone();
            if p.as_object().is_some_and(|o| !o.is_empty()) {
                p["id"] = json!(s.selection.id_list()[0]);
                run(s, "stroke.set_style", p);
            }
        }
    };
    let zigzag = "M40 250L120 60L170 240L230 80L260 230L350 50";
    let waves = "M30 150C90 20 130 280 200 150C270 20 300 280 370 150";
    // A hairpin far tighter than the line is wide, and a loop that crosses itself.
    let tight = "M60 80C330 80 330 104 60 104M120 260C330 60 80 60 290 260";
    converts_to_the_same_picture("round corners", true, path(zigzag, json!({})));
    converts_to_the_same_picture("miters", true, path(zigzag, json!({"join": "miter", "cap": "none", "miter": 10})));
    converts_to_the_same_picture("miters past the limit", true, path(zigzag, json!({"join": "miter", "cap": "square", "miter": 1.5})));
    converts_to_the_same_picture("bevels", true, path(zigzag, json!({"join": "bevel", "cap": "none"})));
    converts_to_the_same_picture("dashes round corners", true, path(zigzag, json!({"join": "miter", "cap": "none", "dash": [60, 14]})));
    converts_to_the_same_picture("curves", true, path(waves, json!({})));
    converts_to_the_same_picture("dashed curves", true, path(waves, json!({"cap": "none", "dash": [18, 9]})));
    converts_to_the_same_picture("tight curves", true, path(tight, json!({})));
    converts_to_the_same_picture("tight curves, flat ends", true, path(tight, json!({"cap": "none", "join": "bevel"})));
    converts_to_the_same_picture("a closed outline", true, |s| {
        run(s, "draw.rect", json!({"x": 60, "y": 50, "w": 260, "h": 180, "radius": 30, "fill": null, "stroke": "#000000", "stroke_width": 14, "join": "miter"}));
    });
    converts_to_the_same_picture("an oval, dotted", true, |s| {
        run(s, "draw.ellipse", json!({"x": 60, "y": 50, "w": 260, "h": 180, "fill": null, "stroke": "#000000", "stroke_width": 4, "dot_spacing": 40}));
    });
}

/// The renderer covers a line once, as it covers the fill made from it (ADR 0110): drawn at
/// 100 %, no pixel changes when the line is converted, beyond what the two outlines' own
/// shapes differ by. Corners, flat and square ends and dashes on straight runs are the same
/// polygons either way: nothing changes at all (it was up to 0.5 of a pixel where the
/// stroke's pieces overlapped). A curve, a round corner or a round end is flattened once as
/// the line's middle and once as the fill's edge, each within the renderer's 0.1 px.
#[test]
fn a_line_is_drawn_as_its_converted_fill_is() {
    let zigzag = "M40 250L120 60L170 240L230 80L260 230L350 50";
    let waves = "M30 150C90 20 130 280 200 150C270 20 300 280 370 150";
    let knot = "M50 50C300 50 50 280 300 280C50 280 300 50 50 250";
    let cases: [(&str, &str, f64, Value, f32); 9] = [
        ("miters", zigzag, 11.0, json!({"join": "miter", "cap": "none", "miter": 10}), 1.0 / 255.0),
        ("bevels and square ends", zigzag, 11.0, json!({"join": "bevel", "cap": "square"}), 1.0 / 255.0),
        ("dashes round corners", zigzag, 11.0, json!({"join": "miter", "cap": "none", "dash": [60, 14]}), 1.0 / 255.0),
        ("a wide zigzag that overlaps itself", zigzag, 40.0, json!({"join": "miter", "cap": "none", "miter": 2}), 1.0 / 255.0),
        ("round corners and ends", zigzag, 11.0, json!({}), 0.1),
        ("a wide curve", waves, 22.0, json!({}), 0.1),
        ("a curve with flat ends", waves, 11.0, json!({"cap": "none"}), 0.1),
        ("a knot", knot, 14.0, json!({}), 0.1),
        ("round dashes that overlap on a zigzag", zigzag, 12.0, json!({"dash": [20, 8]}), 0.1),
    ];
    for (what, d, width, style, may) in cases {
        let mut s = blank(true);
        run(&mut s, "draw.path", json!({"d": d, "stroke": "#000000", "stroke_width": width, "fill": null, "object": true}));
        if style.as_object().is_some_and(|o| !o.is_empty()) {
            let mut p = style.clone();
            p["id"] = json!(s.selection.id_list()[0]);
            run(&mut s, "stroke.set_style", p);
        }
        run(&mut s, "edit.select_all", json!({}));
        let line = ink(&s);
        run(&mut s, "modify.lines_to_fills", json!({}));
        let (sum, max) = apart(&line, &ink(&s));
        assert!(max <= may, "{what}: a pixel changed by {max} (of {sum} px in all)");
        if std::env::var("SHAPE_NUMBERS").is_ok() {
            eprintln!("{what}: worst pixel {max:.4}, {sum:.2} px in all");
        }
    }
}

#[test]
fn a_scaled_skewed_and_mirrored_shape_converts_where_it_is_drawn() {
    for profile in [json!(null), json!([[0.0, 1.5, 0.2], [1.0, 0.2, 1.5]])] {
        converts_to_the_same_picture("a transformed line", true, |s| {
            run(s, "draw.path", json!({"d": "M-60 -20C-20 -60 20 20 60 -20L60 30", "stroke": "#000000", "stroke_width": 8, "fill": null, "object": true}));
            if !profile.is_null() {
                run(s, "element.set_stroke", json!({"profile": profile, "cap": "none", "dash": [14, 6]}));
            }
            // Mirrored, scaled unevenly, sheared and moved onto the stage.
            run(s, "modify.transform", json!({"a": -2.2, "b": 0.4, "c": 0.7, "d": 1.6, "e": 200, "f": 150}));
        });
    }
}

#[test]
fn a_straight_line_becomes_four_points_and_keeps_its_colour() {
    let mut s = blank(true);
    let id = run(&mut s, "draw.line", json!({"x0": 50, "y0": 100, "x1": 250, "y1": 100, "stroke": "#cc336680", "stroke_width": 10, "cap": "none"}))["element"].as_u64().unwrap();
    let v = run(&mut s, "modify.lines_to_fills", json!({"id": id}));
    assert_eq!(v["elements"], json!([id]), "the object is the same object, now a fill");
    let (_, sh, m) = shapes(&s).remove(0);
    assert_eq!(sh.path.cmds.len(), 6, "a rectangle: {}", sh.path.to_svg());
    assert!((even_odd_area(&sh.path, &m) - 2000.0).abs() < 0.01);
    assert_eq!(sh.path.bounds().transformed(&m), anicraft_geom::Rect::new(50.0, 95.0, 250.0, 105.0));
    assert_eq!(solid(&sh), Rgba::parse_hex("#cc336680"), "colour and alpha carried over");
    assert!(!sh.merge && sh.stroke.is_none());
    assert!(s.selection.has_id(ElementId(id)));
}

#[test]
fn an_object_keeps_its_fill_and_gets_the_outline_above_it() {
    let mut s = blank(true);
    let id = run(&mut s, "draw.rect", json!({"x": 100, "y": 100, "w": 100, "h": 60, "fill": "#3366ff", "stroke": "#000000", "stroke_width": 4, "join": "miter"}))["element"].as_u64().unwrap();
    let v = run(&mut s, "modify.lines_to_fills", json!({}));
    assert_eq!(v["converted"], json!(1));
    let all = shapes(&s);
    assert_eq!(all.len(), 2);
    assert_eq!((all[0].0.0, solid(&all[0].1).unwrap().to_hex()), (id, "#3366ff".to_string()), "the fill stays, under");
    assert!(all[0].1.stroke.is_none());
    assert_eq!(solid(&all[1].1).unwrap().to_hex(), "#000000");
    // The frame of a 4 px mitered line round a 100 × 60 rectangle.
    assert!((area_of(&s, all[1].0) - (104.0 * 64.0 - 96.0 * 56.0)).abs() < 0.05, "{}", area_of(&s, all[1].0));
    assert_eq!(s.selection.len(), 2, "both are selected");
}

#[test]
fn a_raw_line_s_fill_joins_the_drawing() {
    // A red raw square, then a raw red line across its top edge and beyond: converted, the
    // line's fill is the square's colour and they become one fill.
    let mut s = blank(false);
    run(&mut s, "draw.rect", json!({"x": 100, "y": 100, "w": 100, "h": 100, "fill": "#ff0000", "stroke": null}));
    run(&mut s, "draw.line", json!({"x0": 60, "y0": 100, "x1": 240, "y1": 100, "stroke": "#ff0000", "stroke_width": 10, "cap": "none"}));
    let before = ink(&s);
    assert!(shapes(&s).len() >= 2);
    run(&mut s, "edit.select_all", json!({}));
    run(&mut s, "modify.lines_to_fills", json!({}));
    let all = shapes(&s);
    assert_eq!(all.len(), 1, "one red fill: {:?}", all.iter().map(|(_, sh, _)| sh.path.to_svg()).collect::<Vec<_>>());
    assert!(all[0].1.merge && all[0].1.stroke.is_none());
    // The square, and the line's 180 × 10 less the 100 × 5 of it inside the square.
    assert!((total_fill_area(&s) - (10_000.0 + 1800.0 - 500.0)).abs() < 0.5, "{}", total_fill_area(&s));
    let (sum, _) = apart(&before, &ink(&s));
    assert!(sum <= 1.0, "{sum}");
    assert!(s.selection.has_id(all[0].0), "what was made is selected");
    // A line of another colour cuts into the fill it lies on, as any raw fill would.
    let mut s = blank(false);
    run(&mut s, "draw.rect", json!({"x": 100, "y": 100, "w": 100, "h": 100, "fill": "#ff0000", "stroke": null}));
    let line = run(&mut s, "draw.line", json!({"x0": 60, "y0": 150, "x1": 240, "y1": 150, "stroke": "#0000ff", "stroke_width": 10, "cap": "none"}));
    assert!(line["element"].is_u64() || line["elements"].is_array());
    run(&mut s, "edit.select_all", json!({}));
    run(&mut s, "modify.lines_to_fills", json!({}));
    let all = shapes(&s);
    assert!(all.iter().all(|(_, sh, _)| sh.stroke.is_none() && sh.merge));
    let blue: f64 = all.iter().filter(|(_, sh, _)| solid(sh).is_some_and(|c| c.to_hex() == "#0000ff")).map(|(_, sh, m)| even_odd_area(&sh.path, m)).sum();
    let red: f64 = all.iter().filter(|(_, sh, _)| solid(sh).is_some_and(|c| c.to_hex() == "#ff0000")).map(|(_, sh, m)| even_odd_area(&sh.path, m)).sum();
    assert!((blue - 1800.0).abs() < 0.5 && (red - 9000.0).abs() < 0.5, "blue {blue}, red {red}");
}

#[test]
fn lines_to_fills_says_what_it_needs() {
    let mut s = blank(true);
    assert!(s.execute("modify.lines_to_fills", &json!({})).unwrap_err().to_string().contains("nothing is selected"));
    run(&mut s, "draw.rect", json!({"x": 10, "y": 10, "w": 50, "h": 50, "fill": "#ff0000", "stroke": null}));
    let e = s.execute("modify.lines_to_fills", &json!({})).unwrap_err().to_string();
    assert!(e.contains("no lines"), "{e}");
    assert!(s.execute("modify.lines_to_fills", &json!({"id": 999_999})).is_err());
    assert!(s.execute("modify.lines_to_fills", &json!({"id": "x"})).is_err());
}

// ----- Expand Fill --------------------------------------------------------------------------------

#[test]
fn rectangles_and_circles_expand_and_inset_by_the_book() {
    for object in [true, false] {
        let mut s = blank(object);
        run(&mut s, "draw.rect", json!({"x": 100, "y": 100, "w": 120, "h": 80, "fill": "#ff0000", "stroke": null}));
        run(&mut s, "edit.select_all", json!({}));
        let v = run(&mut s, "modify.expand_fill", json!({"distance": 10}));
        assert_eq!(v, json!({"changed": 1, "removed": 0}));
        let grown = 120.0 * 80.0 + 2.0 * 10.0 * 200.0 + PI * 100.0;
        assert!((total_fill_area(&s) - grown).abs() < 0.5, "expanded: {} for {grown}", total_fill_area(&s));
        assert_eq!(shapes(&s).len(), 1);
        assert_eq!(s.selection.len(), 1, "still selected");
        // Inset by 25 from the grown shape: the rectangle 15 smaller each side, corners sharp.
        run(&mut s, "modify.expand_fill", json!({"distance": 25, "direction": "inset"}));
        assert!((total_fill_area(&s) - 90.0 * 50.0).abs() < 0.5, "inset: {}", total_fill_area(&s));
        // Undo, one step each.
        run(&mut s, "edit.undo", json!({}));
        assert!((total_fill_area(&s) - grown).abs() < 0.5);

        let mut s = blank(object);
        run(&mut s, "draw.ellipse", json!({"x": 100, "y": 50, "w": 200, "h": 200, "fill": "#00aa00", "stroke": null}));
        run(&mut s, "edit.select_all", json!({}));
        run(&mut s, "modify.expand_fill", json!({"distance": 20}));
        assert!((total_fill_area(&s) - PI * 120.0 * 120.0).abs() < PI * 120.0 * 0.15, "circle expanded: {}", total_fill_area(&s));
        run(&mut s, "modify.expand_fill", json!({"distance": 70, "direction": "inset"}));
        assert!((total_fill_area(&s) - PI * 50.0 * 50.0).abs() < PI * 50.0 * 0.15, "circle inset: {}", total_fill_area(&s));
    }
}

#[test]
fn the_distance_is_a_stage_distance_whatever_the_shape_s_transform() {
    let mut s = blank(true);
    run(&mut s, "draw.rect", json!({"x": -10, "y": -10, "w": 20, "h": 20, "fill": "#ff0000", "stroke": null}));
    // Four times as wide, twice as high, at (200, 150): 80 × 40 on the stage.
    run(&mut s, "modify.transform", json!({"a": 4, "b": 0, "c": 0, "d": 2, "e": 200, "f": 150}));
    run(&mut s, "modify.expand_fill", json!({"distance": 5, "direction": "inset"}));
    let (_, sh, m) = shapes(&s).remove(0);
    assert!((even_odd_area(&sh.path, &m) - 70.0 * 30.0).abs() < 0.05, "{}", even_odd_area(&sh.path, &m));
    assert_eq!(m.m, [4.0, 0.0, 0.0, 2.0, 200.0, 150.0], "the shape keeps its transform");
}

#[test]
fn an_inset_past_the_fill_removes_it_and_a_gradient_keeps_its_place() {
    let mut s = blank(true);
    run(&mut s, "draw.rect", json!({"x": 10, "y": 10, "w": 100, "h": 12, "fill": "#ff0000", "stroke": null}));
    let outlined = run(&mut s, "draw.rect", json!({"x": 10, "y": 60, "w": 100, "h": 12, "fill": "#ff0000", "stroke": "#000000", "stroke_width": 2}))["element"].as_u64().unwrap();
    run(&mut s, "draw.rect", json!({"x": 10, "y": 110, "w": 100, "h": 100, "fill": "#ff0000", "stroke": null}));
    run(&mut s, "paint.set_fill", json!({"type": "linear", "stops": [{"offset": 0, "color": "#ff0000"}, {"offset": 1, "color": "#0000ff"}]}));
    let gradient = shapes(&s).last().unwrap().1.fill.clone();
    run(&mut s, "edit.select_all", json!({}));
    let v = run(&mut s, "modify.expand_fill", json!({"distance": 8, "direction": "inset"}));
    assert_eq!(v, json!({"changed": 1, "removed": 2}));
    let all = shapes(&s);
    assert_eq!(all.len(), 2, "the thin fill is gone; the outlined one keeps its line");
    assert_eq!(all[0].0.0, outlined);
    assert!(all[0].1.fill.is_none() && all[0].1.stroke.is_some());
    assert_eq!(all[1].1.fill, gradient, "the gradient is where it was");
    assert!((area_of(&s, all[1].0) - 84.0 * 84.0).abs() < 0.05);
}

#[test]
fn expand_fill_checks_its_params() {
    let mut s = blank(true);
    run(&mut s, "draw.rect", json!({"x": 10, "y": 10, "w": 50, "h": 50, "fill": "#ff0000", "stroke": null}));
    for bad in [json!({}), json!({"distance": 0}), json!({"distance": -4}), json!({"distance": 1001}), json!({"distance": "far"}), json!({"distance": 5, "direction": "sideways"}), json!({"distance": 5, "direction": 3})] {
        assert!(s.execute("modify.expand_fill", &bad).is_err(), "{bad}");
        assert!(s.execute("modify.soften_fill_edges", &bad).is_err(), "{bad}");
    }
    for bad in [json!({"distance": 5, "steps": 0}), json!({"distance": 5, "steps": 51}), json!({"distance": 5, "steps": -1})] {
        assert!(s.execute("modify.soften_fill_edges", &bad).is_err(), "{bad}");
    }
    run(&mut s, "edit.deselect", json!({}));
    assert!(s.execute("modify.expand_fill", &json!({"distance": 5})).unwrap_err().to_string().contains("nothing is selected"));
    run(&mut s, "draw.line", json!({"x0": 0, "y0": 0, "x1": 50, "y1": 50}));
    let e = s.execute("modify.expand_fill", &json!({"distance": 5})).unwrap_err().to_string();
    assert!(e.contains("no fills"), "{e}");
}

// ----- Soften Fill Edges --------------------------------------------------------------------------

#[test]
fn soften_makes_one_band_per_step_with_alpha_stepping_down() {
    for (object, inset) in [(true, false), (false, false), (true, true), (false, true)] {
        let mut s = blank(object);
        run(&mut s, "draw.rect", json!({"x": 100, "y": 80, "w": 160, "h": 100, "fill": "#2040c0", "stroke": null}));
        run(&mut s, "edit.select_all", json!({}));
        let v = run(&mut s, "modify.soften_fill_edges", json!({"distance": 20, "steps": 5, "direction": if inset { "inset" } else { "expand" }}));
        assert_eq!(v, json!({"softened": 1, "bands": 5}), "object {object}, inset {inset}");
        let all = shapes(&s);
        assert_eq!(all.len(), 6, "the fill and five bands");
        assert_eq!(s.selection.len(), 6, "all selected, to move or undo as one");
        // By alpha, strongest first: the core, then band 1 (5/6) .. band 5 (1/6).
        let mut by_alpha: Vec<(f32, f64)> = all.iter().map(|(_, sh, m)| (solid(sh).unwrap().a, even_odd_area(&sh.path, m))).collect();
        by_alpha.sort_by(|a, b| b.0.total_cmp(&a.0));
        let outline = |d: f64| if d >= 0.0 { 160.0 * 100.0 + 2.0 * d * 260.0 + PI * d * d } else { (160.0 + 2.0 * d) * (100.0 + 2.0 * d) };
        let base = if inset { -20.0 } else { 0.0 };
        assert!((by_alpha[0].0 - 1.0).abs() < 1e-6 && (by_alpha[0].1 - outline(base)).abs() < 0.5, "the core: {:?}", by_alpha[0]);
        for i in 1..=5u32 {
            let (alpha, area) = by_alpha[i as usize];
            assert!((alpha - band_alpha(i, 5)).abs() < 1e-6 && (alpha - (6 - i) as f32 / 6.0).abs() < 1e-6, "band {i} alpha {alpha}");
            let want = outline(base + 4.0 * f64::from(i)) - outline(base + 4.0 * f64::from(i - 1));
            assert!((area - want).abs() < 0.6, "band {i}: {area} for {want}");
        }
        assert!(all.iter().all(|(_, sh, _)| solid(sh).unwrap().with_alpha(1.0).to_hex() == "#2040c0" && sh.merge != object && sh.stroke.is_none()));
        // The core is on top of its bands.
        assert!((solid(&all[5].1).unwrap().a - 1.0).abs() < 1e-6 || !object);
        run(&mut s, "edit.undo", json!({}));
        assert_eq!(shapes(&s).len(), 1, "one undo step");
    }
}

#[test]
fn soften_defaults_to_four_steps_and_fades_gradients_and_translucent_fills() {
    let mut s = blank(true);
    run(&mut s, "draw.rect", json!({"x": 100, "y": 80, "w": 160, "h": 100, "fill": "#ff000080", "stroke": null}));
    assert_eq!(run(&mut s, "modify.soften_fill_edges", json!({"distance": 8}))["bands"], json!(4));
    let mut alphas: Vec<f32> = shapes(&s).iter().map(|(_, sh, _)| solid(sh).unwrap().a).collect();
    alphas.sort_by(|a, b| b.total_cmp(a));
    let half = Rgba::parse_hex("#ff000080").unwrap().a;
    for (i, a) in alphas.iter().enumerate() {
        assert!((a - half * (5 - i) as f32 / 5.0).abs() < 1e-6, "{alphas:?}");
    }
    // A gradient: every stop fades.
    let mut s = blank(true);
    run(&mut s, "draw.ellipse", json!({"x": 100, "y": 50, "w": 150, "h": 150, "fill": "#ff0000", "stroke": null}));
    run(&mut s, "paint.set_fill", json!({"type": "radial", "stops": [{"offset": 0, "color": "#ffffff"}, {"offset": 1, "color": "#0000ff80"}]}));
    run(&mut s, "modify.soften_fill_edges", json!({"distance": 12, "steps": 2, "direction": "inset"}));
    let all = shapes(&s);
    assert_eq!(all.len(), 3);
    let stop_alphas = |sh: &Shape| sh.fill.as_ref().and_then(|f| f.stops()).map(|st| st.iter().map(|x| x.color.a).collect::<Vec<f32>>()).unwrap();
    let last = Rgba::parse_hex("#0000ff80").unwrap().a;
    let near = |a: &[f32], b: [f32; 2]| (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6;
    assert!(near(&stop_alphas(&all[0].1), [1.0 / 3.0, last / 3.0]), "the outer band is the faintest: {:?}", stop_alphas(&all[0].1));
    assert!(near(&stop_alphas(&all[1].1), [2.0 / 3.0, last * 2.0 / 3.0]));
    assert!(near(&stop_alphas(&all[2].1), [1.0, last]));
    // A bitmap fill has no alpha to step: the command says so.
    let mut s = blank(true);
    run(&mut s, "draw.line", json!({"x0": 0, "y0": 0, "x1": 50, "y1": 50}));
    let e = s.execute("modify.soften_fill_edges", &json!({"distance": 5})).unwrap_err().to_string();
    assert!(e.contains("no colour or gradient fills"), "{e}");
}

#[test]
fn softened_edges_draw_as_steps_of_the_fill_s_colour() {
    // Black on white: each band's ink is its alpha.
    let mut s = blank(false);
    run(&mut s, "draw.rect", json!({"x": 100, "y": 100, "w": 200, "h": 100, "fill": "#000000", "stroke": null}));
    run(&mut s, "edit.select_all", json!({}));
    run(&mut s, "modify.soften_fill_edges", json!({"distance": 30, "steps": 3}));
    let px = ink(&s);
    let at = |x: usize, y: usize| px[y * 400 + x];
    assert!((at(200, 150) - 1.0).abs() < 0.01, "inside");
    for (i, x) in [(1u32, 95usize), (2, 85), (3, 75)] {
        assert!((at(x, 150) - band_alpha(i, 3)).abs() < 0.01, "band {i}: {}", at(x, 150));
    }
    assert!(at(65, 150) < 0.01, "outside");
}

// ----- the colour at a point ----------------------------------------------------------------------

#[test]
fn the_sample_is_the_colour_drawn_there() {
    let mut s = blank(true);
    run(&mut s, "draw.rect", json!({"x": 20, "y": 20, "w": 100, "h": 100, "fill": "#ff0000", "stroke": "#0000ff", "stroke_width": 10, "join": "miter"}));
    // A translucent square over its right half: the sample is what shows, not a paint.
    run(&mut s, "draw.rect", json!({"x": 70, "y": 20, "w": 100, "h": 100, "fill": "#00ff0080", "stroke": null}));
    let before = s.doc.clone();
    let style = s.style.clone();
    let color = |s: &mut Session, x: f32, y: f32| run(s, "paint.sample", json!({"x": x, "y": y}))["color"].as_str().unwrap().to_string();
    assert_eq!(color(&mut s, 50.0, 70.0), "#ff0000", "the fill");
    assert_eq!(color(&mut s, 20.0, 70.0), "#0000ff", "the line");
    assert_eq!(color(&mut s, 300.0, 200.0), "#ffffff", "the stage");
    assert_eq!(color(&mut s, -50.0, 2000.0), "#ffffff", "off the stage: its colour");
    let mixed = run(&mut s, "paint.sample", json!({"x": 100, "y": 70}));
    let (r, g, b) = (mixed["r"].as_u64().unwrap(), mixed["g"].as_u64().unwrap(), mixed["b"].as_u64().unwrap());
    assert!(r > 100 && r < 200 && g > 100 && g < 200 && b == 0, "red under half-clear green: {mixed}");
    assert_eq!(mixed["color"].as_str().unwrap(), format!("#{r:02x}{g:02x}{b:02x}"));
    assert_eq!(s.doc, before, "sampling changes nothing");
    assert_eq!(s.style.fill, style.fill);
    assert!(!s.can_undo() || s.doc == before);
    // A gradient gives the colour where the point is, not its first stop.
    run(&mut s, "draw.rect", json!({"x": 200, "y": 200, "w": 100, "h": 50, "fill": "#000000", "stroke": null}));
    run(&mut s, "paint.set_fill", json!({"type": "linear", "stops": [{"offset": 0, "color": "#000000"}, {"offset": 1, "color": "#ffffff"}]}));
    let grey = |s: &mut Session, x: f32| run(s, "paint.sample", json!({"x": x, "y": 225}))["r"].as_u64().unwrap();
    let (left, mid, right) = (grey(&mut s, 205.0), grey(&mut s, 250.0), grey(&mut s, 295.0));
    assert!(left < 40 && right > 215 && mid > left + 60 && mid + 60 < right, "{left} {mid} {right}");
    // Zoomed out, a pixel covers more: an edge is a blend; zoomed in, it is one side.
    let edge = run(&mut s, "paint.sample", json!({"x": 300.0, "y": 225, "scale": 0.05}));
    assert!(edge["r"].as_u64().unwrap() > 100);
    for bad in [json!({}), json!({"x": 1}), json!({"x": 1, "y": "a"}), json!({"x": 1, "y": 1, "scale": 0}), json!({"x": 1, "y": 1, "scale": 1000}), json!({"x": 1e30, "y": 1})] {
        assert!(s.execute("paint.sample", &bad).is_err(), "{bad}");
    }
}

#[test]
fn the_sample_reads_bitmaps_and_inside_symbols() {
    let mut s = blank(true);
    // A 2 × 1 image, red then blue, shown 100 × 50.
    let png = anicraft_render::encode_png(2, 1, &[255, 0, 0, 255, 0, 0, 255, 255]).unwrap();
    let dir = std::env::temp_dir().join(format!("anicraft-sample-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("two.png");
    std::fs::write(&file, png).unwrap();
    run(&mut s, "file.import", json!({"path": file.to_str().unwrap()}));
    std::fs::remove_dir_all(&dir).ok();
    let b = s.selection_bounds();
    assert!(!b.is_empty(), "the image is on the stage");
    let color = |s: &mut Session, x: f32, y: f32| run(s, "paint.sample", json!({"x": x, "y": y, "scale": 8}))["color"].as_str().unwrap().to_string();
    let (y, quarter) = (b.center().y, b.width() / 4.0);
    assert_eq!(color(&mut s, b.min.x + quarter * 0.5, y), "#ff0000", "a bitmap's own pixel");
    assert_eq!(color(&mut s, b.max.x - quarter * 0.5, y), "#0000ff");
    // Through a symbol instance with a tint: the colour as it shows.
    run(&mut s, "draw.rect", json!({"x": 300, "y": 200, "w": 40, "h": 40, "fill": "#ffffff", "stroke": null}));
    run(&mut s, "modify.convert_to_symbol", json!({"name": "Box"}));
    run(&mut s, "element.set_color_effect", json!({"style": "tint", "tint": "#000000", "tint_amount": 100}));
    assert_eq!(color(&mut s, 320.0, 220.0), "#000000");
}

// ----- Smooth, Straighten and Optimize Curves -----------------------------------------------------

/// The bounds of the visible shapes' curves (flattened, so curve handles past the outline don't count).
fn curve_bounds(s: &Session) -> anicraft_geom::Rect {
    let mut r = anicraft_geom::Rect::EMPTY;
    for (_, sh, m) in shapes(s) {
        for pl in sh.path.flatten(&m, 0.1) {
            for p in pl.points {
                r.include(p);
            }
        }
    }
    r
}

/// Closed circle approximated by `sides` straight segments.
fn circle_svg(cx: f32, cy: f32, r: f32, sides: usize) -> String {
    let mut p = Path::new();
    for i in 0..sides {
        let a = i as f32 / sides as f32 * std::f32::consts::TAU;
        let pt = Vec2::new(cx + r * a.cos(), cy + r * a.sin());
        if i == 0 {
            p.move_to(pt);
        } else {
            p.line_to(pt);
        }
    }
    p.close();
    p.to_svg()
}

#[test]
fn optimize_reduces_points_on_raw_fills_and_drawing_objects_and_undoes() {
    let d = circle_svg(200.0, 150.0, 70.0, 120);
    for object in [false, true] {
        let mut s = blank(object);
        run(&mut s, "draw.path", json!({"d": d, "fill": "#3366cc", "stroke": null, "object": object}));
        run(&mut s, "edit.select_all", json!({}));
        let bounds_before = curve_bounds(&s);
        let before_doc = s.doc.clone();
        let v = run(&mut s, "modify.optimize", json!({"strength": 50}));
        let (before, after) = (v["before"].as_u64().unwrap(), v["after"].as_u64().unwrap());
        assert!(after < before, "object={object}: expected fewer points, got {before} -> {after}");
        assert!(after <= 12, "object={object}: circle simplifies tightly: {after}");
        let bounds_after = curve_bounds(&s);
        assert!((bounds_after.width() - bounds_before.width()).abs() < 4.0 && (bounds_after.height() - bounds_before.height()).abs() < 4.0, "bounds stay close: {bounds_before:?} vs {bounds_after:?}");
        // Fill paint and drawing mode stay intact.
        let all = shapes(&s);
        assert_eq!(all.len(), 1);
        assert_eq!(solid(&all[0].1), Rgba::parse_hex("#3366cc"));
        assert_eq!(all[0].1.merge, !object);
        // One undo step restores the original path.
        run(&mut s, "edit.undo", json!({}));
        assert_eq!(s.doc, before_doc, "object={object}: undo restores");
    }
}

#[test]
fn smooth_and_straighten_preserve_stroke_settings_and_undo() {
    let mut s = blank(true);
    let zig = "M 40 150 L 100 90 L 160 210 L 220 90 L 280 210 L 340 150";
    run(&mut s, "draw.path", json!({"d": zig, "fill": null, "stroke": "#224488", "stroke_width": 7, "cap": "square", "join": "bevel", "dash": [18, 6], "profile": "taper_both", "object": true}));
    let before_stroke = shapes(&s)[0].1.stroke.clone();
    let before_doc = s.doc.clone();
    let v = run(&mut s, "modify.smooth", json!({"strength": 60}));
    assert_eq!(v["changed"], json!(1));
    assert_eq!(shapes(&s)[0].1.stroke, before_stroke, "stroke settings and profile survive smooth");
    let smoothed_bounds = s.selection_bounds();
    assert!(smoothed_bounds.height() < 140.0, "zig-zag amplitude shrinks: {smoothed_bounds:?}");
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(s.doc, before_doc);

    // Straighten turns a gently bent curve into a line while keeping stroke settings.
    let mut s = blank(true);
    run(&mut s, "draw.path", json!({"d": "M 40 150 C 140 156 240 156 340 150", "fill": null, "stroke": "#aa2222", "stroke_width": 5, "cap": "none", "dash": [12, 4], "object": true}));
    let before_stroke = shapes(&s)[0].1.stroke.clone();
    let v = run(&mut s, "modify.straighten", json!({"strength": 50}));
    assert_eq!(v["changed"], json!(1));
    let (_, sh, _) = shapes(&s).remove(0);
    assert_eq!(sh.stroke, before_stroke);
    assert!(matches!(sh.path.cmds.get(1), Some(anicraft_geom::PathCmd::LineTo(_))), "bent curve becomes a line: {:?}", sh.path.cmds);
}

#[test]
fn curve_commands_check_params_and_leave_unchangeable_shapes_alone() {
    let mut s = blank(true);
    for cmd in ["modify.smooth", "modify.straighten", "modify.optimize"] {
        assert!(s.execute(cmd, &json!({})).unwrap_err().to_string().contains("nothing is selected"), "{cmd}");
    }
    // A straight 2-point line cannot simplify further: no error.
    run(&mut s, "draw.line", json!({"x0": 20, "y0": 20, "x1": 120, "y1": 20}));
    assert_eq!(run(&mut s, "modify.straighten", json!({}))["changed"], json!(0));
    assert_eq!(run(&mut s, "modify.optimize", json!({"strength": 0})), json!({"before": 2, "after": 2}));
    assert_eq!(run(&mut s, "modify.optimize", json!({"strength": 80, "repeat": true})), json!({"before": 2, "after": 2}));
    // Invalid params error.
    for bad in [json!({"strength": 0}), json!({"strength": 101}), json!({"strength": -5}), json!({"strength": "lots"})] {
        assert!(s.execute("modify.smooth", &bad).is_err(), "smooth {bad}");
        assert!(s.execute("modify.straighten", &bad).is_err(), "straighten {bad}");
    }
    for bad in [json!({"strength": -1}), json!({"strength": 101}), json!({"repeat": "yes"})] {
        assert!(s.execute("modify.optimize", &bad).is_err(), "optimize {bad}");
    }
}

#[test]
fn optimize_at_zero_strength_does_not_charge_shape_budget() {
    let mut s = blank(true);
    let line = Arc::new(Path::line(Vec2::new(0.0, 0.0), Vec2::new(20.0, 0.0)));
    let stroke = Some(Stroke::new(Rgba::BLACK, 2.0));
    let mut ids = Vec::with_capacity(2001);
    {
        let kf = s.editable_keyframe().unwrap();
        for i in 0..2001u32 {
            let id = ElementId(1000 + u64::from(i));
            ids.push(id);
            kf.elements.push(Arc::new(Element {
                id,
                name: String::new(),
                transform: Affine::translate((i % 50) as f32 * 5.0, (i / 50) as f32 * 5.0),
                color: Default::default(),
                kind: ElementKind::Shape(Shape { path: line.clone(), fill: None, stroke: stroke.clone(), fill_rule: FillRule::default(), merge: false, warp: None }),
            }));
        }
    }
    let layer = s.layer;
    s.selection.extend_on(layer, ids);
    let v = run(&mut s, "modify.optimize", json!({"strength": 0}));
    assert_eq!(v, json!({"before": 4002, "after": 4002}));
    // Positive strength still enforces the 2,000-shape budget.
    let err = s.execute("modify.optimize", &json!({"strength": 50})).unwrap_err().to_string();
    assert!(err.contains("more than 2000"), "{err}");
}

#[test]
fn optimize_reports_after_from_committed_remerged_raw_shapes() {
    let mut s = blank(false);
    // A raw stroke along y = 100 with a shallow 3.5 px dip around x = 150.
    let mut p = Path::new();
    p.move_to(Vec2::new(50.0, 100.0));
    for i in 1..=40 {
        let x = 50.0 + i as f32 * 5.0;
        let dx = (x - 150.0).abs();
        let y = if dx < 20.0 { 100.0 + 3.5 * (1.0 - dx / 20.0) } else { 100.0 };
        p.line_to(Vec2::new(x, y));
    }
    run(&mut s, "draw.path", json!({"d": p.to_svg(), "fill": null, "stroke": "#000000", "stroke_width": 2, "object": false}));
    // A short raw stroke ending just above the original dip (at y = 102.5), which the flattened
    // optimized curve will cross around y = 100.
    run(&mut s, "draw.line", json!({"x0": 150, "y0": 80, "x1": 150, "y1": 102.5, "stroke": "#ff0000", "stroke_width": 2}));
    let all = shapes(&s);
    assert_eq!(all.len(), 2, "the two raw strokes do not touch before optimize");
    let curve_id = all.into_iter().find(|(_, sh, _)| sh.stroke.as_ref().is_some_and(|st| st.color == Rgba::BLACK)).expect("black raw stroke").0;
    s.select_only(vec![curve_id]);
    let v = run(&mut s, "modify.optimize", json!({"strength": 80}));
    let sel = s.selection.on(s.layer);
    assert!(sel.len() >= 2, "remerge split the optimized raw stroke at the new crossing: {sel:?}");
    let committed_anchors: usize = shapes(&s)
        .iter()
        .filter(|(id, ..)| sel.contains(id))
        .map(|(_, sh, _)| sh.path.editable_anchors().len())
        .sum();
    assert_eq!(v["after"].as_u64(), Some(committed_anchors as u64), "reported after must match committed raw shapes: {v}");
}
