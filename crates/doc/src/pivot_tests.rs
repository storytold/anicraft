//! The transformation point in tweens and layer opacity in the display list (ADR 0111).

use super::*;
use crate::{ColorEffect, Document, Keyframe, Symbol, SymbolId, SymbolKind, Tween};
use anicraft_geom::Vec2;

fn inst(transform: Affine, pivot: Option<Vec2>) -> Element {
    Element {
        id: ElementId(10),
        name: String::new(),
        transform,
        color: ColorEffect::default(),
        transform_3d: None,
        kind: ElementKind::Symbol { symbol: SymbolId(1), loop_mode: Default::default(), first_frame: 0, blend: Default::default(), filters: Vec::new(), events: Vec::new(), pivot },
    }
}

fn square(id: u64, x: f32, w: f32, h: f32) -> Element {
    Element { id: ElementId(id), name: String::new(), transform: Affine::translate(x, 0.0), color: ColorEffect::default(), transform_3d: None, kind: ElementKind::Shape(Shape::filled(Path::rect(0.0, 0.0, w, h), Paint::Solid(Rgba::BLACK))) }
}

/// A 40 × 20 arm whose registration point is its top left, tweened from `a` to `b` over ten frames.
fn arm_doc(a: Element, b: Element) -> Document {
    let mut d = Document::default();
    let mut tl = Timeline::default();
    let mut l = Layer::new(LayerId(50), "arm");
    l.keyframes = vec![Arc::new(Keyframe { elements: vec![Arc::new(square(2, 0.0, 40.0, 20.0))], ..Keyframe::new(0, 1) })];
    tl.layers.push(Arc::new(l));
    d.library.push(Arc::new(Symbol { id: SymbolId(1), name: "arm".into(), kind: SymbolKind::Graphic, linkage: None, scale_grid_kept: None, scale_grid: None, timeline: tl }));
    let mut k0 = Keyframe::new(0, 10);
    k0.elements.push(Arc::new(a));
    k0.tween = Some(Tween::default());
    let mut k1 = Keyframe::new(10, 1);
    k1.elements.push(Arc::new(b));
    Arc::make_mut(&mut d.timeline_mut(0).layers[0]).keyframes = vec![Arc::new(k0), Arc::new(k1)];
    d
}

fn at(d: &Document, frame: u32) -> Element {
    let (_, els) = evaluate_layer_elements(&d.timeline(0).layers[0], frame).unwrap();
    (*els[0]).clone()
}

fn near(a: Vec2, b: Vec2) -> bool {
    (a - b).length() < 1e-3
}

/// `m` turned by `deg` about stage point `c`.
fn turned(m: &Affine, c: Vec2, deg: f32) -> Affine {
    Affine::translate(c.x, c.y).mul(&Affine::rotate(deg.to_radians())).mul(&Affine::translate(-c.x, -c.y)).mul(m)
}

#[test]
fn a_tween_turns_an_instance_about_its_transformation_point() {
    // The point is the middle of the arm's far end (40, 10); the arm turns 90° about it.
    let p = Vec2::new(40.0, 10.0);
    let start = Affine::translate(100.0, 100.0);
    let hinge = start.apply(p);
    let d = arm_doc(inst(start, Some(p)), inst(turned(&start, hinge, 90.0), Some(p)));
    for f in 0..=10 {
        let e = at(&d, f);
        // The point stays on the hinge on every frame, and the arm has turned 9° a frame.
        assert!(near(e.transform.apply(p), hinge), "frame {f}: {:?}", e.transform.apply(p));
        assert!((e.transform.decompose().rotation.to_degrees() - 9.0 * f as f32).abs() < 1e-3, "frame {f}");
        assert_eq!(e.pivot(), Some(p));
    }
    // Analytic: halfway, the registration point is the hinge plus (-40, -10) turned 45°.
    let (s, c) = 45f32.to_radians().sin_cos();
    let want = hinge + Vec2::new(-40.0 * c + 10.0 * s, -40.0 * s - 10.0 * c);
    assert!(near(at(&d, 5).transform.translation(), want));
}

#[test]
fn without_a_point_a_tween_turns_about_the_registration_point_as_before() {
    let start = Affine::translate(100.0, 100.0);
    let end = turned(&start, Vec2::new(140.0, 110.0), 90.0);
    let d = arm_doc(inst(start, None), inst(end, None));
    let e = at(&d, 5);
    // The registration point goes straight from one key to the other: files from before the
    // point keep their motion exactly.
    let want = start.decompose().lerp(&end.decompose(), 0.5).compose();
    assert_eq!(e.transform, want);
    assert_eq!(e.pivot(), None);
}

#[test]
fn the_point_travels_and_one_keyframe_lends_it_to_the_other() {
    // Only the first key has a point; the arm also moves 60 px right while it turns.
    let p = Vec2::new(40.0, 10.0);
    let start = Affine::translate(100.0, 100.0);
    let hinge = start.apply(p);
    let end = Affine::translate(60.0, 0.0).mul(&turned(&start, hinge, 90.0));
    let d = arm_doc(inst(start, Some(p)), inst(end, None));
    for f in 0..=10 {
        let want = hinge + Vec2::new(6.0 * f as f32, 0.0);
        assert!(near(at(&d, f).transform.apply(p), want), "frame {f}");
    }
    // A point that is not a number, or is out of reach, counts as none.
    assert_eq!(inst(start, Some(Vec2::new(f32::NAN, 0.0))).pivot(), None);
    assert_eq!(inst(start, Some(Vec2::new(crate::MAX_PIVOT * 2.0, 0.0))).pivot(), None);
}

#[test]
fn a_motion_tween_turns_its_target_about_the_point() {
    use crate::motion::{MotionTween, Property};
    let p = Vec2::new(40.0, 10.0);
    let start = Affine::translate(100.0, 100.0);
    let hinge = start.apply(p);
    let base = inst(start, Some(p));
    let mut m = MotionTween::new(ElementId(10));
    m.set_key(Property::Rotation, 0, 0.0, 0.0);
    m.set_key(Property::Rotation, 10, 90.0, 0.0);
    for f in 0..=10 {
        let e = m.apply(&base, f as f32);
        assert!(near(e.transform.apply(p), hinge), "frame {f}: {:?}", e.transform.apply(p));
    }
    assert!((m.apply(&base, 10.0).transform.decompose().rotation.to_degrees() - 90.0).abs() < 1e-3);
    // Position keys carry the point with them.
    m.set_key(Property::X, 0, 100.0, 100.0);
    m.set_key(Property::X, 10, 150.0, 100.0);
    assert!(near(m.apply(&base, 10.0).transform.apply(p), hinge + Vec2::new(50.0, 0.0)));
    // Without a point the target is placed as before.
    let plain = inst(start, None);
    assert!(near(m.apply(&plain, 10.0).transform.translation(), Vec2::new(150.0, 100.0)));
}

#[test]
fn the_point_is_saved_with_the_instance_and_older_files_have_none() {
    let e = inst(Affine::IDENTITY, Some(Vec2::new(3.0, -4.5)));
    let json = serde_json::to_string(&e).unwrap();
    assert!(json.contains("\"pivot\""), "{json}");
    let back: Element = serde_json::from_str(&json).unwrap();
    assert_eq!(back, e);
    let plain = serde_json::to_string(&inst(Affine::IDENTITY, None)).unwrap();
    assert!(!plain.contains("pivot"), "{plain}");
    assert_eq!(serde_json::from_str::<Element>(&plain).unwrap().pivot(), None);
    // Only an instance keeps one.
    let mut shape = square(2, 0.0, 1.0, 1.0);
    assert!(!shape.set_pivot(Some(Vec2::ZERO)));
    assert_eq!(shape.pivot(), None);
    // A new scale or rotation placed about the point leaves the point where it was.
    let e = inst(Affine::translate(10.0, 20.0), Some(Vec2::new(5.0, 5.0)));
    let m = e.about_pivot(Affine::translate(10.0, 20.0).mul(&Affine::rotate(1.0)).mul(&Affine::scale(2.0, 3.0)));
    assert!(near(m.apply(Vec2::new(5.0, 5.0)), Vec2::new(15.0, 25.0)));
}

fn two_layers() -> Document {
    let mut d = Document::default();
    let tl = d.timeline_mut(0);
    let mut top = Layer::new(LayerId(70), "top");
    top.keyframes = vec![Arc::new(Keyframe { elements: vec![Arc::new(square(1, 0.0, 10.0, 10.0)), Arc::new(square(2, 5.0, 10.0, 10.0))], ..Keyframe::new(0, 1) })];
    let mut below = Layer::new(LayerId(71), "below");
    below.keyframes = vec![Arc::new(Keyframe { elements: vec![Arc::new(square(3, 0.0, 10.0, 10.0))], ..Keyframe::new(0, 1) })];
    tl.layers = vec![Arc::new(top), Arc::new(below)];
    d
}

#[test]
fn a_faded_layer_is_one_group_at_its_opacity() {
    let mut d = two_layers();
    assert!(evaluate(&d, 0).groups.is_empty(), "opaque layers need no group");
    Arc::make_mut(&mut d.timeline_mut(0).layers[0]).opacity = 0.4;
    for dl in [evaluate(&d, 0), evaluate_scene(&d, 0, 0)] {
        assert_eq!(dl.groups.len(), 1);
        assert_eq!((dl.groups[0].alpha, dl.groups[0].parent, dl.groups[0].blend), (0.4, None, crate::BlendMode::Normal));
        // Back to front: the layer below (no group), then the faded layer's two shapes in it.
        let groups: Vec<_> = dl.items.iter().map(|i| (i.element.0, i.group)).collect();
        assert_eq!(groups, vec![(3, None), (1, Some(0)), (2, Some(0))]);
        // Each item is as opaque as before on its own: the group fades them together.
        assert!(dl.items.iter().all(|i| i.color.opacity() == 1.0));
        assert!((dl.effective_alpha(&dl.items[1]) - 0.4).abs() < 1e-6);
    }
}

#[test]
fn a_folder_fades_the_layers_in_it_and_bad_values_are_tamed() {
    let mut d = two_layers();
    let tl = d.timeline_mut(0);
    let mut folder = Layer::new(LayerId(72), "folder");
    folder.kind = LayerKind::Folder;
    folder.opacity = 0.5;
    tl.layers.insert(0, Arc::new(folder));
    Arc::make_mut(&mut tl.layers[1]).parent = Some(LayerId(72));
    Arc::make_mut(&mut tl.layers[1]).opacity = 0.5;
    assert!((d.timeline(0).layer_opacity(1) - 0.25).abs() < 1e-6);
    assert_eq!(d.timeline(0).layer_opacity(2), 1.0);
    let dl = evaluate(&d, 0);
    assert_eq!(dl.groups.len(), 1);
    assert!((dl.groups[0].alpha - 0.25).abs() < 1e-6);
    // Out of range or not a number: clamped, or opaque.
    for (bad, want) in [(7.0, 1.0), (-1.0, 0.0), (f32::NAN, 1.0), (f32::INFINITY, 1.0)] {
        let mut l = Layer::new(LayerId(1), "l");
        l.opacity = bad;
        assert_eq!(l.shown_opacity(), want);
    }
}

#[test]
fn mask_and_guide_layers_ignore_opacity_and_masked_layers_keep_their_clip() {
    let mut d = two_layers();
    let tl = d.timeline_mut(0);
    Arc::make_mut(&mut tl.layers[0]).kind = LayerKind::Mask;
    Arc::make_mut(&mut tl.layers[0]).opacity = 0.3;
    Arc::make_mut(&mut tl.layers[1]).parent = Some(LayerId(70));
    Arc::make_mut(&mut tl.layers[1]).opacity = 0.5;
    let dl = evaluate(&d, 0);
    assert_eq!(dl.groups.len(), 1, "only the masked layer fades");
    assert_eq!((dl.groups[0].alpha, dl.groups[0].clip), (0.5, Some(0)));
    assert!(dl.items.iter().filter(|i| i.masks.is_some()).all(|i| i.group.is_none()));
    let mut d = two_layers();
    Arc::make_mut(&mut d.timeline_mut(0).layers[0]).kind = LayerKind::Guide;
    Arc::make_mut(&mut d.timeline_mut(0).layers[0]).opacity = 0.3;
    assert!(evaluate_scene(&d, 0, 0).groups.is_empty());
}

#[test]
fn layer_opacity_is_saved_only_when_set_and_older_files_are_opaque() {
    let mut l = Layer::new(LayerId(1), "l");
    let plain = serde_json::to_string(&l).unwrap();
    assert!(!plain.contains("opacity"), "{plain}");
    assert_eq!(serde_json::from_str::<Layer>(&plain).unwrap().opacity, 1.0);
    l.opacity = 0.25;
    let json = serde_json::to_string(&l).unwrap();
    assert!(json.contains("\"opacity\":0.25"), "{json}");
    assert_eq!(serde_json::from_str::<Layer>(&json).unwrap(), l);
}
