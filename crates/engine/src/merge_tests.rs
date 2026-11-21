//! Merge drawing (ADR 0031) through the commands.

use crate::Session;
use anicraft_doc::{ElementKind, Paint, Rgba, Shape};
use serde_json::json;

fn run(s: &mut Session, id: &str, p: serde_json::Value) -> serde_json::Value {
    s.execute(id, &p).unwrap_or_else(|e| panic!("{id}: {e}"))
}

/// A blank document (no sample artwork), drawing raw shapes.
fn blank() -> Session {
    let mut s = Session::new(anicraft_doc::Document::default());
    run(&mut s, "draw.object_drawing", json!({"on": false}));
    s
}

fn shapes(s: &Session) -> Vec<Shape> {
    s.visible_elements()
        .iter()
        .filter_map(|e| match &e.kind {
            ElementKind::Shape(sh) => Some(sh.clone()),
            _ => None,
        })
        .collect()
}

fn count(s: &Session) -> (usize, usize) {
    let sh = shapes(s);
    (sh.iter().filter(|x| x.fill.is_some()).count(), sh.iter().filter(|x| x.stroke.is_some()).count())
}

#[test]
fn a_raw_rectangle_is_a_fill_and_its_sides_selected_together() {
    let mut s = blank();
    let r = run(&mut s, "draw.rect", json!({"x": 0, "y": 0, "w": 100, "h": 80, "fill": "#ff0000", "stroke": "#000000"}));
    assert_eq!(r["merged"], true);
    assert_eq!(count(&s), (1, 4));
    assert!(shapes(&s).iter().all(|x| x.merge));
    assert_eq!(s.selection.len(), 5, "what was drawn is selected");
    // Fills sit under the strokes.
    let fills_first = shapes(&s).iter().position(|x| x.stroke.is_some()).unwrap_or(0);
    assert!(shapes(&s)[..fills_first].iter().all(|x| x.fill.is_some()));
}

#[test]
fn a_shape_on_top_cuts_the_one_below_and_undo_takes_it_back() {
    let mut s = blank();
    run(&mut s, "draw.rect", json!({"x": 0, "y": 0, "w": 100, "h": 100, "fill": "#ff0000", "stroke": null}));
    run(&mut s, "draw.rect", json!({"x": 50, "y": 50, "w": 100, "h": 100, "fill": "#0000ff", "stroke": null}));
    assert_eq!(count(&s), (2, 0));
    let red = shapes(&s).into_iter().find(|x| x.fill == Some(Paint::Solid(Rgba::hex(0xff0000)))).map(|x| x.path.bounds());
    assert!(red.is_some_and(|b| (b.width() - 100.0).abs() < 0.01), "the red keeps its outline, less the corner");
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(count(&s), (1, 0));
    // The same colour joins it.
    run(&mut s, "draw.rect", json!({"x": 50, "y": 50, "w": 100, "h": 100, "fill": "#ff0000", "stroke": null}));
    assert_eq!(count(&s), (1, 0));
    assert_eq!(s.selection.len(), 1);
}

#[test]
fn a_line_across_a_raw_fill_splits_it() {
    let mut s = blank();
    run(&mut s, "draw.ellipse", json!({"x": 0, "y": 0, "w": 100, "h": 100, "fill": "#00ff00", "stroke": null}));
    run(&mut s, "draw.line", json!({"x0": -10, "y0": 50, "x1": 110, "y1": 50, "stroke": "#000000"}));
    assert_eq!(count(&s), (2, 1));
    assert_eq!(s.selection.len(), 1, "the line");
}

#[test]
fn objects_keep_to_themselves() {
    let mut s = blank();
    run(&mut s, "draw.rect", json!({"x": 0, "y": 0, "w": 100, "h": 100, "fill": "#ff0000", "stroke": null}));
    let r = run(&mut s, "draw.rect", json!({"x": 50, "y": 50, "w": 100, "h": 100, "fill": "#0000ff", "stroke": null, "object": true}));
    assert!(r["merged"].is_null());
    assert_eq!(count(&s), (2, 0));
    // Raw shapes drawn over an object do not cut it either.
    run(&mut s, "draw.line", json!({"x0": -10, "y0": 60, "x1": 200, "y1": 60, "stroke": "#000000"}));
    let object = shapes(&s).into_iter().find(|x| !x.merge).map(|x| x.path.bounds());
    assert!(object.is_some_and(|b| (b.width() - 100.0).abs() < 0.01));
    // A new session (scripts) draws objects unless asked.
    let mut fresh = Session::new(anicraft_doc::Document::default());
    run(&mut fresh, "draw.rect", json!({"x": 0, "y": 0, "w": 10, "h": 10, "stroke": "#000000"}));
    assert_eq!(fresh.visible_elements().len(), 1);
    assert_eq!(run(&mut fresh, "draw.object_drawing", json!({}))["object_drawing"], false);
}

#[test]
fn raw_shapes_survive_save_and_load() {
    let mut s = blank();
    run(&mut s, "draw.rect", json!({"x": 0, "y": 0, "w": 40, "h": 40, "fill": "#ff0000", "stroke": "#000000"}));
    let back = crate::format::from_json(&crate::format::to_json(&s.doc).unwrap_or_default()).unwrap_or_default();
    let s2 = Session::new(back);
    assert_eq!(count(&s2), (1, 4));
    assert!(shapes(&s2).iter().all(|x| x.merge));
}

#[test]
fn dropping_a_moved_fill_cuts_what_it_lands_on() {
    let mut s = blank();
    run(&mut s, "draw.rect", json!({"x": 0, "y": 0, "w": 100, "h": 100, "fill": "#ff0000", "stroke": null}));
    run(&mut s, "draw.rect", json!({"x": 200, "y": 0, "w": 50, "h": 50, "fill": "#0000ff", "stroke": null}));
    assert_eq!(count(&s), (2, 0));
    // Move the blue square into the red one and let go.
    run(&mut s, "element.translate", json!({"dx": -175, "dy": 25}));
    let r = run(&mut s, "draw.drop", json!({}));
    assert_eq!(r["dropped"], 1);
    let red = shapes(&s).into_iter().find(|x| x.fill == Some(Paint::Solid(Rgba::hex(0xff0000)))).map(|x| x.path.cmds.iter().filter(|c| matches!(c, anicraft_geom::PathCmd::MoveTo(_))).count());
    assert_eq!(red, Some(2), "the red square has a hole where the blue one landed");
    assert_eq!(s.selection.len(), 1, "the dropped square stays selected");
}

#[test]
fn double_click_selects_connected_strokes_or_a_fill_with_its_outline() {
    let mut s = blank();
    run(&mut s, "draw.rect", json!({"x": 0, "y": 0, "w": 100, "h": 100, "fill": "#ff0000", "stroke": "#000000"}));
    run(&mut s, "draw.line", json!({"x0": 300, "y0": 0, "x1": 400, "y1": 0, "stroke": "#000000"}));
    // One side of the outline: the other three come with it, not the fill or the far line.
    let r = run(&mut s, "element.select_connected", json!({"x": 50, "y": 0}));
    assert_eq!(r["selected"].as_array().map(Vec::len), Some(4));
    // The fill: it and its four sides.
    let r = run(&mut s, "element.select_connected", json!({"x": 50, "y": 50}));
    assert_eq!(r["selected"].as_array().map(Vec::len), Some(5));
    let r = run(&mut s, "element.select_connected", json!({"x": 350, "y": 0}));
    assert_eq!(r["selected"].as_array().map(Vec::len), Some(1));
}

#[test]
fn dashed_raw_strokes_keep_their_style_when_cut() {
    let mut s = blank();
    // A dashed, butt-capped, mitred square outline, then a solid line across it.
    run(&mut s, "draw.rect", json!({"x": 0, "y": 0, "w": 100, "h": 100, "fill": null, "stroke": "#000000", "stroke_width": 2, "cap": "none", "join": "miter", "dash": [6, 3]}));
    run(&mut s, "draw.line", json!({"x0": -20, "y0": 50, "x1": 120, "y1": 50, "stroke": "#000000", "stroke_width": 2}));
    let strokes: Vec<anicraft_doc::Stroke> = shapes(&s).into_iter().filter_map(|x| x.stroke).collect();
    let dashed = strokes.iter().filter(|st| st.style.dash == anicraft_doc::Dash::dashed(6.0, 3.0)).count();
    assert!(dashed >= 5, "the cut sides are pieces, all still dashed: {dashed} of {}", strokes.len());
    assert!(strokes.iter().filter(|st| st.style.dash.is_some()).all(|st| st.style.cap == anicraft_doc::Cap::Butt && st.style.join == anicraft_doc::Join::Miter));
    assert!(strokes.iter().any(|st| st.style.is_default()), "the solid line stays solid, a style of its own");
    // Saved and loaded, the pieces keep it.
    let back = crate::format::from_json(&crate::format::to_json(&s.doc).unwrap_or_default()).unwrap_or_default();
    assert_eq!(back, s.doc);
}

/// #389: changing the stroke of a whole raw drawing changes its stroke pieces. Its fills take no
/// stroke of their own, which would draw the outline twice; a fill selected alone still takes one.
#[test]
fn stroke_changes_on_a_raw_drawing_leave_its_fills_bare() {
    let mut s = blank();
    run(&mut s, "draw.rect", json!({"x": 100, "y": 100, "w": 200, "h": 100, "fill": "#3366ff", "stroke": "#000000", "stroke_width": 4}));
    run(&mut s, "draw.line", json!({"x0": 80, "y0": 150, "x1": 320, "y1": 150, "stroke": "#000000", "stroke_width": 4}));
    let (fills, strokes) = count(&s);
    assert_eq!(fills, 2, "the line cuts the fill in two");
    let bare_fills = |s: &Session| shapes(s).iter().filter(|x| x.fill.is_some() && x.stroke.is_none()).count();
    run(&mut s, "edit.select_all", json!({}));
    for p in [json!({"width": 32}), json!({"color": "#ff0000"}), json!({"style": "dashed"})] {
        run(&mut s, "element.set_stroke", p.clone());
        assert_eq!(count(&s), (fills, strokes), "{p}: no new strokes");
        assert_eq!(bare_fills(&s), 2, "{p}: the fills stay bare");
    }
    assert!(shapes(&s).iter().filter_map(|x| x.stroke.as_ref()).all(|st| st.width == 32.0 && st.color == Rgba::hex(0xff0000)));
    // Selected alone, a raw fill takes the stroke it is given.
    run(&mut s, "edit.deselect", json!({}));
    let fill = s.visible_elements().iter().find(|e| matches!(&e.kind, ElementKind::Shape(sh) if sh.fill.is_some())).map(|e| e.id.0).unwrap();
    run(&mut s, "element.select", json!({"ids": [fill]}));
    run(&mut s, "element.set_stroke", json!({"width": 6}));
    assert_eq!(bare_fills(&s), 1);
}

/// A raw regular `n`-gon of radius 80 at the origin with `fill` and `stroke` (`null` for none).
fn polygon(s: &mut Session, n: u32, fill: &serde_json::Value, stroke: &serde_json::Value) {
    let outer = 80.0f32;
    let inner = outer * (std::f32::consts::PI / n as f32).cos();
    run(s, "draw.star", json!({"cx": 0, "cy": 0, "outer": outer, "inner": inner, "points": n, "fill": fill, "stroke": stroke}));
}

/// A shape tween from polygon `a` on frame 0 to polygon `b` on frame 10 (sides, fill, stroke).
fn polygon_tween(a: (u32, serde_json::Value, serde_json::Value), b: (u32, serde_json::Value, serde_json::Value)) -> Session {
    let mut s = blank();
    polygon(&mut s, a.0, &a.1, &a.2);
    run(&mut s, "timeline.goto", json!({"frame": 10}));
    run(&mut s, "timeline.insert_blank_keyframe", json!({}));
    polygon(&mut s, b.0, &b.1, &b.2);
    run(&mut s, "timeline.goto", json!({"frame": 0}));
    run(&mut s, "timeline.create_shape_tween", json!({}));
    s
}

/// The width of everything stroked at `frame`, and how many strokes there are.
fn stroked_width(s: &Session, frame: u32) -> (f32, usize) {
    let list = anicraft_doc::evaluate(&s.doc, frame);
    let strokes: Vec<_> = list.items.iter().filter(|i| i.stroke.is_some()).collect();
    let r = strokes.iter().fold(anicraft_geom::Rect::EMPTY, |r, i| r.union(&i.path.transformed(&i.transform).bounds()));
    (if strokes.is_empty() { 0.0 } else { r.max.x - r.min.x }, strokes.len())
}

/// A stroke on only one keyframe of a shape tween shrinks away or grows in over the tween, in
/// place of standing still and vanishing at the end (#119).
#[test]
fn a_stroke_on_one_keyframe_only_shrinks_away_or_grows_in() {
    let red = json!("#ff0000");
    // Stroked on the start only: it shrinks to nothing.
    let s = polygon_tween((5, red.clone(), json!("#000000")), (3, red.clone(), json!(null)));
    let (w0, _) = stroked_width(&s, 0);
    let (w5, n5) = stroked_width(&s, 5);
    let (w9, _) = stroked_width(&s, 9);
    assert!(w0 > 100.0, "{w0}");
    assert!(n5 > 0 && (w5 - w0 * 0.5).abs() < w0 * 0.1, "halfway: {w5} of {w0}");
    assert!(w9 < w0 * 0.2, "near the end: {w9} of {w0}");
    assert_eq!(stroked_width(&s, 10).1, 0);
    // Stroked on the end only: it grows from nothing.
    let s = polygon_tween((3, red.clone(), json!(null)), (5, red, json!("#000000")));
    let (w10, _) = stroked_width(&s, 10);
    let (w5, n5) = stroked_width(&s, 5);
    assert!(w10 > 100.0, "{w10}");
    assert_eq!(stroked_width(&s, 0).1, 0);
    assert!(n5 > 0 && (w5 - w10 * 0.5).abs() < w10 * 0.1, "halfway: {w5} of {w10}");
}

/// A filled, stroked raw shape tween that follows a curved motion guide: the joined stroke
/// travels the guide with its fill, oriented to the path or not (#119).
#[test]
fn a_guided_shape_tween_carries_the_stroke_along_the_guide_with_the_fill() {
    let mut s = polygon_tween((3, json!("#ff0000"), json!("#000000")), (5, json!("#ff0000"), json!("#000000")));
    let art = s.layer;
    run(&mut s, "timeline.goto", json!({"frame": 10}));
    run(&mut s, "edit.select_all", json!({}));
    run(&mut s, "element.translate", json!({"dx": 200, "dy": 0}));
    run(&mut s, "layer.add_motion_guide", json!({"layer": art.0}));
    run(&mut s, "draw.path", json!({"d": "M0 0 C0 -300 200 -300 200 0"}));
    let bounds = |items: &[&anicraft_doc::DisplayItem]| items.iter().fold(anicraft_geom::Rect::EMPTY, |r, i| r.union(&i.path.transformed(&i.transform).bounds()));
    for (frame, orient) in [3u32, 5, 7].into_iter().flat_map(|f| [(f, false), (f, true)]) {
        run(&mut s, "layer.select", json!({"layer": art.0}));
        run(&mut s, "timeline.goto", json!({"frame": 0}));
        assert_eq!(run(&mut s, "timeline.set_orient", json!({"orient": orient}))["orient"], orient);
        let list = anicraft_doc::evaluate(&s.doc, frame);
        let on = |want_fill: bool| -> Vec<&anicraft_doc::DisplayItem> {
            list.items.iter().filter(|i| i.layer == art && if want_fill { i.fill.is_some() } else { i.stroke.is_some() }).collect()
        };
        let (fill, stroke) = (bounds(&on(true)), bounds(&on(false)));
        assert!(!fill.is_empty() && !stroke.is_empty(), "frame {frame}");
        assert!(fill.center().y < -60.0, "frame {frame}: the fill rides up the arch: {fill:?}");
        for (a, b) in [(fill.min, stroke.min), (fill.max, stroke.max)] {
            assert!(a.distance(b) < 1.0, "frame {frame} (orient {orient}): the stroke left its fill: {fill:?} vs {stroke:?}");
        }
    }
}

/// A shape hint placed on a raw outline's stroke steers the stroke's morph when the keyframes'
/// outlines have different numbers of runs (#119).
#[test]
fn a_hint_on_a_raw_stroke_steers_its_morph() {
    let mut s = polygon_tween((3, json!(null), json!("#000000")), (5, json!(null), json!("#000000")));
    // The triangle's top corner goes to the pentagon's lower right corner.
    let a = anicraft_geom::Vec2::new(0.0, -80.0);
    let angle = (-90.0f32 + 144.0).to_radians();
    let b = anicraft_geom::Vec2::new(80.0 * angle.cos(), 80.0 * angle.sin());
    let half = a.lerp(b, 0.5);
    let anchor_at = |s: &Session, q: anicraft_geom::Vec2| {
        anicraft_doc::evaluate(&s.doc, 5).items.iter().filter(|i| i.stroke.is_some()).any(|i| {
            i.path.cmds.iter().any(|c| match *c {
                anicraft_geom::PathCmd::MoveTo(p) | anicraft_geom::PathCmd::LineTo(p) | anicraft_geom::PathCmd::CubicTo(_, _, p) => {
                    i.transform.apply(p).distance(q) < 0.5
                }
                _ => false,
            })
        })
    };
    assert!(!anchor_at(&s, half), "without a hint the corner goes elsewhere");
    run(&mut s, "tween.add_hint", json!({}));
    run(&mut s, "tween.move_hint", json!({"hint": "a", "x": a.x, "y": a.y}));
    run(&mut s, "tween.move_hint", json!({"hint": "a", "frame": 10, "x": b.x, "y": b.y}));
    assert!(anchor_at(&s, half), "the corner travels to the hinted point");
}

/// A shape tween between raw polygons with different side counts morphs the outline's stroke
/// with its fill: the stroke covers the whole morphed edge, with no runs left behind (#119).
#[test]
fn a_shape_tween_morphs_the_stroke_with_the_fill() {
    // The same stroke at both ends, and a stroke that changes colour.
    for (from, to, end_stroke) in [(3u32, 5u32, "#000000"), (5, 3, "#000000"), (3, 5, "#0000ff"), (5, 3, "#0000ff")] {
        let s = polygon_tween((from, json!("#ff0000"), json!("#000000")), (to, json!("#ff0000"), json!(end_stroke)));
        for frame in [5u32, 9] {
            let list = anicraft_doc::evaluate(&s.doc, frame);
            let fills: Vec<_> = list.items.iter().filter(|i| i.fill.is_some()).collect();
            let strokes: Vec<_> = list.items.iter().filter(|i| i.stroke.is_some()).collect();
            assert_eq!(fills.len(), 1, "{from}->{to} frame {frame}");
            assert!(!strokes.is_empty(), "{from}->{to} frame {frame}");
            let fill = fills[0];
            // Every point of the fill's edge lies on some stroke, and every stroke on the fill's edge.
            for pl in fill.path.flatten(&fill.transform, 0.25) {
                for &p in &pl.points {
                    let d = strokes.iter().map(|st| st.path.distance_to_edge(p, &st.transform)).fold(f32::INFINITY, f32::min);
                    assert!(d < 1.5, "{from}->{to} frame {frame}: fill edge point {p:?} is {d} from any stroke");
                }
            }
            for st in &strokes {
                for pl in st.path.flatten(&st.transform, 0.25) {
                    for &p in &pl.points {
                        let d = fill.path.distance_to_edge(p, &fill.transform);
                        assert!(d < 1.5, "{from}->{to} frame {frame}: stroke point {p:?} is {d} off the fill's edge");
                    }
                }
            }
        }
    }
}

#[test]
fn chaining_a_long_chain_drawn_right_to_left_does_linear_work() {
    use anicraft_geom::{Path, Vec2, morph::chain_counted};
    let n = 4_000;
    let mut p = Path::default();
    for i in (0..n).rev() {
        let x = i as f32;
        p.move_to(Vec2::new(x, 0.0)).line_to(Vec2::new(x + 1.0, 0.0));
    }
    let (c, work) = chain_counted(&p, 0.01);
    assert!(work <= 100 * n, "{work} work for {n} segments");
    let moves = c.cmds.iter().filter(|m| matches!(m, anicraft_geom::PathCmd::MoveTo(_))).count();
    assert_eq!(moves, 1);
}

/// #487 (ADR 0122): a selection moved onto another fill does not cut it while it stays selected,
/// as in Animate. The editor floats it (`draw.float`) when the drag ends; it can be moved on or
/// back, and it cuts what it lies on only when the selection lets go of it.
#[test]
fn a_moved_selection_floats_until_it_is_deselected() {
    let holes = |s: &Session| {
        shapes(s).into_iter().find(|x| x.fill == Some(Paint::Solid(Rgba::hex(0xff0000)))).map(|x| x.path.cmds.iter().filter(|c| matches!(c, anicraft_geom::PathCmd::MoveTo(_))).count())
    };
    // One drag of the selection, as the Selection tool does it: one undo step.
    let drag = |s: &mut Session, dx: i32, dy: i32| {
        s.begin_gesture("Move");
        run(s, "element.translate", json!({"dx": dx, "dy": dy}));
        let r = run(s, "draw.float", json!({}));
        s.end_gesture();
        r
    };
    let mut s = blank();
    run(&mut s, "draw.rect", json!({"x": 0, "y": 0, "w": 100, "h": 100, "fill": "#ff0000", "stroke": null}));
    run(&mut s, "draw.rect", json!({"x": 200, "y": 0, "w": 50, "h": 50, "fill": "#0000ff", "stroke": null}));
    let blue = s.selection.id_list();
    assert_eq!(blue.len(), 1, "the square just drawn is selected");

    // Onto the red square and let go of the mouse: still two whole shapes, the blue one selected
    // and on top.
    let r = drag(&mut s, -175, 25);
    assert_eq!(r["floating"], 1);
    assert_eq!((count(&s), holes(&s)), ((2, 0), Some(1)), "the red square is untouched under the selection");
    assert_eq!(s.selection.id_list(), blue);
    assert!(s.floating.is_some());
    let top = s.active_layer().and_then(|l| l.keyframe_at(0)).and_then(|k| k.elements.last()).map(|e| e.id.0);
    assert_eq!(top, blue.first().copied(), "the floating shape is drawn over what it lies on");

    // Moved back off it and deselected: nothing was cut.
    drag(&mut s, 175, -25);
    run(&mut s, "edit.deselect", json!({}));
    assert!(s.floating.is_none());
    assert_eq!((count(&s), holes(&s)), ((2, 0), Some(1)));
    // (Settling gives the shape a new id; the drawing is the two whole squares, apart.)
    let apart = s.doc.clone();

    // Moved on again and deselected: now it cuts the red square.
    run(&mut s, "element.pick", json!({"x": 225, "y": 25}));
    drag(&mut s, -175, 25);
    assert_eq!(holes(&s), Some(1));
    run(&mut s, "edit.deselect", json!({}));
    assert_eq!(holes(&s), Some(2), "deselected: the red square has a hole where the blue one lies");
    let cut = s.doc.clone();
    // One undo takes back the move and the cut it led to; redo brings both back.
    run(&mut s, "edit.undo", json!({}));
    assert!(s.doc == apart, "undo: before the move");
    run(&mut s, "edit.redo", json!({}));
    assert!(s.doc == cut, "redo: moved and cut");

    // Selecting something else lets go of it too, and a command that needs the drawing merged
    // (scripts) still has `draw.drop`.
    run(&mut s, "edit.undo", json!({}));
    drag(&mut s, -175, 25);
    run(&mut s, "element.pick", json!({"x": 90, "y": 90}));
    assert!(s.floating.is_none());
    assert_eq!(holes(&s), Some(2), "picking the red square let go of the blue one");
    // Objects (not raw) never float: there is nothing to merge.
    let mut o = blank();
    run(&mut o, "draw.rect", json!({"x": 0, "y": 0, "w": 10, "h": 10, "object": true}));
    assert_eq!(run(&mut o, "draw.float", json!({}))["floating"], 0);
    assert!(o.floating.is_none());
}
