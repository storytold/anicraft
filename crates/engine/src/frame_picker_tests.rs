//! Tests for Pack 1 (P-026 & P-054): Keyframe Navigation (`timeline.prev_keyframe`,
//! `timeline.next_keyframe`), 12-Viseme Studio Auto Lip-Sync (`sound.analyse {what: "visemes_12"}`,
//! `sound.lip_sync` with 12-viseme mappings, 12-viseme `RigViseme`), and Frame Picker engine
//! commands (`frame_picker.frames`, `frame_picker.pick`).

use super::*;
use anicraft_doc::autorig::RigViseme;
use anicraft_doc::{ElementKind, LayerId, LoopMode, SymbolKind};
use serde_json::json;
use std::f32::consts::TAU;

const RATE: u32 = 24_000;
const FPS: f32 = 24.0;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    match s.execute(id, &p) {
        Ok(v) => v,
        Err(e) => panic!("{id} {p}: {e}"),
    }
}

fn samples(frames: u32) -> usize {
    (f64::from(frames) * f64::from(RATE) / f64::from(FPS)).round() as usize
}

fn silence(frames: u32) -> Vec<f32> {
    vec![0.0; samples(frames)]
}

fn tone(parts: &[(f32, f32)], frames: u32) -> Vec<f32> {
    (0..samples(frames))
        .map(|i| {
            parts
                .iter()
                .map(|(f, a)| a * (TAU * f * i as f32 / RATE as f32).sin())
                .sum()
        })
        .collect()
}

#[test]
fn prev_and_next_keyframe_navigate_active_and_all_layers() {
    let mut s = Session::default();
    let l1 = s.layer.0;
    run(&mut s, "timeline.insert_frame", json!({"layer": l1, "frame": 0, "count": 20}));
    run(&mut s, "timeline.insert_keyframe", json!({"layer": l1, "frame": 5}));
    run(&mut s, "timeline.insert_keyframe", json!({"layer": l1, "frame": 14}));

    let l2 = run(&mut s, "layer.new", json!({"name": "Layer 2"}))["layer"]
        .as_u64()
        .unwrap();
    run(&mut s, "timeline.insert_keyframe", json!({"layer": l2, "frame": 9}));

    // Active layer is l1: keyframes at 0, 5, 14.
    run(&mut s, "layer.select", json!({"layer": l1}));
    s.set_frame(0);
    assert_eq!(run(&mut s, "timeline.prev_keyframe", json!({}))["frame"], 0);
    assert_eq!(run(&mut s, "timeline.next_keyframe", json!({}))["frame"], 5);
    assert_eq!(run(&mut s, "timeline.next_keyframe", json!({}))["frame"], 14);
    // At last keyframe: stays at 14.
    assert_eq!(run(&mut s, "timeline.next_keyframe", json!({}))["frame"], 14);
    assert_eq!(run(&mut s, "timeline.prev_keyframe", json!({}))["frame"], 5);

    // Across all_layers: keyframes at 0, 5, 9, 14.
    assert_eq!(run(&mut s, "timeline.next_keyframe", json!({"all_layers": true}))["frame"], 9);
    assert_eq!(run(&mut s, "timeline.next_keyframe", json!({"all_layers": true}))["frame"], 14);
    assert_eq!(run(&mut s, "timeline.prev_keyframe", json!({"all_layers": true}))["frame"], 9);
    assert_eq!(run(&mut s, "timeline.prev_keyframe", json!({"all_layers": true}))["frame"], 5);
    assert_eq!(run(&mut s, "timeline.prev_keyframe", json!({"all_layers": true}))["frame"], 0);
}

#[test]
fn frame_picker_lists_poses_and_picks_in_one_undo_step() {
    let mut s = Session::default();
    run(&mut s, "file.new_from_template", json!({"template": "lip-sync"}));
    let mouth_layer = s
        .timeline()
        .layers
        .iter()
        .find(|l| l.name == "Mouth")
        .unwrap()
        .id
        .0;
    run(&mut s, "layer.select", json!({"layer": mouth_layer, "select": true}));

    let info = run(&mut s, "frame_picker.frames", json!({}));
    assert_eq!(info["name"], "Mouth");
    assert_eq!(info["frame_count"], 10);
    assert_eq!(info["loop_mode"], "single_frame");
    assert_eq!(info["first_frame"], 0);
    let frames = info["frames"].as_array().unwrap();
    assert_eq!(frames.len(), 10);
    assert_eq!(frames[0]["label"], "Rest");
    assert_eq!(frames[1]["label"], "AI");
    assert_eq!(frames[6]["label"], "FV");
    assert_eq!(frames[0]["non_empty"], true);

    // Move playhead to frame 8 inside the 24-frame span and pick pose 6 ("FV") with auto_keyframe: true.
    let before = s.doc.clone();
    let hist_before = s.history().undo_len();
    s.set_frame(8);
    let res = run(
        &mut s,
        "frame_picker.pick",
        json!({"frame_index": 6, "loop": "single_frame", "auto_keyframe": true}),
    );
    assert_eq!(res["frame_index"], 6);
    assert_eq!(res["keyframe_created"], true);
    assert_eq!(s.history().undo_len(), hist_before + 1, "single undo step");

    let shown_pose = |s: &Session, f: u32| {
        let l = s.timeline().layer(LayerId(mouth_layer)).unwrap();
        let (_, els) = anicraft_doc::evaluate_layer_elements(l, f).unwrap();
        match &els[0].kind {
            ElementKind::Symbol { first_frame, loop_mode, .. } => (*first_frame, *loop_mode),
            other => panic!("{other:?}"),
        }
    };
    assert_eq!(shown_pose(&s, 0), (0, LoopMode::SingleFrame), "frames before 8 keep Rest");
    assert_eq!(shown_pose(&s, 8), (6, LoopMode::SingleFrame), "frame 8 holds FV");
    assert_eq!(shown_pose(&s, 20), (6, LoopMode::SingleFrame), "frames after 8 hold FV");

    // Undo restores everything in one step.
    run(&mut s, "edit.undo", json!({}));
    assert!(s.doc == before);

    // Without auto_keyframe, picking at frame 8 updates the covering span keyframe in place.
    s.set_frame(8);
    let res2 = run(
        &mut s,
        "frame_picker.pick",
        json!({"frame_index": 3, "loop": "play_once", "auto_keyframe": false}),
    );
    assert_eq!(res2["keyframe_created"], false);
    assert_eq!(shown_pose(&s, 0), (3, LoopMode::PlayOnce));
    assert_eq!(shown_pose(&s, 8), (3, LoopMode::PlayOnce));

    // Out-of-bounds frame_index is cleanly refused.
    assert!(s.execute("frame_picker.pick", &json!({"frame_index": 99})).is_err());
    assert!(crate::cmds_plugin::plugin_may_call("frame_picker.frames").is_ok());
    assert!(crate::cmds_plugin::plugin_may_call("frame_picker.pick").is_ok());
}

#[test]
fn lip_sync_12_visemes_analyses_and_keys_twelve_mouth_chart() {
    // Verify RigViseme has all 12 visemes.
    assert_eq!(RigViseme::ALL.len(), 12);
    for rv in RigViseme::ALL {
        assert_eq!(RigViseme::from_key(rv.key()), Some(rv));
    }

    // Build a 12-pose Graphic symbol with Adobe Animate's 12 viseme labels.
    let mut s = Session::default();
    run(&mut s, "doc.set_stage", json!({"fps": FPS}));
    run(&mut s, "draw.ellipse", json!({"x": 200, "y": 200, "w": 80, "h": 40, "object": true}));
    let conv = run(&mut s, "modify.convert_to_symbol", json!({"name": "Mouth12", "kind": "graphic"}));
    let sym_id = anicraft_doc::SymbolId(conv["symbol"].as_u64().unwrap());
    let mouth_layer = s.layer.0;

    let labels_12 = ["neutral", "ah", "d", "ee", "f", "l", "m", "oh", "r", "s", "uh", "woo"];
    run(&mut s, "symbol.edit", json!({"symbol": sym_id.0}));
    for (idx, lbl) in labels_12.iter().enumerate() {
        if idx > 0 {
            run(&mut s, "timeline.insert_keyframe", json!({"frame": idx}));
        }
        run(&mut s, "timeline.set_label", json!({"frame": idx, "label": lbl}));
    }
    run(&mut s, "symbol.exit", json!({}));
    assert_eq!(s.doc.symbol(sym_id).unwrap().kind, SymbolKind::Graphic);
    assert_eq!(s.doc.symbol(sym_id).unwrap().timeline.frame_count(), 12);

    // Import synthetic speech covering neutral -> ah -> m (short gap) -> s -> r -> woo -> neutral.
    let speech = [
        silence(6),
        tone(&[(730.0, 0.6), (1090.0, 0.3)], 6),
        silence(2),
        tone(&[(2950.0, 0.3), (3350.0, 0.3)], 6),
        tone(&[(420.0, 0.6), (1640.0, 0.24)], 6),
        tone(&[(300.0, 0.6), (870.0, 0.2)], 6),
        silence(6),
    ]
    .concat();
    let dir = std::env::temp_dir().join(format!("anicraft-vis12-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let wav = dir.join("speech12.wav");
    std::fs::write(&wav, crate::cmds_sound::test_wav(RATE, &speech)).unwrap();
    let imp = run(&mut s, "file.import", json!({"path": wav.display().to_string()}));
    let snd_layer = imp["layer"].as_u64().unwrap();
    run(&mut s, "sound.set", json!({"layer": snd_layer, "frame": 0, "sync": "stream"}));
    let _ = std::fs::remove_dir_all(&dir);

    let an = run(&mut s, "sound.analyse", json!({"what": "visemes_12"}));
    let v12: Vec<&str> = an["visemes_12"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(v12.len(), 38);
    assert!(v12.contains(&"neutral"));
    assert!(v12.contains(&"ah"));
    assert!(v12.contains(&"m"));
    assert!(v12.contains(&"s"));
    assert!(v12.contains(&"r"));
    assert!(v12.contains(&"woo"));

    let sync = run(&mut s, "sound.lip_sync", json!({"layer": mouth_layer, "mouths_12": true}));
    assert_eq!(sync["mode"], "visemes_12");
    for (idx, lbl) in labels_12.iter().enumerate() {
        assert_eq!(sync["mouths_12"][lbl], idx, "label {lbl} auto-matched to frame {idx}");
    }
    assert!(sync["counts"]["s"].as_u64().unwrap_or(0) >= 1);
    assert!(sync["counts"]["r"].as_u64().unwrap_or(0) >= 1);
    assert!(sync["counts"]["woo"].as_u64().unwrap_or(0) >= 1);
}
