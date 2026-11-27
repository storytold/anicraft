//! Mixed armatures end to end (ADR 0098): symbol instances and shapes in one armature. A chain
//! that runs shape → instance → shape, posed by angle and by IK as one pose, tweened, with
//! springs and joint translation across it; binding per shape; what removing a bone frees;
//! the versioned file format (old files unchanged, mixed ones version 4); refusals; renders.

use super::*;
use anicraft_doc::{BlendMode, BoneId, ColorEffect, Document, ElementKind, LoopMode, Paint, Rgba, Shape, Stage, Symbol, SymbolId, SymbolKind, Timeline, evaluate_guided, evaluate_shown};
use anicraft_geom::{Affine, Path};
use anicraft_render::render_frame;
use serde_json::json;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    match s.execute(id, &p) {
        Ok(v) => v,
        Err(e) => panic!("{id} {p}: {e}"),
    }
}

pub(crate) const BODY: u64 = 10;
pub(crate) const HEAD: u64 = 11;
pub(crate) const PLUME: u64 = 12;

/// A bar `2 × half` px wide and `steps × step` px tall, standing on the origin (anchors every
/// `step` px up both sides).
fn bar(half: f32, step: f32, steps: usize) -> Path {
    let mut p = Path::new();
    p.move_to(Vec2::new(-half, 0.0));
    for i in 1..=steps {
        p.line_to(Vec2::new(-half, -step * i as f32));
    }
    for i in (0..=steps).rev() {
        p.line_to(Vec2::new(half, -step * i as f32));
    }
    p.close();
    p
}

/// A 400 × 400 stage, 20 frames, one layer with a character in three parts: a black **body**
/// (a raw shape 40 × 160 px standing on (200, 300), its neck at (200, 140)), a red **head** (a
/// symbol instance 40 × 40 px centred on (200, 110)) and a blue **plume** on the head (a raw
/// shape 8 × 40 px standing on (200, 90)).
pub(crate) fn parts() -> Session {
    let mut d = Document { stage: Stage { width: 400, height: 400, ..Stage::default() }, ..Document::default() };
    let head = SymbolId(1);
    let mut inner = Layer::new(LayerId(2), "head");
    inner.keyframes[0] = Arc::new({
        let mut k = Keyframe::new(0, 1);
        k.elements.push(Arc::new(Element { id: ElementId(3), name: String::new(), transform: Affine::IDENTITY, color: ColorEffect::default(), kind: ElementKind::Shape(Shape::filled(Path::rect(-20.0, -20.0, 40.0, 40.0), Paint::Solid(Rgba::hex(0xff0000)))) }));
        k
    });
    d.library.push(Arc::new(Symbol { id: head, name: "Head".into(), kind: SymbolKind::Graphic, linkage: None, scale_grid_kept: None, scale_grid: None, timeline: Timeline { layers: vec![Arc::new(inner)] } }));
    let shape = |id: u64, path: Path, color: u32, x: f32, y: f32| {
        let shape = Shape { merge: true, ..Shape::filled(path, Paint::Solid(Rgba::hex(color))) };
        Arc::new(Element { id: ElementId(id), name: String::new(), transform: Affine::translate(x, y), color: ColorEffect::default(), kind: ElementKind::Shape(shape) })
    };
    let inst = Arc::new(Element { id: ElementId(HEAD), name: String::new(), transform: Affine::translate(200.0, 110.0), color: ColorEffect::default(), kind: ElementKind::Symbol { symbol: head, loop_mode: LoopMode::Loop, first_frame: 0, blend: BlendMode::Normal, filters: Vec::new(), events: Vec::new(), pivot: None } });
    let l = Arc::make_mut(&mut d.timeline_mut(0).layers[0]);
    l.keyframes = vec![Arc::new({
        let mut k = Keyframe::new(0, 20);
        k.elements = vec![shape(BODY, bar(20.0, 20.0, 8), 0x000000, 200.0, 300.0), inst, shape(PLUME, bar(4.0, 10.0, 4), 0x0000ff, 200.0, 90.0)];
        k
    })];
    d.next_id = 100;
    Session::new(d)
}

/// The bones of [`character`], from the hips up.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Rig {
    /// Inside the body: hips (200, 300) → waist (200, 220) → neck (200, 140).
    pub spine1: u64,
    pub spine2: u64,
    /// From the body's neck joint to the head instance, ending at its centre (200, 110).
    pub neck: u64,
    /// From the head to the plume shape as a whole, ending at its foot (200, 90).
    pub crest: u64,
    /// Inside the plume: foot (200, 90) → tip (200, 50).
    pub feather: u64,
}

/// [`parts`] rigged as one armature: two bones inside the body, a bone from the neck joint to
/// the head instance, a bone from the head to the plume, and a bone inside the plume.
pub(crate) fn character() -> (Session, Rig) {
    let mut s = parts();
    let bone = |v: Value| v["bone"].as_u64().unwrap();
    let spine1 = bone(run(&mut s, "bone.add_in_shape", json!({"shape": BODY, "head": [200, 300], "tail": [200, 220]})));
    let spine2 = bone(run(&mut s, "bone.add_in_shape", json!({"parent": spine1, "tail": [200, 140]})));
    let neck = bone(run(&mut s, "bone.add", json!({"parent": spine2, "to": HEAD, "tail": [200, 110]})));
    let crest = bone(run(&mut s, "bone.add", json!({"from": HEAD, "to": PLUME, "tail": [200, 90]})));
    let feather = bone(run(&mut s, "bone.add_in_shape", json!({"parent": crest, "tail": [200, 50]})));
    (s, Rig { spine1, spine2, neck, crest, feather })
}

fn armature_layer(s: &Session) -> &Layer {
    s.doc.timeline(s.scene).layers.iter().find(|l| l.kind == LayerKind::Armature).expect("an armature layer")
}

/// Element `id` as the armature layer shows it at `frame` (keyed pose, no springs).
fn shown(s: &Session, id: u64, frame: u32) -> Arc<Element> {
    let (_, els) = evaluate_layer_elements(armature_layer(s), frame).expect("the armature is on the frame");
    els.iter().find(|e| e.id.0 == id).expect("the element is on the armature layer").clone()
}

/// The anchors of shape `id` on the stage at `frame`.
fn points(s: &Session, id: u64, frame: u32) -> Vec<Vec2> {
    let e = shown(s, id, frame);
    match &e.kind {
        ElementKind::Shape(sh) => anicraft_doc::skin::anchors(&sh.path).into_iter().map(|a| e.transform.apply(a)).collect(),
        _ => panic!("not a shape"),
    }
}

fn turned(p: Vec2, j: Vec2, deg: f32) -> Vec2 {
    let (sn, c) = deg.to_radians().sin_cos();
    let d = p - j;
    Vec2::new(j.x + d.x * c - d.y * sn, j.y + d.x * sn + d.y * c)
}

fn close(a: Vec2, b: Vec2) -> bool {
    a.distance(b) < 2e-3
}

fn bone_info(s: &mut Session, frame: u32, bone: u64) -> Value {
    let v = run(s, "bone.info", json!({"frame": frame}));
    v["armatures"][0]["bones"].as_array().unwrap().iter().find(|b| b["id"] == bone).cloned().unwrap()
}

fn pt(v: &Value) -> Vec2 {
    Vec2::new(v[0].as_f64().unwrap() as f32, v[1].as_f64().unwrap() as f32)
}

const WAIST: Vec2 = Vec2 { x: 200.0, y: 220.0 };

#[test]
fn instances_and_shapes_join_one_armature_as_drawn() {
    let before = render_frame(&parts().doc, 0, 1.0).unwrap().data;
    let body_drawn: Vec<Vec2> = anicraft_doc::skin::anchors(&bar(20.0, 20.0, 8)).into_iter().map(|p| p + Vec2::new(200.0, 300.0)).collect();
    let (mut s, r) = character();
    let tl = s.doc.timeline(0);
    assert_eq!(tl.layers.len(), 2, "one armature layer above the layer the parts were on");
    assert!(tl.layers[1].keyframes[0].elements.is_empty(), "every part left its layer");
    let l = armature_layer(&s);
    assert_eq!(s.layer, l.id);
    assert_eq!(l.keyframes[0].elements.iter().map(|e| e.id.0).collect::<Vec<_>>(), vec![BODY, HEAD, PLUME]);
    let arm = l.armature.as_ref().unwrap();
    assert!(arm.is_mixed());
    assert_eq!(arm.skins.iter().map(|k| k.shape.0).collect::<Vec<_>>(), vec![BODY, PLUME], "several shapes with bones in one armature");
    // The rest pose is the drawing, exactly: outlines bit for bit, transforms unchanged.
    assert_eq!(render_frame(&s.doc, 0, 1.0).unwrap().data, before);
    assert_eq!(points(&s, BODY, 0), body_drawn);
    for (id, at) in [(BODY, (200.0, 300.0)), (HEAD, (200.0, 110.0)), (PLUME, (200.0, 90.0))] {
        let e = shown(&s, id, 0);
        assert_eq!(e.transform, Affine::translate(at.0, at.1), "{id} is where it was drawn");
        if let ElementKind::Shape(sh) = &e.kind {
            assert!(!sh.merge, "an armature's shape is an object");
            assert!(Arc::ptr_eq(&sh.path, &arm.skin(e.id).unwrap().rest), "at rest the outline is the drawn path itself");
        }
    }
    let v = run(&mut s, "bone.info", json!({"points": true}));
    let a = &v["armatures"][0];
    assert_eq!(a["mixed"], true);
    assert_eq!(a["roots"], json!([BODY]));
    assert_eq!(a["members"], json!([BODY, HEAD, PLUME]));
    assert_eq!(a["shape"], BODY);
    let shapes = a["shapes"].as_array().unwrap();
    assert_eq!((shapes[0]["shape"].as_u64(), shapes[1]["shape"].as_u64()), (Some(BODY), Some(PLUME)));
    assert_eq!(shapes[0]["bones"], json!([r.spine1, r.spine2]));
    assert_eq!(shapes[1]["bones"], json!([r.feather]));
    assert_eq!((shapes[0]["points"].as_array().unwrap().len(), shapes[1]["points"].as_array().unwrap().len()), (18, 10));
    let bones = a["bones"].as_array().unwrap();
    let of = |id: u64| bones.iter().find(|b| b["id"] == id).unwrap();
    assert_eq!((of(r.spine2)["shape"].as_u64(), of(r.feather)["shape"].as_u64()), (Some(BODY), Some(PLUME)));
    assert_eq!((&of(r.neck)["shape"], &of(r.crest)["shape"]), (&Value::Null, &Value::Null));
    assert_eq!((of(r.neck)["to"].as_u64(), of(r.crest)["to"].as_u64()), (Some(HEAD), Some(PLUME)));
    // The chain joins end to start across the kinds.
    assert_eq!(of(r.neck)["parent_bone"], r.spine2);
    assert_eq!(of(r.crest)["parent_bone"], r.neck);
    assert_eq!(of(r.feather)["parent_bone"], r.crest);
    assert_eq!(of(r.neck)["head"], json!([200.0, 140.0]), "the head's bone starts on the body's neck joint");
    assert_eq!(of(r.crest)["head"], json!([200.0, 110.0]));
    assert_eq!(of(r.feather)["head"], json!([200.0, 90.0]));
    assert_eq!(of(r.feather)["tail"], json!([200.0, 50.0]));
    // Each bone was one undo step; undoing them all puts the parts back on their layer.
    for _ in 0..5 {
        s.undo().unwrap();
    }
    assert_eq!(s.doc, parts().doc);
}

#[test]
fn one_pose_moves_both_kinds_together() {
    let (mut s, r) = character();
    run(&mut s, "bone.pose", json!({"bone": r.spine2, "angle": 30}));
    // The body's neck points follow the bone (they are bound to it alone)...
    let body = points(&s, BODY, 0);
    assert!(close(body[8], turned(Vec2::new(180.0, 140.0), WAIST, 30.0)), "{:?}", body[8]);
    assert!(close(body[9], turned(Vec2::new(220.0, 140.0), WAIST, 30.0)));
    assert_eq!(body[0], Vec2::new(180.0, 300.0), "the hips stay");
    // ...the head instance turns rigidly about the same joint, still sitting on the neck...
    let head = shown(&s, HEAD, 0);
    assert!(close(head.transform.translation(), turned(Vec2::new(200.0, 110.0), WAIST, 30.0)));
    assert!(close(head.transform.apply(Vec2::new(0.0, 30.0)), turned(Vec2::new(200.0, 140.0), WAIST, 30.0)), "the head's joint is the body's neck");
    assert!(close(pt(&bone_info(&mut s, 0, r.neck)["head"]), pt(&bone_info(&mut s, 0, r.spine2)["tail"])));
    // ...and the plume rides the head, its outline untouched (its own bone is at rest).
    let plume = shown(&s, PLUME, 0);
    assert!(close(plume.transform.translation(), turned(Vec2::new(200.0, 90.0), WAIST, 30.0)));
    let arm = armature_layer(&s).armature.clone().unwrap();
    assert!(matches!(&plume.kind, ElementKind::Shape(sh) if Arc::ptr_eq(&sh.path, &arm.skin(ElementId(PLUME)).unwrap().rest)));
    // The plume's own bone bends it on top of that.
    run(&mut s, "bone.pose", json!({"bone": r.feather, "angle": 45}));
    let tip = points(&s, PLUME, 0)[4];
    assert!(close(tip, turned(turned(Vec2::new(196.0, 50.0), Vec2::new(200.0, 90.0), 45.0), WAIST, 30.0)), "{tip:?}");
    assert_eq!(shown(&s, HEAD, 0).transform, head.transform, "bending the plume leaves the head");
    // The keyframe holds what shows (ADR 0043): the file needs no evaluation to draw the pose.
    let stored = armature_layer(&s).keyframes[0].elements.clone();
    assert_eq!(stored.iter().find(|e| e.id.0 == HEAD).unwrap().transform, head.transform);
    // The render shows the head where the pose put it, and nothing where it was.
    let img = render_frame(&s.doc, 0, 1.0).unwrap();
    let at = turned(Vec2::new(200.0, 110.0), WAIST, 30.0);
    let px = img.pixel(at.x as u32, at.y as u32).unwrap();
    assert!(px[0] > 0.9 && px[1] < 0.1, "the head is red at {at:?}: {px:?}");
    assert!(img.pixel(200, 110).unwrap()[1] > 0.9, "nothing is left where the head was drawn");
}

#[test]
fn ik_runs_through_shape_bones_and_instances_as_one_chain() {
    let (mut s, r) = character();
    let steps = s.history_len();
    let rest = points(&s, BODY, 0);
    s.begin_gesture("Pose Armature");
    for x in [220.0, 240.0, 260.0] {
        run(&mut s, "bone.solve", json!({"bone": r.neck, "x": x, "y": 150}));
    }
    s.end_gesture();
    assert_eq!(s.history_len(), steps + 1, "one drag is one undo step");
    let v = run(&mut s, "bone.solve", json!({"bone": r.neck, "x": 260, "y": 150}));
    assert_eq!((v["reached"].as_bool(), v["chain"].as_u64()), (Some(true), Some(3)), "the head's bone, then the two inside the body: {v}");
    assert!(pt(&bone_info(&mut s, 0, r.neck)["tail"]).distance(Vec2::new(260.0, 150.0)) < 0.5);
    assert!(shown(&s, HEAD, 0).transform.translation().distance(Vec2::new(260.0, 150.0)) < 0.5, "the head's centre is where it was dragged");
    assert_ne!(points(&s, BODY, 0), rest, "the body bent to get it there");
    // Dragging the plume's tip turns the whole chain, bending the body and the plume.
    let v = run(&mut s, "bone.solve", json!({"bone": r.feather, "x": 150, "y": 120}));
    assert_eq!(v["chain"], 5);
    assert_eq!(v["reached"], true, "{v}");
    // A pin on the head's bone holds everything it hangs from: only the plume moves.
    run(&mut s, "bone.pin", json!({"bone": r.neck, "pinned": true}));
    let (body, head) = (points(&s, BODY, 0), shown(&s, HEAD, 0).transform);
    let v = run(&mut s, "bone.solve", json!({"bone": r.feather, "x": 400, "y": 0}));
    assert_eq!(v["chain"], 2, "the plume's bone and the bone that carries the plume");
    assert_eq!((points(&s, BODY, 0), shown(&s, HEAD, 0).transform), (body, head));
}

#[test]
fn a_mixed_pose_tweens_as_one() {
    let (mut s, r) = character();
    run(&mut s, "bone.pose", json!({"bone": r.spine2, "angle": 40, "frame": 10}));
    run(&mut s, "bone.pose", json!({"bone": r.feather, "angle": -60, "frame": 10}));
    assert_eq!(shown(&s, HEAD, 0).transform, Affine::translate(200.0, 110.0), "the first pose is untouched");
    for (frame, deg) in [(5, 20.0), (10, 40.0), (15, 40.0)] {
        let head = shown(&s, HEAD, frame).transform;
        assert!(close(head.translation(), turned(Vec2::new(200.0, 110.0), WAIST, deg)), "frame {frame}");
        assert!(close(points(&s, BODY, frame)[8], turned(Vec2::new(180.0, 140.0), WAIST, deg)), "frame {frame}");
        // The plume: bent by its own bone (half way at frame 5), carried by the head.
        let own = if frame == 5 { -30.0 } else { -60.0 };
        assert!(close(points(&s, PLUME, frame)[4], turned(turned(Vec2::new(196.0, 50.0), Vec2::new(200.0, 90.0), own), WAIST, deg)), "frame {frame}");
    }
    // The display list every export draws sees the same thing.
    let dl = anicraft_doc::evaluate(&s.doc, 5);
    let item = dl.items.iter().find(|i| i.element.0 == BODY).unwrap();
    assert!(close(item.transform.apply(anicraft_doc::skin::anchors(&item.path)[8]), turned(Vec2::new(180.0, 140.0), WAIST, 20.0)));
    assert!(dl.items.iter().any(|i| i.element.0 == HEAD && close(i.transform.apply(Vec2::new(0.0, 0.0)), turned(Vec2::new(200.0, 110.0), WAIST, 20.0))), "the head's artwork is drawn posed");
}

#[test]
fn translation_and_springs_act_across_the_kinds() {
    let (mut s, r) = character();
    // The neck stretches: the head and the plume on it move, the body doesn't.
    run(&mut s, "bone.set_translation", json!({"bone": r.neck, "axis": "y", "min": -20, "max": 20}));
    run(&mut s, "bone.pose", json!({"bone": r.neck, "slide_y": -10}));
    assert!(close(shown(&s, HEAD, 0).transform.translation(), Vec2::new(200.0, 100.0)));
    assert!(close(shown(&s, PLUME, 0).transform.translation(), Vec2::new(200.0, 80.0)));
    assert_eq!(points(&s, BODY, 0), points(&character().0, BODY, 0));
    // A joint inside the body slides: its points, the head and the plume all go with it.
    run(&mut s, "bone.set_translation", json!({"bone": r.spine2, "axis": "x", "min": -30, "max": 30}));
    run(&mut s, "bone.pose", json!({"bone": r.spine2, "slide_x": 15}));
    assert!(close(points(&s, BODY, 0)[8], Vec2::new(195.0, 140.0)));
    assert!(close(shown(&s, HEAD, 0).transform.translation(), Vec2::new(215.0, 100.0)));
    // IK stretches the neck only where turning can't reach.
    let v = run(&mut s, "bone.solve", json!({"bone": r.neck, "x": 200, "y": 95}));
    assert_eq!(v["reached"], true, "{v}");
    // Springs: the head and the plume lag behind the body's swing, on every evaluation alike.
    let (mut s, r) = character();
    run(&mut s, "bone.set_spring", json!({"bone": r.neck, "strength": 30, "damping": 5}));
    run(&mut s, "bone.set_spring", json!({"bone": r.feather, "strength": 30, "damping": 5}));
    run(&mut s, "bone.pose", json!({"bone": r.spine1, "angle": 50, "frame": 6}));
    let fps = s.doc.stage.fps;
    let l = armature_layer(&s);
    let at = |id: u64, els: &[Arc<Element>]| els.iter().find(|e| e.id.0 == id).unwrap().clone();
    let mut swung = 0;
    for frame in 1..14 {
        let (_, sprung) = evaluate_shown(l, frame, None, fps).unwrap();
        let (_, keyed) = evaluate_guided(l, frame, None).unwrap();
        assert_eq!(at(BODY, &sprung), at(BODY, &keyed), "the body's bones have no spring: frame {frame}");
        swung += usize::from(at(HEAD, &sprung).transform != at(HEAD, &keyed).transform && at(PLUME, &sprung) != at(PLUME, &keyed));
        let (_, again) = evaluate_shown(l, frame, None, fps).unwrap();
        assert_eq!(sprung.to_vec(), again.to_vec(), "seeking gives the same bits");
        assert!(sprung.iter().all(|e| e.transform.is_finite()));
    }
    assert!(swung >= 6, "the head and the plume swing while the body moves and after: {swung} frames");
    assert!(bone_info(&mut s, 8, r.neck)["swing"].as_f64().unwrap().abs() > 0.01);
    // Springs off: the keyed poses, exactly.
    run(&mut s, "bone.enable_springs", json!({"bone": r.neck, "enabled": false}));
    let l = armature_layer(&s);
    assert_eq!(evaluate_shown(l, 8, None, fps).unwrap().1.to_vec(), evaluate_guided(l, 8, None).unwrap().1.to_vec());
}

#[test]
fn each_bone_binds_the_points_of_its_own_shape() {
    let (mut s, r) = character();
    let skins = |s: &Session| armature_layer(s).armature.as_ref().unwrap().skins.clone();
    let before = skins(&s);
    assert!(before[1].points.iter().all(|b| b.bones == vec![anicraft_doc::skin::Influence(BoneId(r.feather), 1.0)]), "the plume's points follow its one bone, not the body's");
    assert!(before[0].points.iter().all(|b| b.bones.iter().all(|i| i.0.0 == r.spine1 || i.0.0 == r.spine2)));
    // Point indices are those of the bone's shape.
    let v = run(&mut s, "bone.bind", json!({"bone": r.feather, "points": [0, 9], "mode": "remove"}));
    assert_eq!((v["changed"].as_u64(), v["bound"].as_u64(), v["shape"].as_u64()), (Some(2), Some(8), Some(PLUME)));
    assert_eq!(skins(&s)[0], before[0], "the body's binding is untouched");
    assert!(s.execute("bone.bind", &json!({"bone": r.feather, "points": [10]})).unwrap_err().to_string().contains("below 10"));
    // The weight brush paints the picked bone's shape only, even where shapes are close.
    let v = run(&mut s, "bone.paint_weights", json!({"bone": r.spine2, "at": [[200, 140]], "radius": 400, "strength": 100}));
    assert_eq!(v["bound"], 18);
    assert_eq!(skins(&s)[1].points[1], before[1].points[1], "the plume is inside the brush but isn't the bone's shape");
    let v = run(&mut s, "bone.set_weight", json!({"bone": r.feather, "points": [0], "weight": 100}));
    assert_eq!(v["weights"], json!([[[r.feather, 100.0]]]));
    // Bones that move an instance or a whole shape have no points.
    for b in [r.neck, r.crest] {
        for id in ["bone.bind", "bone.paint_weights", "bone.set_weight"] {
            let e = s.execute(id, &json!({"bone": b, "at": [[0, 0]], "points": [0], "weight": 50})).unwrap_err().to_string();
            assert!(e.contains("only bones inside a shape"), "{id}: {e}");
        }
    }
    // Adding a bone inside one shape rebinds that shape; a hand-bound point elsewhere stays.
    run(&mut s, "bone.bind", json!({"bone": r.spine1, "points": [17], "mode": "only"}));
    run(&mut s, "bone.add_in_shape", json!({"parent": r.feather, "tail": [200, 40]}));
    assert!(skins(&s)[0].points[17].manual);
    assert_eq!(skins(&s)[0].points[17].bones, vec![anicraft_doc::skin::Influence(BoneId(r.spine1), 1.0)]);
}

#[test]
fn removing_a_bone_frees_what_it_moved() {
    // A bone inside a shape: the shape stays in the armature, as it looked, without a skin.
    let (mut s, r) = character();
    run(&mut s, "bone.pose", json!({"bone": r.feather, "angle": 45}));
    let bent = points(&s, PLUME, 0);
    let v = run(&mut s, "bone.remove", json!({"bone": r.feather}));
    assert_eq!((&v["removed"], &v["freed"], &v["unbound"], &v["armature_removed"]), (&json!([r.feather]), &json!([]), &json!([PLUME]), &json!(false)));
    let l = armature_layer(&s);
    assert_eq!(v["layer"], l.id.0);
    let arm = l.armature.as_ref().unwrap();
    assert_eq!(arm.skins.len(), 1);
    assert_eq!(arm.members().iter().map(|e| e.0).collect::<Vec<_>>(), vec![BODY, HEAD, PLUME], "the plume still rides the head");
    assert_eq!(points(&s, PLUME, 0), bent, "it keeps the outline it showed");
    run(&mut s, "bone.pose", json!({"bone": r.spine2, "angle": 30}));
    assert!(close(shown(&s, PLUME, 0).transform.translation(), turned(Vec2::new(200.0, 90.0), WAIST, 30.0)));
    s.undo().unwrap();
    s.undo().unwrap();
    assert_eq!(s.doc, {
        let (mut again, r) = character();
        run(&mut again, "bone.pose", json!({"bone": r.feather, "angle": 45}));
        again.doc
    });
    // The bone that carries the head: the head, the plume on it and the plume's bone go; they
    // land on a layer of their own as they looked, and what is left is a shape armature again.
    let (mut s, r) = character();
    run(&mut s, "bone.pose", json!({"bone": r.spine2, "angle": 30}));
    run(&mut s, "bone.pose", json!({"bone": r.feather, "angle": 45}));
    let (head, plume) = (shown(&s, HEAD, 0), shown(&s, PLUME, 0));
    let v = run(&mut s, "bone.remove", json!({"bone": r.neck}));
    assert_eq!((&v["removed"], &v["freed"], &v["armature_removed"]), (&json!([r.neck, r.crest, r.feather]), &json!([HEAD, PLUME]), &json!(false)));
    let tl = s.doc.timeline(0);
    assert_eq!(tl.layers.len(), 3);
    let freed = &tl.layers[1];
    assert_eq!((freed.id.0, freed.kind), (v["layer"].as_u64().unwrap(), LayerKind::Normal));
    assert_eq!(freed.keyframes[0].elements, vec![head, plume], "posed and bent as they showed");
    let arm = tl.layers[0].armature.as_ref().unwrap();
    assert!(!arm.is_mixed());
    assert_eq!(arm.bones.len(), 2);
    assert!(!s.doc.has_mixed_armatures());
    assert!(close(points(&s, BODY, 0)[8], turned(Vec2::new(180.0, 140.0), WAIST, 30.0)), "the body keeps its pose");
    // The first bone: the whole armature goes, everything where it showed.
    let (mut s, r) = character();
    run(&mut s, "bone.pose", json!({"bone": r.spine2, "angle": 30}));
    let all = [shown(&s, BODY, 0), shown(&s, HEAD, 0), shown(&s, PLUME, 0)];
    let v = run(&mut s, "bone.remove", json!({"bone": r.spine1}));
    assert_eq!((&v["freed"], &v["armature_removed"]), (&json!([BODY, HEAD, PLUME]), &json!(true)));
    let tl = s.doc.timeline(0);
    assert!(tl.layers.iter().all(|l| l.kind == LayerKind::Normal && l.armature.is_none()));
    assert_eq!(tl.layers[0].keyframes[0].elements, all.to_vec());
    // More than one pose: it asks first (ADR 0004).
    let (mut s, r) = character();
    run(&mut s, "bone.pose", json!({"bone": r.spine2, "angle": 30, "frame": 10}));
    assert!(crate::confirm::confirmation(&s, "bone.remove", &json!({"bone": r.neck})).is_some());
}

/// The same text but for the generator line (the version of the AniCraft that wrote it) and
/// line endings (a checkout may rewrite them).
fn comparable(text: &str) -> String {
    text.lines().filter(|l| !l.contains("\"generator\"")).collect::<Vec<_>>().join("\n")
}

#[test]
fn files_before_mixed_armatures_load_and_save_unchanged() {
    for (name, text) in [("instances", include_str!("../tests/fixtures/armature-v3-instances.anicraft")), ("shape", include_str!("../tests/fixtures/armature-v3-shape.anicraft"))] {
        let doc = crate::format::from_json(text).unwrap();
        let arm = doc.timeline(0).layers.iter().find_map(|l| l.armature.clone()).unwrap();
        assert!(!arm.is_mixed() && !doc.has_mixed_armatures(), "{name}");
        assert_eq!(arm.skins.len(), usize::from(name == "shape"), "{name}");
        assert!(arm.bones.iter().all(|b| b.shape == arm.skins.first().map(|k| k.shape)), "{name}: a shape armature's bones are all inside its shape");
        assert_eq!(crate::format::version_for(&doc), 3, "{name}");
        // Saved again it is the file an earlier AniCraft wrote, byte for byte.
        let again = crate::format::to_json(&doc).unwrap();
        assert_eq!(comparable(&again), comparable(text), "{name}");
        // And it still plays: the poses it holds evaluate to the outlines and transforms it holds.
        let l = doc.timeline(0).layers.iter().find(|l| l.kind == LayerKind::Armature).unwrap();
        for k in &l.keyframes {
            let (_, els) = evaluate_layer_elements(l, k.start).unwrap();
            assert_eq!(els.to_vec(), k.elements, "{name}: frame {}", k.start);
        }
    }
}

#[test]
fn mixed_armatures_are_written_as_version_4() {
    let (mut s, r) = character();
    run(&mut s, "bone.pose", json!({"bone": r.spine2, "angle": 30, "frame": 10}));
    run(&mut s, "bone.bind", json!({"bone": r.feather, "points": [0], "mode": "remove"}));
    let text = crate::format::to_json(&s.doc).unwrap();
    let env: Value = serde_json::from_str(&text).unwrap();
    // Builds from before ADR 0098 read up to version 3 and refuse this file by its number.
    assert_eq!(env["version"], 4);
    assert_eq!(crate::format::BASE_VERSION, 3, "the version earlier builds stop at");
    let arm = &env["document"]["scenes"][0]["timeline"]["layers"][0]["armature"];
    assert!(arm["skin"].is_null() && arm["skins"].as_array().is_some_and(|k| k.len() == 2), "the mixed form: {arm}");
    assert_eq!(arm["bones"][0]["shape"], BODY);
    assert!(arm["bones"][2].get("shape").is_none(), "a bone that moves an element says nothing");
    let back = crate::format::from_json(&text).unwrap();
    assert_eq!(back, s.doc);
    assert_eq!(crate::format::to_json(&back).unwrap(), text);
    // Without the bones that mix the kinds the document is version 3 again.
    run(&mut s, "bone.remove", json!({"bone": r.neck, "confirm": true}));
    let env: Value = serde_json::from_str(&crate::format::to_json(&s.doc).unwrap()).unwrap();
    assert_eq!(env["version"], 3);
    let arm = &env["document"]["scenes"][0]["timeline"]["layers"][0]["armature"];
    assert!(arm["skin"].is_object() && arm.get("skins").is_none(), "the first form: {arm}");
    assert!(arm["bones"].as_array().unwrap().iter().all(|b| b.get("shape").is_none()));
    // A file from a later AniCraft is refused with a message that says why.
    let future = crate::format::FORMAT_VERSION + 1;
    let later = text.replacen("\"version\": 4", &format!("\"version\": {future}"), 1);
    let e = crate::format::from_json(&later).unwrap_err().to_string();
    assert!(e.contains(&format!("file version {future} is newer than this AniCraft")), "{e}");
    // A mixed armature inside a symbol counts too.
    let (s, _) = character();
    let mut d = s.doc.clone();
    let layers = std::mem::take(&mut d.timeline_mut(0).layers);
    assert!(!d.has_mixed_armatures());
    Arc::make_mut(&mut d.library[0]).timeline.layers = layers;
    assert!(d.has_mixed_armatures());
    assert_eq!(crate::format::version_for(&d), 4);
}

#[test]
fn mixed_bones_refuse_what_cant_be_joined() {
    let (mut s, r) = character();
    let steps = s.history_len();
    for (id, p, want) in [
        ("bone.add", json!({"parent": r.spine2, "from": BODY, "to": HEAD}), "not both"),
        ("bone.add", json!({"to": HEAD}), "required"),
        ("bone.add", json!({"parent": r.spine2}), "required"),
        ("bone.add", json!({"parent": 9999, "to": HEAD}), "no bone"),
        ("bone.add", json!({"parent": r.spine2, "to": PLUME}), "already"),
        ("bone.add", json!({"parent": r.feather, "to": BODY}), "root"),
        ("bone.add", json!({"parent": r.neck, "to": HEAD}), "two different"),
        ("bone.add", json!({"from": PLUME, "to": 999}), "no instance or shape"),
        ("bone.add_in_shape", json!({"parent": r.neck, "tail": [0, 0]}), "symbol instance"),
        ("bone.add_in_shape", json!({"parent": r.crest, "shape": BODY, "tail": [0, 0]}), "another shape"),
        ("bone.add_in_shape", json!({"shape": HEAD, "head": [200, 110], "tail": [220, 110]}), "need a shape"),
    ] {
        let e = s.execute(id, &p).unwrap_err().to_string();
        assert!(e.contains(want), "{id} {p}: {e}");
    }
    assert_eq!(s.history_len(), steps, "refusals leave no undo steps");
    // Two armatures don't join.
    let mut s = parts();
    run(&mut s, "bone.add_in_shape", json!({"shape": BODY, "head": [200, 300], "tail": [200, 140]}));
    run(&mut s, "bone.add_in_shape", json!({"shape": PLUME, "head": [200, 90], "tail": [200, 50]}));
    let e = s.execute("bone.add", &json!({"from": BODY, "to": PLUME})).unwrap_err().to_string();
    assert!(e.contains("different armatures"), "{e}");
    // Artwork edits are still refused on the armature layer, whatever the member.
    let (mut s, _) = character();
    for el in [BODY, HEAD, PLUME] {
        s.select_only([ElementId(el)]);
        let e = s.execute("element.translate", &json!({"dx": 5, "dy": 0})).unwrap_err().to_string();
        assert!(e.contains("armature"), "{el}: {e}");
    }
}

#[test]
fn an_instance_starts_a_chain_that_continues_into_a_shape() {
    // The other way round: the head is the root, the body hangs from it and bends.
    let mut s = parts();
    let bone = |v: Value| v["bone"].as_u64().unwrap();
    let v = run(&mut s, "bone.add", json!({"from": HEAD, "to": BODY, "head": [200, 130], "tail": [200, 140]}));
    assert_eq!(v["created"], true);
    let hold = bone(v);
    let arm = armature_layer(&s).armature.clone().unwrap();
    assert!(arm.skins.is_empty() && !arm.is_mixed(), "a shape moved as a whole needs no skin: the first file form holds it");
    assert_eq!(crate::format::version_for(&s.doc), 3);
    assert!(matches!(&shown(&s, BODY, 0).kind, ElementKind::Shape(sh) if !sh.merge));
    // Turning it swings the whole body about the joint, outline untouched.
    run(&mut s, "bone.pose", json!({"bone": hold, "angle": 90}));
    assert!(close(points(&s, BODY, 0)[0], turned(Vec2::new(180.0, 300.0), Vec2::new(200.0, 130.0), 90.0)));
    run(&mut s, "bone.pose", json!({"bone": hold, "angle": 0}));
    // From that bone's end, bones inside the body: now its outline bends too.
    let v = run(&mut s, "bone.add_in_shape", json!({"parent": hold, "tail": [200, 220]}));
    assert_eq!((v["shape"].as_u64(), v["created"].as_bool()), (Some(BODY), Some(false)));
    let b1 = bone(v);
    let b2 = bone(run(&mut s, "bone.add_in_shape", json!({"parent": b1, "tail": [200, 300]})));
    assert_eq!(bone_info(&mut s, 0, b1)["head"], json!([200.0, 140.0]), "it starts on the end of the bone into the shape");
    assert!(armature_layer(&s).armature.as_ref().unwrap().is_mixed());
    assert_eq!(crate::format::version_for(&s.doc), 4);
    run(&mut s, "bone.pose", json!({"bone": hold, "angle": 20}));
    run(&mut s, "bone.pose", json!({"bone": b2, "angle": 30}));
    let neck = Vec2::new(200.0, 130.0);
    assert!(close(points(&s, BODY, 0)[0], turned(turned(Vec2::new(180.0, 300.0), WAIST, 30.0), neck, 20.0)), "bent by its own bone, carried by the head's: {:?}", points(&s, BODY, 0)[0]);
    assert_eq!(shown(&s, HEAD, 0).transform, Affine::translate(200.0, 110.0), "the root stays");
    // Reshaping at rest still works on a shape that hangs from a bone (ADR 0090).
    run(&mut s, "bone.pose", json!({"bone": b2, "angle": 0}));
    s.select_only([ElementId(BODY)]);
    assert!(crate::cmds_bone::is_skin_shape(&s, ElementId(BODY)));
}

#[test]
fn plug_ins_may_build_mixed_armatures() {
    for id in ["bone.add", "bone.add_in_shape", "bone.remove", "bone.bind", "bone.info"] {
        assert!(crate::cmds_plugin::plugin_may_call(id).is_ok(), "{id}");
    }
}

/// Evaluation cost of a mixed armature (the test profile is optimised; a release build is
/// faster still).
#[test]
fn evaluating_a_mixed_armature_is_fast() {
    let (mut s, r) = character();
    run(&mut s, "bone.pose", json!({"bone": r.spine2, "angle": 40, "frame": 10}));
    run(&mut s, "bone.pose", json!({"bone": r.feather, "angle": -60, "frame": 10}));
    let l = armature_layer(&s).clone();
    let t = std::time::Instant::now();
    let n = 2000;
    for i in 0..n {
        std::hint::black_box(evaluate_layer_elements(&l, 1 + (i % 9)));
    }
    let per = t.elapsed().as_secs_f64() * 1e3 / f64::from(n);
    println!("mixed armature (5 bones, 2 skinned shapes of 18 and 10 points, 1 instance): {per:.4} ms per evaluated frame");
    assert!(per < 2.0, "{per} ms");
}
