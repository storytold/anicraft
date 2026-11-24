//! SVG export/reimport parity regressions (storytold/craft-libs#778): verifies `viewBox`,
//! gradients, clips, link wrapping, and representative bitmap-fill behavior through `file.export_svg`,
//! `file.import`, and rendered RGBA comparison.

use anicraft_engine::Session;
use anicraft_engine::doc::{Bitmap, Document, Element, ElementId, ElementKind, GradientStop, Layer, LayerId, Paint, Rgba, Shape, Stage};
use anicraft_engine::geom::{Affine, Path, Vec2};
use serde_json::json;
use std::sync::Arc;

fn pixel_diff(a: &[u8], b: &[u8]) -> (u8, f64) {
    let worst = a.iter().zip(b).map(|(x, y)| (i32::from(*x) - i32::from(*y)).unsigned_abs() as u8).max().unwrap_or(0);
    let mean = a.iter().zip(b).map(|(x, y)| (i32::from(*x) - i32::from(*y)).unsigned_abs() as f64).sum::<f64>() / a.len().max(1) as f64;
    (worst, mean)
}

#[test]
fn svg_export_reimport_preserves_viewbox_gradients_clips_and_links() {
    let doc = Document { stage: Stage { width: 160, height: 100, ..Default::default() }, ..Default::default() };
    let mut src = Session::new(doc);

    // Linear gradient shape and radial gradient shape (with focal point) under a coverage clip.
    let linear = Paint::linear(
        Vec2::new(10.0, 10.0),
        Vec2::new(70.0, 10.0),
        vec![GradientStop { offset: 0.0, color: Rgba::hex(0xff3300) }, GradientStop { offset: 1.0, color: Rgba::hex(0x0066ff) }],
    );
    let mut radial = Paint::radial(
        Vec2::ZERO,
        1.0,
        vec![GradientStop { offset: 0.0, color: Rgba::hex(0xffff00) }, GradientStop { offset: 1.0, color: Rgba::hex(0x00aa44) }],
    )
    .with_frame(&Affine { m: [28.0, 0.0, 0.0, 20.0, 115.0, 50.0] }, None);
    if let Paint::Radial { focal, .. } = &mut radial {
        *focal = 0.4;
    }

    {
        let tl = src.doc.timeline_mut(0);
        let art = Arc::make_mut(&mut tl.layers[0]);
        let kf = Arc::make_mut(&mut art.keyframes[0]);
        kf.elements.push(Arc::new(Element {
            id: ElementId(1),
            name: "lin".into(),
            transform: Affine::IDENTITY,
            color: Default::default(),
            transform_3d: None,
            kind: ElementKind::Shape(Shape::filled(Path::rect(10.0, 10.0, 60.0, 80.0), linear)),
        }));
        kf.elements.push(Arc::new(Element {
            id: ElementId(2),
            name: "rad".into(),
            transform: Affine::IDENTITY,
            color: Default::default(),
            transform_3d: None,
            kind: ElementKind::Shape(Shape::filled(Path::rect(80.0, 20.0, 70.0, 60.0), radial)),
        }));
    }
    src.execute("layer.new", &json!({"name": "Clip"})).unwrap();
    src.execute("draw.ellipse", &json!({"x": 15, "y": 15, "w": 130, "h": 70, "fill": "#000000", "stroke": null})).unwrap();
    src.execute("layer.toggle_mask", &json!({})).unwrap();

    let dir = std::env::temp_dir().join(format!("anicraft-svg-parity-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let svg_path = dir.join("scene.svg");
    src.execute("file.export_svg", &json!({"path": svg_path.display().to_string()})).unwrap();
    let svg = std::fs::read_to_string(&svg_path).unwrap();
    assert!(svg.contains("width=\"160\" height=\"100\" viewBox=\"0 0 160 100\""), "viewBox matches stage: {svg}");
    assert!(svg.contains("<linearGradient") && svg.contains("<radialGradient"), "gradients preserved: {svg}");
    assert!(svg.contains("<clipPath"), "coverage clip preserved: {svg}");

    // Reimport through `file.import` into an empty document of the same stage size and compare pixels.
    let dst_doc = Document { stage: src.doc.stage, ..Default::default() };
    let mut dst = Session::new(dst_doc);
    dst.execute("file.import", &json!({"path": svg_path.display().to_string(), "x": 0, "y": 0})).unwrap();

    let want = anicraft_engine::render::render_frame(&src.doc, 0, 1.0).unwrap().to_rgba8();
    let got = anicraft_engine::render::render_frame(&dst.doc, 0, 1.0).unwrap().to_rgba8();
    let (worst, mean) = pixel_diff(&want, &got);
    assert!(mean < 0.05 && worst <= 2, "gradients+clip+viewBox round-trip: mean {mean:.4}, worst {worst}");

    // Representative link behavior: an `<a href="...">` emitted by `to_svg` imports and renders identically.
    let mut dl = anicraft_engine::doc::evaluate(&src.doc, 0);
    dl.links = vec![(0, "https://example.org/?a=1&b=\"ok\"".into())];
    let linked_svg = anicraft_engine::render::svg::to_svg(&dl, &src.doc.stage);
    assert!(linked_svg.contains("<a href=\"https://example.org/?a=1&amp;b=&quot;ok&quot;\">"), "{linked_svg}");
    let linked_path = dir.join("linked.svg");
    std::fs::write(&linked_path, linked_svg.as_bytes()).unwrap();
    let linked_doc = Document { stage: src.doc.stage, ..Default::default() };
    let mut linked = Session::new(linked_doc);
    linked.execute("file.import", &json!({"path": linked_path.display().to_string(), "x": 0, "y": 0})).unwrap();
    let got_linked = anicraft_engine::render::render_frame(&linked.doc, 0, 1.0).unwrap().to_rgba8();
    let (worst_l, mean_l) = pixel_diff(&want, &got_linked);
    assert!(mean_l < 0.05 && worst_l <= 2, "linked SVG round-trip: mean {mean_l:.4}, worst {worst_l}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn svg_bitmap_fill_exports_embedded_image_and_import_reports_unsupported_pattern() {
    let mut doc = Document { stage: Stage { width: 40, height: 40, ..Default::default() }, ..Default::default() };
    let bid = anicraft_engine::doc::BitmapId(1);
    let bmp = Bitmap::from_rgba(bid, "px", 2, 2, vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255]).unwrap();
    doc.bitmaps.push(Arc::new(bmp));
    {
        let tl = doc.timeline_mut(0);
        let layer = Layer::new(LayerId(9), "Bitmap");
        tl.layers.push(Arc::new(layer));
        let l = Arc::make_mut(&mut tl.layers[1]);
        let kf = Arc::make_mut(&mut l.keyframes[0]);
        kf.elements.push(Arc::new(Element {
            id: ElementId(10),
            name: "bmp".into(),
            transform: Affine::IDENTITY,
            color: Default::default(),
            transform_3d: None,
            kind: ElementKind::Shape(Shape::filled(
                Path::rect(4.0, 4.0, 20.0, 20.0),
                Paint::Bitmap { bitmap: bid, repeat: false, matrix: Affine::scale(10.0, 10.0) },
            )),
        }));
    }
    let mut src = Session::new(doc);
    let dir = std::env::temp_dir().join(format!("anicraft-svg-bmp-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let svg_path = dir.join("bitmap.svg");
    src.execute("file.export_svg", &json!({"path": svg_path.display().to_string()})).unwrap();
    let svg = std::fs::read_to_string(&svg_path).unwrap();
    assert!(svg.contains("<image ") && svg.contains("data:image/png;base64,"), "bitmap embedded in defs: {svg}");
    assert!(svg.contains("<pattern ") && svg.contains("fill=\"url(#p"), "bitmap fill references pattern: {svg}");

    // Retained format caveat: SVG import does not represent `<pattern>`/`<image>` yet and reports it cleanly.
    let dst_doc = Document { stage: src.doc.stage, ..Default::default() };
    let mut dst = Session::new(dst_doc);
    let imported = dst.execute("file.import", &json!({"path": svg_path.display().to_string(), "x": 0, "y": 0})).unwrap();
    let report = imported["report"].as_str().unwrap_or("");
    assert!(!report.is_empty(), "unsupported image/pattern fill is reported rather than guessed: {imported}");

    let _ = std::fs::remove_dir_all(&dir);
}
