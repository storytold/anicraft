//! Layer parenting end to end (ADR 0113): the commands, keeping the stage position, undo,
//! editing a follower in stage space, broken links, renders, plug-in policy and robustness.

use super::*;
use anicraft_doc::{ColorEffect, Document, Element, ElementId, ElementKind, Keyframe, Layer, LayerId, LayerKind, Paint, Rgba, Shape, Stage, Tween, sample};
use anicraft_geom::{Affine, Decomposed, Path, Vec2};
use anicraft_render::render_frame;
use serde_json::json;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    match s.execute(id, &p) {
        Ok(v) => v,
        Err(e) => panic!("{id} {p}: {e}"),
    }
}

fn pose(tx: f32, ty: f32, deg: f32) -> Affine {
    Decomposed { tx, ty, sx: 1.0, sy: 1.0, rotation: deg.to_radians(), skew: 0.0 }.compose()
}

fn bar(id: u64, m: Affine) -> Arc<Element> {
    Arc::new(Element { id: ElementId(id), name: String::new(), transform: m, color: ColorEffect::default(), transform_3d: None, kind: ElementKind::Shape(Shape::filled(Path::rect(0.0, 0.0, 40.0, 10.0), Paint::Solid(Rgba::BLACK))) })
}

/// A layer whose one bar (element `id * 100`) tweens from `from` (frame 0) to `to` (frame 10).
fn tweened(id: u64, from: Affine, to: Affine) -> Layer {
    let mut l = Layer::new(LayerId(id), format!("L{id}"));
    let mut k0 = Keyframe::new(0, 10);
    k0.elements.push(bar(id * 100, from));
    k0.tween = Some(Tween::default());
    let mut k1 = Keyframe::new(10, 1);
    k1.elements.push(bar(id * 100, to));
    l.keyframes = vec![Arc::new(k0), Arc::new(k1)];
    l
}

fn held(id: u64, m: Affine) -> Layer {
    let mut l = Layer::new(LayerId(id), format!("L{id}"));
    let mut k = Keyframe::new(0, 11);
    k.elements.push(bar(id * 100, m));
    l.keyframes = vec![Arc::new(k)];
    l
}

/// Layers top to bottom on a 400 × 300 stage; nothing parented yet.
fn session(layers: Vec<Layer>) -> Session {
    let mut d = Document { stage: Stage { width: 400, height: 300, ..Stage::default() }, ..Document::default() };
    d.timeline_mut(0).layers = layers.into_iter().map(Arc::new).collect();
    d.next_id = 10_000;
    d.sanitize();
    Session::new(d)
}

/// Three free layers: an upper arm turning a quarter, a forearm and a hand that hold still.
fn limbs() -> Session {
    session(vec![held(3, pose(180.0, 100.0, 0.0)), held(2, pose(140.0, 100.0, 0.0)), tweened(1, pose(100.0, 100.0, 0.0), pose(100.0, 100.0, 90.0))])
}

/// Where layer `layer`'s bar shows on stage at `frame`.
fn shown(s: &Session, layer: u64, frame: u32) -> Affine {
    anicraft_doc::evaluate(&s.doc, frame).items.iter().find(|i| i.layer == LayerId(layer)).map(|i| i.transform).unwrap()
}

fn close(a: Affine, b: Affine) -> bool {
    a.m.iter().zip(b.m).all(|(x, y)| (x - y).abs() < 2e-3)
}

fn parent_of(s: &Session, layer: u64) -> Option<u64> {
    s.timeline().layer(LayerId(layer)).and_then(|l| l.rig_parent).map(|p| p.0)
}

#[test]
fn parenting_keeps_the_layer_where_it_is_and_then_it_follows() {
    let mut s = limbs();
    // Mid-tween, the upper arm turned 45°: parenting the forearm leaves it where it shows.
    s.set_frame(5);
    let before = shown(&s, 2, 5);
    let v = run(&mut s, "layer.set_parent", json!({"layer": 2, "parent": 1}));
    assert_eq!(v["layers"], json!([2]));
    assert_eq!(parent_of(&s, 2), Some(1));
    assert!(close(shown(&s, 2, 5), before), "{:?}", shown(&s, 2, 5));
    // Its stored key is now in the upper arm's space...
    let own = s.doc.timeline(0).layers[1].keyframes[0].elements[0].transform;
    assert!(close(pose(100.0, 100.0, 45.0).mul(&own), before));
    // ...so on other frames it goes where the upper arm takes it: rigidly attached.
    for f in [0, 10] {
        let arm = shown(&s, 1, f);
        assert!(close(shown(&s, 2, f), arm.mul(&own)), "frame {f}");
    }
    let (p0, p10) = (shown(&s, 2, 0).translation(), shown(&s, 2, 10).translation());
    assert!((p0 - p10).length() > 20.0, "the forearm swings with the upper arm");
    // One undo step puts everything back; redo parents again.
    assert_eq!(s.info()["history"]["undo"], 1, "one step");
    let _ = s.undo();
    assert_eq!(parent_of(&s, 2), None);
    assert!(close(shown(&s, 2, 0), pose(140.0, 100.0, 0.0)));
    let _ = s.redo();
    assert_eq!(parent_of(&s, 2), Some(1));
    assert!(close(shown(&s, 2, 5), before));
}

#[test]
fn a_chain_of_three_follows_and_reparenting_keeps_the_stage_position() {
    let mut s = limbs();
    run(&mut s, "layer.set_parent", json!({"layer": 2, "parent": 1}));
    run(&mut s, "layer.set_parent", json!({"layer": 3, "parent": 2}));
    // At the frame they were parented on, nothing moved.
    assert!(close(shown(&s, 2, 0), pose(140.0, 100.0, 0.0)) && close(shown(&s, 3, 0), pose(180.0, 100.0, 0.0)));
    // At the end the whole arm hangs straight down from the shoulder.
    assert!((shown(&s, 2, 10).translation() - Vec2::new(100.0, 140.0)).length() < 1e-2);
    assert!((shown(&s, 3, 10).translation() - Vec2::new(100.0, 180.0)).length() < 1e-2);
    let rows = run(&mut s, "layer.parents", json!({"frame": 10}));
    assert_eq!(rows["layers"][0]["ancestors"], json!([2, 1]));
    assert_eq!(rows["layers"][2]["children"], json!([2]));
    assert!((rows["layers"][0]["space"][5].as_f64().unwrap() - 140.0).abs() < 1e-2, "the hand's space is the forearm's bar");
    // Re-parenting the hand straight to the upper arm, mid-tween, keeps it in place there.
    s.set_frame(7);
    let before = shown(&s, 3, 7);
    run(&mut s, "layer.set_parent", json!({"layer": 3, "parent": 1}));
    assert!(close(shown(&s, 3, 7), before));
    // So does removing the parent; without `keep` the layer shows its stored keys instead.
    run(&mut s, "layer.remove_parent", json!({"layer": 3}));
    assert_eq!(parent_of(&s, 3), None);
    assert!(close(shown(&s, 3, 7), before));
    let own = s.doc.timeline(0).layers[1].keyframes[0].elements[0].transform;
    run(&mut s, "layer.set_parent", json!({"layer": 2, "parent": null, "keep": false}));
    assert!(close(shown(&s, 2, 7), own));
}

#[test]
fn cycles_and_layers_that_cannot_follow_are_refused() {
    let mut s = limbs();
    run(&mut s, "layer.set_parent", json!({"layer": 2, "parent": 1}));
    run(&mut s, "layer.set_parent", json!({"layer": 3, "parent": 2}));
    let before = s.doc.clone();
    let err = |s: &mut Session, p: Value| s.execute("layer.set_parent", &p).unwrap_err().to_string();
    assert!(err(&mut s, json!({"layer": 1, "parent": 3})).contains("follows"), "a parent can't follow its own descendant");
    assert!(err(&mut s, json!({"layer": 1, "parent": 2})).contains("follows"));
    assert!(err(&mut s, json!({"layer": 1, "parent": 1})).contains("own parent"));
    assert!(err(&mut s, json!({"layer": 1, "parent": 99})).contains("no such layer"));
    assert!(err(&mut s, json!({"layer": 99, "parent": 1})).contains("no layer"));
    assert!(err(&mut s, json!({"layer": 1})).contains("required"));
    assert!(err(&mut s, json!({"layer": 1, "parent": "x"})).contains("layer id"));
    assert_eq!(s.doc, before, "a refused link changes nothing");
    // Folders, guides and the camera neither follow nor lead.
    let made = run(&mut s, "layer.new_folder", json!({}));
    let folder = made["layer"].as_u64().unwrap_or(s.layer.0);
    assert!(err(&mut s, json!({"layer": folder, "parent": 1})).contains("normal and mask"));
    assert!(err(&mut s, json!({"layer": 1, "parent": folder})).contains("no object to follow"));
    run(&mut s, "camera.add", json!({}));
    let cam = s.layer.0;
    assert!(err(&mut s, json!({"layer": cam, "parent": 1})).contains("normal and mask"));
    assert!(err(&mut s, json!({"layer": 1, "parent": cam})).contains("no object to follow"));
    // Several layers in one step.
    let mut s = limbs();
    run(&mut s, "layer.set_parent", json!({"layers": [2, 3], "parent": 1}));
    assert_eq!((parent_of(&s, 2), parent_of(&s, 3)), (Some(1), Some(1)));
    let _ = s.undo();
    assert_eq!((parent_of(&s, 2), parent_of(&s, 3)), (None, None));
}

#[test]
fn a_parent_scaled_to_nothing_refuses_the_link_there() {
    let mut s = session(vec![held(2, pose(50.0, 50.0, 0.0)), held(1, Affine { m: [0.0, 0.0, 0.0, 0.0, 10.0, 10.0] })]);
    let e = s.execute("layer.set_parent", &json!({"layer": 2, "parent": 1})).unwrap_err().to_string();
    assert!(e.contains("scaled to nothing"), "{e}");
    assert_eq!(parent_of(&s, 2), None);
    // Without keeping its place the link is made; editing there is then refused, not wrong.
    run(&mut s, "layer.set_parent", json!({"layer": 2, "parent": 1, "keep": false}));
    run(&mut s, "layer.select", json!({"layer": 2, "select": true}));
    let e = s.execute("element.translate", &json!({"dx": 5, "dy": 0})).unwrap_err().to_string();
    assert!(e.contains("scaled to nothing"), "{e}");
}

/// The arm parented at frame 0, the playhead on frame 10 where the upper arm has turned 90°.
fn turned() -> Session {
    let mut s = limbs();
    run(&mut s, "layer.set_parent", json!({"layer": 2, "parent": 1}));
    run(&mut s, "layer.set_parent", json!({"layer": 3, "parent": 2}));
    s.set_frame(10);
    s
}

#[test]
fn a_follower_is_edited_in_stage_space() {
    let mut s = turned();
    // Moving the hand 30 px right on stage moves it 30 px right on stage, whatever its parents
    // have turned to; the forearm and upper arm stay.
    let (hand, fore) = (shown(&s, 3, 10), shown(&s, 2, 10));
    run(&mut s, "layer.select", json!({"layer": 3, "select": true}));
    run(&mut s, "element.translate", json!({"dx": 30, "dy": 0}));
    assert!(close(shown(&s, 3, 10), Affine::translate(30.0, 0.0).mul(&hand)), "{:?}", shown(&s, 3, 10));
    assert!(close(shown(&s, 2, 10), fore));
    // The stage position commands report is the stage's too.
    let sel = s.selected_elements();
    assert!((sel[0].1.transform.translation() - shown(&s, 3, 10).translation()).length() < 1e-2);
    // Placing it by numbers puts it there on stage.
    run(&mut s, "element.set_transform", json!({"x": 250, "y": 60}));
    assert!((shown(&s, 3, 10).translation() - Vec2::new(250.0, 60.0)).length() < 1e-2);
    // New artwork drawn on the layer lands where it was drawn.
    run(&mut s, "draw.rect", json!({"x": 300, "y": 200, "w": 20, "h": 20, "fill": "#ff0000", "stroke": null}));
    let b = s.selection_bounds();
    assert!((b.min.x - 300.0).abs() < 0.5 && (b.min.y - 200.0).abs() < 0.5 && (b.max.x - 320.0).abs() < 0.5, "{b:?}");
}

#[test]
fn an_edit_leaves_untouched_objects_exactly_as_stored() {
    let mut s = turned();
    run(&mut s, "layer.select", json!({"layer": 3}));
    run(&mut s, "draw.rect", json!({"x": 300, "y": 200, "w": 20, "h": 20, "stroke": null}));
    let stored = s.doc.timeline(0).layers[0].keyframes[0].elements[0].clone();
    // Moving the new rectangle doesn't touch (or even copy) the hand's bar beside it.
    run(&mut s, "element.translate", json!({"dx": 5, "dy": 5}));
    let now = &s.doc.timeline(0).layers[0].keyframes[0].elements[0];
    assert!(Arc::ptr_eq(&stored, now), "the untouched object is the very same one");
    // A command that changes nothing leaves no undo step.
    let steps = s.info()["history"]["undo"].as_u64().unwrap();
    run(&mut s, "element.translate", json!({"dx": 0, "dy": 0}));
    assert_eq!(s.info()["history"]["undo"].as_u64().unwrap(), steps);
}

#[test]
fn moving_a_parent_and_its_follower_together_moves_each_once() {
    let mut s = turned();
    let (arm, fore, hand) = (shown(&s, 1, 10), shown(&s, 2, 10), shown(&s, 3, 10));
    run(&mut s, "edit.select_all", json!({}));
    assert_eq!(s.selected_layers().len(), 3);
    run(&mut s, "element.translate", json!({"dx": 25, "dy": -10}));
    let by = Affine::translate(25.0, -10.0);
    assert!(close(shown(&s, 1, 10), by.mul(&arm)));
    assert!(close(shown(&s, 2, 10), by.mul(&fore)), "{:?}", shown(&s, 2, 10));
    assert!(close(shown(&s, 3, 10), by.mul(&hand)), "{:?}", shown(&s, 3, 10));
}

#[test]
fn editing_inside_a_tween_keys_the_follower_where_the_stage_shows_it() {
    // The forearm tweens too; an edit mid-tween keys it at the playhead (auto-key) in the
    // upper arm's space as it is on that frame.
    let mut s = session(vec![tweened(2, pose(140.0, 100.0, 0.0), pose(140.0, 100.0, 60.0)), tweened(1, pose(100.0, 100.0, 0.0), pose(100.0, 100.0, 90.0))]);
    run(&mut s, "layer.set_parent", json!({"layer": 2, "parent": 1}));
    s.set_frame(5);
    let before = shown(&s, 2, 5);
    run(&mut s, "layer.select", json!({"layer": 2, "select": true}));
    run(&mut s, "element.translate", json!({"dx": 0, "dy": 40}));
    assert_eq!(s.doc.timeline(0).layers[0].keyframes.len(), 3, "a keyframe at the playhead");
    assert!(close(shown(&s, 2, 5), Affine::translate(0.0, 40.0).mul(&before)), "{:?}", shown(&s, 2, 5));
}

#[test]
fn a_motion_tweened_follower_is_keyed_in_its_parents_space() {
    let mut s = session(vec![held(2, pose(140.0, 100.0, 0.0)), tweened(1, pose(100.0, 100.0, 0.0), pose(100.0, 100.0, 90.0))]);
    run(&mut s, "layer.select", json!({"layer": 2, "select": true}));
    run(&mut s, "timeline.create_motion_tween", json!({}));
    run(&mut s, "layer.set_parent", json!({"layer": 2, "parent": 1}));
    s.set_frame(10);
    let before = shown(&s, 2, 10);
    run(&mut s, "layer.select", json!({"layer": 2, "select": true}));
    run(&mut s, "element.translate", json!({"dx": 20, "dy": 0}));
    assert!(close(shown(&s, 2, 10), Affine::translate(20.0, 0.0).mul(&before)), "{:?} vs {before:?}", shown(&s, 2, 10));
    // Frame 0 is as it was: the move became property keys at the playhead.
    assert!(close(shown(&s, 2, 0), pose(140.0, 100.0, 0.0)));
    // Re-parenting a motion-tweened layer keeps it in place at the playhead as well.
    let here = shown(&s, 2, 10);
    run(&mut s, "layer.remove_parent", json!({"layer": 2}));
    assert!(close(shown(&s, 2, 10), here), "{:?} vs {here:?}", shown(&s, 2, 10));
}

#[test]
fn a_link_broken_by_another_command_leaves_the_follower_where_it_shows() {
    // Deleting the parent layer.
    let mut s = turned();
    let (fore, hand) = (shown(&s, 2, 10), shown(&s, 3, 10));
    run(&mut s, "layer.delete", json!({"layer": 1}));
    assert_eq!(parent_of(&s, 2), None);
    assert!(close(shown(&s, 2, 10), fore), "{:?}", shown(&s, 2, 10));
    // The hand still follows the forearm, which is where it was.
    assert_eq!(parent_of(&s, 3), Some(2));
    assert!(close(shown(&s, 3, 10), hand));
    let _ = s.undo();
    assert_eq!(parent_of(&s, 2), Some(1));
    assert!(close(shown(&s, 2, 10), fore));
    // Turning the parent into a guide: nothing can follow a guide, so the link goes.
    let mut s = turned();
    let fore = shown(&s, 2, 10);
    run(&mut s, "layer.set_type", json!({"layer": 1, "type": "guide"}));
    assert_eq!(parent_of(&s, 2), None);
    let at = s.display_list().items.iter().find(|i| i.layer == LayerId(2)).map(|i| i.transform).unwrap();
    assert!(close(at, fore));
    assert_eq!(s.doc.timeline(0).layers[2].kind, LayerKind::Guide);
}

#[test]
fn duplicated_and_pasted_layers_keep_sound_links() {
    let mut s = turned();
    // Whatever layer commands do, no link ever points at a missing layer or round a cycle.
    for (id, p) in [("layer.move_up", json!({"layer": 1})), ("layer.move_down", json!({"layer": 3})), ("layer.new", json!({})), ("layer.delete", json!({"layer": 2}))] {
        let _ = s.execute(id, &p);
        let tl = s.timeline();
        for l in &tl.layers {
            if let Some(p) = l.rig_parent {
                assert!(tl.layer(p).is_some(), "after {id}");
                assert!(!tl.rig_ancestors(l.id).contains(&l.id), "after {id}");
            }
        }
    }
}

#[test]
fn renders_and_exports_draw_the_follower_where_it_shows() {
    let mut s = turned();
    // At frame 10 the arm hangs down from (100, 100): upper arm to y 140, forearm to 180,
    // hand to 220, each 10 px wide to the left of x = 100.
    let pm = render_frame(&s.doc, 10, 1.0).unwrap();
    let dark = |x: u32, y: u32| pm.pixel(x, y).unwrap()[0] < 0.5;
    assert!(dark(95, 120) && dark(95, 160) && dark(95, 200), "the three bars, end to end");
    assert!(!dark(150, 105) && !dark(190, 105), "nothing where the unparented keys would be");
    // The picture is the same after a save and a load (the links are in the file).
    let json = serde_json::to_string(&s.doc).unwrap();
    let mut back: Document = serde_json::from_str(&json).unwrap();
    back.sanitize();
    assert_eq!(back.scenes, s.doc.scenes);
    // The SVG export of the frame holds the hand where the stage shows it.
    let svg = anicraft_render::svg::to_svg(&anicraft_doc::evaluate(&s.doc, 10), &s.doc.stage);
    assert!(svg.contains("<path"));
    run(&mut s, "layer.remove_parent", json!({"layers": [2, 3]}));
    let after = render_frame(&s.doc, 10, 1.0).unwrap();
    for (x, y) in [(95, 120), (95, 160), (95, 200), (150, 105)] {
        assert_eq!(pm.pixel(x, y).unwrap()[0] < 0.5, after.pixel(x, y).unwrap()[0] < 0.5, "unparenting kept frame 10 as it was");
    }
}

#[test]
fn works_inside_a_symbol_open_for_editing() {
    let mut s = limbs();
    run(&mut s, "edit.select_all", json!({}));
    run(&mut s, "modify.convert_to_symbol", json!({"name": "Arm"}));
    run(&mut s, "symbol.edit", json!({}));
    assert_eq!(s.editing.len(), 1);
    let ids: Vec<u64> = s.timeline().layers.iter().map(|l| l.id.0).collect();
    if let [a, b, ..] = ids.as_slice() {
        run(&mut s, "layer.set_parent", json!({"layer": a, "parent": b}));
        assert_eq!(parent_of(&s, *a), Some(*b));
        assert!(s.execute("layer.set_parent", &json!({"layer": b, "parent": a})).is_err());
    }
}

#[test]
fn plugins_may_parent_layers() {
    for id in ["layer.set_parent", "layer.remove_parent", "layer.parents"] {
        assert!(crate::cmds_plugin::plugin_may_call(id).is_ok(), "{id}");
    }
}

#[test]
fn the_arm_sample_opens_parented() {
    let mut s = Session::default();
    run(&mut s, "file.new_sample", json!({"which": "arm"}));
    let rows = run(&mut s, "layer.parents", json!({}));
    assert_eq!(rows["layers"].as_array().map(Vec::len), Some(3));
    assert_eq!(rows["layers"][0]["parent_name"], "Forearm");
    assert_eq!(rows["layers"][1]["parent_name"], "Upper arm");
    assert_eq!(s.doc, sample::arm());
}

/// Every command with hostile params on a document of parented layers, mid-tween, with and
/// without a selection: Ok or Err, never a panic; the links stay sound and it still renders.
#[test]
fn panic_hunt_with_parented_layers() {
    let hostile = [
        Value::Null,
        json!({}),
        json!({"parent": u64::MAX, "layer": u64::MAX, "keep": "x"}),
        json!({"parent": null, "layers": [1, 2, 3, u64::MAX], "keep": false}),
        json!({"parent": 1, "layers": [1, 2, 3], "frame": u64::MAX}),
        json!({"parent": 3, "layer": 1, "keep": true, "dx": 1e30, "dy": -1e30}),
        json!({"parent": 2, "layer": 2, "x": 1e7, "y": -1e7, "sx": 0, "sy": 0, "rotation": 1e9}),
        json!({"parent": -1, "layers": "all", "type": "guide", "frame": 99_999}),
        json!({"parent": [1], "layer": 3, "type": "mask", "dx": 5, "dy": 5, "sx": 1e-30, "sy": -1}),
        json!({"layers": (0..2000).collect::<Vec<u64>>(), "parent": 1, "from": 0, "to": 10, "by": 3, "layer_by": 1}),
        json!({"layer": 1, "type": "guide", "x": 0, "y": 0, "w": 50, "h": 50, "count": 5}),
        json!({"layer": 2, "frame": 5, "count": 3, "dx": 10, "dy": 10, "sx": 0, "sy": 0}),
    ];
    for start in [turned(), Session::new(sample::arm())] {
        let mut s = start;
        s.set_frame(5);
        for spec in commands().iter().filter(|c| !c.id.starts_with("file.")) {
            for p in &hostile {
                let _ = s.execute(spec.id, p);
            }
            let _ = s.execute("edit.select_all", &json!({}));
            for p in &hostile {
                let _ = s.execute(spec.id, p);
            }
            let tl = s.timeline();
            for l in &tl.layers {
                if let Some(p) = l.rig_parent {
                    assert!(tl.rig_check(l.id, p).is_ok() || tl.rig_ancestors(l.id).contains(&p), "{}: a link that can't hold", spec.id);
                    assert!(tl.layer(p).is_some(), "{}: a link to a missing layer", spec.id);
                }
            }
        }
        for f in [0, 5, 10, 24, 1000] {
            let _ = render_frame(&s.doc, f, 0.25);
            for item in &anicraft_doc::evaluate(&s.doc, f).items {
                assert!(item.transform.is_finite());
            }
        }
    }
}
