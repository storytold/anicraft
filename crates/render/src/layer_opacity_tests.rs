//! Layer opacity (ADR 0111) in the CPU oracle and the exports that draw evaluated frames: a
//! faded layer's artwork is drawn together, then shown at the layer's opacity.

use crate::{View, render};
use anicraft_doc::{ColorEffect, Document, Element, ElementId, ElementKind, Keyframe, Layer, LayerId, LayerKind, Paint, Rgba, Shape, Stage, evaluate};
use anicraft_geom::{Affine, Path};
use std::sync::Arc;

fn square(id: u64, x: f32, colour: Rgba) -> Arc<Element> {
    Arc::new(Element { id: ElementId(id), name: String::new(), transform: Affine::translate(x, 0.0), color: ColorEffect::default(), transform_3d: None, kind: ElementKind::Shape(Shape::filled(Path::rect(0.0, 0.0, 8.0, 4.0), Paint::Solid(colour))) })
}

/// A 12 × 4 white stage: a red square under a blue one, overlapping from x = 4 to 8, on one
/// layer at `opacity`.
fn overlap(opacity: f32) -> Document {
    let mut d = Document { stage: Stage { width: 12, height: 4, background: Rgba::WHITE, ..Stage::default() }, ..Document::default() };
    let mut k = Keyframe::new(0, 1);
    k.elements = vec![square(1, 0.0, Rgba::new(1.0, 0.0, 0.0, 1.0)), square(2, 4.0, Rgba::new(0.0, 0.0, 1.0, 1.0))];
    let l = Arc::make_mut(&mut d.timeline_mut(0).layers[0]);
    l.keyframes = vec![Arc::new(k)];
    l.opacity = opacity;
    d
}

fn near(p: [f32; 4], q: [f32; 4]) -> bool {
    p.iter().zip(q).all(|(a, b)| (a - b).abs() < 1e-4)
}

#[test]
fn a_faded_layer_shows_its_artwork_as_one() {
    for (opacity, red, both, blue) in [(1.0, [1.0, 0.0, 0.0, 1.0], [0.0, 0.0, 1.0, 1.0], [0.0, 0.0, 1.0, 1.0]), (0.5, [1.0, 0.5, 0.5, 1.0], [0.5, 0.5, 1.0, 1.0], [0.5, 0.5, 1.0, 1.0]), (0.25, [1.0, 0.75, 0.75, 1.0], [0.75, 0.75, 1.0, 1.0], [0.75, 0.75, 1.0, 1.0]), (0.0, [1.0; 4], [1.0; 4], [1.0; 4])] {
        let d = overlap(opacity);
        let (pm, _) = render(&evaluate(&d, 0), &View::stage(&d, 1.0)).unwrap();
        // Where the squares overlap only the top one shows, at the layer's opacity: no red.
        assert!(near(pm.pixel(1, 1).unwrap(), red), "{opacity}: {:?}", pm.pixel(1, 1));
        assert!(near(pm.pixel(6, 1).unwrap(), both), "{opacity}: {:?}", pm.pixel(6, 1));
        assert!(near(pm.pixel(10, 1).unwrap(), blue), "{opacity}: {:?}", pm.pixel(10, 1));
    }
}

#[test]
fn a_folder_fades_its_layers_and_layers_fade_on_top_of_each_other() {
    // Two layers at 50 %, each a full black cover, the top one in a folder at 50 %.
    let mut d = Document { stage: Stage { width: 4, height: 4, background: Rgba::WHITE, ..Stage::default() }, ..Document::default() };
    let cover = |id: u64| Arc::new(Element { id: ElementId(id), name: String::new(), transform: Affine::IDENTITY, color: ColorEffect::default(), transform_3d: None, kind: ElementKind::Shape(Shape::filled(Path::rect(0.0, 0.0, 4.0, 4.0), Paint::Solid(Rgba::BLACK))) });
    let layer = |id: u64, el: u64, parent: Option<LayerId>| {
        let mut l = Layer::new(LayerId(id), "l");
        l.keyframes = vec![Arc::new(Keyframe { elements: vec![cover(el)], ..Keyframe::new(0, 1) })];
        l.opacity = 0.5;
        l.parent = parent;
        Arc::new(l)
    };
    let mut folder = Layer::new(LayerId(30), "folder");
    folder.kind = LayerKind::Folder;
    folder.opacity = 0.5;
    d.timeline_mut(0).layers = vec![Arc::new(folder), layer(31, 1, Some(LayerId(30))), layer(32, 2, None)];
    let (pm, _) = render(&evaluate(&d, 0), &View::stage(&d, 1.0)).unwrap();
    // White under 50 % black is 0.5; under a further 25 % black (50 % of 50 %) it is 0.375.
    assert!(near(pm.pixel(2, 2).unwrap(), [0.375, 0.375, 0.375, 1.0]), "{:?}", pm.pixel(2, 2));
    // The SVG export wraps the faded layers in opacity groups.
    let svg = crate::svg::to_svg(&evaluate(&d, 0), &d.stage);
    assert!(svg.contains("opacity=\"0.25\"") && svg.contains("opacity=\"0.5\""), "{svg}");
}
