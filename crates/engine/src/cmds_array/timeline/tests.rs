use super::*;
use crate::format;
use anicraft_doc::{ColorTransform, ElementId, Layer, MotionTween, Rgba, evaluate_layer_elements};

fn source() -> Session {
    let mut s = Session::default();
    s.execute("draw.rect", &json!({"x": 32, "y": 40, "w": 24, "h": 14})).unwrap();
    s.execute("modify.convert_to_symbol", &json!({"name": "Source"})).unwrap();
    s
}

fn layer(s: &Session) -> &Layer {
    s.timeline().layer(s.layer).unwrap()
}

fn at(s: &Session, frame: u32) -> Arc<Element> {
    let (_, poses) = evaluate_layer_elements(layer(s), frame).unwrap();
    assert_eq!(poses.len(), 1);
    poses.first().unwrap().clone()
}

fn near(a: f32, b: f32) {
    assert!((a - b).abs() < 0.001, "{a} != {b}");
}

fn compare_pose(a: &Element, b: &Element) {
    for (a, b) in a.transform.m.into_iter().zip(b.transform.m) {
        near(a, b);
    }
    let (a_colour, b_colour) = (a.color.transform(), b.color.transform());
    for (a, b) in a_colour.mul.into_iter().chain(a_colour.add).zip(b_colour.mul.into_iter().chain(b_colour.add)) {
        near(a, b);
    }
    let a_filters = a.kind.filters().unwrap();
    let b_filters = b.kind.filters().unwrap();
    assert_eq!(a_filters.len(), b_filters.len());
    for (a, b) in a_filters.iter().zip(b_filters) {
        assert_eq!(a.enabled, b.enabled);
        assert_eq!(a.filter.label(), b.filter.label());
        for param in a.filter.params() {
            near(a.filter.param(param).unwrap(), b.filter.param(param).unwrap());
        }
    }
}

#[test]
fn classic_array_timeline_matches_spatial_source_values_and_independent_eases() {
    let mut s = source();
    s.execute("symbol.set_kind", &json!({"kind": "movie_clip"})).unwrap();
    s.execute(
        "element.set_filters",
        &json!({"filters": [
            {"type": "blur", "blur_x": 2, "blur_y": 4, "quality": 2},
            {"type": "adjust_color", "hue": 35, "enabled": false},
        ]}),
    )
    .unwrap();
    let id = s.selection.first().unwrap();
    let original = s.doc.clone();
    let steps = s.history_len();
    let stops: Vec<_> = [(1.0, 0.0, 1.0, "QuadIn", 2), (2.0, 48.0, 0.5, "QuadOut", 8), (0.8, -12.0, 0.25, "Linear", 5)]
        .into_iter()
        .enumerate()
        .map(|(i, (scale, rotation, alpha, ease, blur))| {
            json!({
                "offset": i as f32 / 2.0, "scale_x": scale, "scale_y": 1.6 - i as f32 * 0.3,
                "rotation": rotation, "alpha": alpha, "outbound_ease": ease,
                "color_effect": ColorEffect::Advanced(ColorTransform { add: [i as f32 * 0.2, 0.0, 0.0, 0.0], ..ColorTransform::IDENTITY }),
                "filters": [{"type": "blur", "blur_x": blur, "blur_y": 4},
                            {"type": "adjust_color", "hue": 150}],
            })
        })
        .collect();
    let mut p = json!({"start_x": 360, "start_y": 20, "dx": -5, "dy": 2,
        "end_x": 120, "end_y": 116, "duration": 49, "count": 49, "source_values": true,
        "property_stops": stops,
        "easing": {"mode": "per_segment", "colour_effect": false}});
    s.preview_array(&p).unwrap();
    let spatial: Vec<_> = s.selected_elements().into_iter().map(|(_, pose)| pose).collect();
    p["count"] = json!(2); // A Classic span depends only on duration, geometry, and stops.
    s.commit_array_timeline_preview(&p).unwrap();
    assert!(!s.array_preview_active());
    assert_eq!(s.history_len(), steps + 1);
    assert_eq!(s.selection.first(), Some(id));
    assert_eq!(s.doc.library.len(), original.library.len());
    assert_eq!(layer(&s).keyframes.iter().map(|k| (k.start, k.duration)).collect::<Vec<_>>(), [(0, 24), (24, 24), (48, 1)]);
    assert_eq!(
        layer(&s).keyframes.first().unwrap().tween.as_ref().unwrap().groups,
        [
            (EaseGroup::Position, Easing::QuadIn),
            (EaseGroup::Rotation, Easing::QuadIn),
            (EaseGroup::Scale, Easing::QuadIn),
            (EaseGroup::Filters, Easing::QuadIn)
        ]
    );
    for (frame, pose) in (0..49).zip(&spatial) {
        let animated = at(&s, frame);
        assert_eq!(animated.id, id.1);
        compare_pose(&animated, pose);
        assert!(matches!(animated.kind.filters().unwrap().last().unwrap().filter, anicraft_doc::Filter::AdjustColor { hue: 35.0, .. }));
    }
    assert!(evaluate_layer_elements(layer(&s), 49).is_none());

    let animated = s.doc.clone();
    let saved = format::to_json(&s.doc).unwrap();
    let mut reopened = Session::new(format::from_json(&saved).unwrap());
    assert_eq!(layer(&reopened).keyframes, layer(&s).keyframes);
    reopened.execute("timeline.create_tween", &json!({"easing": "linear"})).unwrap();
    assert!(layer(&reopened).keyframes.first().unwrap().tween.is_some());
    s.undo().unwrap();
    assert!(s.doc == original);
    s.redo().unwrap();
    assert!(s.doc == animated);
}

#[test]
fn holds_match_eased_spatial_samples_and_preserve_order_at_a_later_playhead() {
    let mut s = source();
    s.frame = 5;
    let source = s.layer_mut(None).unwrap().keyframe_mut(0).unwrap();
    source.start = 5;
    let id = s.selection.first().unwrap().1;
    let p = json!({"animation": "hold", "duration": 48, "count": 8, "start_x": 100,
        "start_y": 100, "end_x": 30, "end_y": 100, "dx": -10, "dy": 0, "source_values": true,
        "easing": {"preset": "BackOut"},
        "property_stops": [{"offset": 0, "alpha": 1}, {"offset": 0.35, "alpha": 0.2}, {"offset": 1, "alpha": 0.8}]});
    s.preview_array(&p).unwrap();
    let preview: Vec<_> = s.selected_elements().into_iter().map(|(_, pose)| pose).collect();
    s.commit_array_timeline_preview(&p).unwrap();
    assert_eq!(layer(&s).keyframes.iter().map(|k| k.start).collect::<Vec<_>>(), [5, 12, 18, 25, 32, 39, 45, 52]);
    for (key, spatial) in layer(&s).keyframes.iter().zip(&preview) {
        assert!(key.tween.is_none() && key.motion.is_none());
        let pose = at(&s, key.end() - 1);
        assert_eq!(pose.id, id);
        compare_pose(&pose, spatial);
    }
    assert!(evaluate_layer_elements(layer(&s), 4).is_none());
    assert!(evaluate_layer_elements(layer(&s), 53).is_none());
}

#[test]
fn validation_keeps_the_live_preview_and_rejects_lossy_classic_conversions() {
    let mut s = source();
    let p = json!({"duration": 48, "count": 8, "start_x": 20, "end_x": 120, "dx": 100.0 / 7.0,
        "source_values": true, "property_stops": [{"offset": 0}, {"offset": 0.45, "alpha": 0.4}, {"offset": 1}]});
    s.preview_array(&p).unwrap();
    let preview = s.doc.clone();
    let selection = s.selection.clone();
    let steps = s.history_len();
    for (key, value, message) in [
        ("duration", json!(2), "same frame"),
        ("duration", json!(MAX_FRAME + 2), "Duration must be"),
        ("easing", json!({"preset": "QuadIn"}), "Global easing"),
        ("property_stops", json!([{"offset": 0}, {"offset": 1, "rotation": 360}]), "shortest rotation"),
        ("property_stops", json!([{"offset": 0}, {"offset": 0}]), "identical positions"),
        ("mode", json!("grid"), "Linear and Radial Arrays only"),
    ] {
        let mut invalid = p.clone();
        invalid[key] = value;
        let error = s.commit_array_timeline_preview(&invalid).unwrap_err().to_string();
        assert!(error.contains(message), "{error}");
        assert!(s.doc == preview && s.selection == selection && s.history_len() == steps);
        assert!(s.array_preview_active());
    }
    let mut short_hold = p.clone();
    short_hold["animation"] = json!("hold");
    short_hold["duration"] = json!(7);
    assert!(s.commit_array_timeline_preview(&short_hold).unwrap_err().to_string().contains("8 frames"));
    assert!(s.doc == preview);

    s.commit_array_timeline_preview(&p).unwrap();
    assert_eq!(layer(&s).keyframes.iter().map(|k| k.start).collect::<Vec<_>>(), [0, 21, 47]);
}

#[test]
fn classic_spans_can_share_a_layer_with_nonoverlapping_motion() {
    let mut s = source();
    let mut unrelated = (*at(&s, 0)).clone();
    unrelated.id = ElementId(s.doc.alloc_id());
    let mut motion_key = Keyframe::new(24, 10);
    motion_key.motion = Some(MotionTween::new(unrelated.id));
    motion_key.elements = vec![Arc::new(unrelated)];
    let motion_key = Arc::new(motion_key);
    let source_layer = s.layer_mut(None).unwrap();
    source_layer.keyframes.push(Arc::new(Keyframe::new(4, 20)));
    source_layer.keyframes.push(motion_key.clone());
    let original = s.doc.clone();
    let params = json!({"end_x": 120, "duration": 30});
    let error = s.execute("array.create_timeline", &params).unwrap_err().to_string();
    assert!(error.contains("frame 25"), "{error}");
    assert!(s.doc == original);
    s.execute("array.create_timeline", &json!({"end_x": 120, "duration": 14})).unwrap();
    assert!(layer(&s).keyframes.first().unwrap().tween.is_some());
    assert_eq!(layer(&s).keyframes.get(2).map(|key| (key.start, key.end(), key.elements.is_empty())), Some((14, 24, true)));
    assert!(Arc::ptr_eq(layer(&s).keyframes.last().unwrap(), &motion_key));
}

#[test]
fn classic_global_easing_can_drive_a_two_stop_span() {
    let mut s = source();
    let p = json!({"start_x": 20, "end_x": 100, "duration": 5,
        "easing": {"preset": "QuadIn", "transform": false},
        "property_stops": [{"offset": 0, "rotation": -60, "alpha": 1},
                           {"offset": 1, "rotation": 50, "alpha": 0.2}]});
    s.execute("array.create_timeline", &p).unwrap();
    let middle = at(&s, 2);
    near(middle.transform.m[4], 40.0);
    near(middle.transform.decompose().rotation.to_degrees(), -5.0);
    near(middle.color.alpha(), 0.8);
}

#[test]
fn duration_reports_all_groups_and_finds_the_first_valid_duration() {
    let stops: Vec<_> = [0.0, 0.12, 0.13, 0.7, 0.71, 1.0].into_iter().map(|t| ArrayPropertyStop::new(t, 1.0)).collect();
    let times = classic_times(&stops, &ArrayEasing::default(), true).unwrap();
    let error = frame_offsets(&times, &stops, true, 20, 10).unwrap_err();
    let EngineError::ArrayDuration { message, suggested } = error else { panic!("duration collision expected") };
    assert!(message.contains("Frame 22: Stop #2 (12%), Stop #3 (13%)"), "{message}");
    assert!(message.contains("Frame 27: Stop #4 (70%), Stop #5 (71%)"), "{message}");
    assert_eq!(suggested, Some(13));
    assert!(frame_offsets(&times, &stops, true, 20, 13).is_ok());
    assert!(frame_offsets(&times, &stops, true, 20, 14).is_err());
    assert!(matches!(frame_offsets(&times, &stops, true, MAX_FRAME - 9, 10),
        Err(EngineError::ArrayDuration { suggested: None, .. })));
}

#[test]
fn duration_reports_synthetic_endpoints_coincident_stops_and_hold_samples() {
    let stops = [ArrayPropertyStop::new(0.03, 1.0), ArrayPropertyStop::new(0.03, 1.0)];
    let times = classic_times(&stops, &ArrayEasing::default(), false).unwrap();
    let error = frame_offsets(&times, &stops, true, 0, 12).unwrap_err();
    let EngineError::ArrayDuration { message, suggested: None } = error else { panic!("coincident stops expected") };
    assert!(message.contains("Start endpoint (0%), Stop #1 (3%), Stop #2 (3%)"), "{message}");
    assert!(message.contains("Increasing the duration cannot separate them"));
    let hold = frame_offsets(&[0.0, 1.0 / 3.0, 2.0 / 3.0, 1.0], &[], false, 4, 2).unwrap_err();
    assert!(hold.to_string().contains("Frame 5: Sample #1 (0%), Sample #2"), "{hold}");
    assert!(matches!(hold, EngineError::ArrayDuration { suggested: Some(4), .. }));
    assert_eq!(array_timeline_default_duration(23.976, 0), 24);
    assert_eq!(array_timeline_default_duration(60.0, MAX_FRAME - 6), 7);
}

fn circle(sweep: f32) -> Value {
    json!({"mode": "radial", "center_x": 421, "center_y": -289, "radius": 150,
           "start_angle": 19, "sweep_angle": sweep, "duration": 49})
}

#[test]
fn circle_motion_follows_signed_arcs_and_full_loops_at_fractional_frames() {
    for sweep in [360.0, -360.0, 107.0, -173.0] {
        let mut s = source();
        let p = circle(sweep);
        let expected = parse_radial(&p, Vec2::ZERO).unwrap();
        let id = s.selection.first().unwrap().1;
        s.execute("array.create_timeline", &p).unwrap();
        let [key] = layer(&s).keyframes.as_slice() else { panic!("expected one Motion Tween span") };
        assert_eq!(key.duration, 49);
        assert!(key.tween.is_none());
        let motion = key.motion.as_ref().unwrap();
        assert_eq!(motion.target, id);
        assert_eq!(motion.position_range(), Some((0, 48)));
        assert_eq!(motion.tracks.len(), 2);
        assert!(motion.pins.is_empty());
        let path = motion.path.as_ref().unwrap();
        assert!(path.cmds.iter().skip(1).all(|cmd| matches!(cmd, anicraft_geom::PathCmd::CubicTo(..))));
        let walk = motion.on_path(&anicraft_doc::Props::of(key.elements.first().unwrap())).unwrap();
        assert_eq!(walk.point(0.0), expected.point(0.0));
        assert_eq!(walk.point(48.0), expected.point(1.0));
        for i in 0..=512 {
            let t = i as f32 / 512.0;
            let at = walk.point(t * 48.0);
            let error = at.distance(expected.point(f64::from(t)));
            assert!(error < 0.01, "sweep {sweep}, t {t}, error {error}");
            let radial_error = (at.distance(Vec2::new(421.0, -289.0)) - 150.0).abs();
            assert!(radial_error < 0.0003, "sweep {sweep}, radial error {radial_error}");
        }
        assert!(walk.point(24.0).distance(walk.point(0.0)) > 100.0);
        assert!(evaluate_layer_elements(layer(&s), 49).is_none());
    }
}

#[test]
fn circle_motion_preserves_the_matrix_and_custom_pivot_and_supports_signed_rotate() {
    use anicraft_geom::{Affine, Vec2};
    for (sweep, rotate, pivot) in [
        (360.0, false, Some(Vec2::new(18.0, -9.0))),
        (360.0, true, None),
        (-360.0, true, Some(Vec2::ZERO)),
    ] {
        let mut s = source();
        let key = s.layer_mut(None).unwrap().keyframe_mut(0).unwrap();
        let base = Arc::make_mut(key.elements.first_mut().unwrap());
        // Rotation-only paths keep the exact authored basis, including shear and reflection.
        base.transform = Affine { m: [0.8, 0.3, -0.55, -1.7, 33.0, 12.0] };
        assert!(base.set_pivot(pivot));
        let original = base.clone();
        let mut p = circle(sweep);
        p["rotate"] = json!(rotate);
        s.execute("array.create_timeline", &p).unwrap();
        let path = parse_radial(&p, Vec2::ZERO).unwrap();
        let end = at(&s, 48);
        assert_eq!(at(&s, 0).transform.translation(), path.point(0.0));
        assert_eq!(end.transform.translation(), path.point(1.0));
        for f in 0..=48 {
            let pose = at(&s, f);
            assert_eq!(pose.pivot(), pivot);
            let t = f as f32 / 48.0;
            let want = if rotate { Affine::rotate((sweep * t).to_radians()).mul(&original.transform) } else { original.transform };
            if !rotate {
                assert_eq!(&pose.transform.m[..4], &original.transform.m[..4]);
            }
            for (a, b) in pose.transform.m[..4].iter().zip(want.m) {
                near(*a, b);
            }
            assert!(pose.transform.translation().distance(path.point(f64::from(t))) < 0.01);
        }
        let motion = layer(&s).keyframes.first().unwrap().motion.as_ref().unwrap();
        if rotate {
            let keys = &motion.track(anicraft_doc::Property::Rotation).unwrap().keys;
            near(keys.last().unwrap().value - keys.first().unwrap().value, sweep);
        }
        assert!(!motion.orient);
    }
}

#[test]
fn radial_exact_affine_survives_native_span_edits_and_round_trip() {
    for spiral in [false, true] {
        let mut s = source();
        s.execute("modify.transform", &json!({"a":1, "b":0, "c":60, "d":1, "e":0, "f":0})).unwrap();
        let mut p = circle(-360.0);
        if spiral {
            p["closed_loop"] = json!(false);
            p["sweep_angle"] = json!(-720);
            p["spiral_offset"] = json!(32);
            p["drift_x"] = json!(44);
            p["drift_y"] = json!(-25);
        }
        let original = s.doc.clone();
        s.execute("array.create_timeline", &p).unwrap();
        let pose = |s: &Session, frame| {
            let at = at(s, frame).transform;
            let registration = at.translation();
            // Eight local pixels of Y have 480 px of horizontal shear, beyond the
            // ordinary Motion Tween property's 89-degree limit.
            assert!((at.apply(Vec2::new(0.0, 8.0)).x - registration.x - 480.0).abs() < 0.001);
            registration
        };
        let radial = parse_radial(&p, Vec2::ZERO).unwrap();
        assert!(pose(&s, 0).distance(radial.point(0.0)) < 0.001);
        assert!(pose(&s, 24).distance(radial.point(0.5)) < 0.03);
        assert!(pose(&s, 48).distance(radial.point(1.0)) < 0.001);
        let middle = pose(&s, 10);
        s.undo().unwrap();
        assert!(s.doc == original);
        s.redo().unwrap();
        assert_eq!(pose(&s, 10), middle);

        let saved = format::to_json(&s.doc).unwrap();
        assert!(saved.contains("exact_path_affine"));
        // Linear Circle has no timing map and must opt in just like Spiral.
        assert_eq!(saved.contains("path_timing"), spiral);
        let mut reopened = Session::new(format::from_json(&saved).unwrap());
        assert_eq!(pose(&reopened, 10), middle);
        let mut path = layer(&reopened).keyframes[0].motion.as_ref().unwrap().path.clone().unwrap();
        let handle = path.point(1, anicraft_geom::edit::C1).unwrap();
        assert!(path.set_point(1, anicraft_geom::edit::C1, handle + Vec2::new(0.0, 60.0)));
        reopened.execute("motion.set_path", &json!({"frame": 0, "d": path.to_svg()})).unwrap();
        let edited = pose(&reopened, 10);
        assert!(edited.distance(middle) > 0.1);
        reopened.undo().unwrap();
        assert_eq!(pose(&reopened, 10), middle);
        reopened.redo().unwrap();
        assert_eq!(pose(&reopened, 10), edited);

        // The Motion Editor can author and retime a property on either generated path.
        reopened.execute("motion.set_key", &json!({"frame": 10, "property": "alpha", "value": 55})).unwrap();
        reopened.execute("motion.move_keys", &json!({"keys": [{"property": "alpha", "frame": 10}], "by": 2})).unwrap();
        near(at(&reopened, 12).color.transform().mul[3], 0.55);
        assert_eq!(pose(&reopened, 10), edited);

        reopened.execute("timeline.copy_frames", &json!({"frame": 0, "count": 49})).unwrap();
        reopened.execute("timeline.paste_frames", &json!({"frame": 49})).unwrap();
        assert_eq!(pose(&reopened, 59), edited);
        reopened.undo().unwrap();
        reopened.execute("timeline.resize_span", &json!({"frame": 0, "frames": 97})).unwrap();
        assert!(pose(&reopened, 20).distance(edited) < 0.001);
        let cut = pose(&reopened, 24);
        reopened.execute("motion.split", &json!({"frame": 24})).unwrap();
        assert!(pose(&reopened, 24).distance(cut) < 0.03);
        let tail = pose(&reopened, 60);
        let saved = format::to_json(&reopened.doc).unwrap();
        let reopened = Session::new(format::from_json(&saved).unwrap());
        assert_eq!(pose(&reopened, 60), tail);
    }
}

#[test]
fn circle_commit_is_atomic_with_existing_classic_spans_and_a_native_editable_path() {
    let mut s = source();
    s.execute("symbol.set_kind", &json!({"kind": "movie_clip"})).unwrap();
    s.execute("element.set_filters", &json!({"filters": [
        {"type": "blur", "blur_x": 7, "quality": 2},
        {"type": "adjust_color", "hue": 42, "enabled": false}
    ]})).unwrap();
    let key = s.layer_mut(None).unwrap().keyframe_mut(0).unwrap();
    Arc::make_mut(key.elements.first_mut().unwrap()).color = ColorEffect::Advanced(ColorTransform {
        mul: [0.7, 0.8, 0.9, 0.4],
        add: [0.1, 0.0, 0.05, 0.0],
    });
    let source = at(&s, 0);
    let mut art = (*source).clone();
    art.id = ElementId(s.doc.alloc_id());
    let mut classic = Keyframe::new(53, 3);
    classic.tween = Some(Tween::new(Easing::Linear, TweenKind::Classic));
    let art = Arc::new(art);
    classic.elements = vec![art.clone()];
    let mut finish = Keyframe::new(56, 1);
    finish.elements = vec![art];
    let classic = Arc::new(classic);
    let keys = &mut s.layer_mut(None).unwrap().keyframes;
    keys.extend([classic.clone(), Arc::new(finish)]);
    let initial = s.doc.clone();
    let steps = s.history_len();

    let mut params = circle(360.0);
    params["count"] = json!(10);
    s.preview_array(&params).unwrap();
    let preview = s.doc.clone();
    for (field, value, message) in [
        ("duration", json!(54), "frame 54"),
        ("sweep_angle", json!(361), "one revolution"),
        ("sweep_angle", json!(0), "nonzero sweep"),
        ("animation", json!("classic"), "uses Motion Tween"),
        ("easing", json!({"preset": "BackIn"}), "monotonic easing"),
        ("easing", json!({"mode": "per_segment", "preset": "BounceOut"}), "monotonic easing"),
    ] {
        let mut invalid = params.clone();
        invalid[field] = value;
        let error = s.commit_array_timeline_preview(&invalid).unwrap_err().to_string();
        assert!(error.contains(message), "{error}");
        assert!(s.doc == preview && s.history_len() == steps && s.array_preview_active());
    }
    // A nonlinear preset for inactive Distribution does not change position.
    params["easing"] = json!({"preset": "QuadIn", "distribution": false});
    s.commit_array_timeline_preview(&params).unwrap();
    assert_eq!(s.history_len(), steps + 1);
    assert_eq!(s.selection.first().unwrap().1, source.id);
    assert_eq!(s.doc.library.len(), initial.library.len());
    for f in [0, 17, 48] {
        let pose = at(&s, f);
        assert_eq!(pose.id, source.id);
        assert_eq!(pose.kind, source.kind);
        assert_eq!(pose.color, source.color);
    }
    assert!(Arc::ptr_eq(&layer(&s).keyframes[1], &classic));
    assert!(evaluate_layer_elements(layer(&s), 49).is_none());

    let committed = s.doc.clone();
    s.undo().unwrap();
    assert!(s.doc == initial);
    s.redo().unwrap();
    assert!(s.doc == committed);
    let mut reopened = Session::new(format::from_json(&format::to_json(&s.doc).unwrap()).unwrap());
    assert_eq!(layer(&reopened).keyframes, layer(&s).keyframes);
    reopened.execute("motion.set_path", &json!({"d": "M 0 0 C 20 0 20 20 40 20"})).unwrap();
    assert_eq!(at(&reopened, 48).transform.translation(), Vec2::new(40.0, 20.0));
}

#[test]
fn circle_rotating_an_offset_pivot_requires_an_explicit_change() {
    let mut s = source();
    let key = s.layer_mut(None).unwrap().keyframe_mut(0).unwrap();
    assert!(Arc::make_mut(key.elements.first_mut().unwrap()).set_pivot(Some(Vec2::new(5.0, 8.0))));
    let original = s.doc.clone();
    let mut params = circle(-270.0);
    params["rotate"] = json!(true);
    let error = s.execute("array.create_timeline", &params).unwrap_err().to_string();
    assert!(error.contains("offset transformation point"), "{error}");
    assert!(s.doc == original);
}

#[test]
fn circle_motion_tracks_match_the_spatial_sampler_with_independent_eases_and_rotate() {
    use anicraft_doc::Property;
    use anicraft_geom::Affine;

    for sweep in [360.0, -360.0] {
        let mut s = source();
        s.execute("symbol.set_kind", &json!({"kind":"movie_clip"})).unwrap();
        s.execute("element.set_filters", &json!({"filters":[{"type":"blur","blur_x":3,"quality":2}]})).unwrap();
        let mut p = circle(sweep);
        p["count"] = json!(48);
        p["source_values"] = json!(true);
        p["rotate"] = json!(true);
        p["easing"] = json!({"mode":"per_segment","distribution":false,"colour_effect":false});
        p["property_stops"] = json!([
            {"offset":0, "scale_x":1, "scale_y":1, "rotation":0, "alpha":1, "outbound_ease":"QuadIn",
             "color_effect":ColorEffect::Tint {amount:0.2,color:Rgba::hex(0x2299ff)}, "filters":[{"type":"blur","blur_x":3}]},
            {"offset":0.5, "scale_x":2, "scale_y":0.8, "rotation":45, "alpha":0.5, "outbound_ease":"QuadOut",
             "color_effect":ColorEffect::Tint {amount:0.6,color:Rgba::hex(0xcc1133)}, "filters":[{"type":"blur","blur_x":19}]},
            {"offset":1, "scale_x":0.5, "scale_y":1.6, "rotation":90, "alpha":1,
             "color_effect":ColorEffect::Tint {amount:0.3,color:Rgba::hex(0x777733)}, "filters":[{"type":"blur","blur_x":5}]}
        ]);
        s.preview_array(&p).unwrap();
        let reference: Vec<_> = s.selected_elements().into_iter().map(|(_, element)| element).collect();
        s.commit_array_timeline_preview(&p).unwrap();
        for (frame, want) in (0..48).zip(reference) {
            let got = at(&s, frame);
            assert!(got.transform.translation().distance(want.transform.translation()) < 0.01);
            let mut comparable = (*got).clone();
            comparable.transform.m[4] = want.transform.m[4];
            comparable.transform.m[5] = want.transform.m[5];
            compare_pose(&comparable, &want);
        }
        let mid = at(&s, 12);
        near(mid.transform.decompose().sx, 1.25);
        near(mid.color.alpha(), 0.75);
        let finish = at(&s, 48);
        near(finish.color.alpha(), 1.0);
        let expected = Affine::rotate((sweep + 90.0).to_radians()).mul(&Affine::scale(0.5, 1.6));
        for (a, b) in finish.transform.m[..4].iter().zip(expected.m) { near(*a, b); }
        let motion = layer(&s).keyframes.first().unwrap().motion.as_ref().unwrap();
        for property in [Property::ScaleX, Property::ScaleY, Property::Rotation, Property::Alpha, Property::TintRed] {
            let track = motion.track(property).unwrap();
            assert_eq!(track.keys.iter().map(|k| k.frame).collect::<Vec<_>>(), [0, 24, 48]);
        }
        assert_eq!(motion.track(Property::X).unwrap().keys.len(), 2);
        assert_eq!(motion.track(Property::Y).unwrap().keys.len(), 2);
        let saved = format::to_json(&s.doc).unwrap();
        let mut reopened = Session::new(format::from_json(&saved).unwrap());
        compare_pose(&at(&s, 12), &at(&reopened, 12));
        let point = at(&reopened, 24).transform.translation();
        reopened.execute("motion.set_key", &json!({"property":"scale_x","frame":24,"value":150})).unwrap();
        near(at(&reopened, 24).transform.decompose().sx, 1.5);
        assert_eq!(at(&reopened, 24).transform.translation(), point);
    }
}

#[test]
fn circle_property_duration_suggestions_and_incompatible_eases_keep_the_preview() {
    let mut s = source();
    let mut p = circle(360.0);
    p["property_stops"] = json!([{"offset":0},{"offset":0.5,"alpha":0.4},{"offset":1}]);
    let original = s.doc.clone();
    let steps = s.history_len();
    s.preview_array(&p).unwrap();
    let preview = s.doc.clone();
    for (field, value, error) in [
        ("easing", json!({"distribution":false,"preset":"QuadIn"}), "Per Segment"),
        ("easing", json!({"distribution":false,"mode":"per_segment","preset":"BackOut"}), "overshoot"),
    ] {
        let mut invalid = p.clone();
        invalid[field] = value;
        assert!(s.commit_array_timeline_preview(&invalid).unwrap_err().to_string().contains(error));
        assert!(s.doc == preview && s.array_preview_active());
    }
    p["duration"] = json!(2);
    let error = s.commit_array_timeline_preview(&p).unwrap_err();
    let EngineError::ArrayDuration { suggested: Some(frames), .. } = error else { panic!("expected a duration suggestion") };
    assert_eq!(frames, 3);
    assert!(s.doc == preview && s.history_len() == steps && s.array_preview_active());
    p["duration"] = json!(frames);
    s.commit_array_timeline_preview(&p).unwrap();
    near(at(&s, 1).color.alpha(), 0.4);
    let animation = s.doc.clone();
    assert_eq!(s.history_len(), steps + 1);
    s.undo().unwrap();
    assert!(s.doc == original);
    s.redo().unwrap();
    assert!(s.doc == animation);
}

#[test]
fn circle_gradient_channels_animate_with_constant_ramp_structure_and_short_filter_angles() {
    use anicraft_doc::{Filter, FilterProp, Property};

    let mut s = source();
    s.execute("symbol.set_kind", &json!({"kind":"movie_clip"})).unwrap();
    let gradient = |offset: f32, color: Rgba, angle| json!({"type":"gradient_glow","angle":angle,
        "stops":[{"offset":0,"color":Rgba::WHITE.with_alpha(0.0)}, {"offset":offset,"color":color}]});
    let from = gradient(1.0, Rgba::hex(0xff0000), 350);
    let to = gradient(1.0, Rgba::hex(0x0000ff), 10);
    s.execute("element.set_filters", &json!({"filters":[from]})).unwrap();
    let mut p = circle(-90.0);
    p["property_stops"] = json!([{"offset":0}, {"offset":1,"filters":[to]}]);
    let mut invalid = p.clone();
    invalid["property_stops"] = json!([{"offset":0}, {"offset":1,"filters":[gradient(0.75, Rgba::hex(0x0000ff), 10)]}]);
    let error = s.execute("array.create_timeline", &invalid).unwrap_err();
    assert!(error.to_string().contains("untracked settings"), "{error}");
    s.execute("array.create_timeline", &p).unwrap();
    let middle = at(&s, 24);
    let [entry] = middle.kind.filters().unwrap() else { panic!("expected the original filter slot") };
    let Filter::GradientGlow { angle, stops, .. } = entry.filter.sanitized() else { panic!("expected Gradient Glow") };
    near(angle, 0.0);
    let end = stops.last().unwrap();
    near(end.offset, 1.0);
    near(end.color.r, 0.5);
    near(end.color.b, 0.5);
    let motion = layer(&s).keyframes.first().unwrap().motion.as_ref().unwrap();
    let unwrapped = motion.track(Property::Filter(FilterProp { slot: 0, param: FilterParam::Angle })).unwrap();
    near(unwrapped.keys.last().unwrap().value, 370.0);
}

#[test]
fn circle_motion_can_scale_the_axis_perpendicular_to_an_offset_pivot() {
    let mut s = source();
    let pivot = Vec2::new(0.0, 15.0);
    let key = s.layer_mut(None).unwrap().keyframe_mut(0).unwrap();
    assert!(Arc::make_mut(key.elements.first_mut().unwrap()).set_pivot(Some(pivot)));
    let mut p = circle(107.0);
    p["property_stops"] = json!([{"offset":0}, {"offset":1,"scale_x":2,"scale_y":2}]);
    assert!(s.execute("array.create_timeline", &p).unwrap_err().to_string().contains("offset transformation point"));
    p["property_stops"] = json!([{"offset":0}, {"offset":1,"scale_x":2}]);
    s.execute("array.create_timeline", &p).unwrap();
    let pose = at(&s, 24);
    let expected = parse_radial(&p, Vec2::ZERO).unwrap().point(0.5);
    near(pose.transform.decompose().sx, 1.5);
    assert_eq!(pose.pivot(), Some(pivot));
    assert!(pose.transform.translation().distance(expected) < 0.01);
}

#[test]
fn circle_global_distribution_uses_native_arc_length_for_signed_sweeps() {
    for sweep in [360.0, -360.0, 107.0, -173.0] {
        let mut params = circle(sweep);
        params["property_stops"] = json!([{"offset":0}, {"offset":0.5,"alpha":0.4}, {"offset":1}]);
        let radial = parse_radial(&params, Vec2::ZERO).unwrap();
        let mut linear = source();
        linear.execute("array.create_timeline", &params).unwrap();
        let motion = |s: &Session| layer(s).keyframes[0].motion.clone().unwrap();
        for preset in [Easing::Linear, Easing::QuadIn, Easing::QuadOut, Easing::QuadInOut, Easing::CircIn, Easing::SineOut] {
            let mut s = source();
            let mut params = params.clone();
            params["easing"] = json!({"preset":preset.name().unwrap(),"colour_effect":false});
            s.execute("array.create_timeline", &params).unwrap();
            assert_eq!(motion(&s).path, motion(&linear).path);
            near(at(&s, 24).color.alpha(), 0.4);
            for frame in [0, 5, 12, 24, 36, 43, 48] {
                let actual = at(&s, frame).transform.translation();
                let t = frame as f32 / 48.0;
                let expected = radial.point(f64::from(preset.apply(t)));
                assert!(actual.distance(expected) < 0.01, "{sweep}, {preset:?}, {frame}: {actual:?} vs {expected:?}");
                if preset == Easing::Linear || frame == 0 || frame == 48 {
                    assert_eq!(actual, at(&linear, frame).transform.translation());
                }
            }
        }
    }
}

#[test]
fn circle_per_segment_distribution_and_property_eases_match_spatial_preview() {
    use anicraft_doc::Property;
    for (distribution, transform) in [(true, false), (false, true), (true, true)] {
        let mut s = source();
        s.execute("symbol.set_kind", &json!({"kind":"movie_clip"})).unwrap();
        s.execute("element.set_filters", &json!({"filters":[
            {"type":"blur","blur_x":3,"quality":2}, {"type":"adjust_color","hue":35,"enabled":false}
        ]})).unwrap();
        let mut p = circle(-360.0);
        p["count"] = json!(48);
        p["rotate"] = json!(true);
        p["easing"] = json!({"mode":"per_segment","distribution":distribution,
            "transform":transform,"colour_effect":false,"filters":false});
        p["property_stops"] = json!([
            {"offset":0,"outbound_ease":"QuadIn","rotation":0,"alpha":1,"filters":[{"type":"blur","blur_x":3}]},
            {"offset":0.5,"outbound_ease":"QuadOut","rotation":45,"scale_x":2,"alpha":0.5,"filters":[{"type":"blur","blur_x":19}]},
            {"offset":1,"rotation":90,"scale_x":0.5,"alpha":1,"filters":[{"type":"blur","blur_x":5}]}
        ]);
        let original = s.doc.clone();
        let steps = s.history_len();
        s.preview_array(&p).unwrap();
        let reference: Vec<_> = s.selected_elements().into_iter().map(|(_, el)| el).collect();
        s.commit_array_timeline_preview(&p).unwrap();
        assert_eq!(s.history_len(), steps + 1);
        for (frame, want) in (0..48).zip(reference) {
            let got = at(&s, frame);
            assert!(got.transform.translation().distance(want.transform.translation()) < 0.01);
            let mut comparable = (*got).clone();
            comparable.transform.m[4] = want.transform.m[4];
            comparable.transform.m[5] = want.transform.m[5];
            compare_pose(&comparable, &want);
        }
        let keys = |s: &Session, property| {
            layer(s).keyframes[0].motion.as_ref().unwrap().track(property).unwrap().keys.iter().map(|k| k.frame).collect::<Vec<_>>()
        };
        assert_eq!(keys(&s, Property::X), [0, 48]);
        assert_eq!(keys(&s, Property::Y), [0, 48]);
        assert_eq!(keys(&s, Property::ScaleX), [0, 24, 48]);
        assert_eq!(keys(&s, Property::Alpha), [0, 24, 48]);
        assert_eq!(keys(&s, Property::from_key("filter.0.blur_x").unwrap()), [0, 24, 48]);
        let committed = s.doc.clone();
        s.undo().unwrap();
        assert!(s.doc == original);
        s.redo().unwrap();
        assert!(s.doc == committed);
    }
}

#[test]
fn circle_distribution_keeps_unrounded_stop_boundaries_and_survives_native_path_edits() {
    use anicraft_doc::Props;

    let mut s = source();
    let mut p = circle(360.0);
    p["easing"] = json!({"mode":"per_segment","transform":false,"colour_effect":false,"filters":false});
    p["property_stops"] = json!([
        {"offset":0.13,"outbound_ease":"QuadIn","alpha":1},
        {"offset":0.37,"outbound_ease":"CubicOut","alpha":0.5},
        {"offset":0.89,"alpha":1}
    ]);
    let stops = parse_stops(&p, None, None).unwrap().unwrap();
    let easing = ArrayEasing::parse(&p).unwrap();
    let radial = parse_radial(&p, Vec2::ZERO).unwrap();
    s.execute("array.create_timeline", &p).unwrap();
    let mut reopened = Session::new(format::from_json(&format::to_json(&s.doc).unwrap()).unwrap());
    let key = &layer(&reopened).keyframes[0];
    let props = Props::of(&key.elements[0]);
    let motion = key.motion.clone().unwrap();
    let op = motion.on_path(&props).unwrap();
    for t in [0.05, 0.13, 0.21, 0.37, 0.62, 0.89, 0.96] {
        let expected = radial.point(easing.eased_position(&stops, t));
        assert!(op.point(t as f32 * 48.0).distance(expected) < 0.01);
    }

    let mut path = motion.path.clone().unwrap();
    let handle = path.point(1, anicraft_geom::edit::C1).unwrap();
    assert!(path.set_point(1, anicraft_geom::edit::C1, handle + Vec2::new(20.0, -35.0)));
    reopened.execute("motion.set_path", &json!({"d":path.to_svg()})).unwrap();
    let updated = layer(&reopened).keyframes[0].motion.as_ref().unwrap();
    assert_ne!(updated.path, motion.path);
    let edited = updated.on_path(&props).unwrap();
    for frame in [4, 10, 22, 40] {
        near(edited.progress(frame as f32), op.progress(frame as f32));
    }
    assert!(at(&reopened, 10).transform.translation().distance(at(&s, 10).transform.translation()) > 0.1);
}

#[test]
fn spiral_cubics_preserve_spatial_geometry_and_angular_timing() {
    use anicraft_doc::{Property, Props};
    use anicraft_geom::{PathCmd, walk::cubic_point};
    for (sweep, offset, drift) in [
        (360.0, 160.0, [0.0, 0.0]),
        (-360.0, -100.0, [0.0, 0.0]),
        (143.0, 0.0, [-85.0, 35.0]),
        (-247.0, 90.0, [58.0, -120.0]),
        (360.0, -150.0, [0.0, 0.0]),
        (420.0, 75.0, [0.0, 0.0]),
        (-420.0, -55.0, [0.0, 0.0]),
        (720.0, -60.0, [-33.0, 18.0]),
        (-720.0, 0.0, [32.0, -40.0]),
        (1080.0, 28.0, [10.0, -22.0]),
    ] {
        let mut s = source();
        let mut p = circle(sweep);
        p["closed_loop"] = json!(false);
        p["count"] = json!(49);
        p["spiral_offset"] = json!(offset);
        p["drift_x"] = json!(drift[0]);
        p["drift_y"] = json!(drift[1]);
        let radial = parse_radial(&p, Vec2::ZERO).unwrap();
        s.preview_array(&p).unwrap();
        let expected: Vec<_> = s.selected_elements().iter().map(|(_, el)| el.transform.translation()).collect();
        s.commit_array_timeline_preview(&p).unwrap();

        let [key] = layer(&s).keyframes.as_slice() else { panic!("expected one native Motion Tween") };
        let motion = key.motion.as_ref().unwrap();
        let path = motion.path.as_ref().unwrap();
        let walker = motion.on_path(&Props::of(&key.elements[0])).unwrap();
        assert_eq!(motion.track(Property::X).unwrap().keys.len(), 2);
        assert_eq!(motion.track(Property::Y).unwrap().keys.len(), 2);
        assert_eq!(walker.point(0.0), radial.point(0.0));
        assert_eq!(walker.point(48.0), radial.point(1.0));
        assert_eq!(expected.len(), 49);

        let count = path.cmds.len() - 1;
        let mut prev = radial.point(0.0);
        let mut tail: Option<Vec2> = None;
        for (i, cmd) in path.cmds.iter().skip(1).enumerate() {
            let PathCmd::CubicTo(a, b, end) = *cmd else { panic!("expected a cubic") };
            if let Some(tail) = tail {
                assert!((prev - tail).distance(a - prev) < 0.0002);
            }
            for u in [0.125, 0.25, 0.5, 0.75, 0.875] {
                let time = (i as f64 + u) / count as f64;
                let ideal = radial.point(time);
                let cubic = cubic_point(&[prev, a, b, end], u as f32);
                assert!(cubic.distance(ideal) < 0.0102, "cubic fit: {sweep}, {offset}, {time}");
                assert!(walker.point(time as f32 * 48.0).distance(ideal) < 0.021, "angular timing: {sweep}, {offset}, {time}");
            }
            prev = end;
            tail = Some(b);
        }
        for (frame, want) in (0..49).zip(expected) {
            let actual = at(&s, frame).transform.translation();
            assert!(actual.distance(want) < 0.021, "{sweep}, {offset}, {frame}: {actual:?} vs {want:?}");
        }
        if sweep == 360.0 && offset == 160.0 {
            let half_length = walker.walk().at(walker.walk().length() * 0.5);
            assert!(half_length.distance(walker.point(24.0)) > 25.0, "constant arc-length speed would be wrong");
        }
    }
}

#[test]
fn open_radial_with_no_offset_or_drift_reuses_circle_conversion() {
    let mut open = source();
    let mut closed = source();
    let mut p = circle(-360.0);
    p["easing"] = json!({"preset": "QuadInOut"});
    p["rotate"] = json!(true);
    closed.execute("array.create_timeline", &p).unwrap();
    p["closed_loop"] = json!(false);
    open.execute("array.create_timeline", &p).unwrap();
    assert_eq!(layer(&open).keyframes[0].motion, layer(&closed).keyframes[0].motion);
}

#[test]
fn spiral_uses_signed_angular_rotation_and_editable_property_keys() {
    use anicraft_doc::{Property, Props};
    let mut s = source();
    s.execute("symbol.set_kind", &json!({"kind":"movie_clip"})).unwrap();
    s.execute("element.set_filters", &json!({"filters":[{"type":"blur","blur_x":2,"quality":2}]})).unwrap();
    let mut p = circle(-360.0);
    p["closed_loop"] = json!(false);
    p["count"] = json!(49);
    p["spiral_offset"] = json!(120);
    p["drift_x"] = json!(90);
    p["drift_y"] = json!(-68);
    p["rotate"] = json!(true);
    p["easing"] = json!({"mode":"per_segment","distribution":false,"filters":false});
    p["property_stops"] = json!([
        {"offset":0,"outbound_ease":"QuadIn","rotation":0,"alpha":1,"filters":[{"type":"blur","blur_x":2}]},
        {"offset":0.5,"outbound_ease":"QuadOut","scale_x":1.6,"scale_y":0.8,"rotation":45,"alpha":0.5,"filters":[{"type":"blur","blur_x":12}]},
        {"offset":1,"scale_x":0.9,"rotation":90,"alpha":1,"filters":[{"type":"blur","blur_x":6}]}
    ]);
    let original = s.doc.clone();
    let steps = s.history_len();
    s.preview_array(&p).unwrap();
    let expected: Vec<_> = s.selected_elements().into_iter().map(|(_, el)| el).collect();
    s.commit_array_timeline_preview(&p).unwrap();
    for (frame, want) in (0..49).zip(expected) {
        let mut actual = (*at(&s, frame)).clone();
        assert!(actual.transform.translation().distance(want.transform.translation()) < 0.021);
        actual.transform.m[4..].copy_from_slice(&want.transform.m[4..]);
        compare_pose(&actual, &want);
    }
    assert_eq!(s.history_len(), steps + 1);
    let animation = s.doc.clone();
    s.undo().unwrap();
    assert!(s.doc == original);
    s.redo().unwrap();
    assert!(s.doc == animation);
    let mut reopened = Session::new(format::from_json(&format::to_json(&s.doc).unwrap()).unwrap());
    let key = &layer(&reopened).keyframes[0];
    let props = Props::of(&key.elements[0]);
    let motion = key.motion.clone().unwrap();
    let keys = &motion.track(Property::Rotation).unwrap().keys;
    assert_eq!(keys.iter().map(|k| (k.frame, k.value)).collect::<Vec<_>>(), [(0, 0.0), (24, 45.0), (48, 90.0)]);
    compare_pose(&at(&s, 12), &at(&reopened, 12));
    let mut path = motion.path.clone().unwrap();
    let slot = anicraft_geom::edit::C1;
    let handle = path.point(1, slot).unwrap();
    assert!(path.set_point(1, slot, handle + Vec2::new(20.0, -40.0)));
    reopened.execute("motion.set_path", &json!({"d":path.to_svg()})).unwrap();
    let edited = layer(&reopened).keyframes[0].motion.as_ref().unwrap();
    assert_ne!(edited.path, motion.path);
    near(edited.on_path(&props).unwrap().progress(12.0), motion.on_path(&props).unwrap().progress(12.0));
    assert!(at(&reopened, 12).transform.translation().distance(at(&s, 12).transform.translation()) > 0.1);
}

#[test]
fn spiral_unsupported_conversion_and_duration_collisions_keep_the_draft() {
    let mut s = source();
    let mut p = circle(360.0);
    p["closed_loop"] = json!(false);
    p["spiral_offset"] = json!(90.0);
    p["property_stops"] = json!([{"offset":0}, {"offset":0.5,"alpha":0.4}, {"offset":1}]);
    let steps = s.history_len();
    s.preview_array(&p).unwrap();
    let draft = s.doc.clone();
    for (field, value, reason) in [
        ("sweep_angle", json!(-36_000), "approximation range"),
        ("easing", json!({"preset":"BounceOut"}), "monotonic easing"),
        ("easing", json!({"preset":"BackIn"}), "monotonic easing"),
        ("easing", json!({"mode":"per_segment","preset":"ElasticOut"}), "monotonic easing"),
    ] {
        let mut invalid = p.clone();
        invalid[field] = value;
        let error = s.commit_array_timeline_preview(&invalid).unwrap_err();
        assert!(error.to_string().contains(reason), "{error}");
        assert!(s.doc == draft && s.history_len() == steps && s.array_preview_active());
    }
    p["duration"] = json!(2);
    let error = s.commit_array_timeline_preview(&p).unwrap_err();
    assert!(matches!(error, EngineError::ArrayDuration { suggested: Some(3), .. }));
    assert!(s.doc == draft && s.array_preview_active());
    p["duration"] = json!(3);
    s.commit_array_timeline_preview(&p).unwrap();
    near(at(&s, 1).color.alpha(), 0.4);
}

#[test]
fn spiral_global_easing_keeps_intersections_and_repeated_sections_in_order() {
    use anicraft_doc::Props;

    for (sweep, drift, revisit) in [(720.0, 600.0, 12), (-1080.0, 0.0, 16)] {
        let mut p = circle(sweep);
        p["closed_loop"] = json!(false);
        p["start_angle"] = json!(0.0);
        p["drift_x"] = json!(drift);
        let radial = parse_radial(&p, Vec2::ZERO).unwrap();
        let mut linear = source();
        linear.execute("array.create_timeline", &p).unwrap();
        let first = at(&linear, 0).transform.translation();
        assert!(at(&linear, revisit).transform.translation().distance(first) < 0.01);
        assert!(at(&linear, revisit / 2).transform.translation().distance(first) > 50.0);
        let path = layer(&linear).keyframes[0].motion.as_ref().unwrap().path.clone();
        for preset in [Easing::QuadIn, Easing::QuadOut, Easing::QuadInOut, Easing::CubicOut, Easing::QuartIn, Easing::QuintInOut, Easing::SineIn, Easing::CircOut] {
            let mut s = source();
            p["easing"] = json!({"preset":preset.name().unwrap(),"colour_effect":false});
            p["property_stops"] = json!([{"offset":0}, {"offset":0.5,"alpha":0.4}, {"offset":1}]);
            s.execute("array.create_timeline", &p).unwrap();
            let key = &layer(&s).keyframes[0];
            let motion = key.motion.as_ref().unwrap();
            assert_eq!(motion.path, path);
            let walker = motion.on_path(&Props::of(&key.elements[0])).unwrap();
            assert!(walker.progress(revisit as f32) > 0.0);
            for frame in [0, 6, 12, 18, 24, 34, 43, 48] {
                let ideal = radial.point(f64::from(preset.apply(frame as f32 / 48.0)));
                assert!(at(&s, frame).transform.translation().distance(ideal) < 0.025);
            }
            near(at(&s, 24).color.alpha(), 0.4);
        }
    }
}

#[test]
fn spiral_per_segment_distribution_and_rotate_follow_spatial_progress_independently_of_properties() {
    for (sweep, distribution, transform, filters) in [
        (720.0, true, false, true), (-720.0, true, true, false), (-720.0, false, true, true),
    ] {
        let mut s = source();
        s.execute("symbol.set_kind", &json!({"kind":"movie_clip"})).unwrap();
        s.execute("element.set_filters", &json!({"filters":[
            {"type":"blur","blur_x":2,"quality":2}, {"type":"adjust_color","hue":23,"enabled":false}
        ]})).unwrap();
        let mut p = circle(sweep);
        p["closed_loop"] = json!(false);
        p["count"] = json!(49);
        p["spiral_offset"] = json!(58.0);
        p["drift_x"] = json!(22.0);
        p["drift_y"] = json!(-44.0);
        p["rotate"] = json!(true);
        p["easing"] = json!({"mode":"per_segment","distribution":distribution,"transform":transform,"colour_effect":false,"filters":filters});
        p["property_stops"] = json!([
            {"offset":0,"outbound_ease":"QuadIn","rotation":0,"alpha":1,"filters":[{"type":"blur","blur_x":2}]},
            {"offset":0.5,"outbound_ease":"QuadOut","rotation":45,"scale_x":2,"alpha":0.5,"filters":[{"type":"blur","blur_x":14}]},
            {"offset":1,"rotation":90,"scale_x":0.5,"alpha":1,"filters":[{"type":"blur","blur_x":6}]}
        ]);
        let original = s.doc.clone();
        let steps = s.history_len();
        s.preview_array(&p).unwrap();
        let expected: Vec<_> = s.selected_elements().into_iter().map(|(_, el)| el).collect();
        s.commit_array_timeline_preview(&p).unwrap();
        for (frame, want) in (0..49).zip(expected) {
            let mut actual = (*at(&s, frame)).clone();
            assert!(actual.transform.translation().distance(want.transform.translation()) < 0.025);
            actual.transform.m[4..].copy_from_slice(&want.transform.m[4..]);
            compare_pose(&actual, &want);
        }

        let radial = parse_radial(&p, Vec2::ZERO).unwrap();
        assert!(at(&s, 24).transform.translation().distance(radial.point(0.5)) < 0.025);
        near(at(&s, 24).color.alpha(), 0.5);
        assert_eq!(s.history_len(), steps + 1);
        let animation = s.doc.clone();
        s.undo().unwrap();
        assert!(s.doc == original);
        s.redo().unwrap();
        assert!(s.doc == animation);
        let mut reopened = Session::new(format::from_json(&format::to_json(&s.doc).unwrap()).unwrap());
        compare_pose(&at(&s, 12), &at(&reopened, 12));
        let key = &layer(&reopened).keyframes[0];
        let props = anicraft_doc::Props::of(&key.elements[0]);
        let original_motion = key.motion.clone().unwrap();
        let mut path = original_motion.path.clone().unwrap();
        let handle = anicraft_geom::edit::C1;
        let from = path.point(1, handle).unwrap();
        assert!(path.set_point(1, handle, from + Vec2::new(22.0, -38.0)));
        reopened.execute("motion.set_path", &json!({"d":path.to_svg()})).unwrap();
        let edited = layer(&reopened).keyframes[0].motion.as_ref().unwrap();
        assert_ne!(edited.path, original_motion.path);
        for frame in [12.0, 24.0, 39.0] {
            near(edited.on_path(&props).unwrap().progress(frame), original_motion.on_path(&props).unwrap().progress(frame));
        }
    }
}

#[test]
fn inward_multi_turn_spirals_hold_or_follow_drift_after_radius_reaches_zero() {
    use anicraft_geom::Affine;

    for (drift, ease) in [([0.0, 0.0], Easing::Linear), ([0.0, 0.0], Easing::QuadIn), ([40.0, -30.0], Easing::QuadOut)] {
        let mut s = source();
        let mut p = circle(-720.0);
        p["closed_loop"] = json!(false);
        p["rotate"] = json!(true);
        p["spiral_offset"] = json!(-120.0);
        p["drift_x"] = json!(drift[0]);
        p["drift_y"] = json!(drift[1]);
        p["easing"] = json!({"preset":ease.name().unwrap()});
        let radial = parse_radial(&p, Vec2::ZERO).unwrap();
        s.execute("array.create_timeline", &p).unwrap();
        assert_eq!(at(&s, 0).transform.translation(), radial.point(0.0));
        assert_eq!(at(&s, 48).transform.translation(), radial.point(1.0));
        for frame in [8, 16, 24, 29, 30, 31, 34, 39, 42, 47, 48] {
            let t = f64::from(ease.apply(frame as f32 / 48.0));
            let actual = at(&s, frame);
            assert!(actual.transform.translation().distance(radial.point(t)) < 0.025, "{drift:?}, {ease:?}, frame {frame}");
            let expected = Affine::rotate(radial.travel(t).to_radians() as f32);
            for (a, b) in actual.transform.m[..4].iter().zip(expected.m) {
                near(*a, b);
            }
            if drift == [0.0, 0.0] && t >= 0.625 {
                assert_eq!(actual.transform.translation(), radial.point(1.0));
            }
        }
    }
}
