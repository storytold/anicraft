//! The 2kool grid, measured from the 3840×3840 Clip Studio template.
//!
//! Paper is the stage colour. The art is a 3589×3589 block of 5 px lines at (125, 125).
//! Solid lines are #2CB0FF. The guides are the same blue at alpha 158. Coordinates in
//! [`LINES`] are inside that block, before the placement offset.
//!
//! On a new document the grid is one movie clip on a locked top layer, so blending applies
//! (a raw shape cannot carry a blend mode). It starts at Multiply and 15% opacity. The layer
//! under it is the one to draw on.

use crate::{BlendMode, ColorEffect, Document, Element, ElementId, ElementKind, Keyframe, Layer, LayerId, LayerKind, LoopMode, Paint, Rgba, Shape, Symbol, SymbolId, SymbolKind, Timeline};
use anicraft_geom::{Affine, Path};
use std::sync::Arc;

/// The plate this template was measured on.
pub const PLATE: u32 = 3840;
/// Where the 3589 block sits on the plate.
pub const OFFSET: f32 = 125.0;
/// Line weight, in plate pixels.
pub const THICK: f32 = 5.0;
/// #2CB0FF.
pub const BLUE: u32 = 0x2cb0ff;
/// Guide lines: the same blue at alpha 158.
pub const FAINT_ALPHA: f32 = 158.0 / 255.0;
/// The clip starts at 15% opacity.
pub const LAYER_ALPHA: f32 = 0.15;
/// The top layer and the library clip.
pub const LAYER_NAME: &str = "2Kool Grid";

/// `(x, y, w, h, faint)` inside the 3589 block. Faint guides first, then solid lines.
const LINES: [(f32, f32, f32, f32, bool); 26] = [
    (0.0, 1280.0, 3589.0, THICK, true),
    (0.0, 2304.0, 3589.0, THICK, true),
    (1280.0, 0.0, THICK, 3589.0, true),
    (2304.0, 0.0, THICK, 3589.0, true),
    (1280.0, 0.0, 1029.0, THICK, true),
    (1280.0, 3584.0, 1029.0, THICK, true),
    (0.0, 1280.0, THICK, 1029.0, true),
    (3584.0, 1280.0, THICK, 1029.0, true),
    (1152.0, 256.0, 1285.0, THICK, false),
    (1024.0, 640.0, 1541.0, THICK, false),
    (640.0, 1024.0, 2309.0, THICK, false),
    (256.0, 1152.0, 3077.0, THICK, false),
    (640.0, 1792.0, 2309.0, THICK, false),
    (256.0, 2432.0, 3077.0, THICK, false),
    (640.0, 2560.0, 2309.0, THICK, false),
    (1024.0, 2944.0, 1541.0, THICK, false),
    (1152.0, 3328.0, 1285.0, THICK, false),
    (256.0, 1152.0, THICK, 1285.0, false),
    (640.0, 1024.0, THICK, 1541.0, false),
    (1024.0, 640.0, THICK, 2309.0, false),
    (1152.0, 256.0, THICK, 3077.0, false),
    (1792.0, 640.0, THICK, 2309.0, false),
    (2432.0, 256.0, THICK, 3077.0, false),
    (2560.0, 640.0, THICK, 2309.0, false),
    (2944.0, 1024.0, THICK, 1541.0, false),
    (3328.0, 1152.0, THICK, 1285.0, false),
];

fn bars(faint: bool) -> Path {
    let mut path = Path::new();
    for (x, y, w, h, is_faint) in LINES {
        if is_faint != faint {
            continue;
        }
        path.cmds.extend(Path::rect(x + OFFSET, y + OFFSET, w, h).cmds);
    }
    path
}

fn filled(path: Path, alpha: f32) -> Shape {
    Shape::filled(path, Paint::Solid(Rgba::hex(BLUE).with_alpha(alpha)))
}

fn element(id: ElementId, kind: ElementKind, color: ColorEffect) -> Arc<Element> {
    Arc::new(Element { id, name: LAYER_NAME.into(), transform: Affine::IDENTITY, color, kind })
}

fn keyed(elements: Vec<Arc<Element>>) -> Arc<Keyframe> {
    let mut key = Keyframe::new(0, 1);
    key.elements = elements;
    Arc::new(key)
}

/// Adds the grid to a 3840×3840 document and returns the layer under it, which stays current.
/// The stage must already be that plate.
pub fn place(doc: &mut Document) -> Result<LayerId, &'static str> {
    if doc.stage.width != PLATE || doc.stage.height != PLATE {
        return Err("the 2kool grid is 3840 by 3840");
    }
    let sym_id = SymbolId(doc.alloc_id());
    let faint_id = ElementId(doc.alloc_id());
    let solid_id = ElementId(doc.alloc_id());
    let inner_id = LayerId(doc.alloc_id());
    let inst_id = ElementId(doc.alloc_id());
    let grid_id = LayerId(doc.alloc_id());

    let mut inner = Layer::new(inner_id, "lines");
    inner.keyframes = vec![keyed(vec![
        element(faint_id, ElementKind::Shape(filled(bars(true), FAINT_ALPHA)), ColorEffect::default()),
        element(solid_id, ElementKind::Shape(filled(bars(false), 1.0)), ColorEffect::default()),
    ])];
    doc.library.push(Arc::new(Symbol {
        id: sym_id,
        name: LAYER_NAME.into(),
        kind: SymbolKind::MovieClip,
        linkage: None,
        scale_grid: None,
        scale_grid_kept: None,
        timeline: Timeline { layers: vec![Arc::new(inner)] },
    }));

    let instance = element(
        inst_id,
        ElementKind::Symbol { symbol: sym_id, loop_mode: LoopMode::SingleFrame, first_frame: 0, blend: BlendMode::Multiply, filters: Vec::new(), events: Vec::new() },
        ColorEffect::Alpha(LAYER_ALPHA),
    );
    let mut grid = Layer::new(grid_id, LAYER_NAME);
    grid.locked = true;
    grid.color = Rgba::hex(BLUE);
    grid.keyframes = vec![keyed(vec![instance])];
    doc.timeline_mut(0).layers.insert(0, Arc::new(grid));

    doc.timeline(0)
        .layers
        .iter()
        .find(|l| l.id != grid_id && l.kind == LayerKind::Normal && !l.locked)
        .map(|l| l.id)
        .ok_or("the 2kool grid needs a layer to draw on")
}

/// Whether this plate pixel is a guide (`Some(false)`), a solid line (`Some(true)`), or empty.
pub fn covers(x: f32, y: f32) -> Option<bool> {
    let (ix, iy) = (x - OFFSET, y - OFFSET);
    let mut faint = false;
    for (left, top, w, h, is_faint) in LINES {
        if ix >= left && iy >= top && ix < left + w && iy < top + h {
            if !is_faint {
                return Some(true);
            }
            faint = true;
        }
    }
    faint.then_some(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evaluate;
    use anicraft_geom::{FillRule, Vec2};

    fn plate() -> Document {
        let mut doc = Document::default();
        doc.stage.width = PLATE;
        doc.stage.height = PLATE;
        place(&mut doc).expect("a 3840 plate takes the grid");
        doc.sanitize();
        doc
    }

    #[test]
    fn the_measured_lines_match_the_template_samples() {
        // The same plate pixels the VMI Studio measurement checks.
        assert_eq!(covers(1920.0, 1920.0), Some(true));
        assert_eq!(covers(200.0, 1407.0), Some(false));
        assert_eq!(covers(1505.0, 125.0), Some(false));
        assert_eq!(covers(10.0, 10.0), None);
        let doc = plate();
        let sym = doc.library.iter().find(|s| s.name == LAYER_NAME).expect("clip");
        let shapes: Vec<&Shape> = sym.timeline.layers[0].keyframes[0].elements.iter().filter_map(|e| match &e.kind {
            ElementKind::Shape(s) => Some(s),
            _ => None,
        }).collect();
        assert_eq!(shapes.len(), 2);
        let hit = |shape: &Shape, x, y| shape.path.contains(Vec2::new(x, y), &Affine::IDENTITY, FillRule::NonZero);
        let faint = shapes.iter().find(|s| matches!(s.fill, Some(Paint::Solid(c)) if (c.a - FAINT_ALPHA).abs() < 1e-6)).expect("guides");
        let solid = shapes.iter().find(|s| matches!(s.fill, Some(Paint::Solid(c)) if (c.a - 1.0).abs() < 1e-6)).expect("solid");
        assert!(hit(solid, 1920.0, 1920.0) && !hit(faint, 1920.0, 1920.0));
        assert!(hit(faint, 200.0, 1407.0) && !hit(solid, 200.0, 1407.0));
        assert!(!hit(solid, 10.0, 10.0) && !hit(faint, 10.0, 10.0));
    }

    #[test]
    fn the_grid_is_the_locked_top_layer_at_multiply_and_15_percent() {
        let doc = plate();
        let layers = &doc.timeline(0).layers;
        assert_eq!(layers[0].name, LAYER_NAME);
        assert!(layers[0].locked);
        assert_eq!(layers[1].name, "Layer 1");
        assert!(!layers[1].locked);
        let el = &layers[0].keyframes[0].elements[0];
        assert!(matches!(el.color, ColorEffect::Alpha(a) if (a - LAYER_ALPHA).abs() < 1e-6));
        assert!(matches!(el.kind, ElementKind::Symbol { blend: BlendMode::Multiply, .. }));
        if let ElementKind::Symbol { symbol, .. } = &el.kind {
            assert_eq!(doc.symbol(*symbol).map(|s| s.kind), Some(SymbolKind::MovieClip));
        }
        let dl = evaluate(&doc, 0);
        assert!(dl.groups.iter().any(|g| g.blend == BlendMode::Multiply && (g.alpha - LAYER_ALPHA).abs() < 1e-6));
        let mut small = Document::default();
        assert!(place(&mut small).is_err());
    }
}
