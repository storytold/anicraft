//! Integration tests on committed fixtures. The fixture is a genuine Flash 8 file: its maker
//! drew it in their own copy of Flash 8 and released it under MIT (provenance in
//! `tests/fixtures/README.md` and `ATTRIBUTION.md`). The EDAP corpus tests (`corpus.rs`,
//! `corpus_text.rs`) need the uncommitted corpus; this one always runs.

use anicraft_doc::{Element, ElementKind, LayerKind, SymbolKind, Timeline, TweenKind};
use anicraft_geom::Vec2;
use std::sync::Arc;

const FLASH8_TESTER: &[u8] = include_bytes!("../fixtures/flash8-tester.fla");

fn layer<'a>(tl: &'a Timeline, name: &str) -> &'a anicraft_doc::Layer {
    tl.layers.iter().find(|l| l.name == name).unwrap_or_else(|| panic!("no layer {name:?}"))
}

/// Where the only symbol instance a layer shows at `frame` puts its registration point.
fn instance_at(l: &anicraft_doc::Layer, frame: u32) -> Vec2 {
    let (_, els) = anicraft_doc::evaluate_layer_elements(l, frame).expect("the layer shows a keyframe");
    let e: &Arc<Element> = els.iter().find(|e| matches!(e.kind, ElementKind::Symbol { .. })).expect("an instance");
    e.transform.apply(Vec2::new(0.0, 0.0))
}

/// A Flash 8 file opens with its symbols, the playback of its graphic instances, its
/// classic and shape tweens and its static text, and skips nothing but the movie clip's
/// component data.
#[test]
fn a_flash_8_file_opens_with_its_symbols_tweens_and_text() {
    let (doc, report) = anicraft_fla::import(FLASH8_TESTER).expect("the Flash 8 file imports");
    let skipped: Vec<&String> = report.skipped.keys().filter(|k| !k.contains("component")).collect();
    assert!(skipped.is_empty(), "skipped more than the component data: {skipped:?}");

    // One symbol of each kind.
    for kind in [SymbolKind::Graphic, SymbolKind::MovieClip, SymbolKind::Button] {
        assert!(doc.library.iter().any(|s| s.kind == kind), "no {kind:?} symbol in {:?}", doc.library.iter().map(|s| &s.name).collect::<Vec<_>>());
    }

    let scene = &doc.scenes[0].timeline;
    assert_eq!(layer(scene, "guide").kind, LayerKind::Guide);

    // The ball: a graphic on classic tweens, each keyframe starting its loop further on.
    let ball = layer(scene, "ball");
    assert!(ball.keyframes.len() >= 3, "{} keyframes", ball.keyframes.len());
    let first_frames: Vec<u32> = ball.keyframes.iter().filter_map(|k| {
        k.elements.iter().find_map(|e| match e.kind {
            ElementKind::Symbol { first_frame, .. } => Some(first_frame),
            _ => None,
        })
    }).collect();
    assert_eq!(first_frames.len(), ball.keyframes.len(), "an instance on every keyframe");
    assert!(first_frames.windows(2).all(|w| w[0] < w[1]), "first frames {first_frames:?}");
    // Mid-way through an unbraked tween the ball lies between its two keyframes.
    let k = ball.keyframes.iter().find(|k| k.duration > 2 && k.tween.as_ref().is_some_and(|t| t.kind == TweenKind::Classic && t.easing == anicraft_doc::Easing::Linear)).expect("a linear classic tween");
    let (a, b) = (instance_at(ball, k.start), instance_at(ball, k.start + k.duration));
    let mid = instance_at(ball, k.start + k.duration / 2);
    let along = (mid - a).dot(b - a) / (b - a).dot(b - a);
    assert!((b - a).length() > 1.0, "the keyframes {a:?} and {b:?} sit apart");
    assert!(along > 0.0 && along < 1.0, "mid-tween {mid:?} is not between {a:?} and {b:?} ({along})");

    // The graphic morphs its outline on a shape tween.
    let graphic = doc.library.iter().find(|s| s.kind == SymbolKind::Graphic).expect("graphic");
    assert!(
        graphic.timeline.layers.iter().flat_map(|l| &l.keyframes).any(|k| k.tween.as_ref().is_some_and(|t| t.kind == TweenKind::Shape)),
        "no shape tween in {:?}",
        graphic.name
    );

    // The button's Up frame says what the twin says, in Flash 8's UTF-16 static text.
    let button = doc.library.iter().find(|s| s.kind == SymbolKind::Button).expect("button");
    let up = button.timeline.layers.iter().filter_map(|l| l.keyframes.first()).flat_map(|k| &k.elements);
    let texts: Vec<&str> = up.filter_map(|e| match &e.kind {
        ElementKind::Text(t) => Some(t.text.as_str()),
        _ => None,
    }).collect();
    assert!(texts.iter().any(|t| t.contains("this is not a complete test")), "Up frame texts {texts:?}");
}
