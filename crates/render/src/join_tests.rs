//! Fills of one merged drawing join without a seam (ADR 0138, issue #182): where two of them
//! share an edge, nothing below shows through it.

use super::*;
use anicraft_doc::{ElementId, LayerId};
use anicraft_geom::Path;
use std::sync::Arc;

fn item(path: Path, color: Rgba, joins: bool) -> DisplayItem {
    DisplayItem {
        joins,
        path: Arc::new(path),
        transform: Affine::IDENTITY,
        fill: Some(Paint::Solid(color)),
        stroke: None,
        fill_rule: FillRule::NonZero,
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

const RED: Rgba = Rgba::new(1.0, 0.0, 0.0, 1.0);
const BLUE: Rgba = Rgba::new(0.0, 0.0, 1.0, 1.0);

/// The two halves of an 80 × 80 square cut by a curve from its top to its bottom (and by a
/// slanted line across the left half: three fills, as the merge leaves them).
fn halves(joins: bool, left: Rgba, right: Rgba) -> Vec<DisplayItem> {
    let v = Vec2::new;
    let mut a = Path::new();
    a.move_to(v(0.0, 0.0));
    a.line_to(v(40.0, 0.0));
    a.cubic_to(v(63.0, 25.0), v(17.0, 55.0), v(40.0, 80.0));
    a.line_to(v(0.0, 80.0));
    a.close();
    let mut b = Path::new();
    b.move_to(v(40.0, 0.0));
    b.line_to(v(80.0, 0.0));
    b.line_to(v(80.0, 80.0));
    b.line_to(v(40.0, 80.0));
    b.cubic_to(v(17.0, 55.0), v(63.0, 25.0), v(40.0, 0.0));
    b.close();
    vec![item(a, left, false), item(b, right, joins)]
}

fn draw(items: Vec<DisplayItem>, transform: Affine, background: Option<Rgba>) -> Pixmap {
    let dl = DisplayList { items, ..Default::default() };
    render(&dl, &View { transform, width: 100, height: 100, background }).unwrap().0
}

/// The view the tests look through: off the pixel grid, turned a little, so the shared edge
/// crosses pixels every way.
fn askew() -> Affine {
    Affine::translate(10.3, 9.7).mul(&Affine::rotate(0.07))
}

/// The most green in any pixel well inside the square: the white background showing through
/// red and blue.
fn leak(pm: &Pixmap) -> f32 {
    let mut worst = 0.0f32;
    for y in 22..78 {
        for x in 22..78 {
            worst = worst.max(pm.pixel(x, y).unwrap()[1]);
        }
    }
    worst
}

#[test]
fn joined_fills_hide_what_is_below_along_their_shared_edge() {
    let apart = leak(&draw(halves(false, RED, BLUE), askew(), Some(Rgba::WHITE)));
    let joined = leak(&draw(halves(true, RED, BLUE), askew(), Some(Rgba::WHITE)));
    // Drawn one over the other, the edge lets up to a quarter of the background through.
    assert!(apart > 0.1, "the picture has a seam to hide: {apart}");
    assert!(joined < apart / 50.0, "joined {joined}, apart {apart}");
}

#[test]
fn joined_fills_keep_their_opacity_along_the_edge() {
    // Two half-transparent fills side by side are half-transparent everywhere, the edge too
    // (one over the other, the edge would be denser where both cover a pixel).
    let half = |c: Rgba| Rgba::new(c.r, c.g, c.b, 0.5);
    let pm = draw(halves(true, half(RED), half(BLUE)), askew(), None);
    for y in 22..78 {
        for x in 22..78 {
            let a = pm.pixel(x, y).unwrap()[3];
            assert!((a - 0.5).abs() < 0.01, "({x}, {y}): {a}");
        }
    }
}

#[test]
fn a_joined_fill_lying_over_another_is_drawn_over_it() {
    // Not side by side (a shape moved onto another and not merged yet): wherever one of them
    // covers a pixel whole the picture is the one drawn one over the other, exactly.
    let pair = |joins: bool| {
        let under = item(Path::rect(5.25, 5.5, 70.0, 60.0), Rgba::new(1.0, 0.0, 0.0, 0.8), false);
        let over = item(Path::ellipse(40.0, 36.0, 21.3, 17.7), Rgba::new(0.0, 0.0, 1.0, 0.6), joins);
        draw(vec![under, over], Affine::IDENTITY, Some(Rgba::WHITE))
    };
    let (joined, apart) = (pair(true), pair(false));
    let worst = joined.data.iter().zip(&apart.data).flat_map(|(a, b)| a.iter().zip(b).map(|(x, y)| (x - y).abs())).fold(0.0f32, f32::max);
    assert!(worst < 1e-5, "{worst}");
}

#[test]
fn a_stroke_between_two_fills_keeps_them_apart() {
    // The first fill's stroke is drawn between the two fills, so they can't be added up.
    let mut items = halves(true, RED, BLUE);
    items[0].stroke = Some(anicraft_doc::Stroke::new(Rgba::BLACK, 2.0));
    let dl = DisplayList { items, ..Default::default() };
    assert_eq!(joined_runs(&dl), vec![0, 0]);
    // Three in a row are one run; a fill in another clip or group is not of it.
    let third = || item(Path::rect(80.0, 0.0, 10.0, 80.0), RED, true);
    let mut items = halves(true, RED, BLUE);
    items.push(third());
    let dl = DisplayList { items, ..Default::default() };
    let runs = joined_runs(&dl);
    assert!(runs[0] != 0 && runs[0] == runs[1] && runs[1] == runs[2], "{runs:?}");
    let mut items = halves(true, RED, BLUE);
    items.push(DisplayItem { group: Some(0), ..third() });
    let dl = DisplayList { items, ..Default::default() };
    let runs = joined_runs(&dl);
    assert!(runs[0] != 0 && runs[0] == runs[1] && runs[2] == 0, "{runs:?}");
}

#[test]
fn a_mask_of_joined_fills_reveals_along_their_shared_edge() {
    // The two halves as a mask layer's artwork over a white square: all of it shows.
    let reveal = |joins: bool| {
        let mut items = vec![DisplayItem { clip: Some(0), ..item(Path::rect(0.0, 0.0, 80.0, 80.0), Rgba::WHITE, false) }];
        items.extend(halves(joins, RED, BLUE).into_iter().map(|i| DisplayItem { masks: Some(0), ..i }));
        let dl = DisplayList { items, clip_parents: vec![None], ..Default::default() };
        let pm = render(&dl, &View { transform: askew(), width: 100, height: 100, background: None }).unwrap().0;
        let mut least = 1.0f32;
        for y in 22..78 {
            for x in 22..78 {
                least = least.min(pm.pixel(x, y).unwrap()[3]);
            }
        }
        least
    };
    assert!(reveal(false) < 0.9, "apart, the mask has a seam: {}", reveal(false));
    assert!(reveal(true) > 0.995, "{}", reveal(true));
}

#[test]
fn joined_fills_fade_together_in_a_group() {
    // Inside a symbol instance at half opacity the edge is as opaque as the rest.
    let mut items = halves(true, RED, BLUE);
    for i in &mut items {
        i.group = Some(0);
    }
    let groups = vec![anicraft_doc::Group { parent: None, alpha: 0.5, blend: BlendMode::Normal, filters: Arc::from(Vec::new()), clip: None }];
    let dl = DisplayList { items, groups, ..Default::default() };
    let pm = render(&dl, &View { transform: askew(), width: 100, height: 100, background: None }).unwrap().0;
    for y in 22..78 {
        for x in 22..78 {
            let a = pm.pixel(x, y).unwrap()[3];
            assert!((a - 0.5).abs() < 0.01, "({x}, {y}): {a}");
        }
    }
}
