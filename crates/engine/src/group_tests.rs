//! Groups and Combine Objects (ADR 0073).

use crate::Session;
use crate::cmds_group::even_odd_area;
use anicraft_doc::{Element, ElementId, ElementKind};
use anicraft_geom::{Affine, Vec2};
use serde_json::{Value, json};
use std::sync::Arc;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    s.execute(id, &p).unwrap_or_else(|e| panic!("{id}: {e}"))
}

fn id_of(v: &Value, key: &str) -> ElementId {
    ElementId(v[key].as_u64().unwrap_or_else(|| panic!("no {key} in {v}")))
}

fn top(s: &Session) -> Vec<Arc<Element>> {
    s.visible_elements()
}

/// Every drawn item's transform, path and fill, in order: what the movie shows, exactly.
fn picture(s: &Session) -> Vec<([u32; 6], usize, String)> {
    let dl = anicraft_doc::evaluate_scene(&s.doc, s.scene, s.frame);
    dl.items.iter().map(|i| (i.transform.m.map(f32::to_bits), i.path.cmds.len(), format!("{:?}{:?}{:?}", i.fill, i.stroke, i.color))).collect()
}

/// Three objects: a rotated, scaled red square, a blue oval and a green star; Object Drawing on.
fn three() -> (Session, [ElementId; 3]) {
    let mut s = Session::new(anicraft_doc::Document::default());
    let a = run(&mut s, "draw.rect", json!({"x": 10, "y": 20, "w": 100, "h": 60, "fill": "#ff0000", "stroke": "#000000"}));
    let a = id_of(&a, "element");
    run(&mut s, "element.set_transform", json!({"rotation": 33.0, "sx": 1.3, "sy": 0.7}));
    let b = id_of(&run(&mut s, "draw.ellipse", json!({"x": 140, "y": 50, "w": 80, "h": 50, "fill": "#0000ff"})), "element");
    let c = id_of(&run(&mut s, "draw.star", json!({"cx": 300, "cy": 200, "outer": 40, "fill": "#00ff00"})), "element");
    (s, [a, b, c])
}

#[test]
fn a_groups_registration_is_the_centre_of_what_was_grouped() {
    let (mut s, ids) = three();
    let before = picture(&s);
    run(&mut s, "element.select", json!({"ids": ids.map(|i| i.0)}));
    let center = s.selection_bounds().center();
    run(&mut s, "modify.group", json!({}));
    let at = top(&s)[0].transform.translation();
    assert!(at.distance(center) < 0.5, "registration {at:?}, centre of the artwork {center:?}");
    assert!(at.x > 1.0 && at.y > 1.0, "not the corner of the stage: {at:?}");
    assert_eq!(picture(&s), before, "grouping changes nothing drawn");
    run(&mut s, "modify.ungroup", json!({}));
    assert_eq!(picture(&s), before, "ungrouping changes nothing drawn");
}

#[test]
fn group_and_ungroup_keep_every_members_place_exactly() {
    let (mut s, ids) = three();
    let before = picture(&s);
    run(&mut s, "element.select", json!({"ids": ids.map(|i| i.0)}));
    let v = run(&mut s, "modify.group", json!({}));
    let g = id_of(&v, "group");
    assert_eq!(v["members"], json!(3));
    assert_eq!(top(&s).len(), 1, "one group on the layer");
    assert_eq!(s.selection.id_list(), vec![g.0]);
    assert_eq!(picture(&s), before, "grouping changes nothing drawn");
    // Picking anything in it picks the group.
    assert_eq!(s.hit_test(Vec2::new(300.0, 200.0), 1.0).map(|x| x.1), Some(g));
    // One undo step.
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(top(&s).len(), 3);
    run(&mut s, "edit.redo", json!({}));
    // Moved and scaled as one, then ungrouped: each member is where the group showed it, bit for bit.
    run(&mut s, "modify.transform", json!({"a": 1.5, "b": 0.2, "c": -0.1, "d": 0.8, "e": 12.5, "f": -7.25}));
    let moved = picture(&s);
    let v = run(&mut s, "modify.ungroup", json!({}));
    assert_eq!(v["members"], json!(3));
    assert_eq!(picture(&s), moved, "ungrouping changes nothing drawn");
    let back: Vec<ElementId> = top(&s).iter().map(|e| e.id).collect();
    assert_eq!(back, ids.to_vec(), "members keep their ids and order");
    assert_eq!(s.selection.id_list().len(), 3);
}

#[test]
fn groups_nest_and_take_the_topmost_members_place() {
    let (mut s, [a, b, c]) = three();
    let before = picture(&s);
    run(&mut s, "element.select", json!({"ids": [a.0, b.0]}));
    let g1 = id_of(&run(&mut s, "modify.group", json!({})), "group");
    // The group stands where its topmost member stood: under the star.
    assert_eq!(top(&s).iter().map(|e| e.id).collect::<Vec<_>>(), vec![g1, c]);
    run(&mut s, "element.select", json!({"ids": [g1.0, c.0]}));
    let g2 = id_of(&run(&mut s, "modify.group", json!({})), "group");
    assert_eq!(picture(&s), before);
    assert_eq!(crate::cmds_group::group_depth(&top(&s)[0]), 2);
    // Break Apart on a group ungroups it, one level.
    run(&mut s, "element.select", json!({"ids": [g2.0]}));
    run(&mut s, "modify.break_apart", json!({}));
    assert_eq!(top(&s).len(), 2);
    assert_eq!(picture(&s), before);
    // Ungroup a group that holds nothing selected says so.
    run(&mut s, "edit.deselect", json!({}));
    assert!(s.execute("modify.ungroup", &json!({})).is_err());
}

#[test]
fn a_group_opens_in_place_and_edits_there_with_every_tool() {
    let (mut s, ids) = three();
    run(&mut s, "element.select", json!({"ids": ids.map(|i| i.0)}));
    let g = id_of(&run(&mut s, "modify.group", json!({})), "group");
    let reg = top(&s)[0].transform.translation();
    run(&mut s, "modify.transform", json!({"a": 1, "b": 0, "c": 0, "d": 1, "e": 100, "f": 50}));
    // The translation is applied on top of the registration, which is the artwork's centre.
    let placed = reg + Vec2::new(100.0, 50.0);
    let steps = s.history_len();
    let v = run(&mut s, "symbol.edit", json!({"id": g.0}));
    assert_eq!(v["editing"][0]["group"], json!(g.0), "{v}");
    assert!(s.editing_group() && s.editing_symbol().is_none());
    assert_eq!(s.history_len(), steps, "opening is not an edit");
    let view = s.edit_view().unwrap_or_else(|| panic!("open"));
    assert!(view.in_place && view.origin.distance(placed) < 1e-3, "{view:?} registration {reg:?}");
    // Inside: the members, in the group's space.
    assert_eq!(top(&s).len(), 3);
    run(&mut s, "draw.rect", json!({"x": 0, "y": 0, "w": 10, "h": 10, "fill": "#123456"}));
    assert_eq!(top(&s).len(), 4);
    // A group is one layer and one frame.
    assert!(s.execute("layer.new", &json!({})).is_err());
    assert!(s.execute("timeline.insert_keyframe", &json!({})).is_err());
    let _ = s.execute("timeline.goto", &json!({"frame": 5}));
    assert_eq!(s.frame, 0, "the playhead stays on the group's frame");
    // The stage shows the edit where the group stands, the rest dimmed.
    let pic = s.stage_picture();
    assert!(pic.front.is_some());
    run(&mut s, "symbol.exit", json!({}));
    assert_eq!(s.selection.id_list(), vec![g.0], "the group is selected again");
    let members = top(&s)[0].kind.group_members();
    assert_eq!(members.len(), 4);
    let new = members.last().cloned().unwrap_or_else(|| panic!("member"));
    // The 10×10 rect was drawn at the group's local origin, so its own centre is 5 px from the registration.
    let world = top(&s)[0].transform.mul(&new.transform).translation();
    let want = placed + Vec2::new(5.0, 5.0);
    assert!(world.distance(want) < 1e-3, "new rect at {world:?}, want {want:?}");
    assert!(matches!(new.kind, ElementKind::Shape(_)));
    // Undo goes back inside, where the change was made.
    run(&mut s, "edit.undo", json!({}));
    assert!(s.editing_group());
    assert_eq!(top(&s).len(), 3);
    // An ungroup undone from inside closes the group when it goes.
    run(&mut s, "symbol.exit", json!({}));
    run(&mut s, "modify.ungroup", json!({}));
    assert!(s.editing.is_empty());
}

#[test]
fn raw_shapes_in_a_group_merge_only_with_each_other() {
    let mut s = Session::new(anicraft_doc::Document::default());
    run(&mut s, "draw.object_drawing", json!({"on": false}));
    run(&mut s, "draw.rect", json!({"x": 0, "y": 0, "w": 100, "h": 100, "fill": "#ff0000", "stroke": null}));
    run(&mut s, "edit.select_all", json!({}));
    let g = id_of(&run(&mut s, "modify.group", json!({})), "group");
    // A raw square drawn over it leaves the grouped square whole.
    run(&mut s, "draw.rect", json!({"x": 50, "y": 50, "w": 100, "h": 100, "fill": "#0000ff", "stroke": null}));
    let group = top(&s).into_iter().find(|e| e.id == g).unwrap_or_else(|| panic!("group"));
    let m = group.kind.group_members();
    assert_eq!(m.len(), 1);
    let ElementKind::Shape(sh) = &m[0].kind else { panic!("shape") };
    assert!((even_odd_area(&sh.path, &m[0].transform) - 10_000.0).abs() < 1.0);
    // The group sits above the raw drawing, as objects do. Inside, the square occupies local
    // −50..50 (its registration is the centre). A raw square from local (0, −50) covers stage
    // 50..150, 0..100 and joins it.
    run(&mut s, "symbol.edit", json!({"id": g.0}));
    run(&mut s, "draw.rect", json!({"x": 0, "y": -50, "w": 100, "h": 100, "fill": "#ff0000", "stroke": null}));
    let fills = top(&s);
    assert_eq!(fills.len(), 1, "one red fill: the two squares joined");
    run(&mut s, "symbol.exit", json!({}));
    // Ungrouped, the red fill merges with the blue square it now stands in.
    run(&mut s, "modify.ungroup", json!({}));
    let raw = top(&s);
    assert!(raw.iter().all(|e| matches!(&e.kind, ElementKind::Shape(sh) if sh.merge)), "all raw");
    let red: f64 = raw.iter().filter_map(|e| match &e.kind {
        ElementKind::Shape(sh) if sh.fill == Some(anicraft_doc::Paint::Solid(anicraft_doc::Rgba::hex(0xff0000))) => Some(even_odd_area(&sh.path, &e.transform)),
        _ => None,
    }).sum();
    // Red (150 × 100) on top of blue: all of it shows.
    assert!((red - 15_000.0).abs() < 2.0, "{red}");
}

#[test]
fn copies_of_a_group_ungroup_with_their_own_ids() {
    let (mut s, ids) = three();
    run(&mut s, "element.select", json!({"ids": ids.map(|i| i.0)}));
    run(&mut s, "modify.group", json!({}));
    run(&mut s, "edit.duplicate", json!({"dx": 10, "dy": 10}));
    run(&mut s, "edit.select_all", json!({}));
    run(&mut s, "modify.ungroup", json!({}));
    let all: Vec<u64> = top(&s).iter().map(|e| e.id.0).collect();
    let mut unique = all.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!((all.len(), unique.len()), (6, 6), "{all:?}");
}

#[test]
fn groups_save_and_open_and_older_files_have_none() {
    let (mut s, ids) = three();
    let plain = crate::format::to_json(&s.doc).unwrap_or_default();
    assert!(!plain.contains("Group"), "nothing new in files without groups");
    run(&mut s, "element.select", json!({"ids": [ids[0].0, ids[1].0]}));
    run(&mut s, "modify.group", json!({}));
    run(&mut s, "element.select", json!({"ids": [ids[2].0]}));
    let text = crate::format::to_json(&s.doc).unwrap_or_default();
    let back = crate::format::from_json(&text).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(back, s.doc);
    // Ids inside groups count: what a loaded document makes next is new.
    let mut d = back.clone();
    let next = d.alloc_id();
    assert!(next > ids.iter().map(|i| i.0).max().unwrap_or(0));
}

#[test]
fn symbols_inside_groups_count_as_used_and_go_with_their_symbol() {
    let mut s = Session::new(anicraft_doc::Document::default());
    run(&mut s, "draw.rect", json!({"x": 0, "y": 0, "w": 50, "h": 50, "fill": "#ff0000"}));
    let sym = run(&mut s, "modify.convert_to_symbol", json!({"name": "Box"}))["symbol"].as_u64().unwrap_or(0);
    run(&mut s, "draw.rect", json!({"x": 100, "y": 0, "w": 50, "h": 50, "fill": "#00ff00"}));
    run(&mut s, "edit.select_all", json!({}));
    run(&mut s, "modify.group", json!({}));
    assert!(s.doc.unused_items().is_empty(), "the instance inside the group uses it");
    assert_eq!(s.doc.library_uses().len(), 1);
    // Deleting the symbol takes its instance out of the group, and keeps the rest.
    run(&mut s, "library.delete", json!({"items": [{"symbol": sym}], "confirm": true}));
    let g = top(&s);
    assert_eq!(g.len(), 1);
    assert_eq!(g[0].kind.group_members().len(), 1);
}

/// Two 100 × 100 squares, overlapping by 50 × 50, as drawing objects; the second on top.
fn squares(s: &mut Session) {
    run(s, "draw.rect", json!({"x": 0, "y": 0, "w": 100, "h": 100, "fill": "#ff0000", "stroke": null}));
    run(s, "draw.rect", json!({"x": 50, "y": 50, "w": 100, "h": 100, "fill": "#0000ff", "stroke": null}));
    run(s, "edit.select_all", json!({}));
}

fn areas(s: &Session) -> Vec<f64> {
    top(s)
        .iter()
        .map(|e| match &e.kind {
            ElementKind::Shape(sh) => even_odd_area(&sh.path, &e.transform),
            ElementKind::Group { .. } => e.kind.group_members().iter().filter_map(|m| match &m.kind {
                ElementKind::Shape(sh) if sh.fill.is_some() => Some(even_odd_area(&sh.path, &e.transform.mul(&m.transform))),
                _ => None,
            }).sum(),
            _ => 0.0,
        })
        .collect()
}

#[test]
fn combine_objects_areas_are_exact() {
    let near = |a: &[f64], b: &[f64]| a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.5);
    // Union of two colours: a group of the merged pieces, whose area is the union's.
    let mut s = Session::new(anicraft_doc::Document::default());
    squares(&mut s);
    let v = run(&mut s, "modify.combine", json!({"op": "union"}));
    assert_eq!(v["created"].as_array().map(Vec::len), Some(1));
    assert!(matches!(top(&s)[0].kind, ElementKind::Group { .. }));
    assert!(near(&areas(&s), &[17_500.0]), "{:?}", areas(&s));
    // Two squares, 0..100 and 50..150: the group's registration is the centre of that union.
    let at = top(&s)[0].transform.translation();
    assert!(at.distance(Vec2::new(75.0, 75.0)) < 1.0, "combine group registration {at:?}");
    // Intersect: the overlap, in the top's colour.
    let mut s = Session::new(anicraft_doc::Document::default());
    squares(&mut s);
    run(&mut s, "modify.intersect", json!({}));
    assert!(near(&areas(&s), &[2_500.0]), "{:?}", areas(&s));
    assert!(matches!(&top(&s)[0].kind, ElementKind::Shape(sh) if sh.fill == Some(anicraft_doc::Paint::Solid(anicraft_doc::Rgba::hex(0x0000ff))) && !sh.merge));
    // Punch: the bottom square less the top one, which goes.
    let mut s = Session::new(anicraft_doc::Document::default());
    squares(&mut s);
    run(&mut s, "modify.punch", json!({}));
    assert!(near(&areas(&s), &[7_500.0]), "{:?}", areas(&s));
    // Crop: the bottom square inside the top one.
    let mut s = Session::new(anicraft_doc::Document::default());
    squares(&mut s);
    run(&mut s, "modify.crop", json!({}));
    assert!(near(&areas(&s), &[2_500.0]), "{:?}", areas(&s));
    assert!(matches!(&top(&s)[0].kind, ElementKind::Shape(sh) if sh.fill == Some(anicraft_doc::Paint::Solid(anicraft_doc::Rgba::hex(0xff0000)))));
    // Union of one colour: one drawing object.
    let mut s = Session::new(anicraft_doc::Document::default());
    run(&mut s, "draw.rect", json!({"x": 0, "y": 0, "w": 100, "h": 100, "fill": "#ff0000", "stroke": null}));
    run(&mut s, "draw.rect", json!({"x": 50, "y": 50, "w": 100, "h": 100, "fill": "#ff0000", "stroke": null}));
    run(&mut s, "edit.select_all", json!({}));
    run(&mut s, "modify.union", json!({}));
    assert!(matches!(&top(&s)[0].kind, ElementKind::Shape(sh) if !sh.merge), "one object");
    assert!(near(&areas(&s), &[17_500.0]), "{:?}", areas(&s));
    // A hole punched in the middle: a ring, 100² − 40².
    let mut s = Session::new(anicraft_doc::Document::default());
    run(&mut s, "draw.rect", json!({"x": 0, "y": 0, "w": 100, "h": 100, "fill": "#ff0000", "stroke": null}));
    run(&mut s, "draw.rect", json!({"x": 30, "y": 30, "w": 40, "h": 40, "fill": "#0000ff", "stroke": null}));
    run(&mut s, "edit.select_all", json!({}));
    run(&mut s, "modify.punch", json!({}));
    assert!(near(&areas(&s), &[8_400.0]), "{:?}", areas(&s));
    // A circle cropped by a square through its centre: a quarter of π r² (cubic arcs: 0.03 %).
    let mut s = Session::new(anicraft_doc::Document::default());
    run(&mut s, "draw.ellipse", json!({"x": -100, "y": -100, "w": 200, "h": 200, "fill": "#ff0000", "stroke": null}));
    run(&mut s, "draw.rect", json!({"x": 0, "y": 0, "w": 200, "h": 200, "fill": "#0000ff", "stroke": null}));
    run(&mut s, "edit.select_all", json!({}));
    run(&mut s, "modify.crop", json!({}));
    let a = areas(&s);
    let quarter = std::f64::consts::PI * 100.0 * 100.0 / 4.0;
    assert!(a.len() == 1 && (a[0] - quarter).abs() < quarter * 0.001, "{a:?} vs {quarter}");
}

#[test]
fn combine_refuses_what_it_cannot_combine() {
    let mut s = Session::new(anicraft_doc::Document::default());
    run(&mut s, "draw.rect", json!({"x": 0, "y": 0, "w": 100, "h": 100, "fill": "#ff0000"}));
    run(&mut s, "edit.select_all", json!({}));
    assert!(s.execute("modify.union", &json!({})).is_err(), "one shape");
    run(&mut s, "draw.rect", json!({"x": 300, "y": 0, "w": 100, "h": 100, "fill": "#ff0000"}));
    run(&mut s, "edit.select_all", json!({}));
    assert!(s.execute("modify.intersect", &json!({})).is_err(), "apart: nothing in common");
    assert!(s.execute("modify.combine", &json!({"op": "xor"})).is_err());
    // The document is as it was after a refusal.
    assert_eq!(top(&s).len(), 2);
    // A line can't punch (it has no area).
    run(&mut s, "draw.line", json!({"x0": 0, "y0": 50, "x1": 400, "y1": 50, "stroke": "#000000"}));
    run(&mut s, "edit.select_all", json!({}));
    assert!(s.execute("modify.punch", &json!({})).is_err());
}

#[test]
fn create_object_turns_one_raw_shape_into_a_drawing_object_and_undoes() {
    let mut s = Session::new(anicraft_doc::Document::default());
    run(&mut s, "draw.object_drawing", json!({"on": false}));
    run(&mut s, "draw.rect", json!({"x": 20, "y": 30, "w": 100, "h": 60, "fill": "#ff0000", "stroke": null}));
    run(&mut s, "edit.select_all", json!({}));
    assert!(matches!(&top(&s)[0].kind, ElementKind::Shape(sh) if sh.merge), "starts as raw");
    let before_bounds = s.selection_bounds();
    let before_pic = picture(&s);
    let v = run(&mut s, "modify.create_object", json!({}));
    let created: Vec<u64> = v["created"].as_array().into_iter().flatten().filter_map(Value::as_u64).collect();
    assert_eq!(created.len(), 1, "{v}");
    let els = top(&s);
    assert_eq!(els.len(), 1);
    assert!(matches!(&els[0].kind, ElementKind::Shape(sh) if !sh.merge), "became a drawing object");
    assert_eq!(s.selection.id_list(), created, "new drawing object stays selected");
    let after_bounds = s.selection_bounds();
    assert!(after_bounds.min.distance(before_bounds.min) < 1e-3 && after_bounds.max.distance(before_bounds.max) < 1e-3, "bounds {after_bounds:?} vs {before_bounds:?}");
    assert_eq!(picture(&s), before_pic, "look unchanged");
    // Undo restores the raw shape.
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(top(&s).len(), 1);
    assert!(matches!(&top(&s)[0].kind, ElementKind::Shape(sh) if sh.merge), "undo restores raw");
    // Partial raw selection cut out by a marquee becomes a drawing object, and undo heals the cut.
    run(&mut s, "raw.select_rect", json!({"x0": 70, "y0": 0, "x1": 150, "y1": 120}));
    let total_before: f64 = areas(&s).into_iter().sum();
    let v = run(&mut s, "modify.create_object", json!({}));
    let cut_id = v["created"][0].as_u64().unwrap_or(0);
    assert_eq!(s.selection.id_list(), vec![cut_id]);
    let els = top(&s);
    assert_eq!(els.len(), 2);
    assert!(els.iter().any(|e| e.id.0 == cut_id && matches!(&e.kind, ElementKind::Shape(sh) if !sh.merge)), "marquee part became drawing object");
    assert!(els.iter().any(|e| e.id.0 != cut_id && matches!(&e.kind, ElementKind::Shape(sh) if sh.merge)), "rest stays raw");
    let total_after: f64 = areas(&s).into_iter().sum();
    assert!((total_after - total_before).abs() < 1.0, "{total_after} vs {total_before}");
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(top(&s).len(), 1, "undo heals the marquee cut in one step");
    assert!(matches!(&top(&s)[0].kind, ElementKind::Shape(sh) if sh.merge));
}

#[test]
fn create_object_combines_two_separate_raw_shapes() {
    // Two separate raw shapes of the same fill become one drawing object.
    let mut s = Session::new(anicraft_doc::Document::default());
    run(&mut s, "draw.object_drawing", json!({"on": false}));
    run(&mut s, "draw.rect", json!({"x": 0, "y": 0, "w": 60, "h": 60, "fill": "#ff0000", "stroke": null}));
    run(&mut s, "draw.rect", json!({"x": 120, "y": 0, "w": 60, "h": 60, "fill": "#ff0000", "stroke": null}));
    run(&mut s, "edit.select_all", json!({}));
    assert_eq!(top(&s).len(), 2);
    let before_bounds = s.selection_bounds();
    let before_area: f64 = areas(&s).into_iter().sum();
    let v = run(&mut s, "modify.create_object", json!({}));
    let created: Vec<u64> = v["created"].as_array().into_iter().flatten().filter_map(Value::as_u64).collect();
    assert_eq!(created.len(), 1, "{v}");
    let els = top(&s);
    assert_eq!(els.len(), 1);
    assert!(matches!(&els[0].kind, ElementKind::Shape(sh) if !sh.merge), "one drawing object");
    assert_eq!(s.selection.id_list(), created);
    let after_bounds = s.selection_bounds();
    assert!(after_bounds.min.distance(before_bounds.min) < 1e-3 && after_bounds.max.distance(before_bounds.max) < 1e-3);
    assert!((areas(&s).into_iter().sum::<f64>() - before_area).abs() < 1.0);
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(top(&s).len(), 2);
    // Two separate raw shapes of different fills become a group per Union's rule.
    let mut s = Session::new(anicraft_doc::Document::default());
    run(&mut s, "draw.object_drawing", json!({"on": false}));
    run(&mut s, "draw.rect", json!({"x": 0, "y": 0, "w": 60, "h": 60, "fill": "#ff0000", "stroke": null}));
    run(&mut s, "draw.rect", json!({"x": 120, "y": 0, "w": 60, "h": 60, "fill": "#0000ff", "stroke": null}));
    run(&mut s, "edit.select_all", json!({}));
    let before_area: f64 = areas(&s).into_iter().sum();
    let v = run(&mut s, "modify.create_object", json!({}));
    let created: Vec<u64> = v["created"].as_array().into_iter().flatten().filter_map(Value::as_u64).collect();
    assert_eq!(created.len(), 1, "{v}");
    assert_eq!(top(&s).len(), 1);
    assert!(matches!(top(&s)[0].kind, ElementKind::Group { .. }), "multi-colour union becomes a group");
    assert_eq!(s.selection.id_list(), created);
    assert!((areas(&s).into_iter().sum::<f64>() - before_area).abs() < 1.0);
}

#[test]
fn create_object_refuses_drawing_objects_and_non_shapes() {
    let mut s = Session::new(anicraft_doc::Document::default());
    run(&mut s, "draw.rect", json!({"x": 0, "y": 0, "w": 80, "h": 80, "fill": "#ff0000", "object": true}));
    run(&mut s, "edit.select_all", json!({}));
    let before = s.doc.clone();
    assert!(matches!(s.execute("modify.create_object", &json!({})), Err(crate::EngineError::State(_))), "only a drawing object selected");
    assert_eq!(s.doc, before, "document unchanged on refusal");
    // A symbol in the selection is refused like Combine Objects.
    run(&mut s, "modify.convert_to_symbol", json!({"name": "Sym"}));
    run(&mut s, "draw.rect", json!({"x": 100, "y": 0, "w": 50, "h": 50, "fill": "#00ff00", "object": false}));
    run(&mut s, "edit.select_all", json!({}));
    let before = s.doc.clone();
    assert!(matches!(s.execute("modify.create_object", &json!({})), Err(crate::EngineError::State(_))), "symbol in selection");
    assert_eq!(s.doc, before, "document unchanged on symbol refusal");
}

#[test]
fn combine_and_create_object_refuse_groups_past_max_depth() {
    let mut s = Session::new(anicraft_doc::Document::default());
    run(&mut s, "draw.rect", json!({"x": 0, "y": 0, "w": 40, "h": 40, "fill": "#ff0000", "stroke": null}));
    for _ in 0..anicraft_doc::MAX_GROUP_DEPTH {
        run(&mut s, "edit.select_all", json!({}));
        let g = id_of(&run(&mut s, "modify.group", json!({})), "group");
        run(&mut s, "symbol.edit", json!({"id": g.0}));
    }
    assert_eq!(crate::cmds_group::open_groups(&s), anicraft_doc::MAX_GROUP_DEPTH);
    // Two disjoint raw shapes with different fills would produce a group at depth MAX_GROUP_DEPTH + 1.
    run(&mut s, "edit.select_all", json!({}));
    run(&mut s, "edit.delete", json!({}));
    run(&mut s, "draw.object_drawing", json!({"on": false}));
    run(&mut s, "draw.rect", json!({"x": 0, "y": 0, "w": 40, "h": 40, "fill": "#ff0000", "stroke": null}));
    run(&mut s, "draw.rect", json!({"x": 80, "y": 0, "w": 40, "h": 40, "fill": "#0000ff", "stroke": null}));
    run(&mut s, "edit.select_all", json!({}));
    let before = s.doc.clone();
    for cmd in ["modify.create_object", "modify.union"] {
        assert!(matches!(s.execute(cmd, &json!({})), Err(crate::EngineError::State(ref m)) if m.contains("groups nest at most")), "{cmd} should refuse past max depth");
        assert_eq!(s.doc, before, "{cmd} leaves document unchanged on refusal");
    }
    // Punch and Crop on a stroked shape (fill + stroke produces a group) must also refuse.
    run(&mut s, "edit.select_all", json!({}));
    run(&mut s, "edit.delete", json!({}));
    run(&mut s, "draw.object_drawing", json!({"on": true}));
    run(&mut s, "draw.rect", json!({"x": 0, "y": 0, "w": 100, "h": 100, "fill": "#ff0000", "stroke": "#000000"}));
    run(&mut s, "draw.rect", json!({"x": 50, "y": 50, "w": 100, "h": 100, "fill": "#0000ff", "stroke": null}));
    run(&mut s, "edit.select_all", json!({}));
    let before = s.doc.clone();
    for cmd in ["modify.punch", "modify.crop"] {
        assert!(matches!(s.execute(cmd, &json!({})), Err(crate::EngineError::State(ref m)) if m.contains("groups nest at most")), "{cmd} should refuse past max depth");
        assert_eq!(s.doc, before, "{cmd} leaves document unchanged on refusal");
    }
    // Results that stay a single drawing object (not a group) remain allowed at MAX_GROUP_DEPTH.
    run(&mut s, "edit.select_all", json!({}));
    run(&mut s, "edit.delete", json!({}));
    run(&mut s, "draw.object_drawing", json!({"on": false}));
    run(&mut s, "draw.rect", json!({"x": 0, "y": 0, "w": 40, "h": 40, "fill": "#ff0000", "stroke": null}));
    run(&mut s, "draw.rect", json!({"x": 80, "y": 0, "w": 40, "h": 40, "fill": "#ff0000", "stroke": null}));
    run(&mut s, "edit.select_all", json!({}));
    run(&mut s, "modify.create_object", json!({}));
    assert_eq!(top(&s).len(), 1);
    assert!(matches!(&top(&s)[0].kind, ElementKind::Shape(sh) if !sh.merge), "same-fill create_object produces a drawing object");
    // Single-shape Create Object also stays allowed at MAX_GROUP_DEPTH.
    run(&mut s, "draw.rect", json!({"x": 200, "y": 0, "w": 40, "h": 40, "fill": "#00ff00", "stroke": null}));
    let raw_id = top(&s).iter().find(|e| matches!(&e.kind, ElementKind::Shape(sh) if sh.merge)).map(|e| e.id).unwrap_or_else(|| panic!("raw"));
    run(&mut s, "element.select", json!({"ids": [raw_id.0]}));
    run(&mut s, "modify.create_object", json!({}));
    assert!(top(&s).iter().all(|e| matches!(&e.kind, ElementKind::Shape(sh) if !sh.merge)), "single raw shape became a drawing object");
}

#[test]
fn create_object_refuses_warped_bitmap_fill_but_allows_warped_solid_fill() {
    // Mixed selection with a raw shape and a warped bitmap-fill drawing object: refused,
    // leaving the document, selection and undo history untouched.
    let mut s = Session::new(anicraft_doc::Document::default());
    let bm = anicraft_doc::BitmapId(s.doc.alloc_id());
    let mut rgba = Vec::with_capacity(40 * 20 * 4);
    for y in 0..20u32 {
        for x in 0..40u32 {
            rgba.extend_from_slice(&[(x * 6) as u8, (y * 12) as u8, 128, 255]);
        }
    }
    let png = anicraft_render::encode_png(40, 20, &rgba).unwrap_or_else(|e| panic!("encode_png: {e}"));
    s.doc.bitmaps.push(Arc::new(anicraft_doc::Bitmap::from_png(bm, "ramp.png", png).unwrap_or_else(|| panic!("bitmap"))));
    let bmp = id_of(&run(&mut s, "library.place", json!({"bitmap": bm.0, "x": 300, "y": 230})), "element");
    run(&mut s, "warp.pin_add", json!({"element": bmp.0, "x": 285, "y": 230}));
    let pin = run(&mut s, "warp.pin_add", json!({"element": bmp.0, "x": 315, "y": 230}))["pin"].as_u64().unwrap_or_else(|| panic!("pin"));
    run(&mut s, "warp.pin_move", json!({"element": bmp.0, "pin": pin, "x": 315, "y": 270}));
    assert!(
        top(&s).iter().any(|e| e.id == bmp && matches!(&e.kind, ElementKind::Shape(sh) if !sh.merge && sh.warp.is_some() && matches!(sh.fill, Some(anicraft_doc::Paint::Bitmap { .. })))),
        "placed bitmap must be a warped bitmap-fill drawing object"
    );
    run(&mut s, "draw.rect", json!({"x": 20, "y": 20, "w": 60, "h": 60, "fill": "#ff0000", "stroke": null, "object": false}));
    run(&mut s, "edit.select_all", json!({}));
    let before_doc = s.doc.clone();
    let before_sel = s.selection.id_list();
    let before_hist = s.history_len();
    assert!(
        matches!(s.execute("modify.create_object", &json!({})), Err(crate::EngineError::State(ref m)) if m.contains("warped bitmap")),
        "warped bitmap fill in mixed selection must be refused"
    );
    assert_eq!(s.doc, before_doc, "document unchanged on warped bitmap refusal");
    assert_eq!(s.selection.id_list(), before_sel, "selection unchanged on warped bitmap refusal");
    assert_eq!(s.history_len(), before_hist, "no undo step recorded on warped bitmap refusal");

    // Mixed selection with a raw shape and a warped solid-fill drawing object: still converts,
    // preserving the bent outline.
    let mut s = Session::new(anicraft_doc::Document::default());
    let solid = id_of(&run(&mut s, "draw.rect", json!({"x": 100, "y": 100, "w": 200, "h": 80, "fill": "#3080ff", "stroke": null, "object": true})), "element");
    run(&mut s, "warp.pin_add", json!({"element": solid.0, "x": 110, "y": 140}));
    let pin = run(&mut s, "warp.pin_add", json!({"element": solid.0, "x": 290, "y": 140}))["pin"].as_u64().unwrap_or_else(|| panic!("pin"));
    run(&mut s, "warp.pin_move", json!({"element": solid.0, "pin": pin, "x": 290, "y": 200}));
    let bent_bounds = top(&s)[0].bounds(&s.doc);
    assert!(bent_bounds.max.y > 180.0, "solid drawing object was bent downward: {bent_bounds:?}");
    run(&mut s, "draw.rect", json!({"x": 10, "y": 10, "w": 50, "h": 50, "fill": "#3080ff", "stroke": null, "object": false}));
    run(&mut s, "edit.select_all", json!({}));
    let before_area: f64 = areas(&s).into_iter().sum();
    let v = run(&mut s, "modify.create_object", json!({}));
    let created: Vec<u64> = v["created"].as_array().into_iter().flatten().filter_map(Value::as_u64).collect();
    assert_eq!(created.len(), 1, "{v}");
    let els = top(&s);
    assert_eq!(els.len(), 1);
    assert!(matches!(&els[0].kind, ElementKind::Shape(sh) if !sh.merge), "warped solid + raw same fill becomes a drawing object");
    assert_eq!(s.selection.id_list(), created);
    assert!(els[0].bounds(&s.doc).max.y >= bent_bounds.max.y - 1.0, "bent outline kept after Create Object");
    let after_area: f64 = areas(&s).into_iter().sum();
    assert!((after_area - before_area).abs() < 1.0, "combined area preserved: {after_area} vs {before_area}");
}

#[test]
fn ctrl_drag_pulls_a_sharp_corner_out_of_a_line() {
    // What the Selection tool's Ctrl-drag runs (issue #200): a point where the edge was grabbed,
    // then that point moved, in one gesture.
    let mut s = Session::new(anicraft_doc::Document::default());
    run(&mut s, "draw.object_drawing", json!({"on": false}));
    run(&mut s, "draw.rect", json!({"x": 0, "y": 0, "w": 100, "h": 100, "fill": "#ff0000", "stroke": "#000000"}));
    run(&mut s, "edit.deselect", json!({}));
    s.begin_gesture("Pull Corner");
    let v = run(&mut s, "raw.add_point", json!({"x": 50, "y": 0.4, "slop": 3}));
    let (x, y) = (v["x"].as_f64().unwrap_or(-1.0), v["y"].as_f64().unwrap_or(-1.0));
    assert!((x - 50.0).abs() < 0.5 && y.abs() < 1e-3, "{v}");
    run(&mut s, "raw.move_point", json!({"x": x, "y": y, "to_x": 50, "to_y": -40, "merge": false}));
    let ids: Vec<u64> = top(&s).iter().map(|e| e.id.0).collect();
    run(&mut s, "raw.merge", json!({"ids": ids}));
    s.end_gesture();
    // The fill gained a triangle 100 wide and 40 high: straight sides meeting at a sharp point.
    let fill = top(&s).into_iter().find(|e| matches!(&e.kind, ElementKind::Shape(sh) if sh.fill.is_some())).unwrap_or_else(|| panic!("fill"));
    let ElementKind::Shape(sh) = &fill.kind else { panic!("shape") };
    assert!((even_odd_area(&sh.path, &fill.transform) - 12_000.0).abs() < 0.5);
    assert!(sh.path.cmds.iter().all(|c| !matches!(c, anicraft_geom::PathCmd::CubicTo(..) | anicraft_geom::PathCmd::QuadTo(..))), "straight lines, no curve: {:?}", sh.path.cmds);
    assert!(s.history_len() >= 2);
    run(&mut s, "edit.undo", json!({}));
    let fill = top(&s).into_iter().find(|e| matches!(&e.kind, ElementKind::Shape(sh) if sh.fill.is_some())).unwrap_or_else(|| panic!("fill"));
    let ElementKind::Shape(sh) = &fill.kind else { panic!("shape") };
    assert!((even_odd_area(&sh.path, &fill.transform) - 10_000.0).abs() < 0.5, "one undo step");
}

#[test]
fn alt_drag_copies_leave_the_original_whole() {
    // What the Selection tool's Alt-drag runs: a copy in place, moved, dropped into the drawing.
    let mut s = Session::new(anicraft_doc::Document::default());
    run(&mut s, "draw.object_drawing", json!({"on": false}));
    run(&mut s, "draw.rect", json!({"x": 0, "y": 0, "w": 100, "h": 100, "fill": "#ff0000", "stroke": null}));
    run(&mut s, "element.pick", json!({"x": 50, "y": 50}));
    s.begin_gesture("Copy");
    run(&mut s, "edit.duplicate", json!({"dx": 0, "dy": 0}));
    run(&mut s, "element.translate", json!({"dx": 200, "dy": 0}));
    run(&mut s, "draw.drop", json!({}));
    s.end_gesture();
    let a: Vec<f64> = areas(&s);
    assert!(a.len() == 2 && a.iter().all(|x| (x - 10_000.0).abs() < 0.5), "{a:?}");
}

#[test]
fn groups_draw_their_colour_effect_and_fade_as_one() {
    let (mut s, ids) = three();
    run(&mut s, "element.select", json!({"ids": ids.map(|i| i.0)}));
    run(&mut s, "modify.group", json!({}));
    run(&mut s, "element.set_alpha", json!({"alpha": 0.5}));
    let dl = s.display_list();
    assert_eq!(dl.groups.len(), 1, "a translucent group composites as one");
    assert!(dl.items.iter().all(|i| i.group == Some(0)));
    assert!((dl.effective_alpha(&dl.items[0]) - 0.5).abs() < 1e-6);
    // Placement composes: a member's item is the group's transform times its own.
    let g = top(&s)[0].clone();
    let m = g.kind.group_members()[0].clone();
    let want = Affine::IDENTITY.mul(&g.transform).mul(&m.transform);
    assert_eq!(dl.items[0].transform.m, want.m);
}
