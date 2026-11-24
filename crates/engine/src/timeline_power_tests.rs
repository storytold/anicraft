//! Comprehensive unit tests for Pack 2 (`P-021`, `P-022`, `P-041`):
//! * `symbol.new_empty` (`Ctrl+F8`)
//! * `timeline.reverse_frames`
//! * `timeline.distribute_to_layers` (`Ctrl+Shift+D`)
//! * `timeline.distribute_to_keyframes`
//! * `timeline.convert_to_frame_by_frame`
//! * `timeline.copy_motion` & `timeline.paste_motion`
//! * `motion.preset_list`, `motion.preset_apply`, `motion.preset_save`, `motion.preset_delete`
//! * `sound.set` (`in_sec`, `out_sec`) & `sound.split`

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use crate::{Session, format};
use anicraft_doc::audio::{self, MixOptions, mix, wav};
use anicraft_doc::{
    BUILTIN_PRESET_IDS, ColorEffect, Document, ElementId, FolderId,
    LibraryEntry, LibraryItem, Property, Props, Sound, SoundId, SymbolId, SymbolKind,
    evaluate_layer_elements, evaluate_scene, sample,
};
use serde_json::{Value, json};
use std::sync::Arc;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    s.execute(id, &p).unwrap_or_else(|e| panic!("{id}({p}) failed: {e}"))
}

fn err(s: &mut Session, id: &str, p: Value) -> String {
    s.execute(id, &p).expect_err(&format!("{id}({p}) unexpectedly succeeded")).to_string()
}

#[test]
fn symbol_new_empty_creates_clean_library_symbol_and_opens_or_stays_put() {
    let mut s = Session::default();
    let folder = run(&mut s, "library.new_folder", json!({"name": "Characters"}))["folder"]
        .as_u64()
        .unwrap();

    // Default `edit: true` creates an empty Graphic symbol with 1 layer & 1 blank keyframe and enters it.
    let v = run(&mut s, "symbol.new_empty", json!({"name": "Hero", "folder": folder}));
    let sym_id = SymbolId(v["symbol"].as_u64().unwrap());
    assert_eq!(v["name"], "Hero");
    assert_eq!(v["kind"], "graphic");
    assert_eq!(v["folder"], folder);
    assert_eq!(v["editing"], true);
    assert_eq!(s.editing_symbol(), Some(sym_id));

    let sym = s.doc.symbol(sym_id).unwrap();
    assert_eq!(sym.kind, SymbolKind::Graphic);
    assert_eq!(sym.timeline.layers.len(), 1);
    assert_eq!(sym.timeline.layers[0].keyframes.len(), 1);
    assert!(sym.timeline.layers[0].keyframes[0].elements.is_empty());
    assert_eq!(
        s.doc.parent_of(LibraryEntry::Item(LibraryItem::Symbol(sym_id))),
        Some(FolderId(folder))
    );

    // Exit back to main document and create a MovieClip with `edit: false` and automatic numbering.
    run(&mut s, "symbol.exit", json!({}));
    assert_eq!(s.editing_symbol(), None);

    let mc = run(&mut s, "symbol.new_empty", json!({"kind": "movie_clip", "edit": false}));
    let mc_id = SymbolId(mc["symbol"].as_u64().unwrap());
    assert_eq!(mc["kind"], "movie_clip");
    assert_eq!(mc["editing"], false);
    assert_eq!(s.editing_symbol(), None);
    assert_eq!(s.doc.symbol(mc_id).unwrap().kind, SymbolKind::MovieClip);

    let btn = run(&mut s, "symbol.new_empty", json!({"name": "Hero", "kind": "button", "folder": folder, "edit": false}));
    assert_eq!(btn["name"], "Hero 2", "duplicate name in same folder auto-numbers");
    assert_eq!(btn["kind"], "button");

    // Undo restores previous state cleanly.
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(s.doc.library.len(), 2);

    // Reject invalid params without mutating.
    assert!(err(&mut s, "symbol.new_empty", json!({"name": "bad/slash"})).contains("'/'"));
    assert!(err(&mut s, "symbol.new_empty", json!({"kind": "unknown"})).contains("symbol type"));
}

#[test]
fn reverse_frames_reverses_fbf_classic_tween_and_motion_tween_ranges() {
    // 1. Frame-by-frame spans with boundary splitting:
    // Layer holds: [0..3) at x=10, [3..5) at x=40, [5..8) at x=90, [8..10) at x=200.
    // Reversing sub-range 2..=7 leaves [0..2) at x=10 and [8..10) at x=200 untouched!
    let mut s = Session::default();
    run(&mut s, "draw.rect", json!({"x": 10, "y": 20, "w": 10, "h": 10}));
    run(&mut s, "timeline.insert_frame", json!({"frame": 0, "count": 9})); // total 10 frames (0..10)
    run(&mut s, "timeline.insert_blank_keyframe", json!({"frame": 3}));
    s.set_frame(3);
    run(&mut s, "draw.rect", json!({"x": 40, "y": 20, "w": 10, "h": 10}));
    run(&mut s, "timeline.insert_blank_keyframe", json!({"frame": 5}));
    s.set_frame(5);
    run(&mut s, "draw.rect", json!({"x": 90, "y": 20, "w": 10, "h": 10}));
    run(&mut s, "timeline.insert_blank_keyframe", json!({"frame": 8}));
    s.set_frame(8);
    run(&mut s, "draw.rect", json!({"x": 200, "y": 20, "w": 10, "h": 10}));

    let x_at = |doc: &Document, f: u32| -> f32 {
        let dl = evaluate_scene(doc, 0, f);
        dl.items[0].path.bounds().transformed(&dl.items[0].transform).min.x.round()
    };
    let orig: Vec<f32> = (0..10).map(|f| x_at(&s.doc, f)).collect();
    assert_eq!(orig, vec![10.0, 10.0, 10.0, 40.0, 40.0, 90.0, 90.0, 90.0, 200.0, 200.0]);

    run(&mut s, "timeline.reverse_frames", json!({"from": 2, "to": 7}));
    let rev: Vec<f32> = (0..10).map(|f| x_at(&s.doc, f)).collect();
    assert_eq!(rev[0], 10.0, "before range untouched");
    assert_eq!(rev[1], 10.0, "before range untouched");
    for f in 2..=7 {
        assert_eq!(rev[f as usize], orig[(2 + 7 - f) as usize], "frame {f} mirrored");
    }
    assert_eq!(rev[8], 200.0, "after range untouched");
    assert_eq!(rev[9], 200.0, "after range untouched");

    // 2. Classic tween reversal (with QuadIn -> QuadOut): every frame matches the time-reversed original!
    let mut sc = Session::default();
    let r = run(&mut sc, "draw.rect", json!({"x": 0, "y": 0, "w": 20, "h": 20}));
    let eid = ElementId(r["element"].as_u64().unwrap());
    run(&mut sc, "timeline.insert_frame", json!({"frame": 0, "count": 10})); // 11 frames (0..=10)
    run(&mut sc, "timeline.insert_keyframe", json!({"frame": 10}));
    sc.set_frame(10);
    sc.select_only([eid]);
    run(&mut sc, "modify.transform", json!({"a": 1.0, "b": 0.0, "c": 0.0, "d": 1.0, "e": 200.0, "f": 0.0}));
    run(&mut sc, "timeline.create_tween", json!({"frame": 0, "easing": "QuadIn"}));

    let before_tween: Vec<f32> = (0..=10)
        .map(|f| evaluate_scene(&sc.doc, 0, f).items[0].transform.translation().x)
        .collect();
    run(&mut sc, "timeline.reverse_frames", json!({"from": 0, "to": 10}));
    for f in 0..=10 {
        let now_x = evaluate_scene(&sc.doc, 0, f).items[0].transform.translation().x;
        let want_x = before_tween[(10 - f) as usize];
        assert!((now_x - want_x).abs() < 1e-2, "classic tween frame {f}: {now_x} vs {want_x}");
    }

    // 3. Motion tween reversal:
    let mut sm = Session::default();
    run(&mut sm, "draw.rect", json!({"x": 50, "y": 50, "w": 20, "h": 20}));
    run(&mut sm, "timeline.create_motion_tween", json!({"frame": 0}));
    let dur = sm.timeline().layers[0].keyframes[0].duration;
    run(&mut sm, "motion.set_key", json!({"property": "x", "frame": dur - 1, "value": 350.0}));
    run(&mut sm, "motion.set_ease", json!({"property": "x", "frame": 0, "easing": "CubicIn"}));
    let before_m: Vec<f32> = (0..dur)
        .map(|f| evaluate_scene(&sm.doc, 0, f).items[0].transform.translation().x)
        .collect();
    run(&mut sm, "timeline.reverse_frames", json!({"from": 0, "to": dur - 1}));
    for f in 0..dur {
        let now_x = evaluate_scene(&sm.doc, 0, f).items[0].transform.translation().x;
        let want_x = before_m[(dur - 1 - f) as usize];
        assert!((now_x - want_x).abs() < 1e-2, "motion tween frame {f}: {now_x} vs {want_x}");
    }
}

#[test]
fn distribute_to_layers_preserves_exact_draw_order_and_names_layers() {
    let mut s = Session::default();
    let src_layer = s.layer;
    let a = ElementId(run(&mut s, "draw.rect", json!({"x": 0, "y": 0, "w": 30, "h": 30}))["element"].as_u64().unwrap());
    let b = ElementId(run(&mut s, "draw.rect", json!({"x": 10, "y": 10, "w": 30, "h": 30}))["element"].as_u64().unwrap());
    let c = ElementId(run(&mut s, "draw.rect", json!({"x": 20, "y": 20, "w": 30, "h": 30}))["element"].as_u64().unwrap());

    // Name element `b` as "HeroArm" so its new layer picks up that exact name.
    s.select_only([b]);
    if let Some(kf) = s.doc.timeline_mut(0).layer_mut(src_layer).and_then(|l| l.keyframe_mut(0)) {
        Arc::make_mut(&mut kf.elements[0]).name = "BackSky".into();
        Arc::make_mut(&mut kf.elements[1]).name = "HeroArm".into();
    }
    s.selection.clear();

    let order_before: Vec<ElementId> = evaluate_scene(&s.doc, 0, 0).items.iter().map(|i| i.element).collect();
    assert_eq!(order_before, vec![a, b, c]);

    // With nothing selected and >= 2 elements on active layer, all 3 distribute onto new layers.
    let res = run(&mut s, "timeline.distribute_to_layers", json!({}));
    assert_eq!(res["distributed"], 3);
    assert!(
        s.doc.timeline(0).layer(src_layer).unwrap().keyframes[0].elements.is_empty(),
        "source keyframe emptied"
    );

    // Visual back-to-front DisplayList order must remain [a, b, c]!
    let order_after: Vec<ElementId> = evaluate_scene(&s.doc, 0, 0).items.iter().map(|i| i.element).collect();
    assert_eq!(order_after, order_before, "exact visual draw order preserved");

    // Top-to-bottom layers: Shape (for c), HeroArm (for b), BackSky (for a), Layer 1 (source).
    let names: Vec<&str> = s.doc.timeline(0).layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, vec!["Shape", "HeroArm", "BackSky", "Layer 1"]);
}

#[test]
fn distribute_to_keyframes_places_elements_at_stepped_intervals() {
    let mut s = Session::default();
    let e0 = ElementId(run(&mut s, "draw.rect", json!({"x": 0, "y": 0, "w": 10, "h": 10}))["element"].as_u64().unwrap());
    let e1 = ElementId(run(&mut s, "draw.rect", json!({"x": 30, "y": 0, "w": 10, "h": 10}))["element"].as_u64().unwrap());
    let e2 = ElementId(run(&mut s, "draw.rect", json!({"x": 60, "y": 0, "w": 10, "h": 10}))["element"].as_u64().unwrap());
    s.selection.clear();

    let res = run(&mut s, "timeline.distribute_to_keyframes", json!({"start": 2, "step": 3}));
    assert_eq!(res["keyframes"], 3);
    assert_eq!(res["start"], 2);
    assert_eq!(res["end"], 10);

    // Frame 0..2 was emptied because e0,e1,e2 were moved to [2..5), [5..8), [8..11).
    assert!(evaluate_scene(&s.doc, 0, 0).items.is_empty());
    for (range, expected) in [(2..5, e0), (5..8, e1), (8..11, e2)] {
        for f in range {
            let items = evaluate_scene(&s.doc, 0, f).items;
            assert_eq!(items.len(), 1, "frame {f}");
            assert_eq!(items[0].element, expected, "frame {f}");
        }
    }
}

#[test]
fn convert_to_frame_by_frame_bakes_tweens_on_1s_2s_and_3s_and_clears_tweens() {
    // Bake a MotionTween on 2s (`step: 2`).
    let mut s = Session::default();
    run(&mut s, "draw.rect", json!({"x": 20, "y": 40, "w": 30, "h": 30}));
    run(&mut s, "timeline.create_motion_tween", json!({"frame": 0}));
    // Motion tween defaults to 24 frames (0..=23); bake it on 2s (`step: 2`) across the whole span.
    run(&mut s, "motion.set_key", json!({"property": "x", "frame": 23, "value": 260.0}));
    run(&mut s, "motion.set_key", json!({"property": "rotation", "frame": 23, "value": 90.0}));

    let lid = s.layer;
    let sampled_before: Vec<_> = (0..24)
        .step_by(2)
        .map(|f| (f as u32, evaluate_layer_elements(s.doc.timeline(0).layer(lid).unwrap(), f as u32).unwrap().1[0].transform))
        .collect();

    let res = run(&mut s, "timeline.convert_to_frame_by_frame", json!({"from": 0, "to": 23, "step": 2}));
    assert_eq!(res["keyframes"], 12);

    let layer = s.doc.timeline(0).layer(lid).unwrap();
    assert!(layer.keyframes.iter().all(|k| k.tween.is_none() && k.motion.is_none()), "all tweens cleared");
    for (f, want_xf) in sampled_before {
        let got_xf = layer.keyframe_at(f).unwrap().elements[0].transform;
        for (a, b) in got_xf.m.iter().zip(want_xf.m.iter()) {
            assert!((a - b).abs() < 1e-4, "frame {f}: {got_xf:?} vs {want_xf:?}");
        }
        // Held on `f + 1` because `step == 2` (On 2s)!
        let held_xf = evaluate_layer_elements(layer, f + 1).unwrap().1[0].transform;
        assert_eq!(got_xf, held_xf);
    }

    // Also verify demo scene's Classic Tween ball converts on 1s (`step: 1`) with identical positions!
    let mut sd = Session::new(sample::demo());
    let ball_lid = sd.doc.timeline(0).layers[0].id;
    let expected_ball: Vec<_> = (0..=12)
        .map(|f| evaluate_layer_elements(sd.doc.timeline(0).layer(ball_lid).unwrap(), f).unwrap().1[0].transform)
        .collect();
    run(&mut sd, "timeline.convert_to_frame_by_frame", json!({"layer": ball_lid.0, "from": 0, "to": 12, "step": 1}));
    let baked_ball = sd.doc.timeline(0).layer(ball_lid).unwrap();
    for (f, want) in expected_ball.iter().enumerate() {
        let k = baked_ball.keyframe_at(f as u32).unwrap();
        assert!(k.tween.is_none() && k.motion.is_none());
        for (a, b) in k.elements[0].transform.m.iter().zip(want.m.iter()) {
            assert!((a - b).abs() < 1e-4, "demo ball frame {f}");
        }
    }
}

#[test]
fn copy_paste_motion_and_studio_and_custom_presets_work_and_persist() {
    let mut s = Session::default();

    // 1. Verify all 6 built-in studio presets are listed.
    let list = run(&mut s, "motion.preset_list", json!({}));
    let builtin_names: Vec<&str> = list["builtin"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["name"].as_str().unwrap())
        .collect();
    assert_eq!(builtin_names, BUILTIN_PRESET_IDS);

    // 2. Apply every built-in preset to a MovieClip instance (including `pulse_glow` with Glow filter!).
    let mc_sym = SymbolId(run(&mut s, "symbol.new_empty", json!({"name": "Badge", "kind": "movie_clip", "edit": true}))["symbol"].as_u64().unwrap());
    run(&mut s, "draw.rect", json!({"x": -20, "y": -20, "w": 40, "h": 40}));
    run(&mut s, "symbol.exit", json!({}));
    // Place an instance via draw + convert_to_symbol as MovieClip.
    run(&mut s, "draw.rect", json!({"x": 300, "y": 250, "w": 50, "h": 50}));
    let conv = run(&mut s, "modify.convert_to_symbol", json!({"name": "Card", "kind": "movie_clip"}));
    let card_id = ElementId(conv["instance"].as_u64().unwrap());
    let _ = mc_sym;

    for preset_id in BUILTIN_PRESET_IDS {
        s.select_only([card_id]);
        let res = run(&mut s, "motion.preset_apply", json!({"preset": preset_id}));
        assert_eq!(res["preset"], preset_id);
        assert_eq!(res["builtin"], true);
        let kf = &s.doc.timeline(0).layers[0].keyframes[0];
        assert!(kf.motion.is_some(), "{preset_id} must create a MotionTween");
        if preset_id == "pulse_glow" {
            assert!(
                kf.elements[0].kind.filters().is_some_and(|f| !f.is_empty()),
                "pulse_glow attaches a Glow filter on MovieClip targets"
            );
        }
    }

    // 3. Copy Motion from an element at (100, 200) moving +180px X & 2x scale, and Paste Motion
    //    onto another element at (500, 400) in a second Session via `take_shared`!
    let mut s1 = Session::default();
    run(&mut s1, "draw.rect", json!({"x": 90, "y": 190, "w": 20, "h": 20})); // centre = (100, 200)
    run(&mut s1, "timeline.create_motion_tween", json!({"frame": 0}));
    let span_dur = s1.doc.timeline(0).layers[0].keyframes[0].duration;
    let base_p1 = Props::of(&s1.doc.timeline(0).layers[0].keyframes[0].elements[0]);
    run(&mut s1, "motion.set_key", json!({"property": "x", "frame": span_dur - 1, "value": base_p1.get(Property::X) + 180.0}));
    run(&mut s1, "motion.set_key", json!({"property": "scale_x", "frame": span_dur - 1, "value": 200.0}));
    run(&mut s1, "motion.set_key", json!({"property": "rotation", "frame": span_dur - 1, "value": 45.0}));
    run(&mut s1, "motion.set_key", json!({"property": "alpha", "frame": span_dur - 1, "value": 30.0}));

    let copied = run(&mut s1, "timeline.copy_motion", json!({}));
    assert_eq!(copied["copied"], true);

    // Save as custom preset on s1, round-trip through `.anicraft` JSON, apply & delete.
    let saved = run(&mut s1, "motion.preset_save", json!({"name": "Hero Dash"}));
    assert_eq!(saved["name"], "Hero Dash");
    assert_eq!(saved["count"], 1);

    let serialized = format::to_json(&s1.doc).unwrap();
    let reloaded = format::from_json(&serialized).unwrap();
    assert_eq!(reloaded.motion_presets, s1.doc.motion_presets);

    // Transfer shared clipboards across tabs (`take_shared`) into `s2` and Paste Motion!
    let mut s2 = Session::default();
    s2.take_shared(&mut s1);
    assert!(s2.motion_clipboard.is_some());
    run(&mut s2, "draw.rect", json!({"x": 480, "y": 380, "w": 40, "h": 40})); // centre = (500, 400)
    let base_p2 = Props::of(&s2.doc.timeline(0).layers[0].keyframes[0].elements[0]);
    let pasted = run(&mut s2, "timeline.paste_motion", json!({}));
    assert_eq!(pasted["pasted"], true);
    assert_eq!(pasted["duration"], span_dur);

    let eval_end = &evaluate_layer_elements(&s2.doc.timeline(0).layers[0], span_dur - 1).unwrap().1[0];
    let end_props = Props::of(eval_end);
    assert!((end_props.get(Property::X) - (base_p2.get(Property::X) + 180.0)).abs() < 0.1, "relative delta X applied: {end_props:?}");
    assert!((end_props.get(Property::Y) - base_p2.get(Property::Y)).abs() < 0.1, "Y preserved");
    assert!((end_props.get(Property::ScaleX) - 200.0).abs() < 0.5, "2x scale ratio applied");
    assert!((end_props.get(Property::Rotation) - 45.0).abs() < 0.5, "+45 deg rotation applied");
    assert_eq!(eval_end.color, ColorEffect::Alpha(0.3));

    // Delete custom preset on reloaded session; built-in deletion is refused.
    let mut s_reload = Session::new(reloaded);
    assert!(err(&mut s_reload, "motion.preset_delete", json!({"name": "bounce_drop"})).contains("built-in"));
    let del = run(&mut s_reload, "motion.preset_delete", json!({"name": "hero dash"}));
    assert_eq!(del["deleted"], "Hero Dash");
    assert_eq!(del["count"], 0);
}

#[test]
fn sound_set_trim_and_sound_split_produce_sample_accurate_playback() {
    let mut doc = Document::default();
    doc.stage.fps = 10.0; // 1 frame = 0.1s = 1000 samples at 10 kHz
    let samples: Vec<f32> = (0..10_000).map(|i| (i as f32) / 10_000.0).collect();
    let wav_bytes = wav::encode(10_000, 1, &samples, wav::Bits::Float32);
    let snd = Sound::from_data(SoundId(50), "ramp10k.wav", wav_bytes).unwrap();
    doc.sounds.push(Arc::new(snd));
    doc.next_id = 100;

    let mut s = Session::new(doc);
    run(&mut s, "sound.place", json!({"sound": 50, "frame": 0, "sync": "stream", "extend": true}));
    assert_eq!(s.doc.timeline(0).layers[0].keyframes[0].duration, 10);

    // Trim to [0.2s .. 0.8s] -> [2000 .. 8000] sample frames (6 frames = 0.6s).
    let trimmed = run(&mut s, "sound.set", json!({"frame": 0, "in_sec": 0.2, "out_sec": 0.8}));
    assert_eq!(trimmed["in_frames"], 2000);
    assert_eq!(trimmed["out_frames"], 8000);
    assert!((trimmed["in_sec"].as_f64().unwrap() - 0.2).abs() < 1e-6);
    assert!((trimmed["out_sec"].as_f64().unwrap() - 0.8).abs() < 1e-6);

    // Reject inverted trim range (`out_sec <= in_sec`).
    assert!(err(&mut s, "sound.set", json!({"frame": 0, "in_sec": 0.6, "out_sec": 0.4})).contains("out_sec"));

    // Split the sounding span at timeline frame 3 (0.3s elapsed = 3000 samples into trimmed sound):
    // First span [0..3) plays [2000..5000), second span [3..10) plays [5000..8000)!
    let split = run(&mut s, "sound.split", json!({"frame": 3}));
    assert_eq!(split["first_keyframe"], 0);
    assert_eq!(split["first_duration"], 3);
    assert_eq!(split["first_in_frames"], 2000);
    assert_eq!(split["first_out_frames"], 5000);
    assert_eq!(split["second_keyframe"], 3);
    assert_eq!(split["second_duration"], 7);
    assert_eq!(split["second_in_frames"], 5000);
    assert_eq!(split["second_out_frames"], 8000);

    // Mix full 10 frames (10,000 samples at 10 kHz):
    // Samples 0..6000 must equal original ramp samples 2000..8000 seamlessly across the split at 3000,
    // and samples 6000..10000 must be silent (0.0) because out_frames == 8000!
    let v = anicraft_doc::sound::voices(&s.doc, 10_000);
    let mut mixed = vec![0.0f32; 2 * 10_000];
    mix(&v, 10_000, MixOptions::default(), 0, &mut mixed);
    for i in [0usize, 500, 2999, 3000, 3001, 4500, 5999] {
        let want = (2000 + i) as f32 / 10_000.0;
        assert!((mixed[2 * i] - want).abs() < 1e-5, "sample {i}: got {}, want {want}", mixed[2 * i]);
    }
    for i in [6000usize, 7500, 9999] {
        assert_eq!(mixed[2 * i], 0.0, "silent past out_frames at sample {i}");
    }

    // Clear `out_sec: null` on second keyframe -> plays all the way from 5000 to 10000!
    let cleared = run(&mut s, "sound.set", json!({"frame": 3, "out_sec": null}));
    assert!(cleared["out_frames"].is_null());
    let v2 = anicraft_doc::sound::voices(&s.doc, 10_000);
    let mut mixed2 = vec![0.0f32; 2 * 10_000];
    mix(&v2, 10_000, MixOptions::default(), 0, &mut mixed2);
    assert!((mixed2[2 * 7999] - 0.9999).abs() < 1e-5);
    let _ = audio::MixOptions::default();
}
