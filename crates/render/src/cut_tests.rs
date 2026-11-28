//! What the renderer leaves out far off the surface (ADR 0132) changes no pixel: every picture
//! here is drawn twice, cut and whole, and compared bit for bit.

use super::*;
use anicraft_doc::{ElementId, GradientStop, LayerId, sample};
use anicraft_geom::Path;
use std::sync::Arc;

fn item(path: Path, fill: Paint, rule: FillRule) -> DisplayItem {
    DisplayItem {
        joins: false,
        path: Arc::new(path),
        transform: Affine::IDENTITY,
        fill: Some(fill),
        stroke: None,
        fill_rule: rule,
        color: ColorTransform::IDENTITY,
        layer: LayerId(0),
        element: ElementId(0),
        outline: None,
        clip: None,
        masks: None,
        guide: false,
        group: None,
        image: None,
        smooth: true,
        warp: None,
    }
}

/// A drawing as the Brush tool leaves one: `n` wavy ribbons across a 1280 × 720 stage, all
/// subpaths of one path (strokes merged into a shape), made of curves.
fn ribbons(n: u32) -> Path {
    let mut p = Path::new();
    for k in 0..n {
        let y = 30.0 + k as f32 * 660.0 / n as f32;
        let wave = |i: u32, off: f32| Vec2::new(40.0 + i as f32 * 30.0, y + off + ((i + k) as f32 * 0.9).sin() * 18.0);
        p.move_to(wave(0, 0.0));
        for i in 1..=40 {
            let (a, b) = (wave(i - 1, 0.0), wave(i, 0.0));
            p.cubic_to(a + Vec2::new(10.0, -14.0), b + Vec2::new(-10.0, 14.0), b);
        }
        for i in (0..40).rev() {
            let (a, b) = (wave(i + 1, 9.0), wave(i, 9.0));
            p.cubic_to(a + Vec2::new(-10.0, 14.0), b + Vec2::new(10.0, -14.0), b);
        }
        p.close();
    }
    p
}

fn drawing() -> DisplayList {
    let gradient = Paint::linear(Vec2::new(0.0, 0.0), Vec2::new(1280.0, 720.0), vec![GradientStop { offset: 0.0, color: Rgba::hex(0xff3300) }, GradientStop { offset: 1.0, color: Rgba::hex(0x0066ff) }]);
    let mut over = item(ribbons(7), gradient, FillRule::EvenOdd);
    over.transform = Affine::translate(15.0, 22.0);
    // A stroked curve and a quadratic blob too: strokes are never cut, quadratics are.
    let mut line = item(Path::ellipse(640.0, 360.0, 500.0, 300.0), Paint::Solid(Rgba::BLACK), FillRule::NonZero);
    line.fill = None;
    line.stroke = Some(anicraft_doc::Stroke::new(Rgba::hex(0x116633), 6.0));
    let mut blob = Path::new();
    blob.move_to(Vec2::new(100.0, 100.0));
    blob.quad_to(Vec2::new(640.0, -500.0), Vec2::new(1180.0, 100.0));
    blob.quad_to(Vec2::new(1900.0, 360.0), Vec2::new(1180.0, 620.0));
    blob.quad_to(Vec2::new(640.0, 1300.0), Vec2::new(100.0, 620.0));
    blob.quad_to(Vec2::new(-600.0, 360.0), Vec2::new(100.0, 100.0));
    blob.close();
    let items = vec![item(blob, Paint::Solid(Rgba::hex(0xffe9a8)), FillRule::NonZero), item(ribbons(24), Paint::Solid(Rgba::hex(0x202020)), FillRule::NonZero), over, line];
    DisplayList { items, ..Default::default() }
}

/// A `size` px tile whose corner is at device pixel `(ox, oy)` of the stage at scale `z`.
fn tile(z: f32, ox: f32, oy: f32, size: u32) -> View {
    View { transform: Affine::translate(-ox, -oy).mul(&Affine::scale(z, z)), width: size, height: size, background: Some(Rgba::hex(0xffffff)) }
}

/// Tiles over the stage at `z`: the middle, each edge and corner (so shapes leave the surface
/// on every side), and one out on the pasteboard.
fn tiles(z: f32) -> Vec<View> {
    let (w, h) = (1280.0 * z, 720.0 * z);
    let mut out = Vec::new();
    for fx in [0.0, 0.31, 0.5, 0.93] {
        for fy in [0.0, 0.47, 0.9] {
            out.push(tile(z, (w * fx).floor(), (h * fy).floor(), 96));
        }
    }
    out.push(tile(z, -40.0, -30.0, 96));
    out.push(tile(z, w - 50.0, h - 60.0, 96));
    out
}

#[test]
fn what_is_left_out_changes_no_pixel() {
    let demo = evaluate(&sample::demo(), 12);
    for (name, dl) in [("the drawing", drawing()), ("the sample", demo)] {
        let mut fewer = false;
        for z in [0.4f32, 1.0, 3.7, 16.0, 64.0] {
            for v in tiles(z) {
                let (whole, all) = render_cut(&dl, &v, false).unwrap();
                let (cut, kept) = render_cut(&dl, &v, true).unwrap();
                assert!(whole.data == cut.data, "{name} at {z}×, {:?}: the pictures differ", v.transform.m);
                assert_eq!((kept.drawn, kept.culled, kept.pixels_touched), (all.drawn, all.culled, all.pixels_touched), "{name} at {z}×: the same fills over the same pixels");
                assert!(kept.edges <= all.edges);
                fewer |= kept.edges < all.edges;
            }
        }
        assert!(fewer, "{name}: something was left out somewhere");
    }
}

/// Masks are prepared the same way: a clipped drawing under a mask of curves, cut and whole.
#[test]
fn masks_are_cut_without_changing_a_pixel() {
    let mut dl = drawing();
    let mut mask = item(Path::ellipse(640.0, 360.0, 560.0, 330.0), Paint::Solid(Rgba::BLACK), FillRule::NonZero);
    mask.masks = Some(0);
    for it in &mut dl.items {
        it.clip = Some(0);
    }
    dl.items.insert(0, mask);
    dl.clip_parents = vec![None];
    for z in [1.0f32, 16.0, 64.0] {
        for v in tiles(z) {
            let (whole, _) = render_cut(&dl, &v, false).unwrap();
            let (cut, _) = render_cut(&dl, &v, true).unwrap();
            assert!(whole.data == cut.data, "at {z}×, {:?}", v.transform.m);
        }
    }
}

/// #477: zoomed far into a big merged drawing, a tile used to walk every edge of the whole
/// drawing, cut finer the deeper the zoom. Now it walks fewer than the whole drawing has at
/// 100 %, however deep.
#[test]
fn a_tile_deep_inside_a_drawing_walks_fewer_edges_than_the_drawing_has() {
    let ink = item(ribbons(24), Paint::Solid(Rgba::BLACK), FillRule::NonZero);
    let dl = DisplayList { items: vec![ink], ..Default::default() };
    let whole_stage = View { transform: Affine::IDENTITY, width: 1280, height: 720, background: None };
    let normal = render(&dl, &whole_stage).unwrap().1.edges;
    let mut before = 0;
    for z in [8.0f32, 64.0] {
        let v = tile(z, (640.0 * z).floor(), (360.0 * z).floor(), 512);
        let deep = render(&dl, &v).unwrap().1.edges;
        let uncut = render_cut(&dl, &v, false).unwrap().1.edges;
        assert!(deep > 0 && deep < normal, "{z}×: {deep} edges for one tile, {normal} for the stage at 100 %");
        assert!(uncut > normal && uncut > before, "{z}×: whole, the tile's edges grow with the zoom ({uncut})");
        before = uncut;
    }
}
