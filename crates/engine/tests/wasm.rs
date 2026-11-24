//! The engine inside WebAssembly (ADR 0036), as the web editor runs it: one thread, no file
//! system, no `std::time` clock. Run in Node by wasm-bindgen's test runner (docs/web.md):
//!
//! ```sh
//! CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner \
//!     cargo test -p anicraft-engine --target wasm32-unknown-unknown --test wasm
//! ```
//!
//! A build can't catch what traps at run time (a clock or thread call that compiles but isn't
//! there); these do. Natively this file is empty.
#![cfg(target_arch = "wasm32")]

use anicraft_engine::plugins::{Host, Limits, Plugin};
use anicraft_engine::render_service::RenderService;
use anicraft_engine::{Session, vfs};
use serde_json::{Map, Value, json};
use std::path::Path;
use wasm_bindgen_test::wasm_bindgen_test;

fn names() -> Vec<String> {
    vfs::take_written().into_iter().map(|(p, _)| p.display().to_string()).collect()
}

#[wasm_bindgen_test]
fn saves_opens_and_exports_through_memory() {
    assert!(vfs::in_memory(), "the browser has no disk: files live in memory");
    let mut s = Session::new(anicraft_engine::doc::sample::demo());
    s.execute("file.save", &json!({"path": "walk.anicraft"})).unwrap();
    let saved = vfs::take_written();
    assert_eq!(saved.len(), 1);
    vfs::put(Path::new("walk.anicraft"), saved[0].1.clone()).unwrap();
    let mut t = Session::default();
    t.execute("file.open", &json!({"path": "walk.anicraft"})).unwrap();
    assert_eq!(t.frame_count(), s.frame_count());
    // Parallel renders (rayon on the one thread), timing (web clock), encoders.
    s.execute("file.export_png", &json!({"path": "f.png"})).unwrap();
    s.execute("file.export_gif", &json!({"path": "m.gif", "to": 4})).unwrap();
    s.execute("file.export_html", &json!({"path": "m.html"})).unwrap();
    s.execute("file.export_png_sequence", &json!({"dir": "seq", "to": 2})).unwrap();
    assert_eq!(names().len(), 6);
    assert!(s.execute("file.export_video", &json!({"path": "m.webm"})).is_err());
}

#[wasm_bindgen_test]
fn the_stage_renders_without_a_thread() {
    let s = Session::new(anicraft_engine::doc::sample::demo());
    let svc = RenderService::new(64 << 20, None);
    let view = anicraft_engine::render::View::stage(&s.doc, 0.5);
    // No worker thread can start, so a request renders right away.
    let img = svc.request(svc.job(s.state, &s.doc, 3, view, None)).expect("rendered on this thread");
    assert!(img.error.is_none() && img.width > 0 && img.rgba_premul.iter().any(|&b| b != 0));
}

struct Quiet;
impl Host for Quiet {
    fn request(&mut self, _: &str, _: &Value) -> Result<Value, String> {
        Ok(Value::Null)
    }
}

#[wasm_bindgen_test]
fn plugins_run_in_the_browser_too() {
    let manifest = r#"{"id":"org.test.web","name":"Web","version":"1.0.0","kind":"command","author":"Tests"}"#;
    let text = format!(
        r#"(module
  (import "anicraft" "call" (func (param i32 i32) (result i32)))
  (import "anicraft" "response" (func (param i32 i32) (result i32)))
  (memory (export "memory") 1)
  (data (i32.const 1024) "{manifest}")
  (func (export "ac_abi_version") (result i32) i32.const 1)
  (func (export "ac_manifest") (result i64) (i64.or (i64.shl (i64.const {len}) (i64.const 32)) (i64.const 1024)))
  (func (export "ac_alloc") (param i32) (result i32) i32.const 4096)
  (func (export "ac_run") (param i32 i32) (result i32) i32.const 0))"#,
        manifest = manifest.replace('"', "\\22"),
        len = manifest.len()
    );
    let plugin = Plugin::load(&wat::parse_str(&text).unwrap(), Limits::default()).unwrap();
    assert_eq!(plugin.id(), "org.test.web");
    plugin.run(&mut Quiet, &Map::new()).unwrap();
}

#[wasm_bindgen_test]
fn exports_and_reimports_svg_and_lottie_in_memory() {
    use anicraft_engine::doc::{Document, Element, ElementId, ElementKind, GradientStop, Paint, Rgba, Shape, Stage};
    use anicraft_engine::geom::{Affine, Path as GeomPath, Vec2};
    use std::sync::Arc;

    let mut doc = Document { stage: Stage { width: 120, height: 80, ..Default::default() }, ..Default::default() };
    let linear = Paint::linear(
        Vec2::new(10.0, 10.0),
        Vec2::new(110.0, 10.0),
        vec![GradientStop { offset: 0.0, color: Rgba::hex(0xff3300) }, GradientStop { offset: 1.0, color: Rgba::hex(0x0066ff) }],
    );
    {
        let tl = doc.timeline_mut(0);
        let layer = Arc::make_mut(&mut tl.layers[0]);
        let kf = Arc::make_mut(&mut layer.keyframes[0]);
        kf.elements.push(Arc::new(Element {
            id: ElementId(1),
            name: "grad".into(),
            transform: Affine::IDENTITY,
            color: Default::default(),
            transform_3d: None,
            kind: ElementKind::Shape(Shape::filled(GeomPath::rect(10.0, 10.0, 100.0, 60.0), linear)),
        }));
    }
    let mut s = Session::new(doc);
    s.execute("layer.new", &json!({"name": "Clip"})).unwrap();
    s.execute("draw.ellipse", &json!({"x": 20, "y": 15, "w": 80, "h": 50, "fill": "#000000", "stroke": null})).unwrap();
    s.execute("layer.toggle_mask", &json!({})).unwrap();

    s.execute("file.export_svg", &json!({"path": "frame.svg"})).unwrap();
    let written = vfs::take_written();
    let svg_bytes = written.into_iter().find(|(p, _)| p == Path::new("frame.svg")).unwrap().1;
    let svg = String::from_utf8(svg_bytes.clone()).unwrap();
    assert!(svg.contains("viewBox=\"0 0 120 80\"") && svg.contains("<linearGradient") && svg.contains("<clipPath"));

    vfs::put(Path::new("frame.svg"), svg_bytes).unwrap();
    let mut imported_doc = Document::default();
    imported_doc.stage = s.doc.stage;
    let mut imported = Session::new(imported_doc);
    imported.execute("file.import", &json!({"path": "frame.svg", "x": 0, "y": 0})).unwrap();

    let want = anicraft_engine::render::render_frame(&s.doc, 0, 1.0).unwrap().to_rgba8();
    let got = anicraft_engine::render::render_frame(&imported.doc, 0, 1.0).unwrap().to_rgba8();
    let worst = want.iter().zip(&got).map(|(x, y)| (i32::from(*x) - i32::from(*y)).unsigned_abs()).max().unwrap_or(0);
    assert!(worst <= 2, "WASM SVG export/reimport pixel diff worst={worst}");

    // Animated Lottie export in WASM.
    let mut anim = Session::new(anicraft_engine::doc::sample::demo());
    anim.execute("file.export_lottie", &json!({"path": "movie.json"})).unwrap();
    let lottie_bytes = vfs::take_written().into_iter().find(|(p, _)| p == Path::new("movie.json")).unwrap().1;
    let lottie: Value = serde_json::from_slice(&lottie_bytes).unwrap();
    assert!(lottie["layers"].as_array().is_some_and(|l| !l.is_empty()));
    assert!(lottie_bytes.windows(5).any(|w| w == b"\"a\":1"));
}
