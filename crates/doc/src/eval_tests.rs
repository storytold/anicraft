use super::*;
use crate::{ColorEffect, Easing, Keyframe, Symbol, SymbolId, SymbolKind, Tween, TweenKind};

fn shape_el(id: u64, x: f32) -> Element {
    Element {
        id: ElementId(id),
        name: String::new(),
        transform: Affine::translate(x, 0.0),
        color: ColorEffect::default(),
        kind: ElementKind::Shape(Shape::filled(Path::rect(0.0, 0.0, 10.0, 10.0), Paint::Solid(Rgba::BLACK))),
    }
}

fn layer0(d: &mut Document) -> &mut Layer {
    Arc::make_mut(&mut d.timeline_mut(0).layers[0])
}

fn push(layer: &mut Layer, kf: usize, e: Element) {
    layer.keyframe_mut(kf).unwrap().elements.push(Arc::new(e));
}

fn tween_doc(easing: Easing) -> Document {
    let mut d = Document::default();
    let mut k0 = Keyframe::new(0, 10);
    k0.elements.push(Arc::new(shape_el(10, 0.0)));
    k0.tween = Some(Tween { easing, ..Default::default() });
    let mut k1 = Keyframe::new(10, 1);
    k1.elements.push(Arc::new(shape_el(10, 100.0)));
    layer0(&mut d).keyframes = vec![Arc::new(k0), Arc::new(k1)];
    d
}

#[test]
fn linear_tween_interpolates_translation() {
    let d = tween_doc(Easing::Linear);
    let x = |f| evaluate(&d, f).items[0].transform.translation().x;
    assert_eq!(x(0), 0.0);
    assert!((x(5) - 50.0).abs() < 1e-3);
    assert_eq!(x(10), 100.0);
}

#[test]
fn eased_tween_differs_from_linear() {
    let d = tween_doc(Easing::QuadIn);
    let x5 = evaluate(&d, 5).items[0].transform.translation().x;
    assert!((x5 - 25.0).abs() < 1e-3);
}

#[test]
fn static_frames_share_elements_with_the_document() {
    let d = tween_doc(Easing::Linear);
    let (_, els) = evaluate_layer_elements(&d.timeline(0).layers[0], 10).unwrap();
    assert!(matches!(els, Cow::Borrowed(_)), "a non-tweened frame must not copy elements");
}

#[test]
fn hidden_layers_are_skipped_and_guide_artwork_is_marked() {
    let mut d = tween_doc(Easing::Linear);
    layer0(&mut d).visible = false;
    assert!(evaluate(&d, 0).items.is_empty());
    layer0(&mut d).visible = true;
    layer0(&mut d).kind = LayerKind::Guide;
    // Guide artwork is listed for editing, but marked so nothing draws or exports it.
    let dl = evaluate(&d, 0);
    assert!(!dl.items.is_empty() && dl.items.iter().all(|i| i.guide));
}

#[test]
fn nested_symbols_compose_transforms_and_cycles_terminate() {
    let mut d = Document::default();
    let sid = SymbolId(99);
    // A symbol that contains a shape and an instance of itself: a cycle.
    let mut tl = Timeline::default();
    let mut layer = Layer::new(LayerId(50), "s");
    push(&mut layer, 0, shape_el(51, 0.0));
    push(
        &mut layer,
        0,
        Element {
            id: ElementId(52),
            name: String::new(),
            transform: Affine::translate(1.0, 0.0),
            color: ColorEffect::default(),
            kind: ElementKind::Symbol { symbol: sid, loop_mode: Default::default(), first_frame: 0, blend: Default::default(), filters: Vec::new(), events: Vec::new(), pivot: None },
        },
    );
    tl.layers.push(Arc::new(layer));
    d.library.push(Arc::new(Symbol { id: sid, name: "loop".into(), kind: SymbolKind::Graphic, linkage: None, scale_grid_kept: None, scale_grid: None, timeline: tl }));
    push(
        layer0(&mut d),
        0,
        Element {
            id: ElementId(60),
            name: String::new(),
            transform: Affine::translate(100.0, 0.0),
            color: ColorEffect::Alpha(0.5),
            kind: ElementKind::Symbol { symbol: sid, loop_mode: Default::default(), first_frame: 0, blend: Default::default(), filters: Vec::new(), events: Vec::new(), pivot: None },
        },
    );
    let dl = evaluate(&d, 0);
    assert_eq!(dl.items.len(), MAX_SYMBOL_DEPTH);
    assert_eq!(dl.items[0].transform.translation().x, 100.0);
    // The instance's opacity is its group's: it fades as one.
    assert_eq!((dl.items[0].color.mul[3], dl.effective_alpha(&dl.items[0])), (1.0, 0.5));
    assert_eq!(dl.groups.len(), 1);
    assert!(dl.items.iter().all(|i| i.element == ElementId(60)));
}

/// Layers top to bottom: mask (id 1), masked (2), masked (3), plain (4).
fn mask_doc() -> Document {
    let mut d = Document { scenes: vec![Arc::new(crate::Scene::new("Scene 1"))], ..Document::default() };
    for (id, kind, parent) in [(1, LayerKind::Mask, None), (2, LayerKind::Normal, Some(1)), (3, LayerKind::Normal, Some(1)), (4, LayerKind::Normal, None)] {
        let mut l = Layer::new(LayerId(id), format!("L{id}"));
        l.kind = kind;
        l.parent = parent.map(LayerId);
        push(&mut l, 0, shape_el(10 + id, 0.0));
        d.timeline_mut(0).layers.push(Arc::new(l));
    }
    d
}

#[test]
fn mask_layers_clip_the_layers_they_mask() {
    let d = mask_doc();
    let dl = evaluate(&d, 0);
    assert_eq!(dl.clip_parents, vec![None]);
    let by = |e: u64| dl.items.iter().find(|i| i.element == ElementId(e)).unwrap();
    assert_eq!((by(14).clip, by(14).masks), (None, None), "unmasked layer");
    assert_eq!((by(13).clip, by(12).clip), (Some(0), Some(0)), "masked layers");
    assert_eq!(by(11).masks, Some(0), "the mask layer's artwork forms the clip");
    // A hidden mask masks nothing; its layers draw normally.
    let mut h = d.clone();
    Arc::make_mut(&mut h.timeline_mut(0).layers[0]).visible = false;
    let dl = evaluate(&h, 0);
    assert!(dl.items.iter().all(|i| i.clip.is_none() && i.masks.is_none()) && dl.items.len() == 3);
}

#[test]
fn masks_inside_symbols_nest_in_the_instance_clip() {
    // The main timeline's masked layer holds an instance of a symbol that has its own mask.
    let mut d = mask_doc();
    let sid = SymbolId(99);
    d.library.push(Arc::new(Symbol { id: sid, name: "masked inside".into(), kind: SymbolKind::Graphic, linkage: None, scale_grid_kept: None, scale_grid: None, timeline: mask_doc().timeline(0).clone() }));
    let inst = Element {
        id: ElementId(60),
        name: String::new(),
        transform: Affine::IDENTITY,
        color: ColorEffect::default(),
        kind: ElementKind::Symbol { symbol: sid, loop_mode: Default::default(), first_frame: 0, blend: Default::default(), filters: Vec::new(), events: Vec::new(), pivot: None },
    };
    push(Arc::make_mut(&mut d.timeline_mut(0).layers[1]), 0, inst);
    let dl = evaluate(&d, 0);
    assert_eq!(dl.clip_parents, vec![None, Some(0)]);
    let inner: Vec<_> = dl.items.iter().filter(|i| i.element == ElementId(60)).map(|i| (i.clip, i.masks)).collect();
    // inside: unmasked layer keeps the outer clip; masked layers use the nested clip.
    assert_eq!(inner, vec![(Some(0), None), (Some(1), None), (Some(1), None), (Some(0), Some(1))]);
}

#[test]
fn a_soft_mask_layer_gives_its_clip_its_mode() {
    let mut d = mask_doc();
    assert_eq!((evaluate(&d, 0).clip_mode(0), evaluate(&d, 0).clip_modes.len()), (crate::MaskMode::Coverage, 0), "coverage stays implicit");
    Arc::make_mut(&mut d.timeline_mut(0).layers[0]).mask_mode = crate::MaskMode::Luminance;
    let dl = evaluate(&d, 0);
    assert_eq!(dl.clip_mode(0), crate::MaskMode::Luminance);
    // Appended lists keep their modes on their renumbered clips.
    let mut a = evaluate(&mask_doc(), 0);
    a.append(dl);
    assert_eq!((a.clip_mode(0), a.clip_mode(1)), (crate::MaskMode::Coverage, crate::MaskMode::Luminance));
    // The mode is saved only when it isn't coverage, and older files (without it) open as coverage.
    let json = serde_json::to_string(&*d.timeline(0).layers[0]).unwrap();
    assert!(json.contains("\"mask_mode\":\"luminance\""), "{json}");
    let plain = serde_json::to_string(&*mask_doc().timeline(0).layers[0]).unwrap();
    assert!(!plain.contains("mask_mode"));
    let back: Layer = serde_json::from_str(&json).unwrap();
    assert_eq!(back.mask_mode, crate::MaskMode::Luminance);
    assert_eq!(serde_json::from_str::<Layer>(&plain).unwrap().mask_mode, crate::MaskMode::Coverage);
}

#[test]
fn appending_lists_renumbers_clips() {
    let d = mask_doc();
    let mut a = evaluate(&d, 0);
    a.append(evaluate(&d, 0));
    assert_eq!(a.clip_parents, vec![None, None]);
    let second = &a.items[4..];
    assert!(second.iter().all(|i| i.clip.is_none_or(|c| c == 1) && i.masks.is_none_or(|c| c == 1)));
    assert!(second.iter().any(|i| i.masks == Some(1)));
}

#[test]
fn mask_links_must_stay_directly_under_their_mask() {
    let mut d = mask_doc();
    d.timeline_mut(0).repair_masks();
    assert_eq!(d.timeline(0).layers.iter().map(|l| l.parent).collect::<Vec<_>>(), vec![None, Some(LayerId(1)), Some(LayerId(1)), None]);
    assert_eq!(d.timeline(0).group_range(0), 0..3);
    assert_eq!(d.timeline(0).mask_above(3), Some(LayerId(1)), "a layer under the group would join it");
    // The plain layer moved between the masked ones breaks the run: the lower one lets go.
    d.timeline_mut(0).layers.swap(2, 3);
    d.timeline_mut(0).repair_masks();
    assert_eq!(d.timeline(0).layers[3].parent, None);
    assert_eq!(d.timeline(0).layers[1].parent, Some(LayerId(1)));
    // A masked layer above its mask, or a mask that is no longer a mask, lets go too.
    let mut e = mask_doc();
    e.timeline_mut(0).layers.swap(0, 1);
    e.timeline_mut(0).repair_masks();
    assert_eq!(e.timeline(0).layers[0].parent, None);
    assert_eq!(e.timeline(0).layers[2].parent, Some(LayerId(1)), "still directly under the mask");
    let mut f = mask_doc();
    Arc::make_mut(&mut f.timeline_mut(0).layers[0]).kind = LayerKind::Normal;
    f.sanitize();
    assert!(f.timeline(0).layers.iter().all(|l| l.parent.is_none()));
}

#[test]
fn layer_order_is_back_to_front() {
    let mut d = Document::default();
    push(layer0(&mut d), 0, shape_el(1, 0.0));
    let mut bottom = Layer::new(LayerId(7), "bottom");
    push(&mut bottom, 0, shape_el(2, 0.0));
    d.timeline_mut(0).layers.push(Arc::new(bottom));
    let dl = evaluate(&d, 0);
    assert_eq!(dl.items[0].element, ElementId(2));
    assert_eq!(dl.items[1].element, ElementId(1));
}

#[test]
fn shape_tweens_morph_between_different_shapes_and_classic_ones_hold() {
    // A square at frame 0 becomes a circle at frame 10.
    let mut d = Document::default();
    let shape = |id, path: Path| Element {
        id: ElementId(id),
        name: String::new(),
        transform: Affine::IDENTITY,
        color: ColorEffect::default(),
        kind: ElementKind::Shape(Shape::filled(path, Paint::Solid(Rgba::BLACK))),
    };
    let mut k0 = Keyframe::new(0, 10);
    k0.elements.push(Arc::new(shape(1, Path::rect(0.0, 0.0, 100.0, 100.0))));
    let mut k1 = Keyframe::new(10, 1);
    k1.elements.push(Arc::new(shape(1, Path::ellipse(250.0, 50.0, 50.0, 50.0))));
    layer0(&mut d).keyframes = vec![Arc::new(k0), Arc::new(k1)];
    let path_at = |d: &Document, f| evaluate(d, f).items[0].path.clone();

    // Classic tween: incompatible shapes hold the first one until the next keyframe.
    layer0(&mut d).keyframe_mut(0).unwrap().tween = Some(Tween { easing: Easing::Linear, kind: TweenKind::Classic, ..Default::default() });
    assert_eq!(*path_at(&d, 5), Path::rect(0.0, 0.0, 100.0, 100.0));
    // Shape tween: halfway, the shape sits between the two and is neither of them.
    layer0(&mut d).keyframe_mut(0).unwrap().tween = Some(Tween { easing: Easing::Linear, kind: TweenKind::Shape, ..Default::default() });
    let mid = path_at(&d, 5);
    let c = mid.bounds().center();
    assert!((c.x - 150.0).abs() < 1.0 && (c.y - 50.0).abs() < 1.0, "{c:?}");
    assert_ne!(*mid, Path::rect(0.0, 0.0, 100.0, 100.0));
    // Keyframes themselves show exactly what was drawn.
    assert_eq!(*path_at(&d, 0), Path::rect(0.0, 0.0, 100.0, 100.0));
    assert_eq!(*path_at(&d, 10), Path::ellipse(250.0, 50.0, 50.0, 50.0));
    // The kind survives a save (and older files without it read as classic).
    let t: Tween = serde_json::from_str(&serde_json::to_string(&Tween { easing: Easing::Linear, kind: TweenKind::Shape, ..Default::default() }).unwrap()).unwrap();
    assert_eq!(t.kind, TweenKind::Shape);
    assert_eq!(serde_json::from_str::<Tween>(r#"{"easing":"Linear"}"#).unwrap().kind, TweenKind::Classic);
}

#[test]
fn motion_guides_steer_tweens_and_their_artwork_is_marked() {
    // Guide (id 1): an L-shaped line, right 100 then down 100. Guided layer (id 2): a box
    // tweened from (0,0) to (100,100) — a straight diagonal without the guide.
    let mut d = Document { scenes: vec![Arc::new(crate::Scene::new("Scene 1"))], ..Document::default() };
    let mut g = Layer::new(LayerId(1), "Guide");
    g.kind = LayerKind::Guide;
    let mut l = Path::new();
    l.move_to(anicraft_geom::Vec2::new(0.0, 0.0)).line_to(anicraft_geom::Vec2::new(100.0, 0.0)).line_to(anicraft_geom::Vec2::new(100.0, 100.0));
    let mut line = Shape::filled(l, Paint::Solid(Rgba::BLACK)).with_stroke(Rgba::BLACK, 1.0);
    line.fill = None;
    g.keyframe_mut(0).unwrap().duration = 11;
    push(&mut g, 0, Element { id: ElementId(5), name: String::new(), transform: Affine::IDENTITY, color: ColorEffect::default(), kind: ElementKind::Shape(line) });
    let mut moving = Layer::new(LayerId(2), "Box");
    moving.parent = Some(LayerId(1));
    let mut k0 = Keyframe::new(0, 10);
    k0.elements.push(Arc::new(shape_el(10, 0.0)));
    k0.tween = Some(Tween { easing: Easing::Linear, ..Default::default() });
    let mut k1 = Keyframe::new(10, 1);
    k1.elements.push(Arc::new(Element { transform: Affine::translate(100.0, 100.0), ..shape_el(10, 0.0) }));
    moving.keyframes = vec![Arc::new(k0), Arc::new(k1)];
    d.timeline_mut(0).layers = vec![Arc::new(g), Arc::new(moving)];
    d.timeline_mut(0).repair_masks();
    assert_eq!(d.timeline(0).layers[1].parent, Some(LayerId(1)), "a guided layer under its guide holds");

    let at = |d: &Document, f| evaluate(d, f).items.iter().find(|i| i.element == ElementId(10)).unwrap().transform;
    // Halfway along the 200-long path is the corner (100, 0), not the diagonal's (50, 50).
    let mid = at(&d, 5).translation();
    assert!((mid.x - 100.0).abs() < 1e-3 && mid.y.abs() < 1e-3, "{mid:?}");
    let q = at(&d, 2).translation();
    assert!((q.x - 40.0).abs() < 1e-3 && q.y.abs() < 1e-3, "{q:?}");
    // Keyframes stay where they were drawn.
    assert_eq!(at(&d, 0).translation(), anicraft_geom::Vec2::new(0.0, 0.0));
    // Orient to path: on the downward leg the box has turned a quarter turn.
    Arc::make_mut(&mut Arc::make_mut(&mut d.timeline_mut(0).layers[1]).keyframes[0]).tween.as_mut().unwrap().orient = true;
    let m = at(&d, 8);
    assert!((m.m[0]).abs() < 1e-3 && (m.m[1] - 1.0).abs() < 1e-3, "rotated 90°: {:?}", m.m);
    // The guide's line is in the list, marked, so it can be edited but is never drawn.
    let dl = evaluate(&d, 0);
    assert!(dl.items.iter().any(|i| i.element == ElementId(5) && i.guide));
    assert!(dl.items.iter().filter(|i| i.element != ElementId(5)).all(|i| !i.guide));
}

#[test]
fn movie_clips_play_their_own_timeline_graphics_keep_step_buttons_hold_up() {
    // A 4-frame symbol: a box at x = 10·frame.
    let sid = SymbolId(99);
    let mut st = Layer::new(LayerId(50), "anim");
    st.keyframes = (0..4u32)
        .map(|f| {
            let mut k = Keyframe::new(f, 1);
            k.elements.push(Arc::new(shape_el(51, f as f32 * 10.0)));
            Arc::new(k)
        })
        .collect();
    let mut d = Document::default();
    d.library.push(Arc::new(Symbol { id: sid, name: "Clip".into(), kind: SymbolKind::MovieClip, linkage: None, scale_grid_kept: None, scale_grid: None, timeline: Timeline { layers: vec![Arc::new(st)] } }));
    // The same instance on two keyframes back to back (frames 0–4 and 5–11), moved on the second.
    let inst = |x: f32| Element {
        id: ElementId(60),
        name: String::new(),
        transform: Affine::translate(x, 0.0),
        color: ColorEffect::default(),
        kind: ElementKind::Symbol { symbol: sid, loop_mode: crate::LoopMode::Loop, first_frame: 0, blend: Default::default(), filters: Vec::new(), events: Vec::new(), pivot: None },
    };
    let (mut k0, mut k1) = (Keyframe::new(0, 5), Keyframe::new(5, 7));
    k0.elements.push(Arc::new(inst(0.0)));
    k1.elements.push(Arc::new(inst(1000.0)));
    layer0(&mut d).keyframes = vec![Arc::new(k0), Arc::new(k1)];
    let x = |d: &Document, f| evaluate(d, f).items[0].transform.translation().x;
    // Movie clip: frame 7 of the parent is frame 7 of its run (7 mod 4 = 3), across the keyframe.
    assert_eq!(x(&d, 7), 1030.0);
    assert_eq!(x(&d, 2), 20.0);
    // Graphic: in step with the keyframe it is on (7 − 5 = 2).
    let kind = |d: &mut Document, k| Arc::make_mut(&mut d.library[0]).kind = k;
    kind(&mut d, SymbolKind::Graphic);
    assert_eq!(x(&d, 7), 1020.0);
    // Button: always its first (Up) frame.
    kind(&mut d, SymbolKind::Button);
    assert_eq!((x(&d, 2), x(&d, 7)), (0.0, 1000.0));
    // A different instance on the next keyframe starts its movie clip over.
    kind(&mut d, SymbolKind::MovieClip);
    let mut k1b = Keyframe::new(5, 7);
    k1b.elements.push(Arc::new(Element { id: ElementId(61), ..inst(1000.0) }));
    layer0(&mut d).keyframes[1] = Arc::new(k1b);
    assert_eq!(x(&d, 7), 1020.0);
}

/// A 24-frame motion tween on one shape: X keyed at 0, 12 (ease in) and 23; alpha keyed at 23.
fn motion_doc() -> Document {
    use crate::{MotionTween, Property};
    let mut d = Document::default();
    let mut k = Keyframe::new(0, 24);
    k.elements.push(Arc::new(shape_el(10, 0.0)));
    let mut m = MotionTween::new(ElementId(10));
    m.set_key(Property::X, 12, 120.0, 0.0);
    m.set_key(Property::X, 23, 120.0, 0.0);
    if let Some(t) = m.track_mut(Property::X) {
        t.keys[0].ease = Easing::QuadIn;
    }
    m.set_key(Property::Alpha, 23, 0.0, 100.0);
    k.motion = Some(m);
    // A classic tween on the same span is ignored: the motion tween wins.
    k.tween = Some(Tween::default());
    layer0(&mut d).keyframes = vec![Arc::new(k)];
    d
}

#[test]
fn a_motion_tween_drives_each_property_on_its_own_curve() {
    let d = motion_doc();
    let at = |f| {
        let dl = evaluate(&d, f);
        (dl.items[0].transform.translation().x, dl.items[0].color.mul[3])
    };
    assert_eq!(at(0), (0.0, 1.0));
    // X eases in over 0..12 (a quarter of the way at half time); alpha runs linearly over 0..23.
    let (x6, a6) = at(6);
    assert!((x6 - 30.0).abs() < 1e-3, "{x6}");
    assert!((a6 - (1.0 - 6.0 / 23.0)).abs() < 1e-5, "{a6}");
    // X holds on 12..23 while alpha keeps fading.
    let (x18, a18) = at(18);
    assert!((x18 - 120.0).abs() < 1e-3 && a18 < a6);
    assert_eq!(at(23).1, 0.0);
    // Y, scale and rotation were never keyed: they stay the element's own.
    let item = &evaluate(&d, 18).items[0];
    let dc = item.transform.decompose();
    assert!(dc.ty.abs() < 1e-4 && (dc.sx - 1.0).abs() < 1e-5 && dc.rotation.abs() < 1e-5);
}

#[test]
fn motion_tweens_survive_a_file_round_trip_and_old_files_have_none() {
    let d = motion_doc();
    let json = serde_json::to_string(&d).unwrap();
    let back: Document = serde_json::from_str(&json).unwrap();
    assert_eq!(back, d);
    assert!(back.timeline(0).layers[0].keyframes[0].motion.is_some());
    // A keyframe written before motion tweens existed loads with none.
    let old: Keyframe = serde_json::from_str(r#"{"start": 0, "duration": 3}"#).unwrap();
    assert!(old.motion.is_none());
    // Loading repairs a hostile tween and drops a classic tween beside it.
    let mut d2 = d.clone();
    let k = layer0(&mut d2).keyframe_mut(0).unwrap();
    if let Some(m) = k.motion.as_mut() {
        m.tracks[0].keys.reverse();
        m.tracks[0].keys[0].value = f32::INFINITY;
    }
    d2.sanitize();
    let k = &d2.timeline(0).layers[0].keyframes[0];
    assert!(k.tween.is_none() && k.motion.as_ref().is_some_and(crate::MotionTween::is_sane));
    for f in 0..30 {
        assert!(evaluate(&d2, f).items.iter().all(|i| i.transform.is_finite()));
    }
}

/// A square on frame 0 shape-tweening to a circle on frame 10 (the square moved 200 right,
/// so element transforms count too).
fn square_to_circle(hints: Vec<crate::ShapeHint>) -> Document {
    let mut d = Document::default();
    let el = |id, x, path| Element { id: ElementId(id), name: String::new(), transform: Affine::translate(x, 0.0), color: ColorEffect::default(), kind: ElementKind::Shape(Shape::filled(path, Paint::Solid(Rgba::BLACK))) };
    let mut k0 = Keyframe::new(0, 10);
    k0.elements.push(Arc::new(el(1, 0.0, Path::rect(0.0, 0.0, 100.0, 100.0))));
    k0.tween = Some(Tween { easing: Easing::Linear, kind: TweenKind::Shape, ..Default::default() });
    k0.hints = hints;
    let mut k1 = Keyframe::new(10, 1);
    k1.elements.push(Arc::new(el(1, 200.0, Path::ellipse(50.0, 50.0, 50.0, 50.0))));
    layer0(&mut d).keyframes = vec![Arc::new(k0), Arc::new(k1)];
    d
}

fn has_anchor(p: &Path, m: &Affine, q: anicraft_geom::Vec2) -> bool {
    p.cmds.iter().any(|c| match *c {
        anicraft_geom::PathCmd::MoveTo(a) | anicraft_geom::PathCmd::CubicTo(_, _, a) => m.apply(a).distance(q) < 1e-2,
        _ => false,
    })
}

#[test]
fn shape_hints_carry_the_hinted_corner_where_asked() {
    use anicraft_geom::Vec2;
    let (corner, bottom) = (Vec2::new(0.0, 0.0), Vec2::new(250.0, 100.0));
    let hinted = square_to_circle(vec![crate::ShapeHint { start: corner, end: bottom }]);
    let at = |d: &Document, f| {
        let dl = evaluate(d, f);
        let i = &dl.items[0];
        ((*i.path).clone(), i.transform)
    };
    // Halfway the corner is exactly halfway along its way (stage space).
    let (p, m) = at(&hinted, 5);
    assert!(has_anchor(&p, &m, corner.lerp(bottom, 0.5)), "{p:?}");
    // Without the hint it goes elsewhere.
    let (p, m) = at(&square_to_circle(Vec::new()), 5);
    assert!(!has_anchor(&p, &m, corner.lerp(bottom, 0.5)));
    // An unplaced hint (off the outline, as when just added) changes nothing.
    let off = square_to_circle(vec![crate::ShapeHint { start: Vec2::new(50.0, 50.0), end: bottom }]);
    let plain = square_to_circle(Vec::new());
    assert_eq!(*evaluate(&off, 5).items[0].path, *evaluate(&plain, 5).items[0].path);
    // The keyframes themselves are untouched by hints.
    assert_eq!(*evaluate(&hinted, 0).items[0].path, Path::rect(0.0, 0.0, 100.0, 100.0));
    // Placement and the hinted tween lookup.
    let l = &hinted.timeline(0).layers[0];
    assert_eq!(l.hint_tween_at(0), Some((0, crate::HintSide::Start)));
    assert_eq!(l.hint_tween_at(9), Some((0, crate::HintSide::Start)));
    assert_eq!(l.hint_tween_at(10), Some((0, crate::HintSide::End)));
    assert_eq!(crate::placed_on(&l.keyframes[0].elements, corner), Some(0));
    assert_eq!(crate::placed_on(&l.keyframes[1].elements, bottom), Some(0));
    assert_eq!(crate::placed_on(&l.keyframes[0].elements, Vec2::new(50.0, 50.0)), None);
    assert!(crate::shapes_centre(&l.keyframes[1].elements).unwrap().distance(Vec2::new(250.0, 50.0)) < 1e-3);
}

#[test]
fn shape_hints_survive_a_round_trip_and_old_files_have_none() {
    let d = square_to_circle(vec![crate::ShapeHint { start: anicraft_geom::Vec2::new(0.0, 0.0), end: anicraft_geom::Vec2::new(250.0, 100.0) }]);
    let k = &d.timeline(0).layers[0].keyframes[0];
    let json = serde_json::to_string(&**k).unwrap();
    assert!(json.contains("\"hints\""));
    let back: Keyframe = serde_json::from_str(&json).unwrap();
    assert_eq!(back, **k);
    // A keyframe without hints writes none, and a file from before hints reads with none.
    let plain = serde_json::to_string(&Keyframe::new(0, 1)).unwrap();
    assert!(!plain.contains("hints"));
    let old: Keyframe = serde_json::from_str(r#"{"start":0,"duration":3}"#).unwrap();
    assert!(old.hints.is_empty());
}

/// A tween from (0, 0), unturned, full size and opaque to (100, 0), half a turn, double size
/// and transparent, over 10 frames.
fn group_doc(tween: Tween) -> Document {
    let mut d = Document::default();
    let mut a = shape_el(10, 0.0);
    a.color = ColorEffect::Alpha(1.0);
    let mut b = shape_el(10, 100.0);
    b.transform = Affine::translate(100.0, 0.0).mul(&Affine::rotate(std::f32::consts::FRAC_PI_2)).mul(&Affine::scale(2.0, 2.0));
    b.color = ColorEffect::Alpha(0.0);
    let mut k0 = Keyframe::new(0, 10);
    k0.elements.push(Arc::new(a));
    k0.tween = Some(tween);
    let mut k1 = Keyframe::new(10, 1);
    k1.elements.push(Arc::new(b));
    layer0(&mut d).keyframes = vec![Arc::new(k0), Arc::new(k1)];
    d
}

#[test]
fn classic_tweens_ease_each_property_group_its_own_way() {
    use crate::EaseGroup;
    let mut tw = Tween::new(Easing::Linear, TweenKind::Classic);
    tw.set_ease(EaseGroup::Position, Some(Easing::BounceOut));
    tw.set_ease(EaseGroup::Scale, Some(Easing::QuadIn));
    tw.set_ease(EaseGroup::Color, Some(Easing::CubicOut));
    let d = group_doc(tw.clone());
    let it = |f| {
        let dl = evaluate(&d, f);
        let i = &dl.items[0];
        (i.transform.decompose(), i.color.mul[3])
    };
    let (m, alpha) = it(5);
    assert!((m.tx - 100.0 * Easing::BounceOut.apply(0.5)).abs() < 1e-2, "position: {}", m.tx);
    assert!((m.sx - (1.0 + Easing::QuadIn.apply(0.5))).abs() < 1e-3, "scale: {}", m.sx);
    // Rotation has no ease of its own: the tween's (linear).
    assert!((m.rotation - std::f32::consts::FRAC_PI_4).abs() < 1e-3, "rotation: {}", m.rotation);
    assert!((alpha - (1.0 - Easing::CubicOut.apply(0.5))).abs() < 1e-3, "colour: {alpha}");
    // Groups stay in order, once each; clearing one gives it the tween's ease again.
    tw.set_ease(EaseGroup::Position, Some(Easing::SineIn));
    assert_eq!(tw.groups.iter().map(|g| g.0).collect::<Vec<_>>(), [EaseGroup::Position, EaseGroup::Scale, EaseGroup::Color]);
    tw.set_ease(EaseGroup::Scale, None);
    assert_eq!(tw.ease(EaseGroup::Scale), &Easing::Linear);
    // With one ease for everything, every group follows it exactly as before.
    let one = group_doc(Tween::new(Easing::QuadOut, TweenKind::Classic));
    let mut same = Tween::new(Easing::QuadOut, TweenKind::Classic);
    for g in EaseGroup::ALL {
        same.set_ease(g, Some(Easing::QuadOut));
    }
    let both = group_doc(same);
    for f in 0..=10 {
        assert_eq!(evaluate(&one, f).items[0].transform, evaluate(&both, f).items[0].transform);
    }
    // Older files' tweens (no groups) still read.
    let t: Tween = serde_json::from_str(r#"{"easing": "QuadIn"}"#).unwrap();
    assert!(t.groups.is_empty());
}

#[test]
fn a_custom_ease_drives_a_classic_tween() {
    let c = crate::CustomEase::from_flat(&[(0.0, 0.0), (0.2, 1.5), (0.4, 1.5), (0.5, 1.2), (0.6, 0.9), (0.8, 1.0), (1.0, 1.0)].map(|(x, y)| anicraft_geom::Vec2::new(x, y))).unwrap();
    let e = Easing::Custom(Arc::new(c.clone()));
    let d = tween_doc(e);
    for f in 0..10 {
        let x = evaluate(&d, f).items[0].transform.translation().x;
        assert!((x - 100.0 * c.apply(f as f32 / 10.0)).abs() < 1e-2, "frame {f}: {x}");
    }
    // It overshoots past the second keyframe and comes back.
    assert!(evaluate(&d, 5).items[0].transform.translation().x > 100.0);
}

#[test]
fn instance_filters_tween_when_they_pair_up() {
    use crate::Filter;
    let mut d = Document::default();
    let sid = SymbolId(90);
    d.library.push(Arc::new(Symbol { id: sid, name: "Clip".into(), kind: SymbolKind::MovieClip, linkage: None, scale_grid_kept: None, scale_grid: None, timeline: Timeline { layers: vec![Arc::new(Layer::new(LayerId(91), "x"))] } }));
    let inst = |filters: Vec<Filter>| Element { id: ElementId(92), name: String::new(), transform: Affine::IDENTITY, color: ColorEffect::default(), kind: ElementKind::Symbol { symbol: sid, loop_mode: Default::default(), first_frame: 0, blend: Default::default(), filters: filters.into_iter().map(Into::into).collect(), events: Vec::new(), pivot: None } };
    let blur = |b: f32| Filter::Blur { blur_x: b, blur_y: b, quality: 1 };
    let mut tw = Tween::new(Easing::Linear, TweenKind::Classic);
    tw.set_ease(crate::EaseGroup::Filters, Some(Easing::QuadIn));
    let mut k0 = Keyframe::new(0, 10);
    k0.elements.push(Arc::new(inst(vec![blur(0.0), Filter::new("glow").unwrap()])));
    k0.tween = Some(tw);
    let mut k1 = Keyframe::new(10, 1);
    k1.elements.push(Arc::new(inst(vec![blur(40.0), Filter::new("glow").unwrap()])));
    layer0(&mut d).keyframes = vec![Arc::new(k0), Arc::new(k1)];
    let filters = |f| match &crate::evaluate_layer_elements(&d.timeline(0).layers[0], f).unwrap().1[0].kind {
        ElementKind::Symbol { filters, .. } => filters.clone(),
        _ => Vec::new(),
    };
    assert_eq!(filters(5)[0], blur(40.0 * 0.25).into(), "eased by the filters group's ease");
    let e = |f: Filter| crate::FilterEntry::from(f);
    // A switched-off filter still pairs up (and stays off, as the first keyframe has it).
    let off = crate::FilterEntry { filter: blur(0.0), enabled: false };
    let got = crate::filter::lerp_filters(std::slice::from_ref(&off), &[e(blur(10.0))], 0.5).unwrap();
    assert_eq!(got, vec![crate::FilterEntry { filter: blur(5.0), enabled: false }]);
    assert_eq!(crate::filter::lerp_filters(&[e(blur(1.0))], &[e(blur(1.0))], 0.5), None, "equal lists hold as they are");
}

/// ADR 0099: lists that differ pair as Animate pairs them. Filters of the same kind in the same
/// order travel to each other; one without a partner travels to, or from, itself doing nothing,
/// so it fades out or in. The first keyframe itself shows exactly its own list.
#[test]
fn mismatched_filter_lists_pair_and_the_rest_fade() {
    use crate::Filter;
    use crate::filter::{lerp_filters, pairing};
    let e = |f: Filter| crate::FilterEntry::from(f);
    let blur = |b: f32| Filter::Blur { blur_x: b, blur_y: b, quality: 1 };
    let glow = |s: f32, knockout: bool| Filter::Glow { blur_x: 8.0, blur_y: 8.0, strength: s, quality: 2, color: Rgba::hex(0x00ff00), inner: false, knockout };
    // A glow only the second keyframe has fades in from nothing, with its own colour and
    // switches; the blur both have travels as before.
    let (a, b) = (vec![e(blur(2.0))], vec![e(blur(6.0)), e(glow(2.0, true))]);
    assert_eq!(pairing(&a, &b), Some(vec![(Some(0), Some(0)), (None, Some(1))]));
    let mid = lerp_filters(&a, &b, 0.5).unwrap();
    assert_eq!(mid[0], e(blur(4.0)));
    assert_eq!(mid[1], e(Filter::Glow { blur_x: 4.0, blur_y: 4.0, strength: 1.0, quality: 2, color: Rgba::hex(0x00ff00), inner: false, knockout: true }));
    assert_eq!(lerp_filters(&a, &b, 0.0), None, "on the first keyframe its own list shows, exactly");
    assert_eq!(lerp_filters(&a, &b, 1.0).unwrap(), b);
    // One only the first keyframe has fades out.
    let out = lerp_filters(&b, &a, 0.75).unwrap();
    assert_eq!(out.len(), 2);
    assert!(matches!(out[1].filter, Filter::Glow { strength, blur_x, .. } if (strength - 0.5).abs() < 1e-6 && (blur_x - 2.0).abs() < 1e-6), "{out:?}");
    // Another kind in the same place: the old one fades out as the new one fades in.
    let swap = lerp_filters(&[e(blur(8.0))], &[e(glow(1.0, false))], 0.25).unwrap();
    assert_eq!(swap.len(), 2);
    assert_eq!(swap[0], e(blur(6.0)));
    assert!(matches!(swap[1].filter, Filter::Glow { strength, .. } if (strength - 0.25).abs() < 1e-6));
    // Reordered lists keep the longest run of kinds they share paired; the rest fade.
    let (x, y) = (vec![e(blur(1.0)), e(glow(1.0, false)), e(Filter::new("bevel").unwrap())], vec![e(glow(3.0, false)), e(Filter::new("bevel").unwrap()), e(blur(9.0))]);
    assert_eq!(pairing(&x, &y), Some(vec![(Some(0), None), (Some(1), Some(0)), (Some(2), Some(1)), (None, Some(2))]));
    // A filter doing nothing keeps its look for when it fades in: colours, angle, switches.
    let Filter::DropShadow { blur_x, strength, distance, angle, color, hide_object, .. } = (Filter::DropShadow { blur_x: 9.0, blur_y: 9.0, strength: 2.0, quality: 3, angle: 120.0, distance: 7.0, color: Rgba::hex(0x112233), inner: false, knockout: false, hide_object: true }).neutral() else { panic!() };
    assert_eq!((blur_x, strength, distance, angle, color, hide_object), (0.0, 0.0, 0.0, 120.0, Rgba::hex(0x112233), true));
    assert_eq!(Filter::AdjustColor { brightness: 40.0, contrast: -20.0, saturation: 10.0, hue: 90.0 }.neutral(), Filter::new("adjust_color").unwrap());
    // Hostile lengths are refused, not walked.
    let long: Vec<crate::FilterEntry> = (0..200).map(|_| e(blur(1.0))).collect();
    assert_eq!(pairing(&long, &a), None);
    assert_eq!(lerp_filters(&long, &a, 0.5), None);
    assert_eq!(pairing(&[], &[]), Some(Vec::new()));
}

fn text_el(id: u64, filters: Vec<crate::FilterEntry>, field: Option<crate::TextField>) -> Element {
    let block = crate::TextBlock {
        text: "Hi".into(),
        font: "Test".into(),
        size: 20.0,
        bold: false,
        italic: false,
        color: Rgba::BLACK,
        align: Default::default(),
        letter_spacing: 0.0,
        line_spacing: 0.0,
        width: None,
        outline: Arc::new(Path::rect(2.0, 2.0, 30.0, 16.0)),
        box_width: 40.0,
        box_height: 24.0,
        field,
        on_path: None,
        filters,
        ..crate::TextBlock::default()
    };
    Element { id: ElementId(id), name: String::new(), transform: Affine::translate(50.0, 40.0), color: ColorEffect::Alpha(0.5), kind: ElementKind::Text(Arc::new(block)) }
}

/// ADR 0099: text with filters draws in a group of its own that the filters run on, as a movie
/// clip does: static text and fields alike, their box and border included. Switched-off
/// filters draw nothing, and files from before open without any.
#[test]
fn text_with_filters_draws_in_a_filtered_group() {
    use crate::{Filter, FilterEntry, TextField, TextKind};
    let shadow = FilterEntry::from(Filter::new("drop_shadow").unwrap());
    let off = FilterEntry { filter: Filter::new("blur").unwrap(), enabled: false };
    let field = TextField { border: true, background: true, ..TextField::new(TextKind::Dynamic) };
    for field in [None, Some(field)] {
        let mut d = Document::default();
        push(layer0(&mut d), 0, text_el(1, vec![off.clone(), shadow.clone()], field.clone()));
        let dl = evaluate(&d, 0);
        assert_eq!(dl.groups.len(), 1, "one group for the text");
        let g = &dl.groups[0];
        assert_eq!(&*g.filters, std::slice::from_ref(&shadow.filter), "only the filter that is on");
        assert_eq!(g.alpha, 0.5, "the text's alpha fades the group, after the filters");
        assert!(!dl.items.is_empty() && dl.items.iter().all(|i| i.group == Some(0)), "every item of the text is in it");
        assert!(dl.items.iter().all(|i| i.color.mul[3] == 1.0), "and none fades on its own");
        // Without filters that draw, the text is as it was before: no group.
        let mut plain = Document::default();
        push(layer0(&mut plain), 0, text_el(1, vec![off.clone()], field));
        let dl = evaluate(&plain, 0);
        assert!(dl.groups.is_empty() && dl.items.iter().all(|i| i.group.is_none()));
    }
    // The file: filters ride with the block, left out when there are none, and old files open.
    let with = text_el(1, vec![shadow.clone(), off], None);
    let json = serde_json::to_value(&with).unwrap();
    assert_eq!(json["kind"]["Text"]["filters"][0]["type"], "drop_shadow");
    assert_eq!(json["kind"]["Text"]["filters"][1]["enabled"], false);
    assert_eq!(serde_json::from_value::<Element>(json).unwrap(), with);
    let mut old = serde_json::to_value(text_el(1, Vec::new(), None)).unwrap();
    assert!(old["kind"]["Text"].get("filters").is_none(), "no key without filters: files stay as they were");
    old["kind"]["Text"]["text"] = serde_json::json!("Old");
    assert!(serde_json::from_value::<Element>(old).unwrap().kind.filters().is_some_and(<[FilterEntry]>::is_empty));
    // One list for what takes filters; nothing else has one.
    assert!(shape_el(9, 0.0).kind.filters().is_none() && ElementKind::Camera.filters().is_none());
}

/// ADR 0099: a classic tween between two keyframes of a text tweens its filters as it does a
/// movie clip's.
#[test]
fn text_filters_tween_between_keyframes() {
    use crate::{Filter, FilterEntry};
    let glow = |blur: f32, color: u32| FilterEntry::from(Filter::Glow { blur_x: blur, blur_y: blur, strength: 1.0, quality: 1, color: Rgba::hex(color), inner: false, knockout: false });
    let mut d = Document::default();
    let mut k0 = Keyframe::new(0, 10);
    k0.elements.push(Arc::new(text_el(1, vec![glow(0.0, 0xff0000)], None)));
    k0.tween = Some(Tween::new(Easing::Linear, TweenKind::Classic));
    let mut k1 = Keyframe::new(10, 1);
    k1.elements.push(Arc::new(text_el(1, vec![glow(20.0, 0x0000ff)], None)));
    layer0(&mut d).keyframes = vec![Arc::new(k0), Arc::new(k1)];
    let at5 = crate::evaluate_layer_elements(&d.timeline(0).layers[0], 5).unwrap().1;
    let Some([FilterEntry { filter: Filter::Glow { blur_x, color, .. }, .. }]) = at5[0].kind.filters() else { panic!("{:?}", at5[0].kind) };
    assert_eq!(*blur_x, 10.0);
    assert_eq!(*color, Rgba::new(0.5, 0.0, 0.5, 1.0), "colours travel channel by channel, as a tint does");
    // And the renderers get the tweened filter.
    assert_eq!(&*evaluate(&d, 5).groups[0].filters, &[Filter::Glow { blur_x: 10.0, blur_y: 10.0, strength: 1.0, quality: 1, color: Rgba::new(0.5, 0.0, 0.5, 1.0), inner: false, knockout: false }]);
}

/// Raw stroke runs with the same stroke but different colour effects are not joined, so the
/// run that the tween shrinks away keeps its own colour effect.
#[test]
fn raw_stroke_runs_with_different_colour_effects_keep_them_in_a_shape_tween() {
    let v = anicraft_geom::Vec2::new;
    let run = |id, y: f32, color| {
        let mut p = Path::new();
        p.move_to(v(0.0, y)).line_to(v(50.0, y));
        let mut s = Shape::filled(p, Paint::Solid(Rgba::BLACK)).with_stroke(Rgba::BLACK, 2.0);
        s.fill = None;
        s.merge = true;
        Arc::new(Element { id: ElementId(id), name: String::new(), transform: Affine::IDENTITY, color, kind: ElementKind::Shape(s) })
    };
    let mut d = Document::default();
    let mut k0 = Keyframe::new(0, 10);
    k0.elements = vec![run(1, 0.0, ColorEffect::None), run(2, 100.0, ColorEffect::Alpha(0.5))];
    k0.tween = Some(Tween { easing: Easing::Linear, kind: TweenKind::Shape, ..Default::default() });
    let mut k1 = Keyframe::new(10, 1);
    k1.elements = vec![run(1, 0.0, ColorEffect::None)];
    layer0(&mut d).keyframes = vec![Arc::new(k0), Arc::new(k1)];
    let list = evaluate(&d, 1);
    let strokes: Vec<_> = list.items.iter().filter(|i| i.stroke.is_some()).collect();
    assert_eq!(list.items.len(), 2);
    assert_eq!(strokes.len(), 2);
    // The run that the tween shrinks away (element 2) keeps its alpha; the first keeps none.
    let by = |e: u64| strokes.iter().find(|i| i.element == ElementId(e)).unwrap();
    assert_eq!(by(2).color, ColorEffect::Alpha(0.5).transform(), "the later run lost its alpha");
    assert_eq!(by(1).color, ColorEffect::None.transform(), "the first run took the later run's colour");
}

/// A plain black stroke 2 wide.
fn solid_stroke() -> crate::Stroke {
    Shape::filled(Path::new(), Paint::Solid(Rgba::BLACK)).with_stroke(Rgba::BLACK, 2.0).stroke.unwrap()
}

/// A shape tween from a triangle's raw stroke runs (ids 1-3) to a square's (ids 11-14), all in
/// `stroke`: different run counts, so the runs are joined into outlines that morph.
fn polygon_runs_doc(stroke: &crate::Stroke) -> Document {
    let v = anicraft_geom::Vec2::new;
    let runs = |first: u64, pts: &[anicraft_geom::Vec2]| -> Vec<Arc<Element>> {
        (0..pts.len())
            .map(|k| {
                let mut p = Path::new();
                p.move_to(pts[k]).line_to(pts[(k + 1) % pts.len()]);
                let mut s = Shape::filled(p, Paint::Solid(Rgba::BLACK));
                s.fill = None;
                s.stroke = Some(stroke.clone());
                s.merge = true;
                Arc::new(Element { id: ElementId(first + k as u64), name: String::new(), transform: Affine::IDENTITY, color: ColorEffect::None, kind: ElementKind::Shape(s) })
            })
            .collect()
    };
    let mut d = Document::default();
    let mut k0 = Keyframe::new(0, 10);
    k0.elements = runs(1, &[v(50.0, 0.0), v(100.0, 90.0), v(0.0, 90.0)]);
    k0.tween = Some(Tween { easing: Easing::Linear, kind: TweenKind::Shape, ..Default::default() });
    let mut k1 = Keyframe::new(10, 1);
    k1.elements = runs(11, &[v(0.0, 0.0), v(100.0, 0.0), v(100.0, 100.0), v(0.0, 100.0)]);
    layer0(&mut d).keyframes = vec![Arc::new(k0), Arc::new(k1)];
    d
}

/// Runs joined into one outline in a shape tween all keep their identities in between: the
/// first draws the outline, the others stand over it undrawn, so a selection of any of them
/// survives the tween and has bounds (a selection of a run missing from the frame is dropped).
#[test]
fn joined_stroke_runs_keep_their_identities_in_a_shape_tween() {
    let d = polygon_runs_doc(&solid_stroke());
    let els = crate::evaluate_layer_elements(&d.timeline(0).layers[0], 5).unwrap().1;
    let ids: Vec<u64> = els.iter().map(|e| e.id.0).collect();
    assert_eq!(ids, [1, 2, 3], "every run of the start key is there");
    let stroked: Vec<_> = els.iter().filter(|e| matches!(&e.kind, ElementKind::Shape(s) if s.stroke.is_some())).collect();
    assert_eq!(stroked.len(), 1, "one outline is drawn, once");
    assert_eq!(stroked[0].id, ElementId(1));
    for e in els.iter() {
        assert!(!e.bounds(&d).is_empty(), "{:?} has bounds", e.id);
    }
    let list = evaluate(&d, 5);
    assert_eq!(list.items.iter().filter(|i| i.stroke.is_some() || i.fill.is_some()).count(), 1, "nothing is drawn twice");
    // Horizontal and vertical outlines have zero height or width before stroke padding: undrawn
    // members still keep non-empty bounds.
    let v = anicraft_geom::Vec2::new;
    let seg = |id: u64, a, b| {
        let mut p = Path::new();
        p.move_to(a).line_to(b);
        let mut s = Shape::filled(p, Paint::Solid(Rgba::BLACK));
        s.fill = None;
        s.stroke = Some(solid_stroke());
        s.merge = true;
        Arc::new(Element { id: ElementId(id), name: String::new(), transform: Affine::IDENTITY, color: ColorEffect::None, kind: ElementKind::Shape(s) })
    };
    for (a0, a1, a2, b0, b1, b2, b3) in [
        (v(0.0, 20.0), v(40.0, 20.0), v(80.0, 20.0), v(0.0, 20.0), v(30.0, 20.0), v(60.0, 20.0), v(90.0, 20.0)),
        (v(20.0, 0.0), v(20.0, 40.0), v(20.0, 80.0), v(20.0, 0.0), v(20.0, 30.0), v(20.0, 60.0), v(20.0, 90.0)),
    ] {
        let mut d = Document::default();
        let mut k0 = Keyframe::new(0, 10);
        k0.elements = vec![seg(1, a0, a1), seg(2, a1, a2)];
        k0.tween = Some(Tween { easing: Easing::Linear, kind: TweenKind::Shape, ..Default::default() });
        let mut k1 = Keyframe::new(10, 1);
        k1.elements = vec![seg(11, b0, b1), seg(12, b1, b2), seg(13, b2, b3)];
        layer0(&mut d).keyframes = vec![Arc::new(k0), Arc::new(k1)];
        let els = crate::evaluate_layer_elements(&d.timeline(0).layers[0], 5).unwrap().1;
        assert_eq!(els.len(), 2);
        for e in els.iter() {
            assert!(!e.bounds(&d).is_empty(), "axis-aligned run {:?} has bounds", e.id);
        }
    }
}

/// A dash pattern restarts on each run and a width profile is measured along each, so dashed or
/// profiled runs are not chained into one outline: right after the key they look as they did.
#[test]
fn dashed_or_profiled_stroke_runs_stay_apart_in_a_shape_tween() {
    let subpaths = |d: &Document| {
        let list = evaluate(d, 1);
        list.items.iter().filter(|i| i.stroke.is_some()).map(|i| i.path.cmds.iter().filter(|c| matches!(c, anicraft_geom::PathCmd::MoveTo(_))).count()).sum::<usize>()
    };
    let solid = solid_stroke();
    assert_eq!(subpaths(&polygon_runs_doc(&solid)), 1, "solid runs join into one outline");
    let mut dashed = solid.clone();
    dashed.style.dash = crate::Dash::dashed(6.0, 3.0);
    assert!(dashed.style.dash.is_some());
    // Three runs on the start key: three pieces still, the square's fourth side growing in.
    assert!(subpaths(&polygon_runs_doc(&dashed)) >= 3, "a dashed outline keeps its runs");
    let mut tapered = solid;
    tapered.profile = crate::ProfilePreset::ALL.into_iter().find_map(crate::ProfilePreset::profile);
    assert!(tapered.profile.is_some());
    assert!(subpaths(&polygon_runs_doc(&tapered)) >= 3, "a profiled outline keeps its runs");
}

/// A raw regular polygon centred on `at` as the merge stores it: its fill element (id `first`),
/// then one stroke run per side (ids `first + 1`…), each with its own origin.
fn polygon_at(first: u64, at: anicraft_geom::Vec2, radius: f32, sides: usize, stroke: &crate::Stroke) -> Vec<Arc<Element>> {
    use std::f32::consts::{PI, TAU};
    let pts: Vec<anicraft_geom::Vec2> = (0..sides)
        .map(|k| {
            let a = -PI / 2.0 + k as f32 * TAU / sides as f32;
            at + anicraft_geom::Vec2::new(radius * a.cos(), radius * a.sin())
        })
        .collect();
    let mut outline = Path::new();
    if let Some(&p) = pts.first() {
        outline.move_to(p);
    }
    for &p in pts.iter().skip(1) {
        outline.line_to(p);
    }
    outline.close();
    let piece = |id: u64, path: &Path, centre: anicraft_geom::Vec2, fill: bool| {
        let mut s = Shape::filled(path.transformed(&Affine::translate(-centre.x, -centre.y)), Paint::Solid(Rgba::BLACK));
        s.fill = fill.then_some(Paint::Solid(Rgba::BLACK));
        s.stroke = (!fill).then(|| stroke.clone());
        s.merge = true;
        Arc::new(Element {
            id: ElementId(id),
            name: String::new(),
            transform: Affine::translate(centre.x, centre.y),
            color: ColorEffect::None,
            kind: ElementKind::Shape(s),
        })
    };
    let mut out = vec![piece(first, &outline, at, true)];
    for (k, &a) in pts.iter().enumerate() {
        let b = pts[(k + 1) % sides];
        let mut side = Path::new();
        side.move_to(a).line_to(b);
        out.push(piece(first + 1 + k as u64, &side, (a + b) * 0.5, false));
    }
    out
}

/// A solid outline on one key against a dashed one on the other: the keys' outlines correspond
/// as the fill's do, so the stroke stays on the morphed fill instead of pairing a whole outline
/// with the other key's loose runs.
#[test]
fn a_solid_outline_morphs_against_a_dashed_one() {
    let v = anicraft_geom::Vec2::new;
    let solid = solid_stroke();
    let mut dashed = solid.clone();
    dashed.style.dash = crate::Dash::dashed(6.0, 3.0);
    let mut d = Document::default();
    let mut k0 = Keyframe::new(0, 10);
    k0.elements = polygon_at(1, v(50.0, 45.0), 50.0, 3, &solid);
    k0.tween = Some(Tween { easing: Easing::Linear, kind: TweenKind::Shape, ..Default::default() });
    let mut k1 = Keyframe::new(10, 1);
    k1.elements = polygon_at(11, v(50.0, 50.0), 55.0, 4, &dashed);
    layer0(&mut d).keyframes = vec![Arc::new(k0), Arc::new(k1)];
    for frame in [3u32, 5, 7] {
        let list = evaluate(&d, frame);
        let fill = list.items.iter().find(|i| i.fill.is_some()).expect("the fill");
        let strokes: Vec<_> = list.items.iter().filter(|i| i.stroke.is_some()).collect();
        assert!(!strokes.is_empty(), "frame {frame}: the stroke is drawn");
        for pl in fill.path.flatten(&fill.transform, 0.25) {
            for &p in &pl.points {
                let d = strokes.iter().map(|st| st.path.distance_to_edge(p, &st.transform)).fold(f32::INFINITY, f32::min);
                assert!(d < 1.5, "frame {frame}: fill edge point {p:?} is {d} from the stroke");
            }
        }
        for st in &strokes {
            for pl in st.path.flatten(&st.transform, 0.25) {
                for &p in &pl.points {
                    let d = fill.path.distance_to_edge(p, &fill.transform);
                    assert!(d < 1.5, "frame {frame}: stroke point {p:?} is {d} off the fill's edge");
                }
            }
        }
    }
}

/// A guide places each element by its own origin, so each joined stroke outline takes the origin
/// of the fill it edges: with two merged fill regions, each stroke travels with its own fill
/// rather than following the first region's guide path.
#[test]
fn a_guided_shape_tween_keeps_each_stroke_with_its_own_fill() {
    let v = anicraft_geom::Vec2::new;
    let black = solid_stroke();
    let mut blue = black.clone();
    blue.color = Rgba::hex(0x0000ff);
    // Both different stroke styles and the same stroke style across the two regions: runs that
    // edge different fills stay in separate outlines and each travels its own fill's stretch.
    for second_stroke in [&blue, &black] {
        let mut d = Document::default();
        let mut k0 = Keyframe::new(0, 10);
        k0.elements = polygon_at(1, v(0.0, 0.0), 40.0, 3, &black);
        k0.elements.extend(polygon_at(10, v(300.0, 0.0), 40.0, 4, second_stroke));
        k0.tween = Some(Tween { easing: Easing::Linear, kind: TweenKind::Shape, ..Default::default() });
        let mut k1 = Keyframe::new(10, 1);
        k1.elements = polygon_at(20, v(0.0, 100.0), 40.0, 5, &black);
        k1.elements.extend(polygon_at(30, v(300.0, 100.0), 40.0, 5, second_stroke));
        layer0(&mut d).keyframes = vec![Arc::new(k0), Arc::new(k1)];
        // A straight guide from the first region's start to the second's end: each element travels
        // the stretch of it between its own start and end origins.
        let pts: Vec<anicraft_geom::Vec2> = (0..=20).map(|k| v(300.0 * k as f32 / 20.0, 100.0 * k as f32 / 20.0)).collect();
        let walk = anicraft_geom::walk::PathWalk::new(&pts).expect("the guide");
        let layer = d.timeline(0).layers[0].clone();
        let of = |els: &[Arc<Element>], id: u64| els.iter().find(|e| e.id == ElementId(id)).cloned();
        for frame in [1u32, 3, 5, 7, 9] {
            let (_, els) = crate::evaluate_guided(&layer, frame, Some(&walk)).expect("an in-between frame");
            // The outline standing for a region's first run takes that region's fill's origin, so it
            // travels the same stretch of the guide as the fill it edges.
            for (run, fill) in [(2u64, 1u64), (11, 10)] {
                let (Some(r), Some(f)) = (of(&els, run), of(&els, fill)) else { panic!("frame {frame}: run {run} and fill {fill}") };
                let (a, b) = (r.transform.translation(), f.transform.translation());
                assert!(a.distance(b) < 0.01, "frame {frame}: run {run} at {a:?} left its fill at {b:?}");
            }
            // The regions travel different stretches: a stroke on the first region's path would be
            // far from the second region's fill.
            let (Some(first), Some(second)) = (of(&els, 1), of(&els, 10)) else { panic!("both fills") };
            assert!(first.transform.translation().distance(second.transform.translation()) > 100.0, "frame {frame}: the regions are apart");
            let (Some(run), Some(fill)) = (of(&els, 11), of(&els, 10)) else { panic!("the second region") };
            let off = run.bounds(&d).center().distance(fill.bounds(&d).center());
            assert!(off < 20.0, "frame {frame}: the second region's stroke is {off} from its fill");
        }
    }
}

/// A classic tween from scale `a` to scale `b` (with a turn and a skew at the end), eased.
fn squash_doc(a: (f32, f32), b: (f32, f32), easing: Easing, keep_area: bool) -> Document {
    let at = |s: (f32, f32), rot: f32, skew: f32, x: f32| {
        let mut e = shape_el(10, 0.0);
        e.transform = anicraft_geom::Decomposed { tx: x, ty: 5.0, sx: s.0, sy: s.1, rotation: rot, skew }.compose();
        e
    };
    let mut d = Document::default();
    let mut k0 = Keyframe::new(0, 12);
    k0.elements.push(Arc::new(at(a, 0.0, 0.0, 0.0)));
    k0.tween = Some(Tween { easing, keep_area, ..Default::default() });
    let mut k1 = Keyframe::new(12, 1);
    k1.elements.push(Arc::new(at(b, 0.6, 0.3, 80.0)));
    layer0(&mut d).keyframes = vec![Arc::new(k0), Arc::new(k1)];
    d
}

/// The area factor of a transform: its determinant.
fn area(m: &Affine) -> f32 {
    m.m[0] * m.m[3] - m.m[1] * m.m[2]
}

#[test]
fn keep_area_holds_the_area_on_every_frame_of_a_classic_tween() {
    // Issue #408: 100 x 100 % to 200 x 50 %. Straight lines bulge to 112.5 % halfway.
    let plain = squash_doc((1.0, 1.0), (2.0, 0.5), Easing::Linear, false);
    assert!((area(&evaluate(&plain, 6).items[0].transform) - 1.125).abs() < 1e-4);
    for easing in [Easing::Linear, Easing::QuadIn, Easing::BackOut, Easing::ElasticOut, Easing::Classic(-60.0)] {
        let d = squash_doc((1.0, 1.0), (2.0, 0.5), easing.clone(), true);
        for f in 0..=12 {
            let a = area(&evaluate(&d, f).items[0].transform);
            assert!((a - 1.0).abs() < 2e-5, "{easing:?} frame {f}: area {a}");
        }
    }
    // Halfway is about 141 x 71 %; the turn, the skew and the move tween as before.
    let d = squash_doc((1.0, 1.0), (2.0, 0.5), Easing::Linear, true);
    let (mid, was) = (evaluate(&d, 6).items[0].transform.decompose(), evaluate(&plain, 6).items[0].transform.decompose());
    assert!((mid.sx - std::f32::consts::SQRT_2).abs() < 1e-4 && (mid.sy - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-4, "{mid:?}");
    assert!((mid.rotation - was.rotation).abs() < 1e-5 && (mid.skew - was.skew).abs() < 1e-5 && (mid.tx - was.tx).abs() < 1e-4);
    // The keys themselves show exactly as they are.
    assert_eq!(evaluate(&d, 0).items[0].transform, evaluate(&plain, 0).items[0].transform);
    assert_eq!(evaluate(&d, 12).items[0].transform, evaluate(&plain, 12).items[0].transform);
}

#[test]
fn keep_area_between_keys_of_different_areas_changes_it_smoothly_and_flips_fall_back() {
    // 1 to 4 times the area: each frame multiplies it by the same factor.
    let d = squash_doc((1.0, 1.0), (4.0, 1.0), Easing::Linear, true);
    let a = |f| area(&evaluate(&d, f).items[0].transform);
    for f in 0..12 {
        assert!((a(f + 1) / a(f) - 4f32.powf(1.0 / 12.0)).abs() < 1e-4, "frame {f}");
    }
    // An axis that flips between the keys can't go geometrically: it tweens as before.
    let (flip, plain) = (squash_doc((1.0, 1.0), (-1.0, 2.0), Easing::Linear, true), squash_doc((1.0, 1.0), (-1.0, 2.0), Easing::Linear, false));
    for f in 0..=12 {
        let (m, p) = (evaluate(&flip, f).items[0].transform, evaluate(&plain, f).items[0].transform);
        assert!(m.is_finite());
        let (dm, dp) = (m.decompose(), p.decompose());
        // Whichever axis the decomposition gives the flip to, the other one went geometrically
        // or the same; nothing blows up and the ends agree.
        if f == 0 || f == 12 {
            assert_eq!(m, p, "frame {f}");
        }
        assert!(dm.sx.is_finite() && dm.sy.is_finite() && dp.sx.is_finite());
    }
}

#[test]
fn keep_area_off_is_bit_exact_with_a_tween_that_never_had_it_and_files_stay_small() {
    // Off (the default) takes the same code path as before: every frame identical, bit by bit.
    for easing in [Easing::Linear, Easing::QuadInOut, Easing::BounceOut] {
        let off = squash_doc((1.0, 1.5), (2.5, 0.25), easing.clone(), false);
        let mut old = off.clone();
        // A file from before Keep area has no such key: it reads as off.
        let json = serde_json::to_string(&old).unwrap();
        assert!(!json.contains("keep_area"), "off isn't written");
        old = serde_json::from_str(&json).unwrap();
        for f in 0..=12 {
            let (x, y) = (evaluate(&off, f).items[0].transform, evaluate(&old, f).items[0].transform);
            assert_eq!(x.m.map(f32::to_bits), y.m.map(f32::to_bits), "{easing:?} frame {f}");
            if (1..12).contains(&f) {
                let k = &off.scenes[0].timeline.layers[0].keyframes;
                let d = (k[0].elements[0].transform.decompose(), k[1].elements[0].transform.decompose());
                let t = easing.apply(f as f32 / 12.0);
                assert_eq!(x.m.map(f32::to_bits), d.0.lerp(&d.1, t).compose().m.map(f32::to_bits), "the straight lerp of before, frame {f}");
            }
        }
    }
    // On round-trips.
    let on = squash_doc((1.0, 1.0), (2.0, 0.5), Easing::Linear, true);
    let back: Document = serde_json::from_str(&serde_json::to_string(&on).unwrap()).unwrap();
    assert!(back.scenes[0].timeline.layers[0].keyframes[0].tween.as_ref().unwrap().keep_area);
}

/// A chain of symbols, each holding two instances of the next and the last one a square: 2^depth
/// squares from `depth` symbols, with no cycle anywhere and no nesting deeper than the cap.
fn doubling_doc(depth: u64) -> Document {
    let mut d = Document::default();
    let instance = |id: u64, symbol: u64, x: f32| Element {
        id: ElementId(id),
        name: String::new(),
        transform: Affine::translate(x, 0.0),
        color: ColorEffect::default(),
        kind: ElementKind::Symbol { symbol: SymbolId(symbol), loop_mode: Default::default(), first_frame: 0, blend: Default::default(), filters: Vec::new(), events: Vec::new(), pivot: None },
    };
    for level in 0..depth {
        let mut layer = Layer::new(LayerId(1000 + level), "l");
        layer.keyframes = vec![Arc::new(Keyframe::new(0, 1))];
        if level + 1 == depth {
            push(&mut layer, 0, shape_el(5000, 0.0));
        } else {
            push(&mut layer, 0, instance(2000 + level, 101 + level, 0.0));
            push(&mut layer, 0, instance(3000 + level, 101 + level, 1.0));
        }
        let timeline = Timeline { layers: vec![Arc::new(layer)] };
        d.library.push(Arc::new(Symbol { id: SymbolId(100 + level), name: format!("s{level}"), kind: SymbolKind::Graphic, linkage: None, scale_grid_kept: None, scale_grid: None, timeline }));
    }
    layer0(&mut d).keyframes = vec![Arc::new(Keyframe::new(0, 1))];
    push(layer0(&mut d), 0, instance(1, 100, 0.0));
    d
}

/// #13: the depth cap alone lets symbols that double at each level expand without end. One
/// evaluation, and one bounds query, visit at most `MAX_EVAL_ELEMENTS` elements and then stop.
#[test]
fn symbols_that_double_at_each_level_stay_within_the_allowance() {
    // Below the allowance nothing is left out: 2^10 squares.
    let small = doubling_doc(11);
    assert_eq!(evaluate(&small, 0).items.len(), 1 << 10);
    assert!(!small.timeline(0).layers[0].keyframes[0].elements[0].bounds(&small).is_empty());
    // 2^30 squares would take minutes and gigabytes; the allowance ends it in well under a second.
    let hostile = doubling_doc(31);
    let started = std::time::Instant::now();
    let items = evaluate(&hostile, 0).items.len();
    assert!(items > 0 && items <= crate::MAX_EVAL_ELEMENTS as usize, "{items}");
    let bounds = hostile.timeline(0).layers[0].keyframes[0].elements[0].bounds(&hostile);
    assert!(!bounds.is_empty(), "what was reached still counts");
    assert!(started.elapsed() < std::time::Duration::from_secs(30), "{:?}", started.elapsed());
}
