//! Mixed armatures (ADR 0098): instances and shapes in one armature. The model's own rules:
//! forward kinematics across the kinds, the two file forms, repair of hostile files.

use super::*;
use crate::skin::{Binding, Influence, anchors, rebind};
use crate::{BlendMode, ColorEffect, ElementKind, LoopMode, Paint, Rgba, Shape, SymbolId};
use anicraft_geom::Path;
use std::f32::consts::FRAC_PI_2;

const BODY: ElementId = ElementId(10);
const HEAD: ElementId = ElementId(11);
const HAT: ElementId = ElementId(12);
const N1: ElementId = ElementId(30);
const N2: ElementId = ElementId(31);
const N3: ElementId = ElementId(32);
const SPINE1: BoneId = BoneId(20);
const SPINE2: BoneId = BoneId(21);
const NECK: BoneId = BoneId(22);
const BRIM: BoneId = BoneId(23);
const TIP: BoneId = BoneId(24);

/// A closed bar from (0, -half) to (len, half), anchors every `step` px along both edges.
fn bar(len: f32, half: f32, step: f32) -> Path {
    let n = (len / step) as usize;
    let mut p = Path::new();
    p.move_to(Vec2::new(0.0, -half));
    for i in 1..=n {
        p.line_to(Vec2::new(step * i as f32, -half));
    }
    for i in (0..=n).rev() {
        p.line_to(Vec2::new(step * i as f32, half));
    }
    p.close();
    p
}

fn shape(id: ElementId, path: Arc<Path>, at: Vec2) -> Arc<Element> {
    let shape = Shape { path, ..Shape::filled(Path::new(), Paint::Solid(Rgba::BLACK)) };
    Arc::new(Element { id, name: String::new(), transform: Affine::translate(at.x, at.y), color: ColorEffect::default(), transform_3d: None, kind: ElementKind::Shape(shape) })
}

fn instance(id: ElementId, at: Vec2) -> Arc<Element> {
    let kind = ElementKind::Symbol { symbol: SymbolId(1), loop_mode: LoopMode::Loop, first_frame: 0, blend: BlendMode::Normal, filters: Vec::new(), events: Vec::new(), pivot: None };
    Arc::new(Element { id, name: String::new(), transform: Affine::translate(at.x, at.y), color: ColorEffect::default(), transform_3d: None, kind })
}

/// A body lying along y = 100 from x = 100 to 300 with two bones inside it (joint at x = 200),
/// a head instance at (330, 100) on a bone from the body's end, and a hat shape at (360, 100)
/// that the head's bone carries as a whole, with one bone inside it.
fn rig() -> (Armature, Vec<Arc<Element>>) {
    let (body, hat) = (Arc::new(bar(200.0, 10.0, 25.0)), Arc::new(bar(40.0, 5.0, 10.0)));
    let v = Vec2::new;
    let mut arm = Armature {
        bones: vec![
            Bone::in_shape(SPINE1, BODY, BODY, N1, v(0.0, 0.0), v(100.0, 0.0)),
            Bone::in_shape(SPINE2, BODY, N1, N2, v(100.0, 0.0), v(200.0, 0.0)),
            // From the joint node at the body's end (the body's space) to the head.
            Bone::new(NECK, N2, HEAD, v(200.0, 0.0), v(0.0, 0.0), Affine::translate(230.0, 0.0)),
            // From the head to the hat as a whole.
            Bone::new(BRIM, HEAD, HAT, v(0.0, 0.0), v(0.0, 0.0), Affine::translate(30.0, 0.0)),
            Bone::in_shape(TIP, HAT, HAT, N3, v(0.0, 0.0), v(40.0, 0.0)),
        ],
        skins: vec![Skin::new(BODY, body.clone()), Skin::new(HAT, hat.clone())],
        springs_off: false,
        char_rig: None,
    };
    rebind(&mut arm);
    (arm, vec![shape(BODY, body, v(100.0, 100.0)), instance(HEAD, v(330.0, 100.0)), shape(HAT, hat, v(360.0, 100.0))])
}

fn path_of(e: &Element) -> Arc<Path> {
    match &e.kind {
        ElementKind::Shape(s) => s.path.clone(),
        _ => panic!("not a shape"),
    }
}

fn turned(p: Vec2, j: Vec2, a: f32) -> Vec2 {
    let (s, c) = a.sin_cos();
    let d = p - j;
    Vec2::new(j.x + d.x * c - d.y * s, j.y + d.x * s + d.y * c)
}

fn close(a: Vec2, b: Vec2) -> bool {
    a.distance(b) < 1e-3
}

#[test]
fn the_rest_pose_changes_nothing() {
    let (arm, els) = rig();
    assert!(arm.is_mixed());
    assert_eq!(arm.roots(), vec![BODY]);
    assert_eq!(arm.members(), vec![BODY, HEAD, HAT], "joint nodes are no members");
    let mut posed = els.clone();
    assert!(!apply(&arm, &mut posed, &Pose::default()), "nothing to move at rest");
    for (a, b) in posed.iter().zip(&els) {
        assert!(Arc::ptr_eq(a, b), "not even a copy is made");
    }
    // Bindings stay inside each shape: no point of one follows a bone of the other.
    let ids = |k: &Skin| k.points.iter().flat_map(|b| b.bones.iter().map(|i| i.0)).collect::<HashSet<_>>();
    assert_eq!(ids(&arm.skins[0]), HashSet::from([SPINE1, SPINE2]));
    assert_eq!(ids(&arm.skins[1]), HashSet::from([TIP]));
    assert_eq!((arm.bound_count(SPINE1) > 0, arm.bound_count(NECK), arm.bound_count(TIP)), (true, 0, 10));
    assert_eq!(arm.skin_of(TIP).map(|k| k.shape), Some(HAT));
    assert!(arm.skin_of(NECK).is_none() && arm.skin_of(BRIM).is_none());
}

#[test]
fn one_pose_places_instances_and_bends_shapes_consistently() {
    let (arm, els) = rig();
    let mut pose = Pose::default();
    pose.set(SPINE2, FRAC_PI_2);
    let joint = Vec2::new(200.0, 100.0);
    let mut posed = els.clone();
    assert!(apply(&arm, &mut posed, &pose));
    // The body's end points turn a quarter about its middle joint.
    let body = anchors(&path_of(&posed[0]));
    assert!(close(posed[0].transform.apply(body[8]), turned(Vec2::new(300.0, 90.0), joint, FRAC_PI_2)));
    assert_eq!(posed[0].transform, els[0].transform, "a root shape's transform is never touched");
    // The head and the hat turn with them, rigidly, and stay joined to the body's end.
    assert!(close(posed[1].transform.translation(), turned(Vec2::new(330.0, 100.0), joint, FRAC_PI_2)));
    assert!(close(posed[2].transform.translation(), turned(Vec2::new(360.0, 100.0), joint, FRAC_PI_2)));
    assert!(Arc::ptr_eq(&path_of(&posed[2]), &arm.skins[1].rest), "the hat's own bone is at rest: its outline is the drawn one");
    let at = bones_at(&arm, &posed, &pose);
    let of = |id: BoneId| at.iter().find(|b| b.id == id).copied().unwrap();
    assert!(close(of(NECK).head, of(SPINE2).tail), "the head's bone starts where the body's last bone ends");
    assert!(close(of(BRIM).head, of(NECK).tail));
    assert!(close(of(TIP).head, of(BRIM).tail));
    assert!(close(of(TIP).tail, turned(Vec2::new(400.0, 100.0), joint, FRAC_PI_2)));
    // Placing posed elements again gives the same places (the stage draws bones from them).
    let again = place(&arm, &posed, &pose);
    assert!(close(again[&HEAD].translation(), posed[1].transform.translation()));
    // The hat's bone bends the hat about its own foot, in the hat's space.
    pose.set(TIP, -FRAC_PI_2);
    let mut both = els.clone();
    apply(&arm, &mut both, &pose);
    assert_eq!(both[2].transform, posed[2].transform, "the hat's bone bends it; the head's bone carries it");
    let hat = anchors(&path_of(&both[2]));
    assert!(close(hat[4], turned(Vec2::new(40.0, -5.0), Vec2::new(0.0, 0.0), -FRAC_PI_2)));
    assert_eq!(path_of(&both[0]), path_of(&posed[0]), "and the body is as before");
}

#[test]
fn ik_reaches_through_shape_bones_and_instances() {
    let (arm, els) = rig();
    // Drag the head's centre: the two bones inside the body and the head's bone turn.
    let target = Vec2::new(250.0, 250.0);
    let r = solve(&arm, &els, &Pose::default(), NECK, Vec2::new(0.0, 0.0), target).unwrap();
    assert_eq!(r.chain, 3);
    assert!(r.error <= IK_TOLERANCE, "{}", r.error);
    let mut posed = els.clone();
    apply(&arm, &mut posed, &r.pose);
    assert!(posed[1].transform.translation().distance(target) < 0.02);
    assert_ne!(path_of(&posed[0]), path_of(&els[0]), "the body bent to get there");
    // The grabbed point of a dragged instance follows the pointer.
    let grab = grab_point(&arm, &els, &Pose::default(), NECK, Vec2::new(335.0, 104.0)).unwrap();
    assert!(close(grab, Vec2::new(5.0, 4.0)));
}

#[test]
fn the_two_file_forms() {
    // An armature of instances, and one of a single shape, are written as they always were.
    let (mixed, _) = rig();
    let mut one = mixed.clone();
    one.bones.truncate(2);
    one.skins.truncate(1);
    assert!(!one.is_mixed());
    let text = serde_json::to_string(&one).unwrap();
    assert!(text.contains("\"skin\":{") && !text.contains("\"skins\"") && !text.contains("\"shape\":10,\"parent\""), "{text}");
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(v["bones"].as_array().unwrap().iter().all(|b| b.get("shape").is_none()), "the first form doesn't mark its bones");
    assert_eq!(v.as_object().unwrap().keys().collect::<Vec<_>>(), ["bones", "skin"]);
    let back: Armature = serde_json::from_str(&text).unwrap();
    assert_eq!(back, one, "read back, every bone is inside the one shape again");
    let instances = Armature { bones: vec![Bone::new(NECK, BODY, HEAD, Vec2::ZERO, Vec2::ZERO, Affine::IDENTITY)], ..Default::default() };
    let text = serde_json::to_string(&instances).unwrap();
    assert!(!text.contains("skin") && !text.contains("shape"), "{text}");
    assert_eq!(serde_json::from_str::<Armature>(&text).unwrap(), instances);
    // A mixed one says which shape each bone is inside, and lists its skins.
    let text = serde_json::to_string(&mixed).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v.as_object().unwrap().keys().collect::<Vec<_>>(), ["bones", "skins"]);
    assert_eq!(v["skins"].as_array().unwrap().len(), 2);
    let marks: Vec<Option<u64>> = v["bones"].as_array().unwrap().iter().map(|b| b.get("shape").and_then(|s| s.as_u64())).collect();
    assert_eq!(marks, vec![Some(10), Some(10), None, None, Some(12)]);
    assert_eq!(serde_json::from_str::<Armature>(&text).unwrap(), mixed);
    // A build from before mixed armatures knows `bones` and `skin` only: reading this text it
    // would see bones and no skin, which is why the envelope's version stops it first.
    assert!(v.get("skin").is_none());
    // Springs off is kept in both forms.
    for mut a in [one, mixed] {
        a.springs_off = true;
        assert!(serde_json::from_str::<Armature>(&serde_json::to_string(&a).unwrap()).unwrap().springs_off);
    }
    // A file with both fields (hand-made): the old field's skin comes first, and marks only the
    // bones that say nothing.
    let both: Armature = serde_json::from_str(r#"{"bones":[{"id":1,"parent":10,"child":30,"head":{"x":0,"y":0},"tail":{"x":9,"y":0},"rest":{"m":[1,0,0,1,0,0]}},{"id":2,"parent":12,"child":31,"head":{"x":0,"y":0},"tail":{"x":9,"y":0},"rest":{"m":[1,0,0,1,0,0]},"shape":12}],"skin":{"shape":10,"rest":"M0 0L9 0"},"skins":[{"shape":12,"rest":"M0 0L9 0"}]}"#).unwrap();
    assert_eq!(both.bones.iter().map(|b| b.shape.map(|s| s.0)).collect::<Vec<_>>(), vec![Some(10), Some(12)]);
    assert_eq!(both.skins.iter().map(|k| k.shape.0).collect::<Vec<_>>(), vec![10, 12]);
}

#[test]
fn sanitize_repairs_hostile_mixed_armatures() {
    let (mut arm, _) = rig();
    assert!(!arm.sanitize(), "a sound armature is left alone");
    // Bones out of order come back parent first, across the kinds.
    let (mut arm, _) = rig();
    arm.bones.reverse();
    assert!(arm.sanitize());
    assert_eq!(arm.bones.iter().map(|b| b.id).collect::<Vec<_>>(), vec![SPINE1, SPINE2, NECK, BRIM, TIP]);
    // A bone inside a shape must hang from that shape or a bone inside it: one that starts on
    // an instance, on another shape's node, or names a shape without a skin goes, with what
    // hangs from it.
    let (mut arm, _) = rig();
    arm.bones.push(Bone::in_shape(BoneId(40), BODY, HEAD, ElementId(50), Vec2::ZERO, Vec2::new(9.0, 0.0)));
    arm.bones.push(Bone::in_shape(BoneId(41), HAT, N1, ElementId(51), Vec2::ZERO, Vec2::new(9.0, 0.0)));
    arm.bones.push(Bone::in_shape(BoneId(42), ElementId(77), ElementId(77), ElementId(52), Vec2::ZERO, Vec2::new(9.0, 0.0)));
    arm.bones.push(Bone::new(BoneId(43), ElementId(51), ElementId(53), Vec2::ZERO, Vec2::ZERO, Affine::IDENTITY));
    assert!(arm.sanitize());
    assert_eq!(arm.bones.len(), 5, "{:?}", arm.bones.iter().map(|b| b.id).collect::<Vec<_>>());
    // A joint node can't be a shape with bones; a second bone into one element goes.
    let (mut arm, _) = rig();
    arm.bones.push(Bone::in_shape(BoneId(44), BODY, N2, HAT, Vec2::ZERO, Vec2::new(9.0, 0.0)));
    arm.bones.push(Bone::new(BoneId(45), BODY, HEAD, Vec2::ZERO, Vec2::ZERO, Affine::IDENTITY));
    arm.sanitize();
    assert_eq!(arm.bones.len(), 5);
    assert_eq!(arm.bone_into(HAT).map(|b| b.id), Some(BRIM));
    // A skin without bones, a second skin for one shape and one over the point limit go; the
    // bones of a shape whose skin went go with it.
    let (mut arm, _) = rig();
    arm.skins.push(Skin::new(ElementId(60), Arc::new(bar(10.0, 1.0, 5.0))));
    arm.skins.push(Skin::new(BODY, Arc::new(bar(10.0, 1.0, 5.0))));
    let mut huge = Path::new();
    huge.move_to(Vec2::ZERO);
    for i in 0..=skin::MAX_SKIN_POINTS {
        huge.line_to(Vec2::new(i as f32, 1.0));
    }
    if let Some(k) = arm.skin_mut(HAT) {
        k.rest = Arc::new(huge);
    }
    assert!(arm.sanitize());
    assert_eq!(arm.skins.iter().map(|k| k.shape).collect::<Vec<_>>(), vec![BODY]);
    assert_eq!(anchors(&arm.skins[0].rest).len(), 18, "the first skin of a shape wins");
    assert!(arm.bone(TIP).is_none() && arm.bone(BRIM).is_some(), "the hat is still carried as a whole");
    // Weights naming bones of another shape are dropped, and the point is bound again.
    let (mut arm, _) = rig();
    if let Some(k) = arm.skin_mut(HAT) {
        k.points[0] = Binding { bones: vec![Influence(SPINE1, 1.0)], manual: false };
        k.points[1] = Binding { bones: vec![Influence(NECK, 0.5), Influence(TIP, 0.25)], manual: true };
    }
    assert!(arm.sanitize());
    let hat = arm.skin(HAT).unwrap();
    assert_eq!(hat.points[0].bones, vec![Influence(TIP, 1.0)]);
    assert_eq!(hat.points[1].bones, vec![Influence(TIP, 1.0)]);
    assert!(!arm.sanitize(), "sanitizing twice changes nothing");
    // A cycle through the kinds never loops: nothing of it is placed.
    let mut arm = Armature { bones: vec![Bone::new(BoneId(1), HEAD, HAT, Vec2::ZERO, Vec2::ZERO, Affine::IDENTITY), Bone::new(BoneId(2), HAT, HEAD, Vec2::ZERO, Vec2::ZERO, Affine::IDENTITY)], ..Default::default() };
    arm.sanitize();
    assert!(arm.is_empty());
}

#[test]
fn copies_with_fresh_ids_keep_their_skins() {
    use crate::{Keyframe, Layer, LayerId, LayerKind, Timeline};
    let (arm, els) = rig();
    let mut l = Layer::new(LayerId(1), "rig");
    l.kind = LayerKind::Armature;
    l.armature = Some(Arc::new(arm));
    l.keyframes = vec![Arc::new({
        let mut k = Keyframe::new(0, 5);
        k.elements = els;
        k
    })];
    let mut next = 1000u64;
    let copy = crate::library::fresh_ids(&Timeline { layers: vec![Arc::new(l)] }, &mut || {
        next += 1;
        next
    });
    let l = &copy.layers[0];
    let arm = l.armature.as_ref().unwrap();
    let ids: Vec<ElementId> = l.keyframes[0].elements.iter().map(|e| e.id).collect();
    assert!(ids.iter().all(|e| e.0 > 1000));
    assert_eq!(arm.skins.iter().map(|k| k.shape).collect::<Vec<_>>(), vec![ids[0], ids[2]], "each skin follows its shape's new id");
    assert_eq!(arm.bone(SPINE1).and_then(|b| b.shape), Some(ids[0]));
    assert_eq!(arm.bone(TIP).map(|b| (b.shape, b.parent)), Some((Some(ids[2]), ids[2])));
    let mut fixed = (**arm).clone();
    assert!(!fixed.sanitize(), "the copy is a sound armature");
    let mut pose = Pose::default();
    pose.set(SPINE2, FRAC_PI_2);
    let mut posed = l.keyframes[0].elements.clone();
    assert!(apply(arm, &mut posed, &pose));
    assert_ne!(path_of(&posed[0]), path_of(&l.keyframes[0].elements[0]), "the copy's body still bends");
}

/// A mixed armature's cost: two shapes of 1,000 anchors with 8 bones each, 8 instances on
/// bones between and after them (the test profile is optimised; release is faster still).
#[test]
fn a_big_mixed_armature_is_fast() {
    let v = Vec2::new;
    let (a, b) = (Arc::new(bar(1000.0, 10.0, 2.0)), Arc::new(bar(1000.0, 10.0, 2.0)));
    let (s1, s2) = (ElementId(1), ElementId(2));
    let mut bones = Vec::new();
    let mut els = vec![shape(s1, a.clone(), v(0.0, 0.0))];
    let mut id = 100u64;
    let mut fresh = || {
        id += 1;
        id
    };
    // Eight bones inside the first shape.
    let mut parent = s1;
    for i in 0..8 {
        let node = ElementId(fresh());
        bones.push(Bone::in_shape(BoneId(fresh()), s1, parent, node, v(125.0 * i as f32, 0.0), v(125.0 * (i + 1) as f32, 0.0)));
        parent = node;
    }
    // Four instances in a chain from its end, the second shape on the last, eight bones inside
    // it, and four more instances from its end.
    let chain = |parent: &mut ElementId, at: &mut f32, bones: &mut Vec<Bone>, els: &mut Vec<Arc<Element>>, fresh: &mut dyn FnMut() -> u64, first_head: Vec2| {
        for i in 0..4 {
            let e = ElementId(fresh());
            *at += 50.0;
            els.push(instance(e, v(*at, 0.0)));
            let (head, rest) = if i == 0 { (first_head, Affine::translate(first_head.x + 50.0, 0.0)) } else { (v(0.0, 0.0), Affine::translate(50.0, 0.0)) };
            bones.push(Bone::new(BoneId(fresh()), *parent, e, head, v(0.0, 0.0), rest));
            *parent = e;
        }
    };
    let mut at = 1000.0;
    chain(&mut parent, &mut at, &mut bones, &mut els, &mut fresh, v(1000.0, 0.0));
    els.push(shape(s2, b.clone(), v(at + 50.0, 0.0)));
    bones.push(Bone::new(BoneId(fresh()), parent, s2, v(0.0, 0.0), v(0.0, 0.0), Affine::translate(50.0, 0.0)));
    parent = s2;
    for i in 0..8 {
        let node = ElementId(fresh());
        bones.push(Bone::in_shape(BoneId(fresh()), s2, parent, node, v(125.0 * i as f32, 0.0), v(125.0 * (i + 1) as f32, 0.0)));
        parent = node;
    }
    at += 1050.0;
    chain(&mut parent, &mut at, &mut bones, &mut els, &mut fresh, v(1000.0, 0.0));
    let mut arm = Armature { bones, skins: vec![Skin::new(s1, a), Skin::new(s2, b)], springs_off: false, char_rig: None };
    let t = std::time::Instant::now();
    rebind(&mut arm);
    let bind = t.elapsed();
    let mut check = arm.clone();
    assert!(!check.sanitize(), "the rig is a sound armature, and repairing it again moves no weight");
    assert_eq!(arm.bones.len(), 25);
    let pose = Pose(arm.bones.iter().map(|b| JointAngle::new(b.id, 0.05)).collect());
    let t = std::time::Instant::now();
    let n = 200;
    for _ in 0..n {
        let mut posed = els.clone();
        assert!(apply(&arm, &mut posed, &pose));
        std::hint::black_box(posed);
    }
    let per = t.elapsed().as_secs_f64() * 1e3 / f64::from(n);
    println!("mixed armature, 25 bones, 2 × 1,002 anchors, 8 instances: bind {:.2} ms, pose {per:.3} ms per frame", bind.as_secs_f64() * 1e3);
    assert!(per < 20.0, "{per} ms");
    let target = v(1800.0, 900.0);
    let t = std::time::Instant::now();
    let last = arm.bones.last().map(|b| b.id).unwrap();
    let r = solve(&arm, &els, &Pose::default(), last, v(0.0, 0.0), target).unwrap();
    println!("IK through all 25 bones: {:.2} ms, error {:.4} px", t.elapsed().as_secs_f64() * 1e3, r.error);
    assert_eq!(r.chain, 25);
    assert!(r.error <= IK_TOLERANCE, "{}", r.error);
}
