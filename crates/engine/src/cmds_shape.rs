//! Modify › Shape (ADR 0109): Convert Lines to Fills, Expand Fill, Soften Fill Edges, Smooth,
//! Straighten and Optimize Curves, and the colour under a point as it is drawn (the colour
//! picker's eyedropper).
//!
//! The geometry is `anicraft_geom::outline` and `anicraft_geom::curve`: a stroke's outline built
//! from its curves with the renderer's caps, joins, dashes and width profile, a fill's outline
//! moved out or in by a distance, and pure curve clean-ups (`smooth`, `straighten`, `optimize`).
//! It is worked out in the space of the keyframe (what the artist sees: a stroke's width, an
//! offset and a fitting tolerance are stage distances however the shape is scaled or skewed) and
//! stored back in the shape's own space, so its transform and a gradient's placing stay as they
//! were. Raw shapes are merged again with the drawing they are part of (ADR 0031); drawing
//! objects stay objects.

use crate::cmds_raw::{check_extent, remerge_in_place, remerge_in_place_mapped};
use crate::cmds_width::id_param;
use crate::params::*;
use crate::{CommandSpec, EngineError, Result, Session};
use anicraft_doc::{Cap, Element, ElementId, ElementKind, Join, Paint, Shape, Stroke};
use anicraft_geom::outline::{self, LineCap, LineJoin, MAX_SOFTEN_STEPS, StrokeShape};
use anicraft_geom::{Affine, FillRule, Path, Vec2, curve};
use serde_json::{Value, json};
use std::sync::Arc;

/// The largest distance Expand Fill and Soften Fill Edges take, in stage units.
pub const MAX_DISTANCE: f32 = 1000.0;
/// Soften Fill Edges' steps when none are given (Animate's default).
pub const DEFAULT_STEPS: u32 = 4;
/// Default strength for Smooth, Straighten and Optimize Curves.
pub const DEFAULT_STRENGTH: f32 = 50.0;
/// Most outlines one command works out (shapes × the outlines each needs).
const MAX_OUTLINES: usize = 2000;
/// Most passes `modify.optimize` runs when `repeat` is true.
const MAX_OPTIMIZE_PASSES: usize = 8;

/// Maps Optimize Curves' strength (0..=100) to a fitting tolerance in stage units:
/// 0 at 0 (no change), otherwise `0.05 + 8 * (strength / 100)^2` (2.05 px at 50, 8.05 px at 100).
pub fn optimize_tolerance(strength: f32) -> f32 {
    if !(strength.is_finite() && strength > 0.0) {
        return 0.0;
    }
    let t = strength.clamp(0.0, 100.0) / 100.0;
    0.05 + 8.0 * t * t
}

pub fn specs() -> Vec<CommandSpec> {
    let c = |id, label, menu, shortcut, params, mutates, run| CommandSpec { id, label, menu, shortcut, params, mutates, run };
    vec![
        c(
            "modify.lines_to_fills",
            "Convert Lines to Fills",
            "Modify/Shape",
            None,
            "{id? (default: the selected shapes)} each line becomes a fill of its exact outline in the line's colour: caps, joins, dashes and width profile included. A raw line's fill joins the drawing (it merges with the fills it touches); a drawing object keeps its fill and gets the outline as an object above it → {converted, elements: [ids]}",
            true,
            (|s, p| s.each_layer(false, |s| lines_to_fills(s, p))) as crate::RunFn,
        ),
        c(
            "modify.expand_fill",
            "Expand Fill…",
            "Modify/Shape",
            None,
            "{distance (stage units, above 0 up to 1000), direction?: expand|inset (default expand), id? (default: the selected shapes)} moves each fill's outline out or in by the distance: expanding rounds corners, insetting keeps them; holes shrink as the fill grows; a fill thinner than twice an inset is removed. A drawing object's line follows its new outline → {changed, removed}",
            true,
            |s, p| s.each_layer(false, |s| expand_fill(s, p)),
        ),
        c(
            "modify.soften_fill_edges",
            "Soften Fill Edges…",
            "Modify/Shape",
            None,
            "{distance (the soft edge's width in stage units, above 0 up to 1000), steps? (1..=50 bands, default 4), direction?: expand|inset (default expand: the edge is added round the fill; inset: taken from it), id? (default: the selected shapes)} surrounds each fill with bands of its own paint whose alpha steps down outwards: band i of n has (n + 1 - i) / (n + 1) of the fill's alpha. Colour and gradient fills only → {softened, bands}",
            true,
            |s, p| s.each_layer(false, |s| soften(s, p)),
        ),
        c(
            "modify.smooth",
            "Smooth",
            "Modify/Shape",
            None,
            "{strength? (1..=100, default 50), id? (default: the selected shapes)} softens the curves of the selected lines and fill outlines and evens out small bumps; repeatable → {changed}",
            true,
            |s, p| s.each_layer(false, |s| smooth_cmd(s, p)),
        ),
        c(
            "modify.straighten",
            "Straighten",
            "Modify/Shape",
            None,
            "{strength? (1..=100, default 50), id? (default: the selected shapes)} makes nearly straight segments into lines and flattens gentle curves on the selected lines and fill outlines; repeatable → {changed}",
            true,
            |s, p| s.each_layer(false, |s| straighten_cmd(s, p)),
        ),
        c(
            "modify.optimize",
            "Optimize Curves…",
            "Modify/Shape",
            None,
            "{strength? (0..=100, default 50; tolerance is 0 at 0, else 0.05 + 8 * (strength / 100)^2 stage units), repeat?: bool (run until the point count stops dropping), id? (default: the selected shapes)} removes points from the selected lines and fill outlines while keeping their shape → {before, after}",
            true,
            |s, p| s.each_layer(false, |s| optimize_cmd(s, p)),
        ),
        c(
            "paint.sample",
            "Sample Colour",
            "",
            None,
            "{x, y, scale? (the view's zoom, 0.01..=64, default 1: the colour of one screen pixel there)} the colour drawn at a point of what is being edited, over the stage colour: fills, gradients, bitmaps, lines, symbols and effects alike, as the CPU renderer draws them (the colour picker's eyedropper, #288) → {color: \"#rrggbb\", r, g, b}",
            false,
            sample,
        ),
    ]
}

// ----- the shapes a command changes ---------------------------------------------------------------

/// What a shape turns into: shapes bottom to top, the one marked `true` keeping the element's
/// id and name (at most one; none: the element goes).
type Made = Vec<(Shape, bool)>;

/// Runs `each` over the target shapes (param `id`, else the selection on the active layer) and
/// replaces every shape it answers for with what it made, in place in the stacking. `each`
/// gets the element, its shape and the keyframe-space → shape-space map; `Ok(None)`: this
/// command doesn't apply to that shape. Raw results are merged again with the drawing, and
/// what was made is selected. Returns the number of shapes changed and the ids made.
fn rewrite(s: &mut Session, p: &Value, nothing: &str, mut each: impl FnMut(&Element, &Shape, &Affine) -> Result<Option<Made>>) -> Result<(usize, Vec<ElementId>)> {
    let ids: Vec<ElementId> = match id_param(p)? {
        Some(id) => vec![id],
        None => s.selection.on(s.layer),
    };
    if ids.is_empty() {
        return Err(EngineError::State("nothing is selected".into()));
    }
    let here = s.layer;
    // The frame is keyed first (inside a tween), so the shapes are the ones that will change.
    let visible: Vec<ElementId> = s.visible_elements().iter().map(|e| e.id).collect();
    if let Some(id) = ids.iter().find(|id| !visible.contains(id)) {
        return Err(EngineError::State(format!("element {} is not on the active layer at this frame", id.0)));
    }
    let existing = s.editable_keyframe()?.elements.clone();
    let mut out: Vec<Arc<Element>> = Vec::with_capacity(existing.len());
    let (mut changed, mut made, mut raw, mut gone) = (0usize, Vec::new(), Vec::new(), Vec::new());
    for el in &existing {
        let result = match &el.kind {
            ElementKind::Shape(sh) if ids.contains(&el.id) && sh.warp.is_none() => match el.transform.inverse() {
                Some(inv) => each(el, sh, &inv)?,
                None => None,
            },
            _ => None,
        };
        let Some(shapes) = result else {
            out.push(el.clone());
            continue;
        };
        changed += 1;
        gone.push(el.id);
        for (shape, keeps) in shapes {
            check_extent(&shape.path)?;
            let id = if keeps { el.id } else { ElementId(s.doc.alloc_id()) };
            if shape.merge {
                raw.push(id);
            }
            made.push(id);
            let name = if keeps { el.name.clone() } else { String::new() };
            out.push(Arc::new(Element { id, name, transform: el.transform, color: el.color, kind: ElementKind::Shape(shape) }));
        }
    }
    if changed == 0 {
        return Err(EngineError::State(nothing.into()));
    }
    if out.len() > 100_000 {
        return Err(EngineError::State("too many elements on this keyframe".into()));
    }
    s.editable_keyframe()?.elements = out;
    s.selection.retain(|l, id| l != here || !gone.contains(&id));
    s.selection.extend_on(here, made.iter().copied());
    // Raw shapes join the drawing again: a new fill merges with the fills it touches.
    remerge_in_place(s, &raw)?;
    let present: Vec<ElementId> = s.visible_elements().iter().map(|e| e.id).collect();
    let selected: Vec<ElementId> = s.selection.on(here).into_iter().filter(|id| present.contains(id)).collect();
    Ok((changed, selected))
}

fn too_intricate(what: &str) -> EngineError {
    EngineError::State(format!("{what} is too intricate to work out at once. Try it on a smaller part of the artwork"))
}

// ----- Convert Lines to Fills ----------------------------------------------------------------------

/// The outline of stroke `st` along `path` as the renderer draws it under `m`, in `m`'s output
/// space: one fill of loops that don't overlap. Empty when the stroke draws nothing; `None`
/// when it is too much to outline at once.
pub fn stroke_outline(path: &Path, st: &Stroke, m: &Affine) -> Option<Path> {
    let scale = m.mean_scale();
    let hw = st.width * scale * 0.5;
    if !(hw.is_finite() && hw > 0.0 && m.is_finite()) {
        return Some(Path::new());
    }
    let style = &st.style;
    // Dash lengths are in the shape's units, like the width: they scale with it.
    let pattern = style.dash.as_ref().map(|d| {
        let (lengths, offset) = d.pattern(st.width);
        (lengths.into_iter().map(|x| x * scale).collect::<Vec<f32>>(), offset * scale)
    });
    let widths = st.profile.as_ref().map(|pr| move |t: f32| pr.at(t));
    let shape = StrokeShape {
        half_width: hw,
        cap: match style.effective_cap() {
            Cap::Butt => LineCap::Butt,
            Cap::Round => LineCap::Round,
            Cap::Square => LineCap::Square,
        },
        join: match style.join {
            Join::Miter => LineJoin::Miter,
            Join::Round => LineJoin::Round,
            Join::Bevel => LineJoin::Bevel,
        },
        miter_limit: style.miter_limit,
        dash: pattern.as_ref().map(|(l, o)| (l.as_slice(), *o)),
        widths: widths.as_ref().map(|f| f as &dyn Fn(f32) -> (f32, f32)),
        // A profile's positions are fractions of the length in the shape's own space.
        to_local: st.profile.as_ref().and_then(|_| m.inverse()),
        flip: st.profile.is_some() && m.determinant() < 0.0,
    };
    outline::stroke_outline(&path.transformed(m), &shape)
}

fn lines_to_fills(s: &mut Session, p: &Value) -> Result<Value> {
    let mut budget = MAX_OUTLINES;
    let (converted, made) = rewrite(s, p, "the selection has no lines to convert (select shapes with a stroke)", |el, sh, inv| {
        let Some(st) = &sh.stroke else { return Ok(None) };
        budget = budget.checked_sub(1).ok_or_else(|| EngineError::State(format!("that is more than {MAX_OUTLINES} lines at once. Convert them a part at a time")))?;
        let outline = stroke_outline(&sh.path, st, &el.transform).ok_or_else(|| too_intricate("This line"))?;
        if outline.is_empty() {
            // A line that draws nothing (no width) has no outline to keep.
            return Ok(None);
        }
        let fill = Shape { path: Arc::new(outline.transformed(inv)), fill: Some(Paint::Solid(st.color)), stroke: None, fill_rule: FillRule::EvenOdd, merge: sh.merge, warp: None };
        Ok(Some(match &sh.fill {
            // The shape keeps its fill; the line's outline lies over it, as the line did.
            Some(_) => vec![(Shape { stroke: None, ..sh.clone() }, true), (fill, false)],
            None => vec![(fill, true)],
        }))
    })?;
    Ok(json!({"converted": converted, "elements": made.iter().map(|e| e.0).collect::<Vec<u64>>()}))
}

// ----- Expand Fill and Soften Fill Edges ----------------------------------------------------------

fn distance_param(p: &Value) -> Result<f32> {
    let d = f32_req(p, "distance")?;
    if !(d > 0.0 && d <= MAX_DISTANCE) {
        return Err(EngineError::Param(format!("param 'distance' must be above 0 and at most {MAX_DISTANCE}")));
    }
    Ok(d)
}

/// Param `direction`: whether the fill shrinks.
fn inset_param(p: &Value) -> Result<bool> {
    match opt_str(p, "direction")? {
        None | Some("expand") => Ok(false),
        Some("inset") => Ok(true),
        Some(other) => Err(EngineError::Param(format!("param 'direction' must be expand or inset, not '{other}'"))),
    }
}

fn expand_fill(s: &mut Session, p: &Value) -> Result<Value> {
    let distance = distance_param(p)?;
    let inset = inset_param(p)?;
    let (mut budget, mut removed) = (MAX_OUTLINES, 0usize);
    let (changed, _) = rewrite(s, p, "the selection has no fills to expand (select shapes with a fill)", |el, sh, inv| {
        if sh.fill.is_none() {
            return Ok(None);
        }
        budget = budget.checked_sub(1).ok_or_else(|| EngineError::State(format!("that is more than {MAX_OUTLINES} fills at once. Expand them a part at a time")))?;
        let stage = sh.path.transformed(&el.transform);
        let moved = outline::offset(&stage, sh.fill_rule, if inset { -distance } else { distance }).ok_or_else(|| too_intricate("This fill"))?;
        if moved.is_empty() {
            // Nothing is left of the fill; a line it had stays where it was.
            removed += 1;
            return Ok(Some(match &sh.stroke {
                Some(_) => vec![(Shape { fill: None, ..sh.clone() }, true)],
                None => Vec::new(),
            }));
        }
        Ok(Some(vec![(Shape { path: Arc::new(moved.transformed(inv)), fill_rule: FillRule::EvenOdd, ..sh.clone() }, true)]))
    })?;
    Ok(json!({"changed": changed - removed, "removed": removed}))
}

/// `paint` with `k` of its alpha (each stop of a gradient); `None` for a bitmap, which has none.
fn faded(paint: &Paint, k: f32) -> Option<Paint> {
    let mut out = paint.clone();
    match &mut out {
        Paint::Solid(c) => c.a *= k,
        Paint::Linear { stops, .. } | Paint::Radial { stops, .. } => stops.iter_mut().for_each(|st| st.color.a *= k),
        Paint::Bitmap { .. } => return None,
    }
    Some(out)
}

/// The share of the fill's alpha band `i` of `steps` has (1 is next to the fill).
pub fn band_alpha(i: u32, steps: u32) -> f32 {
    (steps + 1).saturating_sub(i) as f32 / (steps + 1) as f32
}

fn soften(s: &mut Session, p: &Value) -> Result<Value> {
    let distance = distance_param(p)?;
    let inset = inset_param(p)?;
    let steps = opt_u32(p, "steps")?.unwrap_or(DEFAULT_STEPS);
    if !(1..=MAX_SOFTEN_STEPS).contains(&steps) {
        return Err(EngineError::Param(format!("param 'steps' must be between 1 and {MAX_SOFTEN_STEPS}")));
    }
    let (mut budget, mut bands) = (MAX_OUTLINES, 0usize);
    let (softened, _) = rewrite(s, p, "the selection has no colour or gradient fills to soften (a bitmap fill has no alpha to fade)", |el, sh, inv| {
        let Some(fill) = sh.fill.as_ref().filter(|f| !matches!(f, Paint::Bitmap { .. })) else { return Ok(None) };
        budget = budget.checked_sub(steps as usize + 1).ok_or_else(|| EngineError::State(format!("that is more than {MAX_OUTLINES} outlines at once. Soften fewer fills, or use fewer steps")))?;
        let stage = sh.path.transformed(&el.transform);
        let parts = outline::soften(&stage, sh.fill_rule, distance, steps, inset).ok_or_else(|| too_intricate("This fill"))?;
        let mut parts = parts.into_iter();
        let core = parts.next().unwrap_or_default();
        // Bands lie under the fill, the faintest lowest (they don't overlap, so only a line
        // the shape keeps cares).
        let mut made: Made = Vec::new();
        for (i, band) in parts.enumerate() {
            let Some(paint) = faded(fill, band_alpha(i as u32 + 1, steps)) else { continue };
            if !band.is_empty() {
                made.push((Shape { path: Arc::new(band.transformed(inv)), fill: Some(paint), stroke: None, fill_rule: FillRule::EvenOdd, merge: sh.merge, warp: None }, false));
            }
        }
        made.reverse();
        bands += made.len();
        if !core.is_empty() {
            made.push((Shape { path: Arc::new(core.transformed(inv)), fill_rule: FillRule::EvenOdd, ..sh.clone() }, true));
        } else if sh.stroke.is_some() {
            made.push((Shape { fill: None, ..sh.clone() }, true));
        }
        Ok(Some(made))
    })?;
    Ok(json!({"softened": softened, "bands": bands}))
}

// ----- Smooth, Straighten and Optimize Curves -----------------------------------------------------

fn strength_param(p: &Value, min: f32) -> Result<f32> {
    let v = opt_f32(p, "strength")?.unwrap_or(DEFAULT_STRENGTH);
    if !(min..=100.0).contains(&v) {
        return Err(EngineError::Param(format!("param 'strength' must be between {min} and 100")));
    }
    Ok(v)
}

/// Runs a stage-space path clean-up over the target shapes (`id`, else the selection on the active
/// layer). Shapes that cannot change are left as they are (no error); raw shapes that do change
/// merge again with the drawing. When `noop` is true, validates the selection and returns equal
/// point counts without charging the shape budget. Returns `(changed, before_points, after_points)`.
fn rewrite_paths(s: &mut Session, p: &Value, nothing: &str, noop: bool, mut op: impl FnMut(&Path) -> Path) -> Result<(usize, usize, usize)> {
    let ids: Vec<ElementId> = match id_param(p)? {
        Some(id) => vec![id],
        None => s.selection.on(s.layer),
    };
    if ids.is_empty() {
        return Err(EngineError::State("nothing is selected".into()));
    }
    let visible = s.visible_elements();
    let visible_ids: Vec<ElementId> = visible.iter().map(|e| e.id).collect();
    if let Some(id) = ids.iter().find(|id| !visible_ids.contains(id)) {
        return Err(EngineError::State(format!("element {} is not on the active layer at this frame", id.0)));
    }
    if !visible.iter().any(|e| ids.contains(&e.id) && matches!(e.kind, ElementKind::Shape(_))) {
        return Err(EngineError::State(nothing.into()));
    }
    if noop {
        let pts: usize = visible
            .iter()
            .filter(|e| ids.contains(&e.id))
            .filter_map(|e| match &e.kind {
                ElementKind::Shape(sh) => Some(sh.path.editable_anchors().len()),
                _ => None,
            })
            .sum();
        return Ok((0, pts, pts));
    }
    let mut budget = MAX_OUTLINES;
    let mut changed = 0usize;
    let (mut before_pts, mut after_pts) = (0usize, 0usize);
    let mut edits: Vec<(ElementId, Path, bool)> = Vec::new();
    for el in &visible {
        let ElementKind::Shape(sh) = &el.kind else { continue };
        if !ids.contains(&el.id) {
            continue;
        }
        let b = sh.path.editable_anchors().len();
        before_pts += b;
        if sh.warp.is_some() || sh.path.is_empty() {
            after_pts += b;
            continue;
        }
        let Some(inv) = el.transform.inverse() else {
            after_pts += b;
            continue;
        };
        budget = budget.checked_sub(1).ok_or_else(|| EngineError::State(format!("that is more than {MAX_OUTLINES} shapes at once. Edit them a part at a time")))?;
        let stage = sh.path.transformed(&el.transform);
        let next_stage = op(&stage);
        if next_stage.is_empty() || next_stage == stage {
            after_pts += b;
            continue;
        }
        let next_local = next_stage.transformed(&inv);
        check_extent(&next_local)?;
        after_pts += next_local.editable_anchors().len();
        changed += 1;
        edits.push((el.id, next_local, sh.merge));
    }
    if changed == 0 {
        return Ok((0, before_pts, after_pts));
    }
    let existing = s.editable_keyframe()?.elements.clone();
    let mut out: Vec<Arc<Element>> = Vec::with_capacity(existing.len());
    let mut raw: Vec<ElementId> = Vec::new();
    for el in &existing {
        if let Some((_, path, merge)) = edits.iter().find(|(id, ..)| *id == el.id)
            && let ElementKind::Shape(sh) = &el.kind
        {
            let shape = Shape { path: Arc::new(path.clone()), ..sh.clone() };
            if *merge {
                raw.push(el.id);
            }
            out.push(Arc::new(Element { id: el.id, name: el.name.clone(), transform: el.transform, color: el.color, kind: ElementKind::Shape(shape) }));
        } else {
            out.push(el.clone());
        }
    }
    s.editable_keyframe()?.elements = out;
    if !raw.is_empty() {
        let map = remerge_in_place_mapped(s, &raw)?;
        let mut committed: Vec<ElementId> = ids.iter().copied().filter(|id| !map.gone.contains(id)).collect();
        committed.extend(map.from.iter().filter(|(_, src)| ids.contains(src)).map(|(piece, _)| *piece));
        after_pts = s
            .editable_keyframe()?
            .elements
            .iter()
            .filter(|e| committed.contains(&e.id))
            .filter_map(|e| match &e.kind {
                ElementKind::Shape(sh) => Some(sh.path.editable_anchors().len()),
                _ => None,
            })
            .sum();
    }
    Ok((changed, before_pts, after_pts))
}

fn smooth_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let strength = strength_param(p, 1.0)? / 100.0;
    let (changed, _, _) = rewrite_paths(s, p, "the selection has no shapes to smooth (select lines or fills, or break a symbol apart)", false, |path| curve::smooth(path, strength))?;
    Ok(json!({"changed": changed}))
}

fn straighten_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let strength = strength_param(p, 1.0)? / 100.0;
    let (changed, _, _) = rewrite_paths(s, p, "the selection has no shapes to straighten (select lines or fills, or break a symbol apart)", false, |path| curve::straighten(path, strength))?;
    Ok(json!({"changed": changed}))
}

fn optimize_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let strength = strength_param(p, 0.0)?;
    let repeat = opt_bool(p, "repeat")?.unwrap_or(false);
    let tol = optimize_tolerance(strength);
    let (_, before, after) = rewrite_paths(s, p, "the selection has no shapes to optimize (select lines or fills, or break a symbol apart)", tol <= 0.0, |path| {
        let mut cur = curve::optimize(path, tol);
        if repeat {
            for _ in 1..MAX_OPTIMIZE_PASSES {
                let prev_pts = cur.editable_anchors().len();
                let next = curve::optimize(&cur, tol);
                if next.editable_anchors().len() >= prev_pts || next == cur {
                    break;
                }
                cur = next;
            }
        }
        cur
    })?;
    Ok(json!({"before": before, "after": after}))
}

// ----- the colour at a point -----------------------------------------------------------------------

fn sample(s: &mut Session, p: &Value) -> Result<Value> {
    let at = Vec2::new(f32_req(p, "x")?, f32_req(p, "y")?);
    let scale = opt_f32(p, "scale")?.unwrap_or(1.0);
    if !(0.01..=64.0).contains(&scale) {
        return Err(EngineError::Param("param 'scale' must be between 0.01 and 64".into()));
    }
    if at.x.abs() > 1e7 || at.y.abs() > 1e7 {
        return Err(EngineError::Param("the point is too far from the stage".into()));
    }
    let [r, g, b] = sample_at(s, at, scale)?;
    Ok(json!({"color": format!("#{r:02x}{g:02x}{b:02x}"), "r": r, "g": g, "b": b}))
}

/// The colour of the screen pixel centred on `at` (of what is being edited) at zoom `scale`,
/// over the stage colour, as the CPU renderer draws it.
pub fn sample_at(s: &Session, at: Vec2, scale: f32) -> Result<[u8; 3]> {
    let view = anicraft_render::View { transform: Affine::translate(0.5 - at.x * scale, 0.5 - at.y * scale).mul(&Affine::scale(scale, scale)), width: 1, height: 1, background: Some(s.doc.stage.background.with_alpha(1.0)) };
    let (pm, _) = anicraft_render::render(&s.display_list(), &view)?;
    match pm.to_rgba8().as_slice() {
        [r, g, b, ..] => Ok([*r, *g, *b]),
        _ => Err(EngineError::State("nothing was drawn there".into())),
    }
}

#[cfg(test)]
#[path = "shape_cmd_tests.rs"]
mod tests;
