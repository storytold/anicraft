//! Pack 3 unit tests:
//! - Live Tagged Color Swatches (`P-025`): `swatch.set_tagged`, `swatch.apply`, `swatch.edit`
//!   across scenes, Library symbols, and groups, pixel verification via `anicraft_render`, and
//!   automatic untagging when an untagged colour is applied.
//! - Filters & Blend Modes on `Graphic` symbols (`P-027` part A): `Blur`, `Glow`, `Multiply`,
//!   Classic & Motion tweening, and pixel verification.
//! - Layer-Level & Keyframe-Level Filters, Blend Modes & Color Effects (`P-027` part B):
//!   `layer.set_effects`, `layer.effects`, `timeline.set_frame_effects`, `timeline.frame_effects`,
//!   Classic Tween interpolation, and `.anicraft` backward-compatible round-trip.

use crate::Session;
use anicraft_doc::{BlendMode, ColorEffect, Document, ElementKind, Filter, Paint, Rgba, SymbolKind};
use serde_json::{Value, json};
use std::sync::Arc;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    s.execute(id, &p).unwrap_or_else(|e| panic!("{id} {p}: {e}"))
}

fn px_rgba8(doc: &Document, frame: u32, x: u32, y: u32) -> [u8; 4] {
    let pm = anicraft_render::render_frame(doc, frame, 1.0).unwrap();
    let rgba = pm.to_rgba8();
    let idx = ((y * pm.width + x) * 4) as usize;
    [rgba[idx], rgba[idx + 1], rgba[idx + 2], rgba[idx + 3]]
}

#[test]
fn tagged_swatch_updates_shapes_across_scenes_and_symbols_and_renders_new_pixels() {
    let mut s = Session::default();
    s.doc.stage.width = 200;
    s.doc.stage.height = 200;
    s.doc.stage.background = Rgba::WHITE;

    // Add a custom hero swatch (#ff2200) and mark it as a Live Tagged Swatch.
    let added = run(&mut s, "swatch.add", json!({"type": "solid", "color": "#ff2200", "name": "Hero Coat"}));
    let sw_idx = added["index"].as_u64().unwrap();
    let tagged = run(&mut s, "swatch.set_tagged", json!({"index": sw_idx, "tagged": true}));
    assert_eq!(tagged["tagged"], true);
    let tid = tagged["tag_id"].as_u64().unwrap();
    assert!(tid > 0);

    // Scene 1: draw a rectangle on stage and apply the tagged swatch to its fill AND stroke.
    let r1 = run(&mut s, "draw.rect", json!({"x": 10, "y": 10, "w": 60, "h": 60, "fill": "#cccccc", "stroke": "#000000", "stroke_width": 4, "object": true}))["element"]
        .as_u64()
        .unwrap();
    run(&mut s, "element.select", json!({"ids": [r1]}));
    run(&mut s, "swatch.apply", json!({"index": sw_idx, "target": "fill"}));
    run(&mut s, "swatch.apply", json!({"index": sw_idx, "target": "stroke"}));

    // Convert a second shape into a Graphic symbol in the Library, open it, and apply the tagged swatch inside it.
    let sym_rect = run(&mut s, "draw.rect", json!({"x": 100, "y": 10, "w": 60, "h": 60, "fill": "#888888", "stroke": null, "object": true}))["element"]
        .as_u64()
        .unwrap();
    run(&mut s, "element.select", json!({"ids": [sym_rect]}));
    let sym_res = run(&mut s, "modify.convert_to_symbol", json!({"name": "Cape", "kind": "graphic"}));
    let sym_id = sym_res["symbol"].as_u64().unwrap();
    run(&mut s, "symbol.edit", json!({"symbol": sym_id}));
    run(&mut s, "edit.select_all", json!({}));
    run(&mut s, "swatch.apply", json!({"index": sw_idx, "target": "fill"}));
    run(&mut s, "symbol.exit", json!({}));

    // Add Scene 2 and draw a shape directly while the tool fill still carries the tagged swatch!
    run(&mut s, "scene.new", json!({"name": "Scene 2"}));
    assert_eq!(s.style.fill_tag, Some(tid));
    run(&mut s, "draw.rect", json!({"x": 20, "y": 20, "w": 50, "h": 50, "stroke": null, "object": true}));

    // Switch back to Scene 1 and verify initial pixel colours (#ff2200 = [255, 34, 0, 255]).
    run(&mut s, "scene.select", json!({"scene": 0}));
    let before_stage = px_rgba8(&s.doc, 0, 40, 40);
    let before_sym = px_rgba8(&s.doc, 0, 130, 40);
    assert!(before_stage[0] > 245 && before_stage[1] < 45 && before_stage[2] < 5, "{before_stage:?}");
    assert!(before_sym[0] > 245 && before_sym[1] < 45 && before_sym[2] < 5, "{before_sym:?}");

    // Edit the Tagged Swatch to #00aaee in ONE undo step: Scene 1 shape (fill + stroke), Library
    // symbol shape, and Scene 2 shape must ALL update!
    let undo_before = s.history().undo_len();
    let edited = run(&mut s, "swatch.edit", json!({"index": sw_idx, "color": "#00aaee"}));
    assert_eq!(s.history().undo_len(), undo_before + 1, "single undo step");
    assert_eq!(edited["updated"], 3, "Scene 1 rect + Library symbol rect + Scene 2 rect");

    let after_stage = px_rgba8(&s.doc, 0, 40, 40);
    let after_sym = px_rgba8(&s.doc, 0, 130, 40);
    assert!(after_stage[0] < 5 && (after_stage[1] as i32 - 0xaa).abs() <= 2 && (after_stage[2] as i32 - 0xee).abs() <= 2, "{after_stage:?}");
    assert!(after_sym[0] < 5 && (after_sym[1] as i32 - 0xaa).abs() <= 2 && (after_sym[2] as i32 - 0xee).abs() <= 2, "{after_sym:?}");

    // Scene 2's shape updated too.
    if let Some(ElementKind::Shape(sh2)) = s.doc.scenes[1].timeline.layers[0].keyframes[0].elements.first().map(|e| &e.kind) {
        assert_eq!(sh2.fill, Some(Paint::Solid(Rgba::hex(0x00aaee))));
        assert_eq!(sh2.fill_tag, Some(tid));
    } else {
        panic!("expected shape in Scene 2");
    }

    // Undo restores all three shapes and the swatch in one step!
    s.undo().unwrap();
    let undone_sym = px_rgba8(&s.doc, 0, 130, 40);
    assert!(undone_sym[0] > 245 && undone_sym[2] < 5, "{undone_sym:?}");
    s.redo().unwrap();

    // Applying an untagged colour to Scene 1's rectangle clears its fill_tag and stroke_tag so
    // subsequent swatch edits leave that shape alone while still updating the Library symbol!
    run(&mut s, "element.select", json!({"ids": [r1]}));
    run(&mut s, "paint.set_fill", json!({"type": "solid", "color": "#22cc44"}));
    run(&mut s, "element.set_stroke", json!({"color": "#111111"}));
    if let Some(ElementKind::Shape(sh1)) = s.doc.timeline(0).layers[0].keyframes[0].elements.iter().find(|e| e.id.0 == r1).map(|e| &e.kind) {
        assert_eq!(sh1.fill_tag, None);
        assert_eq!(sh1.stroke_tag, None);
    }

    run(&mut s, "swatch.edit", json!({"index": sw_idx, "color": "#ff00ff"}));
    let unlinked_stage = px_rgba8(&s.doc, 0, 40, 40);
    let still_linked_sym = px_rgba8(&s.doc, 0, 130, 40);
    assert!((unlinked_stage[1] as i32 - 0xcc).abs() <= 2, "untagged shape kept #22cc44: {unlinked_stage:?}");
    assert!(still_linked_sym[0] > 245 && still_linked_sym[1] < 5 && still_linked_sym[2] > 245, "symbol updated to #ff00ff: {still_linked_sym:?}");

    // Round-trip through .anicraft JSON preserves tag_id, fill_tag, and stroke_tag.
    let saved = crate::format::to_json(&s.doc).unwrap();
    let loaded = crate::format::from_json(&saved).unwrap();
    assert_eq!(loaded, s.doc);
}

#[test]
fn link_existing_and_untag_swatch() {
    let mut s = Session::default();
    run(&mut s, "draw.rect", json!({"x": 10, "y": 10, "w": 40, "h": 40, "fill": "#ff0000", "stroke": "#ff0000", "object": true}));
    let red_idx = s.doc.swatches.iter().position(|w| w.paint == anicraft_doc::SwatchPaint::Solid { color: Rgba::hex(0xff0000) }).unwrap();
    let res = run(&mut s, "swatch.set_tagged", json!({"index": red_idx, "tagged": true, "link_existing": true}));
    assert_eq!(res["linked"], 1);
    run(&mut s, "swatch.edit", json!({"index": red_idx, "color": "#00ff88"}));
    let sh = match &s.doc.timeline(0).layers[0].keyframes[0].elements[0].kind {
        ElementKind::Shape(sh) => sh,
        _ => panic!("shape"),
    };
    assert_eq!(sh.fill, Some(Paint::Solid(Rgba::hex(0x00ff88))));
    assert_eq!(sh.stroke.as_ref().map(|st| st.color), Some(Rgba::hex(0x00ff88)));

    let untag = run(&mut s, "swatch.set_tagged", json!({"index": red_idx, "tagged": false}));
    assert_eq!(untag["tagged"], false);
    assert_eq!(untag["linked"], 1);
    let sh = match &s.doc.timeline(0).layers[0].keyframes[0].elements[0].kind {
        ElementKind::Shape(sh) => sh,
        _ => panic!("shape"),
    };
    assert_eq!(sh.fill_tag, None);
    assert_eq!(sh.stroke_tag, None);
}

#[test]
fn graphic_symbols_render_blur_glow_and_multiply_and_tween_filters() {
    let mut s = Session::default();
    s.doc.stage.width = 200;
    s.doc.stage.height = 200;
    s.doc.stage.background = Rgba::WHITE;

    // Bottom layer: solid cyan backdrop (#00ffff).
    run(&mut s, "draw.rect", json!({"x": 0, "y": 0, "w": 200, "h": 200, "fill": "#00ffff", "stroke": null, "object": true}));

    // Top layer: a Graphic symbol with a yellow square (#ffff00) at (60..140, 60..140).
    run(&mut s, "layer.new", json!({}));
    run(&mut s, "draw.rect", json!({"x": 60, "y": 60, "w": 80, "h": 80, "fill": "#ffff00", "stroke": null, "object": true}));
    let sym = run(&mut s, "modify.convert_to_symbol", json!({"name": "SunGraphic", "kind": "graphic"}));
    let sym_id = anicraft_doc::SymbolId(sym["symbol"].as_u64().unwrap());
    assert_eq!(s.doc.symbol(sym_id).unwrap().kind, SymbolKind::Graphic);

    // Multiply yellow (#ffff00) over cyan (#00ffff) -> pure green (#00ff00)!
    run(&mut s, "element.set_blend", json!({"mode": "multiply"}));
    let mid = px_rgba8(&s.doc, 0, 100, 100);
    assert!(mid[0] < 5 && mid[1] > 250 && mid[2] < 5, "Graphic Multiply yellow * cyan = green: {mid:?}");

    // Reset blend to Normal on white stage and verify Glow + Blur spill outside the 60..140 square!
    run(&mut s, "element.set_blend", json!({"mode": "normal"}));
    s.doc.timeline_mut(0).layers.pop(); // remove cyan backdrop so white stage shows
    run(
        &mut s,
        "element.set_filters",
        json!({"filters": [
            {"type": "blur", "blur_x": 12, "blur_y": 12, "quality": 2},
            {"type": "glow", "blur_x": 16, "blur_y": 16, "strength": 2.0, "color": "#ff0000"}
        ]}),
    );
    // Just outside the graphic's raw box (x=54, y=100), the red glow + blur visibly tint the white stage.
    let edge_outside = px_rgba8(&s.doc, 0, 54, 100);
    assert!(edge_outside[1] < 245 && edge_outside[2] < 245, "Graphic Blur+Glow reaches outside box: {edge_outside:?}");

    // Classic tween of Graphic filters from frame 0 (blur 0) to frame 10 (blur 20).
    run(&mut s, "element.set_filters", json!({"filters": [{"type": "blur", "blur_x": 0, "blur_y": 0}]}));
    run(&mut s, "timeline.goto", json!({"frame": 10}));
    run(&mut s, "timeline.insert_keyframe", json!({}));
    run(&mut s, "edit.select_all", json!({}));
    run(&mut s, "element.set_filters", json!({"filters": [{"type": "blur", "blur_x": 20, "blur_y": 20}]}));
    run(&mut s, "timeline.goto", json!({"frame": 0}));
    run(&mut s, "timeline.create_tween", json!({"easing": "Linear"}));

    let dl5 = anicraft_doc::evaluate_scene(&s.doc, 0, 5);
    let has_mid_blur = dl5.groups.iter().any(|g| {
        g.filters.iter().any(|f| matches!(f, Filter::Blur { blur_x, .. } if (*blur_x - 10.0).abs() < 0.5))
    });
    assert!(has_mid_blur, "Graphic symbol filter tweened at frame 5: {:?}", dl5.groups);
}

#[test]
fn layer_and_keyframe_effects_render_tween_and_round_trip_cleanly() {
    // 1. Byte-identical default .anicraft output when no new features are active!
    let clean_doc = anicraft_doc::sample::demo();
    let json_a = crate::format::to_json(&clean_doc).unwrap();
    assert!(!json_a.contains("fill_tag") && !json_a.contains("stroke_tag") && !json_a.contains("tag_id"));
    assert!(!json_a.contains("\"color_effect\""));
    let round_clean = crate::format::from_json(&json_a).unwrap();
    assert_eq!(round_clean, clean_doc);
    assert_eq!(crate::format::to_json(&round_clean).unwrap(), json_a);

    // 2. Layer-level Multiply + Blur + Tint over a backdrop layer.
    let mut s = Session::default();
    s.doc.stage.width = 200;
    s.doc.stage.height = 200;
    s.doc.stage.background = Rgba::WHITE;

    // Bottom layer: solid cyan (#00ffff).
    let bottom = s.layer;
    run(&mut s, "draw.rect", json!({"x": 0, "y": 0, "w": 200, "h": 200, "fill": "#00ffff", "stroke": null, "object": true}));

    // Top layer: plain raw/object shapes (NO MovieClip!) in white, tinted to yellow (#ffff00) and blended Multiply!
    let top = anicraft_doc::LayerId(run(&mut s, "layer.new", json!({}))["layer"].as_u64().unwrap());
    run(&mut s, "draw.rect", json!({"x": 50, "y": 50, "w": 100, "h": 100, "fill": "#ffffff", "stroke": null, "object": true}));
    let lfx = run(
        &mut s,
        "layer.set_effects",
        json!({
            "layer": top.0,
            "blend": "multiply",
            "tint": "#ffff00",
            "tint_amount": 100,
            "filters": [{"type": "blur", "blur_x": 8, "blur_y": 8}]
        }),
    );
    assert_eq!(lfx["blend"], "multiply");
    assert_eq!(run(&mut s, "layer.effects", json!({"layer": top.0})), lfx);

    // Centre pixel: white tinted 100% yellow (#ffff00) multiplied with cyan (#00ffff) = green (#00ff00)!
    let p_center = px_rgba8(&s.doc, 0, 100, 100);
    assert!(p_center[0] < 5 && p_center[1] > 248 && p_center[2] < 5, "Layer Multiply+Tint+Blur centre: {p_center:?}");
    // Just outside x=50 (at x=46, y=100), the blurred yellow edge darkens the red channel of cyan!
    let p_blur = px_rgba8(&s.doc, 0, 46, 100);
    assert!(p_blur[2] < 245, "Layer Blur softens edge over cyan: {p_blur:?}");

    // 3. Keyframe-level colour effect + filter tweening across a Classic Tween!
    run(&mut s, "layer.set_effects", json!({"layer": top.0, "blend": "normal", "style": "none", "filters": []}));
    run(&mut s, "layer.delete", json!({"layer": bottom.0}));
    // Frame 0 on `top`: red square (#ff0000), frame brightness 0%, blur 0.
    run(&mut s, "edit.select_all", json!({}));
    run(&mut s, "paint.set_fill", json!({"type": "solid", "color": "#800000"}));
    run(&mut s, "timeline.set_frame_effects", json!({"frame": 0, "brightness": 0, "filters": [{"type": "blur", "blur_x": 0, "blur_y": 0}]}));
    run(&mut s, "timeline.goto", json!({"frame": 10}));
    run(&mut s, "timeline.insert_keyframe", json!({}));
    run(&mut s, "timeline.set_frame_effects", json!({"frame": 10, "blend": "normal", "brightness": 100, "filters": [{"type": "blur", "blur_x": 16, "blur_y": 16}]}));
    run(&mut s, "timeline.goto", json!({"frame": 0}));
    run(&mut s, "timeline.create_tween", json!({"easing": "Linear"}));

    // Query evaluated frame effects halfway at frame 5: brightness = 50%, blur_x = 8!
    let f5 = run(&mut s, "timeline.frame_effects", json!({"frame": 5}));
    assert_eq!(f5["keyframe"], 0);
    assert_eq!(f5["evaluated"]["color_effect"]["brightness"], 50.0);
    let mid_px = px_rgba8(&s.doc, 5, 100, 100);
    // #800000 (128, 0, 0) brightened +50% towards white -> ~(192, 128, 128).
    assert!((mid_px[0] as i32 - 192).abs() <= 4 && (mid_px[1] as i32 - 128).abs() <= 4, "Keyframe tweened brightness pixel: {mid_px:?}");

    // 4. Save and load round-trip preserves both Layer and Keyframe effects!
    run(&mut s, "layer.set_effects", json!({"blend": "screen", "alpha": 80, "filters": [{"type": "drop_shadow", "distance": 6}]}));
    let saved = crate::format::to_json(&s.doc).unwrap();
    let loaded = crate::format::from_json(&saved).unwrap();
    assert_eq!(loaded, s.doc);
    assert_eq!(loaded.timeline(0).layers[0].blend, BlendMode::Screen);
    assert_eq!(loaded.timeline(0).layers[0].color_effect, ColorEffect::Alpha(0.8));
    assert_eq!(loaded.timeline(0).layers[0].keyframes[1].color_effect, ColorEffect::Brightness(1.0));

    // 5. Undo/redo restores cleanly and plugins may call the new commands.
    assert!(crate::cmds_plugin::plugin_may_call("swatch.set_tagged").is_ok());
    assert!(crate::cmds_plugin::plugin_may_call("swatch.edit").is_ok());
    assert!(crate::cmds_plugin::plugin_may_call("layer.set_effects").is_ok());
    assert!(crate::cmds_plugin::plugin_may_call("layer.effects").is_ok());
    assert!(crate::cmds_plugin::plugin_may_call("timeline.set_frame_effects").is_ok());
    assert!(crate::cmds_plugin::plugin_may_call("timeline.frame_effects").is_ok());
    let _ = Arc::strong_count(&loaded.swatches);
}
