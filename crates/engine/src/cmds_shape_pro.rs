//! Advanced selection, shape optimization, perspective distort, gradient/bitmap strokes, and
//! vector art brushes (`P-018`, `P-020`, `P-028`, `P-032`).

use crate::cmds_draw::MAX_EXTENT;
use crate::cmds_paint::{FillKind, FillSpec, fill_json, fill_kind, opt_focal, opt_stops};
use crate::cmds_raw::{check_extent, remerge_in_place};
use crate::cmds_shape::sample_at;
use crate::cmds_width::id_param;
use crate::merge::{self, Part, mergeable};
use crate::params::*;
use crate::{CommandSpec, CutOrigin, EngineError, Result, Session};
use anicraft_doc::{
    BitmapId, ColorEffect, ColorTransform, Element, ElementId, ElementKind, LayerId, MAX_GROUP_DEPTH,
    MAX_SYMBOL_DEPTH, Paint, Rgba, Shape, Spread, Stroke, SymbolId, Timeline,
};
use anicraft_geom::optimize::{
    Homography, count_anchors, distort_path, optimize_path, smooth_path, straighten_path,
    warp_along_spine,
};
use anicraft_geom::walk::CurveWalk;
use anicraft_geom::{Affine, FillRule, Path, Rect, Vec2, fit_curve};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::sync::Arc;

/// Maximum vertices in a lasso polygon or art-brush spine.
const MAX_POLY_POINTS: usize = 4096;
/// Maximum shapes processed in one shape-modify or distort command.
const MAX_MODIFY_SHAPES: usize = 5000;
/// Maximum vector shapes extracted from an art-brush source.
const MAX_BRUSH_SHAPES: usize = 512;

pub fn specs() -> Vec<CommandSpec> {
    let c = |id, label, menu, shortcut, params, mutates, run| CommandSpec {
        id,
        label,
        menu,
        shortcut,
        params,
        mutates,
        run,
    };
    vec![
        c(
            "raw.select_lasso",
            "Lasso Select",
            "",
            None,
            "{points: [[x, y], ..] (3 to 4096 vertices), add?: bool} slices and selects raw fills and strokes inside the closed lasso polygon across visible unlocked layers, and selects objects whose centre or bounds lie inside → {selected, cut}",
            true,
            select_lasso as crate::RunFn,
        ),
        c(
            "raw.magic_wand",
            "Magic Wand",
            "",
            None,
            "{x, y, tolerance?: 0..255 (default 32), all_layers?: bool (default false), add?: bool} selects shapes whose fill colour matches what lies at (x, y) within tolerance on the active layer (or all visible unlocked layers) → {selected, color}",
            true,
            magic_wand,
        ),
        c(
            "modify.smooth_shape",
            "Smooth",
            "Modify/Shape",
            None,
            "{strength?: 0..100 (default 50), id? (default: the selected shapes)} smooths selected shape outlines by relaxing anchors and tangent handles → {shapes, before_anchors, after_anchors}",
            true,
            smooth_shape,
        ),
        c(
            "modify.straighten_shape",
            "Straighten",
            "Modify/Shape",
            None,
            "{tolerance?: 0.01..500 (stage units, default 4.0), id? (default: the selected shapes)} collapses nearly-collinear curves into straight segments and snaps near-horizontal or near-vertical lines → {shapes, before_anchors, after_anchors}",
            true,
            straighten_shape,
        ),
        c(
            "modify.optimize_shape",
            "Optimize…",
            "Modify/Shape",
            None,
            "{tolerance?: 0.1..50.0 (stage units, default 2.5), id? (default: the selected shapes)} merges redundant short or collinear segments and refits minimal cubic curves → {shapes, before_anchors, after_anchors}",
            true,
            optimize_shape,
        ),
        c(
            "modify.distort",
            "Distort",
            "Modify/Transform",
            None,
            "{corners: [[x0, y0], [x1, y1], [x2, y2], [x3, y3]] (TL, TR, BR, BL in stage space), tolerance?: 0.005..50 (default 0.1)} deforms selected shapes on the active layer by an exact 4-corner projective homography from their rest bounds → {shapes, bounds}",
            true,
            distort,
        ),
        c(
            "element.set_stroke_paint",
            "Stroke Paint",
            "",
            None,
            "{type?: solid|linear|radial|bitmap, color?, stops?: [{offset: 0..1, color}, ..], spread?: extend|reflect|repeat, linear_rgb?: bool, focal?: -0.98..0.98, bitmap?: id, id? (default: the selected shapes)} sets solid, linear gradient, radial gradient or bitmap paint on selected shapes' strokes → {stroke_paint, changed}",
            true,
            set_stroke_paint,
        ),
        c(
            "draw.art_brush",
            "Vector Art Brush",
            "",
            None,
            "{source: id (element_id or symbol_id), spine: [[x, y], ..] (2 to 4096 points), width_scale?: 0.05..20.0 (default 1.0)} warps vector outlines from source along the arc-length parameterized spine curve into a drawing object or group → {element, shapes}",
            true,
            art_brush,
        ),
    ]
}

// ----- Helpers ----------------------------------------------------------------------------------

fn xy_list_param(p: &Value, key: &str, min_len: usize, max_len: usize) -> Result<Vec<Vec2>> {
    let bad = || {
        EngineError::Param(format!(
            "param '{key}' must be a list of {min_len} to {max_len} finite [x, y] points"
        ))
    };
    let arr = p.get(key).and_then(Value::as_array).ok_or_else(bad)?;
    if arr.len() < min_len || arr.len() > max_len {
        return Err(bad());
    }
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let pair = item.as_array().filter(|a| a.len() == 2).ok_or_else(bad)?;
        let coord = |i: usize| -> Option<f32> {
            pair.get(i)
                .and_then(Value::as_f64)
                .map(|v| v as f32)
                .filter(|v| v.is_finite() && v.abs() <= MAX_EXTENT)
        };
        out.push(Vec2::new(coord(0).ok_or_else(bad)?, coord(1).ok_or_else(bad)?));
    }
    Ok(out)
}

fn on_editable_key(s: &Session) -> bool {
    s.active_layer()
        .and_then(|l| l.keyframe_at(s.frame))
        .is_some_and(|k| k.motion.is_none() && (k.tween.is_none() || k.start == s.frame))
}

// ----- 1. Lasso & Magic Wand (`P-018`) ----------------------------------------------------------

fn select_lasso(s: &mut Session, p: &Value) -> Result<Value> {
    let raw_pts = xy_list_param(p, "points", 3, MAX_POLY_POINTS)?;
    let add = opt_bool(p, "add")?.unwrap_or(false);

    // Deduplicate consecutive near-identical vertices while keeping polygon fidelity.
    let mut pts: Vec<Vec2> = Vec::with_capacity(raw_pts.len());
    for pt in raw_pts {
        if pts.last().is_none_or(|prev| prev.distance(pt) > 1e-4) {
            pts.push(pt);
        }
    }
    if pts.len() >= 2
        && let (Some(&first), Some(&last)) = (pts.first(), pts.last())
        && first.distance(last) <= 1e-4
    {
        pts.pop();
    }
    if !add {
        s.selection.clear();
    }
    if pts.len() < 3 {
        return Ok(json!({"selected": s.selection.id_list(), "cut": 0}));
    }

    let mut cutter = Path::new();
    for (i, &pt) in pts.iter().enumerate() {
        if i == 0 {
            cutter.move_to(pt);
        } else {
            cutter.line_to(pt);
        }
    }
    cutter.close();

    let cutter_bounds = cutter.bounds();
    if cutter_bounds.is_empty() || (cutter_bounds.width() <= 1e-4 && cutter_bounds.height() <= 1e-4) {
        return Ok(json!({"selected": s.selection.id_list(), "cut": 0}));
    }

    let object_inside = |bounds: &Rect| -> bool {
        if cutter.contains(bounds.center(), &Affine::IDENTITY, FillRule::EvenOdd) {
            return true;
        }
        let corners = [
            bounds.min,
            Vec2::new(bounds.max.x, bounds.min.y),
            bounds.max,
            Vec2::new(bounds.min.x, bounds.max.y),
        ];
        corners
            .iter()
            .all(|&q| cutter.contains(q, &Affine::IDENTITY, FillRule::EvenOdd))
    };

    let mut found: Vec<(LayerId, Vec<ElementId>, Vec<ElementId>)> = Vec::new();
    let layers: Vec<LayerId> = s
        .timeline()
        .layers
        .iter()
        .map(|l| l.id)
        .filter(|l| s.gathers(*l))
        .collect();

    for l in layers {
        let (mut picked_objects, mut raw_candidates) = (Vec::new(), Vec::new());
        for e in s.elements_on(l) {
            let bounds = s.element_bounds_on(l, e.id);
            if bounds.is_empty() || bounds.intersect(&cutter_bounds).is_empty() {
                continue;
            }
            if mergeable(&e).is_some() {
                raw_candidates.push(e.id);
            } else if object_inside(&bounds) {
                picked_objects.push(e.id);
            }
        }
        if !picked_objects.is_empty() || !raw_candidates.is_empty() {
            found.push((l, picked_objects, raw_candidates));
        }
    }

    let mut cut = 0usize;
    let (mut origin, mut inside): (CutOrigin, Vec<ElementId>) = (Vec::new(), Vec::new());
    let mut hit: Vec<LayerId> = Vec::new();

    for (l, picked_objects, raw_candidates) in found {
        let (n, o, i, any_selected) =
            s.with_layer(l, |s| lasso_layer(s, &cutter, picked_objects, &raw_candidates))?;
        cut += n;
        origin.extend(o);
        inside.extend(i);
        if any_selected {
            hit.push(l);
        }
    }
    if !inside.is_empty() {
        s.pending_cut = Some((origin, inside));
    }
    if !hit.contains(&s.layer)
        && let Some(&top) = hit.first()
    {
        s.layer = top;
    }
    Ok(json!({"selected": s.selection.id_list(), "cut": cut}))
}

fn lasso_layer(
    s: &mut Session,
    cutter: &Path,
    mut picked: Vec<ElementId>,
    raw_candidates: &[ElementId],
) -> Result<(usize, CutOrigin, Vec<ElementId>, bool)> {
    let here = s.layer;
    let mut cut = 0usize;
    let (mut origin, mut inside_all) = (Vec::new(), Vec::new());

    if !raw_candidates.is_empty() && on_editable_key(s) {
        let kf = s.editable_keyframe()?;
        let existing = kf.elements.clone();
        let idx: Vec<usize> = existing
            .iter()
            .enumerate()
            .filter(|(_, e)| raw_candidates.contains(&e.id))
            .map(|(i, _)| i)
            .collect();
        let parts: Vec<Part> = idx
            .iter()
            .filter_map(|&i| existing.get(i))
            .map(|e| Part::Shape(e.as_ref(), s.selection.contains(here, e.id)))
            .collect();
        match merge::remerge_with(s, &parts, Some(cutter)) {
            Some(rm) if !rm.inside.is_empty() => {
                origin = rm
                    .from
                    .iter()
                    .filter_map(|&(piece, part)| {
                        idx.get(part)
                            .and_then(|&i| existing.get(i))
                            .map(|e| (piece, e.id))
                    })
                    .collect();
                cut = rm.inside.len();
                let gone: Vec<ElementId> = idx
                    .iter()
                    .filter_map(|&i| existing.get(i))
                    .map(|e| e.id)
                    .collect();
                s.selection.retain(|l, id| l != here || !gone.contains(&id));
                let mut inside = rm.inside;
                inside.extend(
                    rm.picked
                        .iter()
                        .filter(|id| !inside.contains(id))
                        .copied()
                        .collect::<Vec<_>>(),
                );
                picked.extend(inside.iter().copied());
                merge::replace(s, &idx, rm.out, false)?;
                inside_all = inside;
            }
            Some(_) => {}
            None => {
                for &id in raw_candidates {
                    let b = s.element_bounds_on(here, id);
                    if !b.is_empty()
                        && cutter.contains(b.center(), &Affine::IDENTITY, FillRule::EvenOdd)
                    {
                        picked.push(id);
                    }
                }
            }
        }
    } else {
        for &id in raw_candidates {
            let b = s.element_bounds_on(here, id);
            if !b.is_empty() && cutter.contains(b.center(), &Affine::IDENTITY, FillRule::EvenOdd) {
                picked.push(id);
            }
        }
    }
    let any = !picked.is_empty();
    s.selection.extend_on(here, picked);
    Ok((cut, origin, inside_all, any))
}

fn magic_wand(s: &mut Session, p: &Value) -> Result<Value> {
    let at = Vec2::new(f32_req(p, "x")?, f32_req(p, "y")?);
    if !(at.is_finite() && at.x.abs() <= MAX_EXTENT && at.y.abs() <= MAX_EXTENT) {
        return Err(EngineError::Param("point (x, y) is out of bounds".into()));
    }
    let tolerance = opt_f32(p, "tolerance")?.unwrap_or(32.0);
    if !(0.0..=255.0).contains(&tolerance) {
        return Err(EngineError::Param(
            "param 'tolerance' must be between 0 and 255".into(),
        ));
    }
    let all_layers = opt_bool(p, "all_layers")?.unwrap_or(false);
    let add = opt_bool(p, "add")?.unwrap_or(false);

    let candidate_layers: Vec<LayerId> = if all_layers {
        s.timeline()
            .layers
            .iter()
            .map(|l| l.id)
            .filter(|l| s.gathers(*l))
            .collect()
    } else if s.gathers(s.layer) {
        vec![s.layer]
    } else {
        Vec::new()
    };

    // Determine target colour `(Rgba, check_alpha)` at `at`.
    let dl = s.display_list();
    let hit_fill = dl.items.iter().rev().find_map(|it| {
        if it.outline.is_some()
            || it.masks.is_some()
            || !candidate_layers.contains(&it.layer)
            || it.clip.is_some_and(|c| !dl.clip_contains(c, at))
        {
            return None;
        }
        let fill = it.fill.as_ref()?;
        if !it.path.bounds().transformed(&it.transform).contains(at) {
            return None;
        }
        if !it.path.contains(at, &it.transform, it.fill_rule) {
            return None;
        }
        Some(match fill {
            Paint::Solid(c) => (it.color.apply(*c), true),
            _ => match sample_at(s, at, 1.0) {
                Ok([r, g, b]) => (
                    Rgba::new(
                        f32::from(r) / 255.0,
                        f32::from(g) / 255.0,
                        f32::from(b) / 255.0,
                        1.0,
                    ),
                    false,
                ),
                Err(_) => (it.color.apply(fill.primary_color()), true),
            },
        })
    });

    let sampled = match hit_fill {
        Some(pair) => Some(pair),
        None => {
            // Fall back to rendered pixel if an element (e.g. symbol or bitmap) sits at `at`.
            if s.hit_test(at, 0.0)
                .is_some_and(|(l, _)| candidate_layers.contains(&l))
            {
                sample_at(s, at, 1.0).ok().map(|[r, g, b]| {
                    (
                        Rgba::new(
                            f32::from(r) / 255.0,
                            f32::from(g) / 255.0,
                            f32::from(b) / 255.0,
                            1.0,
                        ),
                        false,
                    )
                })
            } else {
                None
            }
        }
    };

    if !add {
        s.selection.clear();
    }
    let Some((target, check_alpha)) = sampled else {
        return Ok(json!({"selected": s.selection.id_list(), "color": null}));
    };

    let tol_norm = tolerance / 255.0 + 1e-5;
    let matches_color = |c: Rgba| -> bool {
        let rgb_diff = (c.r - target.r)
            .abs()
            .max((c.g - target.g).abs())
            .max((c.b - target.b).abs());
        let diff = if check_alpha {
            rgb_diff.max((c.a - target.a).abs())
        } else {
            rgb_diff
        };
        diff <= tol_norm
    };

    let mut hit_layers = Vec::new();
    for l in candidate_layers {
        let mut layer_picks = Vec::new();
        for e in s.elements_on(l) {
            if let ElementKind::Shape(sh) = &e.kind
                && let Some(fill) = &sh.fill
            {
                let effective = e.color.transform().apply(fill.primary_color());
                if matches_color(effective) {
                    layer_picks.push(e.id);
                }
            }
        }
        if !layer_picks.is_empty() {
            hit_layers.push(l);
            s.selection.extend_on(l, layer_picks);
        }
    }
    if !hit_layers.contains(&s.layer)
        && let Some(&top) = hit_layers.first()
    {
        s.layer = top;
    }

    Ok(json!({
        "selected": s.selection.id_list(),
        "color": target.to_hex_alpha(),
    }))
}

// ----- 2. Modify › Shape: Smooth, Straighten, Optimize & Distort (`P-020`) ----------------------

fn rewrite_selected_paths(
    s: &mut Session,
    p: &Value,
    mut transform_path: impl FnMut(&Path) -> Path,
) -> Result<Value> {
    let explicit_id = id_param(p)?;
    let layers: Vec<LayerId> = match explicit_id {
        Some(_) => vec![s.layer],
        None => {
            let sel_layers = s.selected_layers();
            if sel_layers.is_empty() {
                return Err(EngineError::State("select at least one shape first".into()));
            }
            sel_layers
        }
    };

    let mut total_shapes = 0usize;
    let mut before_anchors = 0usize;
    let mut after_anchors = 0usize;

    for l in layers {
        s.with_layer(l, |s| {
            let target_ids: Vec<ElementId> = match explicit_id {
                Some(id) => vec![id],
                None => s.selection.on(s.layer),
            };
            if target_ids.is_empty() {
                return Ok(());
            }
            let visible: Vec<ElementId> = s.visible_elements().iter().map(|e| e.id).collect();
            if let Some(missing) = target_ids.iter().find(|id| !visible.contains(id)) {
                return Err(EngineError::State(format!(
                    "element {} is not on the active layer at this frame",
                    missing.0
                )));
            }
            let kf = s.editable_keyframe()?;
            let mut raw_changed = Vec::new();
            for el in &mut kf.elements {
                if !target_ids.contains(&el.id) {
                    continue;
                }
                let ElementKind::Shape(sh) = &el.kind else {
                    continue;
                };
                if sh.warp.is_some() {
                    return Err(EngineError::State(
                        "cannot reshape warped artwork directly (remove Asset Warp first)".into(),
                    ));
                }
                if total_shapes >= MAX_MODIFY_SHAPES {
                    return Err(EngineError::State(format!(
                        "select at most {MAX_MODIFY_SHAPES} shapes at once"
                    )));
                }
                let Some(inv) = el.transform.inverse() else {
                    continue;
                };
                let stage_path = sh.path.transformed(&el.transform);
                let b_cnt = count_anchors(&stage_path);
                let next_stage = transform_path(&stage_path);
                if next_stage.is_empty() {
                    continue;
                }
                check_extent(&next_stage)?;
                let a_cnt = count_anchors(&next_stage);
                let next_local = next_stage.transformed(&inv);

                before_anchors += b_cnt;
                after_anchors += a_cnt;
                total_shapes += 1;

                let is_raw = sh.merge;
                let id = el.id;
                if let ElementKind::Shape(sh_mut) = &mut Arc::make_mut(el).kind {
                    sh_mut.path = Arc::new(next_local);
                }
                if is_raw {
                    raw_changed.push(id);
                }
            }
            if !raw_changed.is_empty() {
                remerge_in_place(s, &raw_changed)?;
            }
            Ok(())
        })?;
    }

    if total_shapes == 0 {
        return Err(EngineError::State(
            "the selection contains no editable vector shapes (break symbols, text or groups apart first)".into(),
        ));
    }

    Ok(json!({
        "shapes": total_shapes,
        "before_anchors": before_anchors,
        "after_anchors": after_anchors,
    }))
}

fn smooth_shape(s: &mut Session, p: &Value) -> Result<Value> {
    let strength = opt_f32(p, "strength")?.unwrap_or(50.0);
    if !(0.0..=100.0).contains(&strength) {
        return Err(EngineError::Param(
            "param 'strength' must be between 0 and 100".into(),
        ));
    }
    rewrite_selected_paths(s, p, |path| smooth_path(path, strength))
}

fn straighten_shape(s: &mut Session, p: &Value) -> Result<Value> {
    let tolerance = opt_f32(p, "tolerance")?.unwrap_or(4.0);
    if !(0.01..=500.0).contains(&tolerance) {
        return Err(EngineError::Param(
            "param 'tolerance' must be between 0.01 and 500".into(),
        ));
    }
    rewrite_selected_paths(s, p, |path| straighten_path(path, tolerance))
}

fn optimize_shape(s: &mut Session, p: &Value) -> Result<Value> {
    let tolerance = opt_f32(p, "tolerance")?.unwrap_or(2.5);
    if !(0.1..=50.0).contains(&tolerance) {
        return Err(EngineError::Param(
            "param 'tolerance' must be between 0.1 and 50.0".into(),
        ));
    }
    rewrite_selected_paths(s, p, |path| optimize_path(path, tolerance))
}

fn distort(s: &mut Session, p: &Value) -> Result<Value> {
    let pts = xy_list_param(p, "corners", 4, 4)?;
    let (Some(&c0), Some(&c1), Some(&c2), Some(&c3)) =
        (pts.first(), pts.get(1), pts.get(2), pts.get(3))
    else {
        return Err(EngineError::Param(
            "param 'corners' must have 4 [x, y] corners".into(),
        ));
    };
    let corners = [c0, c1, c2, c3];
    let tolerance = opt_f32(p, "tolerance")?.unwrap_or(0.1);
    if !(0.005..=50.0).contains(&tolerance) {
        return Err(EngineError::Param(
            "param 'tolerance' must be between 0.005 and 50".into(),
        ));
    }

    let here = s.layer;
    let sel = s.selection.on(here);
    if sel.is_empty() {
        return Err(EngineError::State(
            "select shapes or drawing objects on the active layer to distort".into(),
        ));
    }
    let els = s.elements_on(here);
    let picked: Vec<Arc<Element>> = els.iter().filter(|e| sel.contains(&e.id)).cloned().collect();
    if picked.is_empty() {
        return Err(EngineError::State(
            "select shapes or drawing objects on the active layer to distort".into(),
        ));
    }
    if picked.len() > MAX_MODIFY_SHAPES {
        return Err(EngineError::State(format!(
            "distort supports at most {MAX_MODIFY_SHAPES} shapes at once"
        )));
    }
    if !picked.iter().all(|e| matches!(e.kind, ElementKind::Shape(_))) {
        return Err(EngineError::State(
            "distort deforms vector shapes and drawing objects only (break symbols, text, groups or bitmaps apart first)".into(),
        ));
    }
    if picked
        .iter()
        .any(|e| matches!(&e.kind, ElementKind::Shape(sh) if sh.warp.is_some()))
    {
        return Err(EngineError::State(
            "cannot distort warped artwork (remove Asset Warp first)".into(),
        ));
    }

    // Compute rest outline bounds in keyframe space.
    let mut rest = Rect::EMPTY;
    for e in &picked {
        if let ElementKind::Shape(sh) = &e.kind {
            for pl in sh.path.flatten(&e.transform, 0.05) {
                for &pt in &pl.points {
                    rest.include(pt);
                }
            }
        }
    }
    if rest.is_empty() || !(rest.min.is_finite() && rest.max.is_finite()) {
        return Err(EngineError::State(
            "the selected shapes have no outline to distort".into(),
        ));
    }
    let pad = |lo: f32, hi: f32| {
        if hi - lo < 1.0 {
            let m = (lo + hi) * 0.5;
            (m - 0.5, m + 0.5)
        } else {
            (lo, hi)
        }
    };
    let ((x0, x1), (y0, y1)) = (pad(rest.min.x, rest.max.x), pad(rest.min.y, rest.max.y));
    let rest = Rect::new(x0, y0, x1, y1);

    let homography = Homography::from_rect_to_quad(&rest, &corners).ok_or_else(|| {
        EngineError::Param(
            "param 'corners' must form a non-degenerate convex quad (TL, TR, BR, BL)".into(),
        )
    })?;

    // Compute affine approximation from rest center/axes for carrying non-solid paints smoothly.
    let paint_xf = {
        let rc = rest.center();
        let qc = (corners[0] + corners[1] + corners[2] + corners[3]) * 0.25;
        let ux = ((corners[1] - corners[0]) + (corners[2] - corners[3])) * (0.5 / rest.width());
        let uy = ((corners[3] - corners[0]) + (corners[2] - corners[1])) * (0.5 / rest.height());
        let linear = Affine {
            m: [ux.x, ux.y, uy.x, uy.y, 0.0, 0.0],
        };
        Affine::translate(qc.x, qc.y)
            .mul(&linear)
            .mul(&Affine::translate(-rc.x, -rc.y))
    };
    let _ = homography;

    let kf = s.editable_keyframe()?;
    let sel_set: HashSet<ElementId> = sel.into_iter().collect();
    let mut deformed_count = 0usize;
    let mut raw_ids = Vec::new();

    for el in &mut kf.elements {
        if !sel_set.contains(&el.id) {
            continue;
        }
        let ElementKind::Shape(sh) = &el.kind else {
            continue;
        };
        let stage_path = sh.path.transformed(&el.transform);
        let warped = distort_path(&stage_path, &rest, &corners, tolerance).ok_or_else(|| {
            EngineError::State("that perspective distortion made the outline too intricate".into())
        })?;
        check_extent(&warped)?;
        let center = warped.bounds().center();
        let to_local = Affine::translate(-center.x, -center.y);
        let local_path = warped.transformed(&to_local);
        let stage_paint_xf = to_local.mul(&paint_xf).mul(&el.transform);
        let new_fill = sh.fill.as_ref().map(|f| f.transformed(&stage_paint_xf));
        let new_stroke = sh.stroke.as_ref().map(|st| {
            let mut s2 = st.scaled(paint_xf.mean_scale().max(0.05) * el.transform.mean_scale());
            s2.paint = st.paint.as_ref().map(|p| p.transformed(&stage_paint_xf));
            s2
        });
        let is_raw = sh.merge;
        let id = el.id;
        let next_el = Element {
            id,
            name: el.name.clone(),
            transform: Affine::translate(center.x, center.y),
            color: el.color,
            transform_3d: el.transform_3d,
            kind: ElementKind::Shape(Shape {
                path: Arc::new(local_path),
                fill: new_fill,
                stroke: new_stroke,
                fill_rule: sh.fill_rule,
                merge: is_raw,
                warp: None,
                fill_tag: sh.fill_tag,
                stroke_tag: sh.stroke_tag,
            }),
        };
        *el = Arc::new(next_el);
        deformed_count += 1;
        if is_raw {
            raw_ids.push(id);
        }
    }

    if !raw_ids.is_empty() {
        remerge_in_place(s, &raw_ids)?;
    }

    Ok(json!({
        "shapes": deformed_count,
        "bounds": [rest.min.x, rest.min.y, rest.width(), rest.height()],
    }))
}

// ----- 3. Gradient / Bitmap Strokes (`P-032`) ---------------------------------------------------

fn set_stroke_paint(s: &mut Session, p: &Value) -> Result<Value> {
    let stops = opt_stops(p)?;
    let bitmap = match opt_u64(p, "bitmap")? {
        None => None,
        Some(id) => Some(
            s.doc
                .bitmap(BitmapId(id))
                .ok_or_else(|| EngineError::Param(format!("there is no bitmap {id} in the Library")))?
                .id,
        ),
    };
    let kind = match opt_str(p, "type")? {
        Some("solid") => FillKind::Solid,
        Some("linear") => FillKind::Linear,
        Some("radial") => FillKind::Radial,
        Some("bitmap") => FillKind::Bitmap,
        Some(other) => {
            return Err(EngineError::Param(format!(
                "unknown stroke paint type '{other}' (solid, linear, radial or bitmap)"
            )));
        }
        None => {
            if bitmap.is_some() {
                FillKind::Bitmap
            } else if stops.is_some() {
                FillKind::Linear
            } else {
                FillKind::Solid
            }
        }
    };
    let spread = match opt_str(p, "spread")? {
        None => None,
        Some(k) => Some(
            Spread::from_key(k).ok_or_else(|| {
                EngineError::Param(format!("unknown spread '{k}' (extend, reflect or repeat)"))
            })?,
        ),
    };
    let spec = FillSpec {
        kind,
        color: opt_color(p, "color")?,
        stops,
        spread,
        linear_rgb: opt_bool(p, "linear_rgb")?,
        focal: opt_focal(p)?,
        bitmap,
    };

    if kind == FillKind::Bitmap && spec.bitmap.is_none() {
        return Err(EngineError::Param(
            "a bitmap stroke needs 'bitmap' (a Library bitmap's id)".into(),
        ));
    }

    let explicit_id = id_param(p)?;
    let layers: Vec<LayerId> = match explicit_id {
        Some(_) => vec![s.layer],
        None => {
            let sel_layers = s.selected_layers();
            if sel_layers.is_empty() {
                return Err(EngineError::State(
                    "select a shape to set its stroke paint".into(),
                ));
            }
            sel_layers
        }
    };

    let default_style = s.style.clone();
    let mut total_changed = 0u64;
    let mut reported: Option<Paint> = None;

    for l in layers {
        s.with_layer(l, |s| -> Result<()> {
            let target_ids: HashSet<ElementId> = match explicit_id {
                Some(id) => [id].into_iter().collect(),
                None => s.selection.on(s.layer).into_iter().collect(),
            };
            if target_ids.is_empty() {
                return Ok(());
            }
            let kf = s.editable_keyframe()?;
            let strokes_selected = kf.elements.iter().any(|e| {
                target_ids.contains(&e.id)
                    && matches!(&e.kind, ElementKind::Shape(sh) if sh.merge && sh.stroke.is_some())
            });
            let mut raw_ids = Vec::new();
            for e in kf.elements_mut_where(|id| target_ids.contains(&id)) {
                let id = e.id;
                let ElementKind::Shape(sh) = &mut e.kind else {
                    continue;
                };
                if strokes_selected && sh.merge && sh.stroke.is_none() {
                    continue;
                }
                let mut st = sh.stroke.take().unwrap_or_else(|| {
                    default_style.new_stroke(default_style.stroke, default_style.stroke_width.max(1.0))
                });
                let old_paint = st.effective_paint();
                let mut new_paint = spec.apply(&old_paint).ok_or_else(|| {
                    EngineError::Param("a bitmap stroke needs 'bitmap' (a Library bitmap's id)".into())
                })?;
                if fill_kind(&old_paint) != spec.kind {
                    let fit_box = sh.path.bounds().inflate(st.width.max(1.0) * 0.5);
                    new_paint = new_paint.fitted(&fit_box);
                }
                let clean = new_paint.sanitized().unwrap_or(Paint::Solid(st.color));
                match &clean {
                    Paint::Solid(c) => {
                        st.color = *c;
                        st.paint = None;
                    }
                    other => {
                        st.color = other.primary_color();
                        st.paint = Some(other.clone());
                    }
                }
                if reported.is_none() {
                    reported = Some(st.effective_paint());
                }
                let is_raw = sh.merge;
                sh.stroke = Some(st);
                total_changed += 1;
                if is_raw {
                    raw_ids.push(id);
                }
            }
            if !raw_ids.is_empty() {
                remerge_in_place(s, &raw_ids)?;
            }
            Ok(())
        })?;
    }

    if total_changed == 0 {
        return Err(EngineError::State(
            "the selection has no shapes to apply stroke paint to".into(),
        ));
    }
    let paint_out = reported.unwrap_or(Paint::Solid(default_style.stroke));
    Ok(json!({
        "stroke_paint": fill_json(&paint_out),
        "changed": total_changed,
    }))
}

// ----- 4. Vector Art Brush (`P-028`) ------------------------------------------------------------

struct BrushPiece {
    path: Path,
    fill: Option<Paint>,
    stroke: Option<Stroke>,
    fill_rule: FillRule,
    color: ColorTransform,
}

fn collect_timeline_pieces(
    doc: &anicraft_doc::Document,
    tl: &Timeline,
    frame: u32,
    parent_xf: &Affine,
    parent_color: &ColorTransform,
    depths: (usize, usize),
    out: &mut Vec<BrushPiece>,
) {
    // Iterate bottom layer to top layer (`layers` has index 0 at the top).
    for (idx, layer) in tl.layers.iter().enumerate().rev() {
        if !layer.visible
            || tl.hidden_by_folder(idx)
            || !matches!(layer.kind, anicraft_doc::LayerKind::Normal)
        {
            continue;
        }
        let Some(kf) = layer.keyframe_at(frame) else {
            continue;
        };
        collect_elements_pieces(
            doc,
            &kf.elements,
            parent_xf,
            parent_color,
            depths,
            out,
        );
    }
}

fn collect_elements_pieces(
    doc: &anicraft_doc::Document,
    elements: &[Arc<Element>],
    parent_xf: &Affine,
    parent_color: &ColorTransform,
    (sym_depth, grp_depth): (usize, usize),
    out: &mut Vec<BrushPiece>,
) {
    for el in elements {
        if out.len() >= MAX_BRUSH_SHAPES {
            return;
        }
        let xf = parent_xf.mul(&el.transform);
        let col = parent_color.concat(&el.color.transform());
        match &el.kind {
            ElementKind::Shape(sh) => {
                let path = sh.path.transformed(&xf);
                if path.is_empty() {
                    continue;
                }
                let fill = sh.fill.as_ref().map(|f| f.transformed(&xf));
                let stroke = sh.stroke.as_ref().map(|st| {
                    let mut s2 = st.scaled(xf.mean_scale());
                    s2.paint = st.paint.as_ref().map(|p| p.transformed(&xf));
                    s2
                });
                out.push(BrushPiece {
                    path,
                    fill,
                    stroke,
                    fill_rule: sh.fill_rule,
                    color: col,
                });
            }
            ElementKind::Group { .. } if grp_depth < MAX_GROUP_DEPTH => {
                collect_elements_pieces(
                    doc,
                    &el.kind.group_members(),
                    &xf,
                    &col,
                    (sym_depth, grp_depth + 1),
                    out,
                );
            }
            ElementKind::Symbol {
                symbol,
                first_frame,
                ..
            } if sym_depth < MAX_SYMBOL_DEPTH => {
                if let Some(sym) = doc.symbol(*symbol) {
                    let f = (*first_frame).min(sym.timeline.frame_count().saturating_sub(1));
                    collect_timeline_pieces(
                        doc,
                        &sym.timeline,
                        f,
                        &xf,
                        &col,
                        (sym_depth + 1, grp_depth),
                        out,
                    );
                }
            }
            ElementKind::Text(tb) => {
                let path = tb.outline.transformed(&xf);
                if !path.is_empty() {
                    out.push(BrushPiece {
                        path,
                        fill: Some(Paint::Solid(tb.color)),
                        stroke: None,
                        fill_rule: FillRule::NonZero,
                        color: col,
                    });
                }
            }
            _ => {}
        }
    }
}

fn art_brush(s: &mut Session, p: &Value) -> Result<Value> {
    let source_raw = opt_u64(p, "source")?
        .ok_or_else(|| EngineError::Param("param 'source' is required (a non-negative integer id)".into()))?;
    let raw_spine = xy_list_param(p, "spine", 2, MAX_POLY_POINTS)?;
    let width_scale = opt_f32(p, "width_scale")?.unwrap_or(1.0);
    if !(0.05..=20.0).contains(&width_scale) {
        return Err(EngineError::Param(
            "param 'width_scale' must be between 0.05 and 20.0".into(),
        ));
    }

    let mut spine_pts: Vec<Vec2> = Vec::with_capacity(raw_spine.len());
    for pt in raw_spine {
        if spine_pts.last().is_none_or(|prev| prev.distance(pt) > 1e-4) {
            spine_pts.push(pt);
        }
    }
    if spine_pts.len() < 2 {
        return Err(EngineError::Param(
            "param 'spine' must contain at least 2 distinct points".into(),
        ));
    }

    // Resolve `source` from timeline elements (active layer first, then other layers at `s.frame`),
    // or from Library symbols (`SymbolId(source_raw)`).
    let mut pieces = Vec::new();
    let target_el_id = ElementId(source_raw);
    let found_el: Option<Arc<Element>> = s
        .elements_on(s.layer)
        .into_iter()
        .find(|e| e.id == target_el_id)
        .or_else(|| {
            s.timeline()
                .layers
                .iter()
                .find_map(|l| s.elements_on(l.id).into_iter().find(|e| e.id == target_el_id))
        });

    if let Some(el) = found_el {
        // Extract in the element's upright local or stage space (using stage space so drawn art
        // brushes follow how the source looks horizontally).
        collect_elements_pieces(
            &s.doc,
            std::slice::from_ref(&el),
            &Affine::IDENTITY,
            &ColorTransform::IDENTITY,
            (0, 0),
            &mut pieces,
        );
    } else if let Some(sym) = s.doc.symbol(SymbolId(source_raw)).cloned() {
        collect_timeline_pieces(
            &s.doc,
            &sym.timeline,
            0,
            &Affine::IDENTITY,
            &ColorTransform::IDENTITY,
            (0, 0),
            &mut pieces,
        );
    } else {
        return Err(EngineError::Param(format!(
            "source {source_raw} is neither an element on this frame nor a Library symbol"
        )));
    }

    if pieces.is_empty() {
        return Err(EngineError::State(
            "the art brush source contains no vector outlines".into(),
        ));
    }

    let mut source_bounds = Rect::EMPTY;
    for pc in &pieces {
        let pad = pc.stroke.as_ref().map_or(0.0, |st| st.width * 0.5);
        source_bounds = source_bounds.union(&pc.path.bounds().inflate(pad));
    }
    if source_bounds.is_empty() || !(source_bounds.min.is_finite() && source_bounds.max.is_finite()) {
        return Err(EngineError::State(
            "the art brush source has empty bounds".into(),
        ));
    }
    if source_bounds.width() < 1.0 {
        let cx = source_bounds.center().x;
        source_bounds.min.x = cx - 0.5;
        source_bounds.max.x = cx + 0.5;
    }

    let spine_path = if spine_pts.len() == 2
        && let (Some(&a), Some(&b)) = (spine_pts.first(), spine_pts.get(1))
    {
        Path::line(a, b)
    } else {
        let fitted = fit_curve(&spine_pts, 1.5, 8.0, false);
        if fitted.is_empty() {
            let mut poly = Path::new();
            for (i, &pt) in spine_pts.iter().enumerate() {
                if i == 0 {
                    poly.move_to(pt);
                } else {
                    poly.line_to(pt);
                }
            }
            poly
        } else {
            fitted
        }
    };

    let walk = CurveWalk::new(&spine_path)
        .filter(|w| w.length() > 1e-3)
        .ok_or_else(|| EngineError::Param("param 'spine' has zero arc length".into()))?;

    // Check editable keyframe before allocating ids.
    {
        let kf = s.editable_keyframe()?;
        if kf.elements.len() >= 100_000 {
            return Err(EngineError::State(
                "too many elements on this keyframe".into(),
            ));
        }
    }

    let mut warped_elements: Vec<Element> = Vec::with_capacity(pieces.len());
    for pc in &pieces {
        let warped = warp_along_spine(&pc.path, &source_bounds, &walk, width_scale, 0.25)
            .ok_or_else(|| {
                EngineError::State("the art brush stroke is too intricate".into())
            })?;
        if warped.is_empty() {
            continue;
        }
        check_extent(&warped)?;
        let c = warped.bounds().center();
        let local = warped.transformed(&Affine::translate(-c.x, -c.y));
        let local_bounds = local.bounds();

        let fill = pc.fill.as_ref().map(|f| match f {
            Paint::Solid(col) => Paint::Solid(pc.color.apply(*col)),
            other => other.fitted(&local_bounds),
        });
        let stroke = pc.stroke.as_ref().map(|st| {
            let mut s2 = st.clone();
            s2.color = pc.color.apply(st.color);
            s2.width = (st.width * width_scale).clamp(0.1, 1000.0);
            s2.paint = st
                .paint
                .as_ref()
                .map(|p| p.fitted(&local_bounds.inflate(s2.width * 0.5)));
            s2
        });

        let id = ElementId(s.doc.alloc_id());
        warped_elements.push(Element {
            id,
            name: String::new(),
            transform: Affine::translate(c.x, c.y),
            color: ColorEffect::None,
            transform_3d: None,
            kind: ElementKind::Shape(Shape {
                path: Arc::new(local),
                fill,
                stroke,
                fill_rule: pc.fill_rule,
                merge: false,
                warp: None,
                fill_tag: None,
                stroke_tag: None,
            }),
        });
    }

    if warped_elements.is_empty() {
        return Err(EngineError::State(
            "no warped shapes were produced along the spine".into(),
        ));
    }

    let shapes_count = warped_elements.len();
    let placed_el = if warped_elements.len() == 1 {
        warped_elements
            .pop()
            .ok_or_else(|| EngineError::State("no warped shapes were produced".into()))?
    } else {
        let mut union_b = Rect::EMPTY;
        for e in &warped_elements {
            if let ElementKind::Shape(sh) = &e.kind {
                union_b = union_b.union(&sh.path.bounds().transformed(&e.transform));
            }
        }
        let gc = if union_b.is_empty() {
            Vec2::ZERO
        } else {
            union_b.center()
        };
        let members: Vec<Arc<Element>> = warped_elements
            .into_iter()
            .map(|mut e| {
                e.transform = Affine::translate(-gc.x, -gc.y).mul(&e.transform);
                Arc::new(e)
            })
            .collect();
        let group_layer = LayerId(s.doc.alloc_id());
        let group_id = ElementId(s.doc.alloc_id());
        Element {
            id: group_id,
            name: String::new(),
            transform: Affine::translate(gc.x, gc.y),
            color: ColorEffect::None,
            transform_3d: None,
            kind: ElementKind::group(group_layer, members),
        }
    };

    let out_id = placed_el.id;
    let here = s.layer;
    s.editable_keyframe()?.elements.push(Arc::new(placed_el));
    s.selection = crate::Selection::only(here, [out_id]);

    Ok(json!({
        "element": out_id.0,
        "shapes": shapes_count,
    }))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use anicraft_doc::Bitmap;
    use anicraft_render::{encode_png, render_frame};

    #[test]
    fn lasso_cuts_raw_fill_and_selects_enclosed_object() {
        let mut s = Session::new(anicraft_doc::Document::default());
        s.execute("draw.object_drawing", &json!({"on": false})).unwrap();
        // Raw rectangle from x = 100..300, y = 100..200.
        s.execute(
            "draw.rect",
            &json!({"x": 100.0, "y": 100.0, "w": 200.0, "h": 100.0, "fill": "#ff4422", "stroke": null}),
        )
        .unwrap();
        // Drawing object inside x = 120..160, y = 260..300 (inside lasso) and another far away.
        let inside_obj = s
            .execute(
                "draw.rect",
                &json!({"x": 120.0, "y": 260.0, "w": 40.0, "h": 40.0, "fill": "#2288ff", "stroke": null, "object": true}),
            )
            .unwrap()["element"]
            .as_u64()
            .unwrap();
        let outside_obj = s
            .execute(
                "draw.rect",
                &json!({"x": 500.0, "y": 260.0, "w": 40.0, "h": 40.0, "fill": "#22cc66", "stroke": null, "object": true}),
            )
            .unwrap()["element"]
            .as_u64()
            .unwrap();

        // Triangular/quad lasso covering x = 80..200, y = 80..320 (slices the raw rect down x = 200!).
        let res = s
            .execute(
                "raw.select_lasso",
                &json!({
                    "points": [[80.0, 80.0], [200.0, 80.0], [200.0, 320.0], [80.0, 320.0]]
                }),
            )
            .unwrap();
        assert!(res["cut"].as_u64().unwrap() >= 1);
        let selected: Vec<u64> = res["selected"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_u64)
            .collect();
        assert!(selected.contains(&inside_obj));
        assert!(!selected.contains(&outside_obj));
        // Raw rectangle was sliced into 2 raw pieces + 2 objects = 4 elements total on the layer.
        assert_eq!(s.visible_elements().len(), 4);
    }

    #[test]
    fn magic_wand_respects_tolerance_and_all_layers() {
        let mut s = Session::new(anicraft_doc::Document::default());
        // Three shapes with slightly varying reds + one blue shape.
        let a = s
            .execute(
                "draw.rect",
                &json!({"x": 40.0, "y": 40.0, "w": 50.0, "h": 50.0, "fill": "#ff0000", "stroke": null, "object": true}),
            )
            .unwrap()["element"]
            .as_u64()
            .unwrap();
        let b = s
            .execute(
                "draw.rect",
                &json!({"x": 120.0, "y": 40.0, "w": 50.0, "h": 50.0, "fill": "#f00000", "stroke": null, "object": true}),
            )
            .unwrap()["element"]
            .as_u64()
            .unwrap();
        let c = s
            .execute(
                "draw.rect",
                &json!({"x": 200.0, "y": 40.0, "w": 50.0, "h": 50.0, "fill": "#0044ff", "stroke": null, "object": true}),
            )
            .unwrap()["element"]
            .as_u64()
            .unwrap();

        // Add a second layer with a matching red shape.
        s.execute("layer.new", &json!({"name": "Top"})).unwrap();
        let top_red = s
            .execute(
                "draw.rect",
                &json!({"x": 300.0, "y": 40.0, "w": 50.0, "h": 50.0, "fill": "#fa0000", "stroke": null, "object": true}),
            )
            .unwrap()["element"]
            .as_u64()
            .unwrap();
        // Switch back to bottom layer (index 1).
        s.execute("layer.select", &json!({"index": 1})).unwrap();

        // Exact tolerance 0 at (65, 65) selects only `a`.
        let r0 = s
            .execute(
                "raw.magic_wand",
                &json!({"x": 65.0, "y": 65.0, "tolerance": 0.0}),
            )
            .unwrap();
        let sel0: Vec<u64> = r0["selected"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_u64)
            .collect();
        assert_eq!(sel0, vec![a]);

        // Tolerance 20 selects `a` and `b` (diff = 15), not `c` and not `top_red` (all_layers=false).
        let r20 = s
            .execute(
                "raw.magic_wand",
                &json!({"x": 65.0, "y": 65.0, "tolerance": 20.0}),
            )
            .unwrap();
        let sel20: Vec<u64> = r20["selected"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_u64)
            .collect();
        assert!(sel20.contains(&a) && sel20.contains(&b));
        assert!(!sel20.contains(&c) && !sel20.contains(&top_red));

        // With `all_layers: true`, `top_red` is also selected!
        let rall = s
            .execute(
                "raw.magic_wand",
                &json!({"x": 65.0, "y": 65.0, "tolerance": 20.0, "all_layers": true}),
            )
            .unwrap();
        let sel_all: Vec<u64> = rall["selected"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_u64)
            .collect();
        assert!(sel_all.contains(&a) && sel_all.contains(&b) && sel_all.contains(&top_red));
        assert!(!sel_all.contains(&c));
    }

    #[test]
    fn shape_smooth_straighten_and_optimize_reduce_anchors() {
        let mut s = Session::new(anicraft_doc::Document::default());
        // Build a noisy 50-vertex semicircle polyline.
        let mut pts = Vec::new();
        for i in 0..=50 {
            let t = i as f32 / 50.0 * std::f32::consts::PI;
            let jitter = if i % 2 == 0 { 0.35 } else { -0.35 };
            pts.push([
                200.0 + (100.0 + jitter) * t.cos(),
                200.0 + (100.0 + jitter) * t.sin(),
            ]);
        }
        let d = pts
            .iter()
            .enumerate()
            .map(|(i, [x, y])| {
                if i == 0 {
                    format!("M{x} {y}")
                } else {
                    format!("L{x} {y}")
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        s.execute(
            "draw.path",
            &json!({"d": d, "stroke": "#112233", "stroke_width": 4.0, "fill": null, "object": true}),
        )
        .unwrap();

        let smooth = s
            .execute("modify.smooth_shape", &json!({"strength": 60.0}))
            .unwrap();
        assert_eq!(smooth["shapes"].as_u64(), Some(1));

        let opt = s
            .execute("modify.optimize_shape", &json!({"tolerance": 3.0}))
            .unwrap();
        let before = opt["before_anchors"].as_u64().unwrap();
        let after = opt["after_anchors"].as_u64().unwrap();
        assert!(
            after < before && after <= 10,
            "expected substantial anchor reduction: before={before}, after={after}"
        );

        // Almost-axis-aligned rectangle straightened cleanly.
        s.execute("edit.select_all", &json!({})).unwrap();
        s.execute("edit.delete", &json!({})).unwrap();
        s.execute(
            "draw.path",
            &json!({
                "d": "M100 100 C130 101 170 99 200 101 L201 200 L100 199 Z",
                "fill": "#44aa88",
                "stroke": null,
                "object": true
            }),
        )
        .unwrap();
        let st = s
            .execute("modify.straighten_shape", &json!({"tolerance": 5.0}))
            .unwrap();
        assert_eq!(st["shapes"].as_u64(), Some(1));
    }

    #[test]
    fn distort_applies_projective_homography() {
        let mut s = Session::new(anicraft_doc::Document::default());
        s.execute(
            "draw.rect",
            &json!({"x": 100.0, "y": 100.0, "w": 200.0, "h": 100.0, "fill": "#ee5533", "stroke": null, "object": true}),
        )
        .unwrap();
        let corners = [
            [130.0, 90.0],
            [270.0, 90.0],
            [340.0, 220.0],
            [60.0, 220.0],
        ];
        let res = s
            .execute("modify.distort", &json!({"corners": corners}))
            .unwrap();
        assert_eq!(res["shapes"].as_u64(), Some(1));
        let b = s.selection_bounds();
        assert!((b.min.x - 60.0).abs() < 1.0);
        assert!((b.max.x - 340.0).abs() < 1.0);
        assert!((b.min.y - 90.0).abs() < 1.0);
        assert!((b.max.y - 220.0).abs() < 1.0);
    }

    #[test]
    fn gradient_and_bitmap_strokes_render_expected_pixels() {
        let mut s = Session::new(anicraft_doc::Document::default());
        s.doc.stage.width = 240;
        s.doc.stage.height = 120;
        s.doc.stage.background = Rgba::BLACK;

        // Draw a thick horizontal line from x = 30 to x = 210 at y = 60.
        s.execute(
            "draw.line",
            &json!({"x0": 30.0, "y0": 60.0, "x1": 210.0, "y1": 60.0, "stroke": "#ffffff", "stroke_width": 28.0, "object": true}),
        )
        .unwrap();
        // Set a linear gradient stroke from pure red (#ff0000) on the left to pure blue (#0000ff) on the right.
        let r = s
            .execute(
                "element.set_stroke_paint",
                &json!({
                    "type": "linear",
                    "stops": [
                        {"offset": 0.0, "color": "#ff0000"},
                        {"offset": 1.0, "color": "#0000ff"}
                    ]
                }),
            )
            .unwrap();
        assert_eq!(r["changed"].as_u64(), Some(1));
        assert_eq!(r["stroke_paint"]["type"], "linear");

        let pm = render_frame(&s.doc, 0, 1.0).unwrap();
        let px = pm.to_rgba8();
        let pixel_at = |x: u32, y: u32| -> [u8; 4] {
            let idx = ((y * 240 + x) * 4) as usize;
            [px[idx], px[idx + 1], px[idx + 2], px[idx + 3]]
        };
        let left = pixel_at(40, 60);
        let right = pixel_at(200, 60);
        assert!(
            left[0] > 200 && left[2] < 60,
            "expected red-dominant left stroke pixel, got {left:?}"
        );
        assert!(
            right[2] > 200 && right[0] < 60,
            "expected blue-dominant right stroke pixel, got {right:?}"
        );

        // Verify bitmap stroke rendering as well.
        let bmp_id = BitmapId(s.doc.alloc_id());
        let png = encode_png(4, 4, &[0u8, 255, 0, 255].repeat(16)).unwrap();
        s.doc
            .bitmaps
            .push(Arc::new(Bitmap::from_png(bmp_id, "GreenTex.png", png).unwrap()));
        s.execute(
            "element.set_stroke_paint",
            &json!({"type": "bitmap", "bitmap": bmp_id.0}),
        )
        .unwrap();
        let pm_bmp = render_frame(&s.doc, 0, 1.0).unwrap();
        let px_bmp = pm_bmp.to_rgba8();
        let mid_idx = ((60 * 240 + 120) * 4) as usize;
        assert!(
            px_bmp[mid_idx + 1] > 220 && px_bmp[mid_idx] < 20,
            "expected green bitmap stroke pixel at midpoint"
        );
    }

    #[test]
    fn art_brush_warps_element_and_symbol_along_curved_spine() {
        let mut s = Session::new(anicraft_doc::Document::default());
        // Create a horizontal capsule/rectangle source element (width 100, height 20).
        let src_id = s
            .execute(
                "draw.rect",
                &json!({"x": 0.0, "y": -10.0, "w": 100.0, "h": 20.0, "fill": "#ff8800", "stroke": "#221100", "stroke_width": 2.0, "object": true}),
            )
            .unwrap()["element"]
            .as_u64()
            .unwrap();

        // Curved spine arc from (100, 300) through (250, 180) to (400, 300).
        let spine = [[100.0, 300.0], [175.0, 210.0], [250.0, 180.0], [325.0, 210.0], [400.0, 300.0]];
        let res = s
            .execute(
                "draw.art_brush",
                &json!({"source": src_id, "spine": spine, "width_scale": 1.2}),
            )
            .unwrap();
        assert_eq!(res["shapes"].as_u64(), Some(1));
        let b = s.selection_bounds();
        assert!(b.min.x < 115.0 && b.max.x > 385.0);
        assert!(b.min.y < 195.0 && b.max.y > 290.0);

        // Convert the source into a 2-shape Symbol and apply `draw.art_brush` by SymbolId!
        let extra = s
            .execute(
                "draw.ellipse",
                &json!({"x": 40.0, "y": -6.0, "w": 20.0, "h": 12.0, "fill": "#ffffff", "stroke": null, "object": true}),
            )
            .unwrap()["element"]
            .as_u64()
            .unwrap();
        s.execute("element.select", &json!({"ids": [src_id, extra]}))
            .unwrap();
        let sym_id = s
            .execute("modify.convert_to_symbol", &json!({"name": "BrushArt"}))
            .unwrap()["symbol"]
            .as_u64()
            .unwrap();
        let sym_res = s
            .execute(
                "draw.art_brush",
                &json!({"source": sym_id, "spine": [[80.0, 420.0], [240.0, 380.0], [420.0, 420.0]], "width_scale": 1.0}),
            )
            .unwrap();
        assert_eq!(sym_res["shapes"].as_u64(), Some(2));
    }
}
