//! The Swatches panel's commands (ADR 0078): the document's saved colours and gradients.
//!
//! Swatches live in the document (`Document::swatches`), so every change here is an undo step
//! and is saved with the file, as Animate keeps them per document. Applying a swatch is
//! `paint.set_fill` (fill) or `element.set_stroke` (stroke) underneath, so the selection and
//! the tool colour change exactly as they do from the Colour panel.
//!
//! A swatch set moves between documents and computers as a small JSON file
//! (`swatch.export` / `swatch.import`); GIMP palettes (`.gpl`, an open plain-text format)
//! import too. Adobe's `.aco`, `.ase` and `.clr` formats are proprietary and not read.

use crate::params::*;
use crate::{CommandSpec, EngineError, Result, Session};
use anicraft_doc::swatch::{MAX_PALETTE_BYTES, clean_name, default_set, parse_gpl};
use anicraft_doc::{Document, Element, ElementKind, GradientStop, MAX_GROUP_DEPTH, MAX_SWATCHES, Paint, Rgba, Shape, Spread, Swatch, SwatchPaint, Timeline};
use anicraft_geom::{Rect, Vec2};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;

pub fn specs() -> Vec<CommandSpec> {
    let c = |id, label, params, mutates, run| CommandSpec { id, label, menu: "", shortcut: None, params, mutates, run };
    vec![
        c("swatch.list", "Swatches", "{} the document's swatches, in the Swatches panel's order → {swatches: [{index, name, type, color | stops, ..}], count}", false, list as crate::RunFn),
        c(
            "swatch.add",
            "Add Swatch",
            "{from?: fill|stroke (default fill: the fill swatch's colour or gradient; stroke: the stroke colour), or the swatch itself: type?: solid|linear|radial, color? (\"#rrggbb\" | \"#rrggbbaa\", solid), stops?: [{offset: 0..1, color}, ..] (2 to 15), spread?: extend|reflect|repeat, linear_rgb?: bool, focal?: -0.98..0.98 (radial); name?, at? (index; default: the end)} → {index, count}",
            true,
            add,
        ),
        c("swatch.remove", "Delete Swatch", "{index} → {count}", true, remove),
        c("swatch.move", "Move Swatch", "{from, to} (indices: the swatch at `from` ends up at `to`) → {index}", true, move_swatch),
        c("swatch.duplicate", "Duplicate Swatch", "{index} puts a copy right after it → {index, count}", true, duplicate),
        c("swatch.rename", "Rename Swatch", "{index, name} (empty: no name, the hex shows on hover)", true, rename),
        c(
            "swatch.set_tagged",
            "Tagged Swatch",
            "{index, tagged?: bool (default true), link_existing?: bool (default false)} turns a swatch into a Live Tagged Swatch (or untags it when false); with link_existing, shapes across scenes and Library symbols whose fill or stroke matches this swatch link to it → {index, tagged, tag_id, linked}",
            true,
            set_tagged,
        ),
        c(
            "swatch.edit",
            "Edit Swatch",
            "{index, type?: solid|linear|radial, color?, stops?, spread?, linear_rgb?: bool, focal?, name?} edits a swatch in place; when it is a Tagged Swatch, every shape linked to it across scenes and Library symbols updates to the new colour or gradient in one undo step → {index, tagged, tag_id, updated}",
            true,
            edit_swatch,
        ),
        c(
            "swatch.apply",
            "Apply Swatch",
            "{index, target?: fill|stroke (default fill), selection?: bool (default true)} sets the fill swatch (as paint.set_fill: a gradient is fitted to each shape) or the stroke colour, and unless `selection` is false the selected shapes' fills or strokes. A gradient swatch can't be a stroke (strokes are one colour) → {target, changed}",
            true,
            apply,
        ),
        c("swatch.clear", "Clear Swatches", "{} removes every swatch but black and white (Animate's Clear Colors; undoable)", true, clear),
        c("swatch.reset", "Default Swatches", "{} replaces the swatches with a new document's set (undoable)", true, reset),
        c("swatch.import", "Import Swatches", "{path: a swatch file (.json, from swatch.export) or a GIMP palette (.gpl), replace?: bool (default false: added at the end)} → {imported, count}", true, import),
        c("swatch.export", "Export Swatches", "{path} writes the document's swatches to a swatch file (JSON) → {written, swatches}", false, export),
    ]
}

/// `#rrggbb`, or `#rrggbbaa` when not opaque.
pub fn hex(c: Rgba) -> String {
    c.to_hex_alpha()
}

fn stops_json(stops: &[GradientStop]) -> Value {
    json!(stops.iter().map(|s| json!({"offset": s.offset, "color": hex(s.color)})).collect::<Vec<_>>())
}

/// A swatch as commands and swatch files write it.
pub fn swatch_json(s: &Swatch) -> Value {
    let mut v = match &s.paint {
        SwatchPaint::Solid { color } => json!({"type": "solid", "color": hex(*color)}),
        SwatchPaint::Linear { stops, spread, linear_rgb } => json!({"type": "linear", "stops": stops_json(stops), "spread": spread.key(), "linear_rgb": linear_rgb}),
        SwatchPaint::Radial { stops, spread, linear_rgb, focal } => json!({"type": "radial", "stops": stops_json(stops), "spread": spread.key(), "linear_rgb": linear_rgb, "focal": focal}),
    };
    if let Some(o) = v.as_object_mut() {
        if !s.name.is_empty() {
            o.insert("name".into(), json!(s.name));
        }
        if let Some(tid) = s.tag_id {
            o.insert("tagged".into(), json!(true));
            o.insert("tag_id".into(), json!(tid));
        }
    }
    v
}

/// A swatch from its JSON (see `swatch.add`). `None` when `v` names no paint (no type, colour
/// or stops), so the caller can fall back to the current one.
pub fn swatch_from_json(v: &Value) -> Result<Option<Swatch>> {
    let kind = opt_str(v, "type")?;
    let color = opt_color(v, "color")?;
    let stops = crate::cmds_paint::opt_stops(v)?;
    let spread = match opt_str(v, "spread")? {
        None => Spread::Pad,
        Some(k) => Spread::from_key(k).ok_or_else(|| EngineError::Param(format!("unknown spread '{k}' (extend, reflect or repeat)")))?,
    };
    let linear_rgb = opt_bool(v, "linear_rgb")?.unwrap_or(false);
    let focal = crate::cmds_paint::opt_focal(v)?.unwrap_or(0.0);
    let kind = match (kind, &color, &stops) {
        (None, None, None) => return Ok(None),
        (Some(k), ..) => k,
        (None, _, Some(_)) => "linear",
        (None, Some(_), None) => "solid",
    };
    let need_stops = || stops.clone().ok_or_else(|| EngineError::Param(format!("a {kind} swatch needs 'stops' (2 to 15 like {{\"offset\": 0, \"color\": \"#ff0000\"}})")));
    let paint = match kind {
        "solid" => SwatchPaint::Solid { color: color.ok_or_else(|| EngineError::Param("a solid swatch needs 'color' (like \"#ff8800\")".into()))? },
        "linear" => SwatchPaint::Linear { stops: need_stops()?, spread, linear_rgb },
        "radial" => SwatchPaint::Radial { stops: need_stops()?, spread, linear_rgb, focal },
        other => return Err(EngineError::Param(format!("unknown swatch type '{other}' (solid, linear or radial)"))),
    };
    let name = opt_str(v, "name")?.unwrap_or("");
    Swatch::new(name, paint).sanitized().map(Some).ok_or_else(|| EngineError::Param("that swatch can't be kept (a gradient needs two stops)".into()))
}

fn index_param(s: &Session, p: &Value, key: &str) -> Result<usize> {
    let n = s.doc.swatches.len();
    let i = opt_u64(p, key)?.ok_or_else(|| EngineError::Param(format!("param '{key}' is required (a swatch's index)")))?;
    usize::try_from(i).ok().filter(|i| *i < n).ok_or_else(|| if n == 0 { EngineError::State("the document has no swatches".into()) } else { EngineError::Param(format!("param '{key}' must be a swatch index, 0 to {}", n - 1)) })
}

fn swatches_mut(s: &mut Session) -> &mut Vec<Swatch> {
    Arc::make_mut(&mut s.doc.swatches)
}

fn list(s: &mut Session, _: &Value) -> Result<Value> {
    let v: Vec<Value> = s
        .doc
        .swatches
        .iter()
        .enumerate()
        .map(|(i, sw)| {
            let mut j = swatch_json(sw);
            if let Some(o) = j.as_object_mut() {
                o.insert("index".into(), json!(i));
            }
            j
        })
        .collect();
    Ok(json!({"count": v.len(), "swatches": v}))
}

/// The swatch `swatch.add` makes from the current colours: the fill swatch, or the stroke.
fn current(s: &Session, from: &str) -> Result<Swatch> {
    match from {
        "fill" => SwatchPaint::from_paint(&s.style.fill_paint()).map(|p| Swatch { name: String::new(), paint: p, tag_id: None }).ok_or_else(|| EngineError::State("a bitmap fill can't be a swatch (swatches hold colours and gradients)".into())),
        "stroke" => Ok(Swatch::solid(s.style.stroke)),
        other => Err(EngineError::Param(format!("param 'from' must be fill or stroke, not '{other}'"))),
    }
}

fn add(s: &mut Session, p: &Value) -> Result<Value> {
    if s.doc.swatches.len() >= MAX_SWATCHES {
        return Err(EngineError::State(format!("a document keeps at most {MAX_SWATCHES} swatches; delete some first")));
    }
    let mut sw = match (opt_str(p, "from")?, swatch_from_json(p)?) {
        (Some(_), Some(_)) => return Err(EngineError::Param("give either 'from' or the swatch's own type, colour or stops, not both".into())),
        (None, Some(sw)) => sw,
        (from, None) => current(s, from.unwrap_or("fill"))?,
    };
    if let Some(n) = opt_str(p, "name")? {
        sw.name = clean_name(n);
    }
    let n = s.doc.swatches.len();
    let at = match opt_u64(p, "at")? {
        None => n,
        Some(i) => usize::try_from(i).ok().filter(|i| *i <= n).ok_or_else(|| EngineError::Param(format!("param 'at' must be 0 to {n}")))?,
    };
    swatches_mut(s).insert(at, sw);
    Ok(json!({"index": at, "count": n + 1}))
}

fn remove(s: &mut Session, p: &Value) -> Result<Value> {
    let i = index_param(s, p, "index")?;
    let old_tag = swatches_mut(s).remove(i).tag_id;
    let count = s.doc.swatches.len();
    if let Some(tid) = old_tag {
        let _ = clear_tag_across_doc(&mut s.doc, tid);
        if s.style.fill_tag == Some(tid) {
            s.style.fill_tag = None;
        }
        if s.style.stroke_tag == Some(tid) {
            s.style.stroke_tag = None;
        }
    }
    Ok(json!({"count": count}))
}

fn move_swatch(s: &mut Session, p: &Value) -> Result<Value> {
    let from = index_param(s, p, "from")?;
    let to = index_param(s, p, "to")?;
    if from != to {
        let v = swatches_mut(s);
        let sw = v.remove(from);
        v.insert(to, sw);
    }
    Ok(json!({"index": to}))
}

fn duplicate(s: &mut Session, p: &Value) -> Result<Value> {
    let i = index_param(s, p, "index")?;
    if s.doc.swatches.len() >= MAX_SWATCHES {
        return Err(EngineError::State(format!("a document keeps at most {MAX_SWATCHES} swatches; delete some first")));
    }
    let v = swatches_mut(s);
    let mut copy = v.get(i).cloned().ok_or_else(|| EngineError::Param("no such swatch".into()))?;
    copy.tag_id = None;
    v.insert(i + 1, copy);
    Ok(json!({"index": i + 1, "count": v.len()}))
}

fn rename(s: &mut Session, p: &Value) -> Result<Value> {
    let i = index_param(s, p, "index")?;
    let name = clean_name(str_req(p, "name")?);
    if let Some(sw) = swatches_mut(s).get_mut(i) {
        sw.name = name.clone();
    }
    Ok(json!({"index": i, "name": name}))
}

fn set_tagged(s: &mut Session, p: &Value) -> Result<Value> {
    let i = index_param(s, p, "index")?;
    let tagged = opt_bool(p, "tagged")?.unwrap_or(true);
    let link_existing = opt_bool(p, "link_existing")?.unwrap_or(false);
    if tagged {
        let tid = match s.doc.swatches.get(i).and_then(|sw| sw.tag_id) {
            Some(id) => id,
            None => {
                let id = s.doc.alloc_id();
                if let Some(sw) = swatches_mut(s).get_mut(i) {
                    sw.tag_id = Some(id);
                }
                id
            }
        };
        let paint = s.doc.swatches.get(i).map(|sw| sw.paint.clone()).ok_or_else(|| EngineError::Param("no such swatch".into()))?;
        let linked = if link_existing { link_matching_shapes(&mut s.doc, &paint, tid) } else { 0 };
        Ok(json!({"index": i, "tagged": true, "tag_id": tid, "linked": linked}))
    } else {
        let old = swatches_mut(s).get_mut(i).and_then(|sw| sw.tag_id.take());
        let mut unlinked = 0u64;
        if let Some(tid) = old {
            unlinked = clear_tag_across_doc(&mut s.doc, tid);
            if s.style.fill_tag == Some(tid) {
                s.style.fill_tag = None;
            }
            if s.style.stroke_tag == Some(tid) {
                s.style.stroke_tag = None;
            }
        }
        Ok(json!({"index": i, "tagged": false, "tag_id": Value::Null, "linked": unlinked}))
    }
}

fn edit_swatch(s: &mut Session, p: &Value) -> Result<Value> {
    let i = index_param(s, p, "index")?;
    let cur = s.doc.swatches.get(i).cloned().ok_or_else(|| EngineError::Param("no such swatch".into()))?;
    let kind_opt = opt_str(p, "type")?;
    let color_opt = opt_color(p, "color")?;
    let stops_opt = crate::cmds_paint::opt_stops(p)?;
    let spread_opt = match opt_str(p, "spread")? {
        None => None,
        Some(k) => Some(Spread::from_key(k).ok_or_else(|| EngineError::Param(format!("unknown spread '{k}' (extend, reflect or repeat)")))?),
    };
    let linear_opt = opt_bool(p, "linear_rgb")?;
    let focal_opt = crate::cmds_paint::opt_focal(p)?;
    let name_opt = opt_str(p, "name")?;
    if kind_opt.is_none()
        && color_opt.is_none()
        && stops_opt.is_none()
        && spread_opt.is_none()
        && linear_opt.is_none()
        && focal_opt.is_none()
        && name_opt.is_none()
    {
        return Err(EngineError::Param("give a swatch field to edit ('color', 'stops', 'type', 'spread', 'linear_rgb', 'focal' or 'name')".into()));
    }
    let target_kind = match (kind_opt, &color_opt, &stops_opt) {
        (Some(k), ..) => k,
        (None, Some(_), None) => "solid",
        (None, _, Some(_)) if !cur.paint.is_gradient() => "linear",
        (None, ..) => cur.paint.kind(),
    };
    let cur_spread = match &cur.paint {
        SwatchPaint::Linear { spread, .. } | SwatchPaint::Radial { spread, .. } => *spread,
        SwatchPaint::Solid { .. } => Spread::Pad,
    };
    let cur_linear = match &cur.paint {
        SwatchPaint::Linear { linear_rgb, .. } | SwatchPaint::Radial { linear_rgb, .. } => *linear_rgb,
        SwatchPaint::Solid { .. } => false,
    };
    let cur_focal = match &cur.paint {
        SwatchPaint::Radial { focal, .. } => *focal,
        _ => 0.0,
    };
    let default_ramp = || {
        let c = cur.paint.primary_color();
        vec![GradientStop { offset: 0.0, color: c }, GradientStop { offset: 1.0, color: Rgba::BLACK }]
    };
    let raw_paint = match target_kind {
        "solid" => SwatchPaint::Solid { color: color_opt.unwrap_or_else(|| cur.paint.primary_color()) },
        "linear" => SwatchPaint::Linear {
            stops: stops_opt.or_else(|| cur.paint.stops().map(<[GradientStop]>::to_vec)).unwrap_or_else(default_ramp),
            spread: spread_opt.unwrap_or(cur_spread),
            linear_rgb: linear_opt.unwrap_or(cur_linear),
        },
        "radial" => SwatchPaint::Radial {
            stops: stops_opt.or_else(|| cur.paint.stops().map(<[GradientStop]>::to_vec)).unwrap_or_else(default_ramp),
            spread: spread_opt.unwrap_or(cur_spread),
            linear_rgb: linear_opt.unwrap_or(cur_linear),
            focal: focal_opt.unwrap_or(cur_focal),
        },
        other => return Err(EngineError::Param(format!("unknown swatch type '{other}' (solid, linear or radial)"))),
    };
    let new_paint = raw_paint.sanitized().ok_or_else(|| EngineError::Param("that swatch can't be kept (a gradient needs two stops)".into()))?;
    let new_name = name_opt.map_or_else(|| cur.name.clone(), clean_name);
    let updated_swatch = Swatch { name: new_name, paint: new_paint.clone(), tag_id: cur.tag_id };
    if let Some(slot) = swatches_mut(s).get_mut(i) {
        *slot = updated_swatch.clone();
    }
    let mut updated = 0u64;
    if let Some(tid) = cur.tag_id {
        updated = propagate_tagged_paint(&mut s.doc, tid, &new_paint);
        if s.style.fill_tag == Some(tid) {
            let stage = Rect::new(0.0, 0.0, s.doc.stage.width as f32, s.doc.stage.height as f32);
            let old_tool = s.style.fill_paint();
            let placed = apply_swatch_to_paint(&new_paint, &old_tool, &stage);
            s.style.fill = placed.primary_color();
            s.style.fill_paint = (!matches!(placed, Paint::Solid(_))).then_some(placed);
        }
        if s.style.stroke_tag == Some(tid) {
            s.style.stroke = new_paint.primary_color();
        }
    }
    Ok(json!({
        "index": i,
        "tagged": cur.tag_id.is_some(),
        "tag_id": cur.tag_id,
        "updated": updated,
        "swatch": swatch_json(&updated_swatch),
    }))
}

fn apply_swatch_to_paint(sw: &SwatchPaint, old: &Paint, bounds: &Rect) -> Paint {
    match sw {
        SwatchPaint::Solid { color } => Paint::Solid(*color),
        SwatchPaint::Linear { stops, spread, linear_rgb } => match old {
            Paint::Linear { start, end, .. } => Paint::Linear {
                start: *start,
                end: *end,
                stops: stops.clone(),
                spread: *spread,
                linear_rgb: *linear_rgb,
            },
            _ => Paint::Linear {
                start: Vec2::new(-1.0, 0.0),
                end: Vec2::new(1.0, 0.0),
                stops: stops.clone(),
                spread: *spread,
                linear_rgb: *linear_rgb,
            }
            .fitted(bounds),
        },
        SwatchPaint::Radial { stops, spread, linear_rgb, focal } => match old {
            Paint::Radial { center, radius, matrix, .. } => Paint::Radial {
                center: *center,
                radius: *radius,
                stops: stops.clone(),
                spread: *spread,
                linear_rgb: *linear_rgb,
                focal: *focal,
                matrix: *matrix,
            },
            _ => Paint::Radial {
                center: Vec2::ZERO,
                radius: 1.0,
                stops: stops.clone(),
                spread: *spread,
                linear_rgb: *linear_rgb,
                focal: *focal,
                matrix: anicraft_geom::Affine::IDENTITY,
            }
            .fitted(bounds),
        },
    }
}

fn walk_shapes_cow(doc: &mut Document, mut pred: impl FnMut(&Shape) -> bool, mut edit: impl FnMut(&mut Shape) -> u64) -> u64 {
    fn tl_has(tl: &Timeline, pred: &mut impl FnMut(&Shape) -> bool, depth: usize) -> bool {
        if depth > MAX_GROUP_DEPTH {
            return false;
        }
        tl.layers.iter().any(|l| {
            l.keyframes.iter().any(|k| {
                k.elements.iter().any(|e| match &e.kind {
                    ElementKind::Shape(sh) => pred(sh),
                    ElementKind::Group { members } => tl_has(members, pred, depth + 1),
                    _ => false,
                })
            })
        })
    }
    fn el_has(e: &Element, pred: &mut impl FnMut(&Shape) -> bool, depth: usize) -> bool {
        if depth > MAX_GROUP_DEPTH {
            return false;
        }
        match &e.kind {
            ElementKind::Shape(sh) => pred(sh),
            ElementKind::Group { members } => tl_has(members, pred, depth + 1),
            _ => false,
        }
    }
    fn edit_tl(tl: &mut Timeline, pred: &mut impl FnMut(&Shape) -> bool, edit: &mut impl FnMut(&mut Shape) -> u64, depth: usize) -> u64 {
        if depth > MAX_GROUP_DEPTH {
            return 0;
        }
        let mut total = 0u64;
        for layer in &mut tl.layers {
            let layer_hits = layer.keyframes.iter().any(|k| k.elements.iter().any(|e| el_has(e, pred, depth)));
            if !layer_hits {
                continue;
            }
            let l = Arc::make_mut(layer);
            for kf in &mut l.keyframes {
                let kf_hits = kf.elements.iter().any(|e| el_has(e, pred, depth));
                if !kf_hits {
                    continue;
                }
                let k = Arc::make_mut(kf);
                for el in &mut k.elements {
                    if !el_has(el, pred, depth) {
                        continue;
                    }
                    match &mut Arc::make_mut(el).kind {
                        ElementKind::Shape(sh) => total += edit(sh),
                        ElementKind::Group { members } => total += edit_tl(members, pred, edit, depth + 1),
                        _ => {}
                    }
                }
            }
        }
        total
    }
    let mut count = 0u64;
    for scene in &mut doc.scenes {
        if tl_has(&scene.timeline, &mut pred, 0) {
            count += edit_tl(&mut Arc::make_mut(scene).timeline, &mut pred, &mut edit, 0);
        }
    }
    for sym in &mut doc.library {
        if tl_has(&sym.timeline, &mut pred, 0) {
            count += edit_tl(&mut Arc::make_mut(sym).timeline, &mut pred, &mut edit, 0);
        }
    }
    count
}

fn link_matching_shapes(doc: &mut Document, target: &SwatchPaint, tid: u64) -> u64 {
    let matches_fill = |sh: &Shape| sh.fill_tag != Some(tid) && sh.fill.as_ref().and_then(SwatchPaint::from_paint).as_ref() == Some(target);
    let matches_stroke = |sh: &Shape| match (&sh.stroke, target) {
        (Some(st), SwatchPaint::Solid { color }) => sh.stroke_tag != Some(tid) && st.color == *color,
        _ => false,
    };
    walk_shapes_cow(
        doc,
        |sh| matches_fill(sh) || matches_stroke(sh),
        |sh| {
            let mut n = 0u64;
            if sh.fill.as_ref().and_then(SwatchPaint::from_paint).as_ref() == Some(target) && sh.fill_tag != Some(tid) {
                sh.fill_tag = Some(tid);
                n = 1;
            }
            if let (Some(st), SwatchPaint::Solid { color }) = (&sh.stroke, target)
                && st.color == *color
                && sh.stroke_tag != Some(tid)
            {
                sh.stroke_tag = Some(tid);
                n = 1;
            }
            n
        },
    )
}

fn clear_tag_across_doc(doc: &mut Document, tid: u64) -> u64 {
    walk_shapes_cow(
        doc,
        |sh| sh.fill_tag == Some(tid) || sh.stroke_tag == Some(tid),
        |sh| {
            let mut hit = false;
            if sh.fill_tag == Some(tid) {
                sh.fill_tag = None;
                hit = true;
            }
            if sh.stroke_tag == Some(tid) {
                sh.stroke_tag = None;
                hit = true;
            }
            u64::from(hit)
        },
    )
}

fn propagate_tagged_paint(doc: &mut Document, tid: u64, new_paint: &SwatchPaint) -> u64 {
    walk_shapes_cow(
        doc,
        |sh| (sh.fill_tag == Some(tid) && sh.fill.is_some()) || (sh.stroke_tag == Some(tid) && sh.stroke.is_some()),
        |sh| {
            let mut hit = false;
            if sh.fill_tag == Some(tid)
                && let Some(old) = &sh.fill
            {
                let bounds = sh.path.bounds();
                sh.fill = Some(apply_swatch_to_paint(new_paint, old, &bounds));
                hit = true;
            }
            if sh.stroke_tag == Some(tid)
                && let Some(st) = &mut sh.stroke
            {
                st.color = new_paint.primary_color();
                hit = true;
            }
            u64::from(hit)
        },
    )
}

fn apply(s: &mut Session, p: &Value) -> Result<Value> {
    let i = index_param(s, p, "index")?;
    let sw = s.doc.swatches.get(i).cloned().ok_or_else(|| EngineError::Param("no such swatch".into()))?;
    let selection = opt_bool(p, "selection")?.unwrap_or(true);
    match opt_str(p, "target")?.unwrap_or("fill") {
        "fill" => {
            let mut fill = swatch_json(&sw);
            if let Some(o) = fill.as_object_mut() {
                o.remove("name");
                o.remove("tagged");
                o.remove("tag_id");
                o.insert("selection".into(), json!(selection));
            }
            let r = crate::cmds_paint::set_fill_tagged(s, &fill, sw.tag_id)?;
            Ok(json!({"target": "fill", "changed": r.get("changed").cloned().unwrap_or(json!(0)), "tag_id": sw.tag_id}))
        }
        "stroke" => {
            let SwatchPaint::Solid { color } = sw.paint else {
                return Err(EngineError::State("strokes are one colour: apply a gradient swatch to the fill instead".into()));
            };
            s.style.stroke = color;
            s.style.stroke_tag = sw.tag_id;
            s.style.use_stroke = true;
            let changed = if selection && !s.selection.is_empty() {
                let tag_id = sw.tag_id;
                s.each_layer(false, |s| crate::cmds_draw::set_stroke_tagged(s, &json!({"color": hex(color)}), tag_id))?;
                s.selection.len()
            } else {
                0
            };
            Ok(json!({"target": "stroke", "changed": changed, "tag_id": sw.tag_id}))
        }
        other => Err(EngineError::Param(format!("param 'target' must be fill or stroke, not '{other}'"))),
    }
}

fn clear(s: &mut Session, _: &Value) -> Result<Value> {
    s.doc.swatches = Arc::new(vec![Swatch::solid(Rgba::BLACK), Swatch::solid(Rgba::WHITE)]);
    Ok(json!({"count": 2}))
}

fn reset(s: &mut Session, _: &Value) -> Result<Value> {
    s.doc.swatches = Arc::new(default_set());
    Ok(json!({"count": s.doc.swatches.len()}))
}

/// The JSON swatch file: `{"format": "anicraft-swatches", "version": 1, "swatches": [..]}`.
pub fn file_json(swatches: &[Swatch]) -> String {
    let v = json!({"format": FORMAT, "version": 1, "swatches": swatches.iter().map(swatch_json).collect::<Vec<_>>()});
    serde_json::to_string_pretty(&v).unwrap_or_default()
}

const FORMAT: &str = "anicraft-swatches";

/// The swatches in a swatch file (JSON) or a GIMP palette, told apart by their content.
pub fn parse_file(text: &str) -> Result<Vec<Swatch>> {
    if text.len() > MAX_PALETTE_BYTES {
        return Err(EngineError::Param("the swatch file is too large".into()));
    }
    if text.trim_start_matches('\u{feff}').trim_start().starts_with("GIMP Palette") {
        return parse_gpl(text).map(|(_, v)| v).map_err(EngineError::Param);
    }
    let v: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|e| EngineError::Param(format!("this isn't a swatch file or a GIMP palette ({e})")))?;
    if v.get("format").and_then(Value::as_str) != Some(FORMAT) {
        return Err(EngineError::Param(format!("this JSON isn't a swatch file (its \"format\" must be \"{FORMAT}\")")));
    }
    let list = v.get("swatches").and_then(Value::as_array).ok_or_else(|| EngineError::Param("the swatch file has no \"swatches\" list".into()))?;
    let mut out = Vec::new();
    for item in list.iter().take(MAX_SWATCHES) {
        // A swatch this version can't read is skipped, so a newer file still opens.
        if let Ok(Some(sw)) = swatch_from_json(item) {
            out.push(sw);
        }
    }
    if out.is_empty() {
        return Err(EngineError::Param("the swatch file has no swatches in it".into()));
    }
    Ok(out)
}

fn import(s: &mut Session, p: &Value) -> Result<Value> {
    let path = PathBuf::from(str_req(p, "path")?);
    if crate::vfs::len(&path)? > MAX_PALETTE_BYTES as u64 {
        return Err(EngineError::Param(format!("{} is too large to be a swatch file", path.display())));
    }
    let found = parse_file(&crate::vfs::read_to_string(&path)?)?;
    let replace = opt_bool(p, "replace")?.unwrap_or(false);
    let v = swatches_mut(s);
    if replace {
        v.clear();
    }
    let room = MAX_SWATCHES.saturating_sub(v.len());
    if room == 0 {
        return Err(EngineError::State(format!("a document keeps at most {MAX_SWATCHES} swatches; delete some first")));
    }
    let n = found.len().min(room);
    v.extend(found.into_iter().take(n));
    Ok(json!({"imported": n, "count": v.len()}))
}

fn export(s: &mut Session, p: &Value) -> Result<Value> {
    let path = PathBuf::from(str_req(p, "path")?);
    if s.doc.swatches.is_empty() {
        return Err(EngineError::State("the document has no swatches to export".into()));
    }
    let text = file_json(&s.doc.swatches);
    crate::vfs::write(&path, text.as_bytes())?;
    Ok(json!({"written": path.display().to_string(), "swatches": s.doc.swatches.len(), "bytes": text.len()}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_swatch_files_accept_a_utf8_byte_order_mark() {
        let text = file_json(&default_set());
        let expected = parse_file(&text).unwrap();
        assert!(!expected.is_empty());
        assert_eq!(parse_file(&format!("\u{feff}{text}")).unwrap(), expected);
    }
}
