//! Layer parenting (ADR 0113): evaluation, chains, repair and the file format.

use crate::*;
use anicraft_geom::{Affine, Decomposed, Path, Vec2};
use std::sync::Arc;

fn pose(tx: f32, ty: f32, deg: f32) -> Affine {
    Decomposed { tx, ty, sx: 1.0, sy: 1.0, rotation: deg.to_radians(), skew: 0.0 }.compose()
}

fn square(id: u64, m: Affine) -> Arc<Element> {
    Arc::new(Element { id: ElementId(id), name: String::new(), transform: m, color: ColorEffect::default(), transform_3d: None, kind: ElementKind::Shape(Shape::filled(Path::rect(0.0, 0.0, 10.0, 10.0), Paint::Solid(Rgba::BLACK))) })
}

/// A layer whose one object goes from `from` (frame 0) to `to` (frame 10), tweened.
fn tweened(id: u64, from: Affine, to: Affine) -> Layer {
    let mut l = Layer::new(LayerId(id), format!("L{id}"));
    let mut k0 = Keyframe::new(0, 10);
    k0.elements.push(square(id * 100, from));
    k0.tween = Some(Tween::default());
    let mut k1 = Keyframe::new(10, 1);
    k1.elements.push(square(id * 100, to));
    l.keyframes = vec![Arc::new(k0), Arc::new(k1)];
    l
}

fn held(id: u64, m: Affine) -> Layer {
    let mut l = Layer::new(LayerId(id), format!("L{id}"));
    let mut k = Keyframe::new(0, 11);
    k.elements.push(square(id * 100, m));
    l.keyframes = vec![Arc::new(k)];
    l
}

fn doc(layers: Vec<Layer>) -> Document {
    let mut d = Document::default();
    d.timeline_mut(0).layers = layers.into_iter().map(Arc::new).collect();
    d.next_id = 10_000;
    d
}

/// Where the object of `layer` shows on stage at `frame`.
fn shown(d: &Document, layer: u64, frame: u32) -> Affine {
    evaluate(d, frame).items.iter().find(|i| i.layer == LayerId(layer)).map(|i| i.transform).unwrap()
}

fn close(a: Affine, b: Affine) -> bool {
    a.m.iter().zip(b.m).all(|(x, y)| (x - y).abs() < 1e-3)
}

#[test]
fn a_child_follows_a_tweening_parent() {
    let mut child = held(2, Affine::translate(20.0, 0.0));
    child.rig_parent = Some(LayerId(1));
    let d = doc(vec![child, tweened(1, pose(100.0, 100.0, 0.0), pose(200.0, 100.0, 90.0))]);
    // Frame 0: the parent at (100, 100), unturned: the child 20 to its right.
    assert!(close(shown(&d, 2, 0), Affine::translate(120.0, 100.0)));
    // Frame 10: the parent at (200, 100) turned a quarter: the child 20 below it, turned too.
    let end = shown(&d, 2, 10);
    assert!((end.translation() - Vec2::new(200.0, 120.0)).length() < 1e-3, "{end:?}");
    assert!((end.decompose().rotation.to_degrees() - 90.0).abs() < 1e-3);
    // Mid-tween it is the parent's tweened transform times the child's own.
    let mid = pose(150.0, 100.0, 45.0).mul(&Affine::translate(20.0, 0.0));
    assert!(close(shown(&d, 2, 5), mid), "{:?} vs {mid:?}", shown(&d, 2, 5));
    // The parent itself is where it always was.
    assert!(close(shown(&d, 1, 5), pose(150.0, 100.0, 45.0)));
}

#[test]
fn a_chain_of_three_composes_and_each_keeps_its_own_tween() {
    let mut fore = tweened(2, pose(50.0, 0.0, 0.0), pose(50.0, 0.0, 90.0));
    fore.rig_parent = Some(LayerId(1));
    let mut hand = tweened(3, pose(40.0, 0.0, 0.0), pose(40.0, 0.0, -30.0));
    hand.rig_parent = Some(LayerId(2));
    let d = doc(vec![hand, fore, tweened(1, pose(100.0, 100.0, 0.0), pose(100.0, 100.0, 90.0))]);
    for f in [0, 3, 5, 10] {
        let t = f as f32 / 10.0;
        let upper = pose(100.0, 100.0, 90.0 * t);
        let fore = upper.mul(&pose(50.0, 0.0, 90.0 * t));
        let hand = fore.mul(&pose(40.0, 0.0, -30.0 * t));
        assert!(close(shown(&d, 1, f), upper), "upper at {f}");
        assert!(close(shown(&d, 2, f), fore), "forearm at {f}");
        assert!(close(shown(&d, 3, f), hand), "hand at {f}");
    }
    assert_eq!(d.timeline(0).rig_ancestors(LayerId(3)), vec![LayerId(2), LayerId(1)]);
    assert_eq!(d.timeline(0).rig_descendants(LayerId(1)), vec![LayerId(3), LayerId(2)]);
}

#[test]
fn scale_and_skew_are_followed_and_colour_is_not() {
    let mut parent = held(1, Decomposed { tx: 10.0, ty: 20.0, sx: 2.0, sy: 0.5, rotation: 0.3, skew: 0.4 }.compose());
    Arc::make_mut(&mut Arc::make_mut(&mut parent.keyframes[0]).elements[0]).color = ColorEffect::Alpha(0.25);
    let mut child = held(2, pose(5.0, 5.0, 10.0));
    child.rig_parent = Some(LayerId(1));
    let d = doc(vec![child, parent.clone()]);
    let want = parent.keyframes[0].elements[0].transform.mul(&pose(5.0, 5.0, 10.0));
    assert!(close(shown(&d, 2, 0), want));
    let dl = evaluate(&d, 0);
    let item = dl.items.iter().find(|i| i.layer == LayerId(2)).unwrap();
    assert_eq!(dl.effective_alpha(item), 1.0, "the parent's colour effect stays the parent's");
}

#[test]
fn an_empty_parent_frame_leaves_the_child_in_its_grandparents_space() {
    let mut mid = Layer::new(LayerId(2), "mid");
    mid.keyframes = vec![Arc::new(Keyframe::new(0, 11))];
    mid.rig_parent = Some(LayerId(1));
    let mut child = held(3, Affine::translate(7.0, 0.0));
    child.rig_parent = Some(LayerId(2));
    let d = doc(vec![child, mid, held(1, Affine::translate(100.0, 0.0))]);
    assert!(close(shown(&d, 3, 0), Affine::translate(107.0, 0.0)));
}

#[test]
fn a_hidden_parent_still_leads() {
    let mut parent = held(1, Affine::translate(100.0, 0.0));
    parent.visible = false;
    let mut child = held(2, Affine::translate(7.0, 0.0));
    child.rig_parent = Some(LayerId(1));
    let d = doc(vec![child, parent]);
    assert!(close(shown(&d, 2, 0), Affine::translate(107.0, 0.0)));
}

#[test]
fn cycles_self_links_and_wrong_kinds_are_refused() {
    let mut b = held(2, Affine::IDENTITY);
    b.rig_parent = Some(LayerId(1));
    let mut folder = Layer::new(LayerId(4), "folder");
    folder.kind = LayerKind::Folder;
    let d = doc(vec![folder, held(3, Affine::IDENTITY), b, held(1, Affine::IDENTITY)]);
    let tl = d.timeline(0);
    assert_eq!(tl.rig_check(LayerId(1), LayerId(2)), Err(RigError::Cycle));
    assert_eq!(tl.rig_check(LayerId(1), LayerId(1)), Err(RigError::SameLayer));
    assert_eq!(tl.rig_check(LayerId(4), LayerId(1)), Err(RigError::ChildKind));
    assert_eq!(tl.rig_check(LayerId(1), LayerId(4)), Err(RigError::ParentKind));
    assert_eq!(tl.rig_check(LayerId(1), LayerId(99)), Err(RigError::Missing));
    assert_eq!(tl.rig_check(LayerId(3), LayerId(2)), Ok(()));
    // A longer cycle: 3 → 2 → 1, then 1 → 3 is refused.
    let mut tl = tl.clone();
    Arc::make_mut(&mut tl.layers[1]).rig_parent = Some(LayerId(2));
    assert_eq!(tl.rig_check(LayerId(1), LayerId(3)), Err(RigError::Cycle));
}

#[test]
fn a_damaged_file_is_repaired_and_evaluates() {
    // A cycle, a link to nothing, a self link and a folder child: every one is dropped, and
    // evaluating before the repair neither hangs nor moves anything by a non-finite amount.
    let mut a = held(1, Affine::translate(1.0, 0.0));
    a.rig_parent = Some(LayerId(2));
    let mut b = held(2, Affine::translate(2.0, 0.0));
    b.rig_parent = Some(LayerId(1));
    let mut c = held(3, Affine::IDENTITY);
    c.rig_parent = Some(LayerId(77));
    let mut e = held(4, Affine::IDENTITY);
    e.rig_parent = Some(LayerId(4));
    let mut d = doc(vec![e, c, b, a]);
    for item in &evaluate(&d, 0).items {
        assert!(item.transform.is_finite());
    }
    d.sanitize();
    let links: Vec<Option<LayerId>> = d.timeline(0).layers.iter().map(|l| l.rig_parent).collect();
    // The cycle is broken at its first link (top to bottom); the other half holds.
    assert_eq!(links, vec![None, None, None, Some(LayerId(2))]);
}

#[test]
fn the_chain_is_bounded() {
    // A chain as long as allowed holds; one longer is refused.
    let n = MAX_RIG_DEPTH as u64 + 1;
    let mut layers: Vec<Layer> = (1..=n).map(|i| held(i, Affine::translate(1.0, 0.0))).collect();
    for (i, l) in layers.iter_mut().enumerate().skip(1) {
        l.rig_parent = Some(LayerId(i as u64));
    }
    let extra = held(n + 1, Affine::IDENTITY);
    layers.push(extra);
    let d = doc(layers);
    let tl = d.timeline(0);
    assert_eq!(tl.rig_ancestors(LayerId(n)).len(), MAX_RIG_DEPTH);
    assert_eq!(tl.rig_check(LayerId(n + 1), LayerId(n)), Err(RigError::TooDeep));
    // The deepest layer sits one step right per ancestor.
    assert!(close(shown(&d, n, 0), Affine::translate(n as f32, 0.0)));
}

#[test]
fn files_keep_the_link_and_old_files_open_without_it() {
    let mut child = held(2, Affine::IDENTITY);
    child.rig_parent = Some(LayerId(1));
    let d = doc(vec![child, held(1, Affine::IDENTITY)]);
    let json = serde_json::to_string(&d).unwrap();
    assert!(json.contains("\"rig_parent\":1"));
    let back: Document = serde_json::from_str(&json).unwrap();
    assert_eq!(back.timeline(0).layers[0].rig_parent, Some(LayerId(1)));
    // A layer without a parent writes nothing, so files without parenting are as before.
    let plain = serde_json::to_string(&doc(vec![held(1, Affine::IDENTITY)])).unwrap();
    assert!(!plain.contains("rig_parent"));
    let old: Document = serde_json::from_str(&plain).unwrap();
    assert_eq!(old.timeline(0).layers[0].rig_parent, None);
}

#[test]
fn parenting_works_inside_a_symbol() {
    let mut child = held(2, Affine::translate(20.0, 0.0));
    child.rig_parent = Some(LayerId(1));
    let sym = Symbol { id: SymbolId(500), name: "Rig".into(), kind: SymbolKind::Graphic, linkage: None, scale_grid_kept: None, scale_grid: None, timeline: Timeline { layers: vec![Arc::new(child), Arc::new(tweened(1, pose(0.0, 0.0, 0.0), pose(0.0, 0.0, 90.0)))] } };
    let mut host = Layer::new(LayerId(9), "host");
    let mut k = Keyframe::new(0, 11);
    k.elements.push(Arc::new(Element { id: ElementId(900), name: String::new(), transform: Affine::translate(300.0, 300.0), color: ColorEffect::default(), transform_3d: None, kind: ElementKind::Symbol { symbol: SymbolId(500), loop_mode: LoopMode::Loop, first_frame: 0, blend: BlendMode::Normal, filters: Vec::new(), events: Vec::new(), pivot: None } }));
    host.keyframes = vec![Arc::new(k)];
    let mut d = doc(vec![host]);
    d.library.push(Arc::new(sym));
    let dl = evaluate(&d, 10);
    // Two items (the parent's square and the child's); the child's is 20 below the instance.
    let at: Vec<Vec2> = dl.items.iter().map(|i| i.transform.translation()).collect();
    assert!(at.iter().any(|p| (*p - Vec2::new(300.0, 320.0)).length() < 1e-3), "{at:?}");
}

#[test]
fn placing_a_layer_keeps_a_motion_tween_on_its_keys() {
    // A motion tween moving right while turning; placed by a quarter turn about the origin it
    // shows, on its keyed frames, exactly where the placing puts what it showed.
    let mut l = Layer::new(LayerId(1), "m");
    let mut k = Keyframe::new(0, 11);
    k.elements.push(square(100, pose(10.0, 0.0, 0.0)));
    let mut m = MotionTween::new(ElementId(100));
    m.set_key(Property::X, 10, 110.0, 10.0);
    m.set_key(Property::Rotation, 10, 400.0, 0.0);
    k.motion = Some(m);
    l.keyframes = vec![Arc::new(k)];
    let before: Vec<Affine> = [0, 10].iter().map(|f| evaluate_layer_elements(&l, *f).unwrap().1[0].transform).collect();
    let place = pose(5.0, 7.0, 90.0);
    l.place_all(&place);
    for (i, f) in [0u32, 10].iter().enumerate() {
        let now = evaluate_layer_elements(&l, *f).unwrap().1[0].transform;
        assert!(close(now, place.mul(&before[i])), "frame {f}: {now:?}");
    }
    // The rotation key keeps its turns (400° + 90°), not the wrapped angle.
    let rot = l.keyframes[0].motion.as_ref().unwrap().track(Property::Rotation).unwrap().keys.last().unwrap().value;
    assert!((rot - 490.0).abs() < 1e-2, "{rot}");
    // A move alone shifts the keys.
    let mut moved = l.clone();
    moved.place_all(&Affine::translate(3.0, 4.0));
    let (a, b) = (evaluate_layer_elements(&l, 10).unwrap().1[0].transform, evaluate_layer_elements(&moved, 10).unwrap().1[0].transform);
    assert!(close(b, Affine::translate(3.0, 4.0).mul(&a)));
}

#[test]
fn the_arm_sample_is_three_parented_layers() {
    let mut d = sample::arm();
    let before = d.clone();
    d.sanitize();
    assert_eq!(d.scenes, before.scenes, "the sample must already satisfy invariants");
    let tl = d.timeline(0);
    assert_eq!(tl.layers.iter().filter(|l| l.rig_parent.is_some()).count(), 2);
    // Mid-tween the hand is not where its own keys alone would put it.
    let hand = tl.layers[0].id;
    let own = evaluate_layer_elements(&tl.layers[0], 12).unwrap().1[0].transform;
    let on_stage = evaluate(&d, 12).items.iter().find(|i| i.layer == hand).unwrap().transform;
    assert!((own.translation() - on_stage.translation()).length() > 50.0);
}

#[test]
#[ignore = "timing; run with --ignored --nocapture"]
fn timing() {
    let mk = |linked: bool| {
        let mut layers: Vec<Layer> = (1..=60u64).map(|i| tweened(i, pose(5.0, 0.0, 0.0), pose(5.0, 0.0, 3.0))).collect();
        if linked {
            for (i, l) in layers.iter_mut().enumerate().skip(1) {
                l.rig_parent = Some(LayerId(i as u64));
            }
        }
        doc(layers)
    };
    for linked in [false, true] {
        let d = mk(linked);
        let t = std::time::Instant::now();
        let mut n = 0;
        for i in 0..2000u32 {
            n += evaluate(&d, i % 10).items.len();
        }
        println!("linked={linked}: {:.1} µs per evaluate ({n} items)", t.elapsed().as_secs_f64() * 1e6 / 2000.0);
    }
}
