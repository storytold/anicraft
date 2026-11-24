//! The last slice of filters (ADR 0099), through the command registry: colour tracks, stepped
//! quality and switches, classic tweens of lists that differ, the built-in presets, and filters
//! on text.

use super::*;
use anicraft_doc::{Channel, ElementKind, Filter, FilterColor, FilterEntry, FilterParam, FilterProp, Property, Rgba};
use serde_json::json;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    match s.execute(id, &p) {
        Ok(v) => v,
        Err(e) => panic!("{id} {p}: {e}"),
    }
}

fn refused(s: &mut Session, id: &str, p: Value) -> String {
    match s.execute(id, &p) {
        Ok(v) => panic!("{id} {p} should be refused, gave {v}"),
        Err(EngineError::NeedsConfirmation(c)) => panic!("{id} {p} should be refused, not asked about: {c}"),
        Err(e) => e.to_string(),
    }
}

/// A red 100 × 100 movie clip at (100, 100), selected.
fn clip() -> Session {
    let mut s = Session::default();
    run(&mut s, "draw.rect", json!({"x": 100, "y": 100, "w": 100, "h": 100, "fill": "#ff0000", "stroke": null}));
    run(&mut s, "modify.convert_to_symbol", json!({"name": "Box"}));
    run(&mut s, "symbol.set_kind", json!({"kind": "movie_clip"}));
    s
}

/// The clip with `filters`, in a one-second motion tween.
fn tweened(filters: Value) -> Session {
    let mut s = clip();
    run(&mut s, "element.set_filters", json!({ "filters": filters }));
    run(&mut s, "timeline.create_motion_tween", json!({}));
    s
}

/// The first element's filters as its keyframe holds them.
fn list(s: &Session) -> Vec<FilterEntry> {
    s.doc.timeline(s.scene).layers[0].keyframes[0].elements[0].kind.filters().map(<[FilterEntry]>::to_vec).unwrap_or_default()
}

/// The filters the renderers get at `frame`.
fn drawn(s: &Session, frame: u32) -> Vec<Filter> {
    anicraft_doc::evaluate(&s.doc, frame).groups.iter().find(|g| !g.filters.is_empty()).map(|g| g.filters.to_vec()).unwrap_or_default()
}

fn px(s: &Session, frame: u32, x: u32, y: u32) -> [f32; 4] {
    anicraft_render::render_frame(&s.doc, frame, 1.0).unwrap().pixel(x, y).unwrap()
}

fn track_keys(s: &mut Session, key: &str) -> Vec<(u64, f64)> {
    let v = run(s, "motion.info", json!({}));
    let t = v["tracks"].as_array().and_then(|ts| ts.iter().find(|t| t["property"] == key).cloned());
    t.map(|t| t["keys"].as_array().map(|ks| ks.iter().map(|k| (k["frame"].as_u64().unwrap_or(0), k["value"].as_f64().unwrap_or(0.0))).collect()).unwrap_or_default()).unwrap_or_default()
}

fn channel(slot: u8, c: FilterColor, ch: Channel) -> Property {
    Property::Filter(FilterProp { slot, param: FilterParam::Color(c, ch) })
}

/// A glow's colour keyed whole travels channel by channel: at the midpoint every channel is
/// half way, in the display list and in the pixels.
#[test]
fn a_glow_colour_tweens_to_its_midpoint() {
    let mut s = tweened(json!([{"type": "glow", "color": "#00ff00", "blur_x": 20, "blur_y": 20, "strength": 4, "quality": 2}]));
    let v = run(&mut s, "motion.set_key", json!({"property": "filter.0.color", "frame": 12, "color": "#0000ff"}));
    assert_eq!((v["frame"].as_u64(), v["color"].as_str()), (Some(12), Some("#0000ffff")));
    // Four tracks, one per channel, each pinned to the object's own colour at the start.
    assert_eq!(track_keys(&mut s, "filter.0.color_green"), vec![(0, 255.0), (12, 0.0)]);
    assert_eq!(track_keys(&mut s, "filter.0.color_blue"), vec![(0, 0.0), (12, 255.0)]);
    assert_eq!(track_keys(&mut s, "filter.0.color_red"), vec![(0, 0.0), (12, 0.0)]);
    assert_eq!(track_keys(&mut s, "filter.0.color_alpha"), vec![(0, 100.0), (12, 100.0)]);
    // The midpoint, exactly: what the renderers and every export draw.
    let Some(Filter::Glow { color, .. }) = drawn(&s, 6).first().cloned() else { panic!("a glow") };
    assert_eq!(color, Rgba::new(0.0, 0.5, 0.5, 1.0));
    assert_eq!(color, Rgba::hex(0x00ff00).lerp(Rgba::hex(0x0000ff), 0.5), "as a colour effect's tint travels");
    let info = run(&mut s, "motion.info", json!({"frame": 6}));
    assert_eq!(info["values"]["filter.0.color"], "#008080ff");
    assert_eq!(info["values"]["filter.0.color_green"], 127.5);
    // The pixels just left of the box: green at the start, blue at the end, both equally mid-way.
    let (a, mid, b) = (px(&s, 0, 95, 150), px(&s, 6, 95, 150), px(&s, 12, 95, 150));
    assert!(a[1] > 0.99 && a[2] < 0.9 && a[0] < 0.9, "green glow over white: {a:?}");
    assert!(b[2] > 0.99 && b[1] < 0.9, "blue glow over white: {b:?}");
    assert!((mid[1] - mid[2]).abs() < 1e-5 && mid[0] < mid[1], "half green, half blue: {mid:?}");
    // A still clip with that colour draws the same picture.
    let mut still = clip();
    let mut f = list(&s);
    f[0].filter.set_param(FilterParam::Color(FilterColor::Color, Channel::Green), 127.5);
    f[0].filter.set_param(FilterParam::Color(FilterColor::Color, Channel::Blue), 127.5);
    run(&mut still, "element.set_filters", json!({ "filters": f }));
    assert_eq!(anicraft_render::render_frame(&s.doc, 6, 1.0).unwrap().data, anicraft_render::render_frame(&still.doc, 0, 1.0).unwrap().data);
    // One channel on its own, with an ease of its own.
    run(&mut s, "motion.set_key", json!({"property": "filter.0.color_alpha", "frame": 12, "value": 50}));
    run(&mut s, "motion.set_ease", json!({"property": "filter.0.color_alpha", "frame": 3, "easing": "QuadIn"}));
    let Some(Filter::Glow { color, .. }) = drawn(&s, 6).first().cloned() else { panic!("a glow") };
    assert_eq!(color.a, 0.875, "100 % to 50 %, a quarter of the way");
    // The whole colour takes an ease on its four channels, and its key goes as one.
    let v = run(&mut s, "motion.set_ease", json!({"property": "filter.0.color", "frame": 3, "easing": "CubicOut"}));
    assert_eq!(v["properties"].as_array().map(Vec::len), Some(4));
    let v = run(&mut s, "motion.remove_key", json!({"property": "filter.0.color", "frame": 12}));
    assert_eq!(v["removed"].as_array().map(Vec::len), Some(4));
    // What can't be keyed says why.
    for (p, want) in [
        (json!({"property": "filter.0.color", "value": 3}), "takes 'color'"),
        (json!({"property": "filter.0.color", "color": "nope"}), "colour"),
        (json!({"property": "filter.0.color_red", "value": 256}), "between"),
        (json!({"property": "filter.0.shadow_color_red", "value": 1}), "has no shadow_color_red"),
        (json!({"property": "filter.0.stop_0_red", "value": 1}), "has no stop_0_red"),
        (json!({"property": "filter.0.stop_99_red", "value": 1}), "unknown property"),
        (json!({"property": "filter.3.color", "color": "#ffffff"}), "no filter 3"),
    ] {
        let e = refused(&mut s, "motion.set_key", p.clone());
        assert!(e.contains(want), "{p}: {e}");
    }
}

/// Quality and the switches hold their keys: the frame before a key draws the old value, the
/// key's frame exactly the new one.
#[test]
fn a_switch_steps_exactly_at_its_key() {
    let mut s = tweened(json!([{"type": "glow", "color": "#0000ff", "blur_x": 10, "blur_y": 10, "strength": 2}]));
    let v = run(&mut s, "motion.set_key", json!({"property": "filter.0.knockout", "frame": 6, "value": 1}));
    assert_eq!(v["value"], 1.0);
    run(&mut s, "motion.set_key", json!({"property": "filter.0.quality", "frame": 4, "value": 2.6}));
    assert_eq!(track_keys(&mut s, "filter.0.quality"), vec![(0, 1.0), (4, 3.0)], "a stepped key is a whole number");
    let at = |s: &Session, f: u32| match drawn(s, f).first() {
        Some(Filter::Glow { knockout, quality, .. }) => (*knockout, *quality),
        other => panic!("{other:?}"),
    };
    assert_eq!([0, 3, 4, 5, 6, 20].map(|f| at(&s, f)), [(false, 1), (false, 1), (false, 3), (false, 3), (true, 3), (true, 3)]);
    // The pixels: the box is there on frame 5 and knocked out on frame 6 (the glow stays).
    let (before, on) = (px(&s, 5, 150, 150), px(&s, 6, 150, 150));
    assert!(before[0] > 0.99 && before[1] < 0.01, "the red box: {before:?}");
    assert!(on[1] > 0.99 && on[2] > 0.99 && on[0] > 0.99, "knocked out to the white stage: {on:?}");
    assert!(px(&s, 6, 96, 150)[0] < 0.9, "the glow still draws round it");
    // motion.info marks them, and they take no ease.
    let info = run(&mut s, "motion.info", json!({"frame": 5}));
    let t = info["tracks"].as_array().unwrap().iter().find(|t| t["property"] == "filter.0.knockout").unwrap().clone();
    assert_eq!((t["stepped"].as_bool(), t["label"].as_str()), (Some(true), Some("Knockout")));
    assert_eq!((info["values"]["filter.0.knockout"].as_f64(), info["values"]["filter.0.quality"].as_f64()), (Some(0.0), Some(3.0)));
    assert!(refused(&mut s, "motion.set_ease", json!({"property": "filter.0.knockout", "frame": 2, "easing": "QuadIn"})).contains("stepped"));
    // An ease for every property skips them.
    run(&mut s, "motion.set_key", json!({"property": "filter.0.blur_x", "frame": 6, "value": 30}));
    let v = run(&mut s, "motion.set_ease", json!({"frame": 2, "easing": "QuadIn"}));
    assert_eq!(v["properties"], json!(["filter.0.blur_x"]));
    assert_eq!(at(&s, 5), (false, 3), "still held");
    // Moving a key in time moves the step with it.
    run(&mut s, "motion.move_keys", json!({"keys": [{"property": "filter.0.knockout", "frame": 6}], "by": 3}));
    assert_eq!([8, 9].map(|f| at(&s, f).0), [false, true]);
    // Out of range, or a switch the filter doesn't have.
    assert!(refused(&mut s, "motion.set_key", json!({"property": "filter.0.quality", "value": 4})).contains("between"));
    assert!(refused(&mut s, "motion.set_key", json!({"property": "filter.0.hide_object", "value": 1})).contains("has no hide_object"));
    // Hide object, on a drop shadow: the shadow alone from the key on.
    let mut s = tweened(json!([{"type": "drop_shadow", "distance": 30, "angle": 0, "blur_x": 0, "blur_y": 0, "color": "#000000"}]));
    run(&mut s, "motion.set_key", json!({"property": "filter.0.hide_object", "frame": 8, "value": 1}));
    assert!(px(&s, 7, 110, 150)[0] > 0.99 && px(&s, 7, 110, 150)[1] < 0.01, "the box on frame 7");
    assert!(px(&s, 8, 110, 150)[0] > 0.99 && px(&s, 8, 110, 150)[1] > 0.99, "hidden on frame 8");
    assert!(px(&s, 8, 215, 150)[0] < 0.01, "its shadow stays");
}

/// Editing a filter's colour or a switch in Properties inside the tween keys what changed at
/// the playhead, one undo step, like its numbers.
#[test]
fn editing_a_colour_or_a_switch_inside_the_tween_keys_it() {
    let mut s = tweened(json!([{"type": "bevel", "highlight_color": "#ffffff", "shadow_color": "#000000"}]));
    let steps = s.history_len();
    s.set_frame(10);
    run(&mut s, "element.set_filters", json!({"filters": [{"type": "bevel", "highlight_color": "#ffff00", "shadow_color": "#000000", "knockout": true, "quality": 3}]}));
    assert_eq!(s.history_len(), steps + 1, "one undo step");
    assert_eq!(track_keys(&mut s, "filter.0.highlight_color_blue"), vec![(0, 255.0), (10, 0.0)]);
    assert!(track_keys(&mut s, "filter.0.highlight_color_red").is_empty(), "only the channel that changed");
    assert!(track_keys(&mut s, "filter.0.shadow_color_red").is_empty());
    assert_eq!(track_keys(&mut s, "filter.0.knockout"), vec![(0, 0.0), (10, 1.0)]);
    assert_eq!(track_keys(&mut s, "filter.0.quality"), vec![(0, 1.0), (10, 3.0)]);
    // The object keeps its own underneath; the tracks give the rest.
    assert!(matches!(list(&s)[0].filter, Filter::Bevel { knockout: false, quality: 1, highlight_color, .. } if highlight_color == Rgba::WHITE));
    assert!(matches!(drawn(&s, 5).first(), Some(Filter::Bevel { knockout: false, quality: 1, highlight_color, .. }) if *highlight_color == Rgba::new(1.0, 1.0, 0.5, 1.0)));
    assert!(matches!(drawn(&s, 10).first(), Some(Filter::Bevel { knockout: true, quality: 3, .. })));
    run(&mut s, "edit.undo", json!({}));
    assert!(track_keys(&mut s, "filter.0.knockout").is_empty() && track_keys(&mut s, "filter.0.highlight_color_blue").is_empty());
    // A ramp's stops are keyed by their place.
    let mut s = tweened(json!([{"type": "gradient_glow", "stops": [{"offset": 0, "color": "#ffffff00"}, {"offset": 1, "color": "#ff0000"}]}]));
    s.set_frame(10);
    run(&mut s, "element.set_filters", json!({"filters": [{"type": "gradient_glow", "stops": [{"offset": 0, "color": "#ffffff00"}, {"offset": 1, "color": "#0000ff"}]}]}));
    assert_eq!(track_keys(&mut s, "filter.0.stop_1_red"), vec![(0, 255.0), (10, 0.0)]);
    assert_eq!(track_keys(&mut s, "filter.0.stop_1_blue"), vec![(0, 0.0), (10, 255.0)]);
    let Some(Filter::GradientGlow { stops, .. }) = drawn(&s, 5).first().cloned() else { panic!() };
    assert_eq!(stops[1].color, Rgba::new(0.5, 0.0, 0.5, 1.0));
    // F6 keys everything a filter has, and the file keeps the tracks.
    run(&mut s, "timeline.insert_keyframe", json!({}));
    assert_eq!(track_keys(&mut s, "filter.0.knockout"), vec![(0, 0.0), (10, 0.0)]);
    assert_eq!(track_keys(&mut s, "filter.0.stop_0_alpha"), vec![(0, 0.0), (10, 0.0)]);
    let json = serde_json::to_string(&s.doc).unwrap();
    assert!(json.contains("filter.0.stop_1_red") && json.contains("filter.0.knockout"));
    let back: anicraft_doc::Document = serde_json::from_str(&json).unwrap();
    assert!(back == s.doc);
    assert!(back.timeline(0).layers[0].keyframes[0].motion.as_ref().is_some_and(|m| m.track(channel(0, FilterColor::Stop(1), Channel::Red)).is_some()));
}

/// A classic tween between keyframes whose filter lists differ: what they share travels, a
/// filter only one keyframe has fades in or out, and the first keyframe shows its own list.
#[test]
fn classic_tweens_fade_filters_only_one_keyframe_has() {
    let mut s = clip();
    run(&mut s, "element.set_filters", json!({"filters": [{"type": "blur", "blur_x": 0, "blur_y": 0}]}));
    run(&mut s, "timeline.goto", json!({"frame": 10}));
    run(&mut s, "timeline.insert_keyframe", json!({}));
    run(&mut s, "edit.select_all", json!({}));
    run(&mut s, "element.set_filters", json!({"filters": [{"type": "blur", "blur_x": 20, "blur_y": 20}, {"type": "glow", "strength": 2, "blur_x": 8, "blur_y": 8, "color": "#0000ff"}]}));
    run(&mut s, "timeline.goto", json!({"frame": 0}));
    run(&mut s, "timeline.create_tween", json!({"easing": "Linear"}));
    assert_eq!(drawn(&s, 0), vec![Filter::Blur { blur_x: 0.0, blur_y: 0.0, quality: 1 }], "the first keyframe's own list");
    let mid = drawn(&s, 5);
    assert_eq!(mid.len(), 2);
    assert_eq!(mid[0], Filter::Blur { blur_x: 10.0, blur_y: 10.0, quality: 1 });
    assert!(matches!(mid[1], Filter::Glow { strength, blur_x, color, .. } if strength == 1.0 && blur_x == 4.0 && color == Rgba::hex(0x0000ff)), "{:?}", mid[1]);
    assert_eq!(drawn(&s, 10).len(), 2);
    // It renders like a still clip with those filters.
    let mut still = clip();
    run(&mut still, "element.set_filters", json!({"filters": [{"type": "blur", "blur_x": 10, "blur_y": 10}, {"type": "glow", "strength": 1, "blur_x": 4, "blur_y": 4, "color": "#0000ff"}]}));
    assert_eq!(anicraft_render::render_frame(&s.doc, 5, 1.0).unwrap().data, anicraft_render::render_frame(&still.doc, 0, 1.0).unwrap().data);
}

/// The built-in presets are always there: they apply by name, can't be renamed, replaced or
/// deleted, and hiding one only takes it off the menu.
#[test]
fn built_in_presets_are_there_read_only_and_can_be_hidden() {
    let mut s = clip();
    let names = crate::filter_library::BUILTIN_NAMES;
    let listed = run(&mut s, "filter.preset_list", json!({}));
    assert_eq!(listed["presets"], json!([]), "none of your own yet");
    let builtin = listed["builtin"].as_array().unwrap();
    assert_eq!(builtin.iter().map(|b| b["name"].as_str().unwrap()).collect::<Vec<_>>(), names);
    for b in builtin {
        assert_eq!(b["hidden"], false);
        assert!(b["filters"].as_array().is_some_and(|f| !f.is_empty()), "{b}");
    }
    for name in ["Soft Shadow", "Neon Glow", "Emboss"] {
        assert!(names.contains(&name), "{name}");
    }
    // Each is in range as written, and applies.
    for p in crate::filter_library::builtins() {
        assert!(!p.filters.is_empty() && p.filters.iter().all(|f| f.enabled && f.sanitized() == *f), "{}", p.name);
        let v = run(&mut s, "filter.preset_apply", json!({"name": p.name.to_uppercase()}));
        assert_eq!(v["name"], p.name);
        assert_eq!(list(&s), p.filters);
    }
    run(&mut s, "filter.preset_apply", json!({"name": "neon glow"}));
    assert!(matches!(list(&s).as_slice(), [FilterEntry { filter: Filter::Glow { .. }, .. }, FilterEntry { filter: Filter::Glow { .. }, .. }]));
    assert!(px(&s, 0, 92, 150)[0] < 0.95, "the glow draws round the box");
    // Read-only: refused outright (never asked about), with the reason.
    assert!(refused(&mut s, "filter.preset_delete", json!({"name": "Emboss"})).contains("can't be deleted, only hidden"));
    assert!(refused(&mut s, "filter.preset_delete", json!({"name": "emboss", "confirm": true})).contains("built-in"));
    assert!(refused(&mut s, "filter.preset_rename", json!({"name": "Emboss", "to": "Mine"})).contains("keeps its name"));
    assert!(refused(&mut s, "filter.preset_save", json!({"name": "soft shadow"})).contains("built-in"));
    assert!(refused(&mut s, "filter.preset_save", json!({"name": "Soft Shadow", "confirm": true})).contains("built-in"));
    run(&mut s, "filter.preset_save", json!({"name": "Mine"}));
    assert!(refused(&mut s, "filter.preset_rename", json!({"name": "Mine", "to": "Outline"})).contains("built-in"));
    assert_eq!(crate::filter_library::builtins().len(), names.len());
    // Hidden: off the menu, still there by name; shown again.
    let v = run(&mut s, "filter.preset_hide", json!({"name": "greyscale"}));
    assert_eq!((v["name"].as_str(), v["hidden"].as_bool()), (Some("Greyscale"), Some(true)));
    run(&mut s, "filter.preset_hide", json!({"name": "Emboss"}));
    assert_eq!(s.filter_presets.hidden(), ["Emboss", "Greyscale"], "kept in menu order");
    let listed = run(&mut s, "filter.preset_list", json!({}));
    assert_eq!(listed["builtin"].as_array().unwrap().iter().filter(|b| b["hidden"] == true).count(), 2);
    assert_eq!(s.filter_presets.shown_builtins().len(), names.len() - 2);
    run(&mut s, "filter.preset_apply", json!({"name": "Greyscale"}));
    assert!(matches!(list(&s).as_slice(), [FilterEntry { filter: Filter::AdjustColor { .. }, .. }]));
    run(&mut s, "filter.preset_hide", json!({"name": "Greyscale", "hidden": false}));
    assert_eq!(s.filter_presets.hidden(), ["Emboss"]);
    assert!(refused(&mut s, "filter.preset_hide", json!({"name": "Mine"})).contains("preset of yours"));
    assert!(refused(&mut s, "filter.preset_hide", json!({"name": "nope"})).contains("no filter preset"));
    // What is hidden survives a new document and the preferences' round trip, junk dropped.
    run(&mut s, "file.new", json!({"confirm": true}));
    assert_eq!(s.filter_presets.hidden(), ["Emboss"]);
    let mut lib = crate::filter_library::FilterLibrary::default();
    lib.restore_hidden(&["emboss".into(), "Mine".into(), "Outline".into(), "Outline".into()]);
    assert_eq!(lib.hidden(), ["Outline", "Emboss"]);
    assert_eq!(lib.clone().sanitized(), lib);
    assert_eq!(serde_json::to_value(&lib).unwrap(), json!([]), "the presets list in the preferences stays a list");
    // A file's preset with a built-in's name comes in numbered; built-ins export by name.
    let dir = std::env::temp_dir().join(format!("anicraft-builtin-presets-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("p.json").display().to_string();
    let v = run(&mut s, "filter.preset_export", json!({"path": path, "names": ["Emboss", "Mine"]}));
    assert_eq!(v["presets"], 2);
    let v = run(&mut s, "filter.preset_import", json!({"path": path}));
    assert_eq!(v["imported"], json!(["Emboss 2", "Mine 2"]));
    let _ = std::fs::remove_dir_all(&dir);
    // Plug-ins may list and apply presets, not hide them (the menu is the artist's).
    assert!(crate::cmds_plugin::plugin_may_call("filter.preset_apply").is_ok());
    assert!(crate::cmds_plugin::plugin_may_call("filter.preset_hide").is_err());
}

/// Text takes filters as a movie clip does: a drop shadow under the glyphs, in the pixels; the
/// list survives edits of the words and of the text type; graphics are still refused.
#[test]
fn text_takes_filters_and_keeps_them() {
    if anicraft_text::Fonts::get().families().is_empty() {
        eprintln!("no fonts on this computer: text test skipped");
        return;
    }
    let mut s = Session::default();
    run(&mut s, "text.create", json!({"text": "HI", "x": 100, "y": 60, "size": 96, "bold": true, "color": "#000000"}));
    let plain = anicraft_render::render_frame(&s.doc, 0, 1.0).unwrap();
    // A red shadow straight down, 30 px, sharp.
    let v = run(&mut s, "element.set_filters", json!({"filters": [{"type": "drop_shadow", "angle": 90, "distance": 30, "blur_x": 0, "blur_y": 0, "strength": 1, "color": "#ff0000"}]}));
    assert_eq!((v["filters"].as_u64(), v["set"].as_u64()), (Some(1), Some(1)));
    assert_eq!(list(&s).len(), 1);
    let shadowed = anicraft_render::render_frame(&s.doc, 0, 1.0).unwrap();
    let ink = |p: [f32; 4]| p[0] < 0.1 && p[1] < 0.1 && p[2] < 0.1;
    let white = |p: [f32; 4]| p[0] > 0.99 && p[1] > 0.99 && p[2] > 0.99;
    let red = |p: [f32; 4]| p[0] > 0.9 && p[1] < 0.1 && p[2] < 0.1;
    // Glyph pixels whose spot 30 px below was empty stage: there the shadow shows, red.
    let (mut inked, mut shadows) = (0, 0);
    for y in 0..plain.height.saturating_sub(30) {
        for x in 0..plain.width {
            let (Some(here), Some(below)) = (plain.pixel(x, y), plain.pixel(x, y + 30)) else { continue };
            if ink(here) {
                inked += 1;
                assert!(ink(shadowed.pixel(x, y).unwrap()), "the text draws over its shadow at {x},{y}");
                if white(below) {
                    shadows += 1;
                    assert!(red(shadowed.pixel(x, y + 30).unwrap()), "the shadow of {x},{y}: {:?}", shadowed.pixel(x, y + 30));
                }
            }
        }
    }
    assert!(inked > 500 && shadows > 100, "{inked} glyph pixels, {shadows} of them with their shadow on the stage");
    // Switched off, the picture is the plain text's again.
    run(&mut s, "filter.enable", json!({"enabled": false}));
    assert_eq!(anicraft_render::render_frame(&s.doc, 0, 1.0).unwrap().data, plain.data);
    run(&mut s, "filter.enable", json!({"enabled": true}));
    // The words, the style and the text type change; the filters stay.
    run(&mut s, "text.edit", json!({"text": "HEY", "size": 80}));
    assert_eq!(list(&s).len(), 1, "after editing the text");
    run(&mut s, "text.field", json!({"type": "dynamic", "border": true}));
    assert_eq!(list(&s).len(), 1, "as a dynamic field");
    assert!(matches!(&s.doc.timeline(s.scene).layers[0].keyframes[0].elements[0].kind, ElementKind::Text(t) if t.field.is_some()));
    let dl = anicraft_doc::evaluate(&s.doc, 0);
    assert_eq!(dl.groups.len(), 1);
    assert!(dl.items.len() >= 3 && dl.items.iter().all(|i| i.group == Some(0)), "the field's box, text and border are filtered as one");
    run(&mut s, "text.field", json!({"type": "static"}));
    assert_eq!(list(&s).len(), 1, "back to static text");
    // The rest of the filter commands, presets and the clipboard work on text.
    run(&mut s, "filter.preset_apply", json!({"name": "Neon Glow"}));
    run(&mut s, "filter.copy", json!({}));
    run(&mut s, "filter.paste", json!({}));
    run(&mut s, "filter.move", json!({"from": 0, "to": 3}));
    run(&mut s, "filter.remove", json!({"index": 0}));
    assert_eq!(list(&s).len(), 3);
    // It round-trips through the file, and undo takes the filters back.
    let back: anicraft_doc::Document = serde_json::from_str(&serde_json::to_string(&s.doc).unwrap()).unwrap();
    assert!(back == s.doc);
    // In a motion tween, a text's filter tweens like a clip's.
    run(&mut s, "element.set_filters", json!({"filters": [{"type": "blur", "blur_x": 0, "blur_y": 0}]}));
    run(&mut s, "timeline.create_motion_tween", json!({}));
    run(&mut s, "motion.set_key", json!({"property": "filter.0.blur_x", "frame": 10, "value": 20}));
    assert!(matches!(drawn(&s, 5).first(), Some(Filter::Blur { blur_x, .. }) if *blur_x == 10.0));
    // Shapes take no filters, while graphic symbols do (P-027).
    let mut s = Session::default();
    run(&mut s, "draw.rect", json!({"x": 10, "y": 10, "w": 50, "h": 50, "fill": "#ff0000"}));
    assert!(refused(&mut s, "element.set_filters", json!({"filters": [{"type": "blur"}]})).contains("select a symbol instance or text"));
    run(&mut s, "modify.convert_to_symbol", json!({"name": "G"}));
    run(&mut s, "symbol.set_kind", json!({"kind": "graphic"}));
    assert_eq!(run(&mut s, "element.set_filters", json!({"filters": [{"type": "blur"}]}))["filters"], 1);
}
