//! Timeline & Motion Power Tools + New Empty Symbol (`P-021`, `P-041`):
//!
//! * `symbol.new_empty` (`Ctrl+F8`): creates a clean empty symbol in the Library and optionally
//!   opens it in symbol-editing mode.
//! * `timeline.reverse_frames`: reverses frame order across `[from..=to]` (splitting boundary
//!   spans, reversing FBF spans, classic/shape tween chains with time-reversed easings, and
//!   motion tween curves/trajectories).
//! * `timeline.distribute_to_layers` (`Ctrl+Shift+D`): places each selected element (or all `>= 2`
//!   elements on the active layer) onto its own newly created layer preserving exact draw order.
//! * `timeline.distribute_to_keyframes`: places each selected element (or all `>= 2` elements on
//!   the active layer) onto consecutive keyframes `start, start + step, ...`.
//! * `timeline.convert_to_frame_by_frame`: bakes evaluated tweened/posed/warped artwork onto
//!   standalone keyframes every `step` frames (`1..=24`, On 1s / On 2s / On 3s) with tweens cleared.
//! * `timeline.copy_motion` & `timeline.paste_motion`: copies and pastes relative [`MotionClip`]
//!   curves, colour effects, filters and trajectories through [`Session::motion_clipboard`].
//! * `motion.preset_list`, `motion.preset_apply`, `motion.preset_save`, `motion.preset_delete`:
//!   6 built-in studio motion presets plus user presets saved in [`anicraft_doc::Document`].

use crate::cmds_timeline::{insert_keyframe_at, shift_after, target, target_layers};
use crate::params::*;
use crate::{CommandSpec, EngineError, LAYER_COLORS, MAX_FRAME, Result, Session};
use anicraft_doc::motion::MAX_KEYS;
use anicraft_doc::{
    CustomEase, EaseDir, EasePoint, Easing, Element, ElementId, ElementKind, Keyframe, Layer,
    LayerId, LayerKind, LibraryEntry, LibraryItem, MAX_MOTION_PRESETS, MotionClip, MotionPreset,
    MotionTween, Paint, PropertyKey, SampledEase, Symbol, SymbolId, SymbolKind, Timeline, Track,
    Tween, builtin_motion_presets, evaluate_shown, evaluate_timeline, find_builtin_preset,
    guide_walk, motion_path, preset_name, sanitized_motion_presets,
};
use anicraft_geom::{Path, PathCmd, Vec2};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::sync::Arc;

/// Maximum frames a single range power-tool command processes at once.
const MAX_RANGE_SPAN: u32 = 10_000;
/// Maximum layers allowed on one timeline.
const MAX_LAYERS: usize = 1_000;
/// Maximum symbols allowed in the Library.
const MAX_SYMBOLS: usize = 10_000;

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
            "symbol.new_empty",
            "New Symbol…",
            "Insert",
            Some("Ctrl+F8"),
            "{name?, kind?: \"graphic\"|\"movie_clip\"|\"button\", folder?: Library folder id, edit?: bool (default true)}",
            true,
            symbol_new_empty as crate::RunFn,
        ),
        c(
            "timeline.reverse_frames",
            "Reverse Frames",
            "Modify/Timeline",
            None,
            "{layer? | layers?: [..], from?: u32, to?: u32}",
            true,
            reverse_frames,
        ),
        c(
            "timeline.distribute_to_layers",
            "Distribute to Layers",
            "Modify/Timeline",
            Some("Ctrl+Shift+D"),
            "{layer?}",
            true,
            distribute_to_layers,
        ),
        c(
            "timeline.distribute_to_keyframes",
            "Distribute to Keyframes",
            "Modify/Timeline",
            None,
            "{layer?, start?: u32, step?: u32 (1..60, default 1)}",
            true,
            distribute_to_keyframes,
        ),
        c(
            "timeline.convert_to_frame_by_frame",
            "Convert to Frame-by-Frame Animation",
            "Modify/Timeline",
            None,
            "{layer?, from?: u32, to?: u32, step?: u32 (1..24, default 1)}",
            true,
            convert_to_frame_by_frame,
        ),
        c(
            "timeline.copy_motion",
            "Copy Motion",
            "Edit/Timeline",
            None,
            "{layer?, frame?}: captures relative motion curves, colour effect, filters and easing onto the motion clipboard",
            false,
            copy_motion,
        ),
        c(
            "timeline.paste_motion",
            "Paste Motion",
            "Edit/Timeline",
            None,
            "{layer?, frame?, duration?: u32}: applies the copied motion clip onto the selected object or active span",
            true,
            paste_motion,
        ),
        c(
            "motion.preset_list",
            "Motion Presets",
            "",
            None,
            "{}: lists built-in studio motion presets and user presets saved in this document",
            false,
            motion_preset_list,
        ),
        c(
            "motion.preset_apply",
            "Apply Motion Preset",
            "Modify/Timeline",
            None,
            "{preset: name, layer?, frame?, duration?: u32}: applies a built-in or saved motion preset onto the target object",
            true,
            motion_preset_apply,
        ),
        c(
            "motion.preset_save",
            "Save Motion Preset",
            "",
            None,
            "{name, layer?, frame?}: saves the motion on the span as a reusable preset in this document",
            true,
            motion_preset_save,
        ),
        c(
            "motion.preset_delete",
            "Delete Motion Preset",
            "",
            None,
            "{name}: deletes a user motion preset from this document",
            true,
            motion_preset_delete,
        ),
    ]
}

// ---------------------------------------------------------------------------
// 1. symbol.new_empty (Ctrl+F8)
// ---------------------------------------------------------------------------

fn symbol_new_empty(s: &mut Session, p: &Value) -> Result<Value> {
    if s.doc.library.len() >= MAX_SYMBOLS {
        return Err(EngineError::State(format!("the Library holds at most {MAX_SYMBOLS} symbols")));
    }
    let edit = opt_bool(p, "edit")?.unwrap_or(true);
    if edit && s.editing_group() {
        return Err(EngineError::State("close the open group before editing a new symbol".into()));
    }
    if edit && s.editing.len() >= 32 {
        return Err(EngineError::State("at most 32 symbols can be open inside each other".into()));
    }
    let folder = crate::cmds_library::opt_folder(s, p, "folder")?;
    let kind = opt_str(p, "kind")?
        .map(crate::cmds_library::kind_of)
        .transpose()?
        .unwrap_or(SymbolKind::Graphic);
    let name = match opt_str(p, "name")? {
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return Err(EngineError::Param("a symbol name cannot be empty".into()));
            }
            if trimmed.contains('/') {
                return Err(EngineError::Param("a symbol name cannot contain '/'".into()));
            }
            s.doc.unique_name(folder, trimmed)
        }
        None => s.doc.numbered_name(folder, "Symbol", s.doc.library.len() + 1),
    };

    let sym_id = SymbolId(s.doc.alloc_id());
    let layer_id = LayerId(s.doc.alloc_id());
    let layer = Layer::new(layer_id, "Layer 1");
    let sym = Symbol {
        id: sym_id,
        name: name.clone(),
        kind,
        linkage: None,
        scale_grid: None,
        scale_grid_kept: None,
        timeline: Timeline { layers: vec![Arc::new(layer)] },
    };
    s.doc.library.push(Arc::new(sym));
    if folder.is_some() {
        s.doc.place_entry(LibraryEntry::Item(LibraryItem::Symbol(sym_id)), folder);
    }
    if edit {
        crate::editing::edit(s, &json!({"symbol": sym_id.0}))?;
    }
    Ok(json!({
        "symbol": sym_id.0,
        "name": name,
        "kind": crate::cmds_library::kind_key(kind),
        "layer": layer_id.0,
        "folder": folder.map(|f| f.0),
        "editing": edit,
    }))
}

// ---------------------------------------------------------------------------
// 2. timeline.reverse_frames
// ---------------------------------------------------------------------------

fn reverse_frames(s: &mut Session, p: &Value) -> Result<Value> {
    let ids = target_layers(s, p)?;
    let max_end = {
        let tl = s.timeline();
        ids.iter()
            .filter_map(|id| tl.layer(*id))
            .map(Layer::end_frame)
            .max()
            .unwrap_or(1)
            .max(1)
    };
    let from = opt_u32(p, "from")?.unwrap_or(0);
    let to = opt_u32(p, "to")?.unwrap_or_else(|| max_end.saturating_sub(1).max(from));
    if to < from || to > MAX_FRAME {
        return Err(EngineError::Param(format!("frames must be in order and within {MAX_FRAME}")));
    }
    if to - from > MAX_RANGE_SPAN {
        return Err(EngineError::Param(format!("at most {MAX_RANGE_SPAN} frames can be reversed at once")));
    }
    if from == to {
        return Ok(json!({"from": from, "to": to, "layers": ids.iter().map(|l| l.0).collect::<Vec<_>>(), "reversed": 0}));
    }

    let mut reversed_layers = 0usize;
    for &id in &ids {
        let layer = s.layer_mut(Some(id))?;
        if layer.kind == LayerKind::Camera {
            continue;
        }
        if reverse_layer_range(layer, from, to)? {
            reversed_layers += 1;
        }
    }
    Ok(json!({
        "from": from,
        "to": to,
        "layers": ids.iter().map(|l| l.0).collect::<Vec<_>>(),
        "reversed": reversed_layers,
    }))
}

fn reverse_layer_range(layer: &mut Layer, from: u32, to: u32) -> Result<bool> {
    let span_end = to.saturating_add(1);
    if layer.end_frame() <= from {
        return Ok(false);
    }
    // Split spans crossing `from` or `span_end` so frames outside `from..=to` stay untouched.
    if from > 0 && layer.keyframe_starting_at(from).is_none() && layer.keyframe_index_at(from).is_some() {
        insert_keyframe_at(layer, from, true)?;
    }
    if span_end <= MAX_FRAME
        && span_end < layer.end_frame()
        && layer.keyframe_starting_at(span_end).is_none()
        && layer.keyframe_index_at(span_end).is_some()
    {
        insert_keyframe_at(layer, span_end, true)?;
    }
    // Also split any tween end-keyframe inside `from..span_end` whose duration > 1, so the tween
    // ends on a 1-frame key and its trailing hold reverses as its own span.
    let mut split_holds: Vec<u32> = Vec::new();
    for (idx, k) in layer.keyframes.iter().enumerate() {
        if k.start >= from
            && k.start < span_end
            && k.tween.is_none()
            && k.duration > 1
            && idx > 0
            && layer.keyframes.get(idx - 1).is_some_and(|prev| prev.start >= from && prev.tween.is_some() && prev.end() == k.start)
        {
            split_holds.push(k.start + 1);
        }
    }
    for f in split_holds {
        if f < span_end && layer.keyframe_starting_at(f).is_none() {
            insert_keyframe_at(layer, f, true)?;
        }
    }

    let mut before: Vec<Arc<Keyframe>> = Vec::new();
    let mut inside: Vec<Keyframe> = Vec::new();
    let mut after: Vec<Arc<Keyframe>> = Vec::new();
    for k in &layer.keyframes {
        if k.end() <= from {
            before.push(k.clone());
        } else if k.start >= span_end {
            after.push(k.clone());
        } else {
            let mut kc = (**k).clone();
            if kc.end() > span_end {
                kc.duration = span_end - kc.start;
            }
            inside.push(kc);
        }
    }
    if inside.is_empty() {
        return Ok(false);
    }

    let mut reversed_inside: Vec<Arc<Keyframe>> = Vec::new();
    let mut i = 0usize;
    while i < inside.len() {
        let Some(first_k) = inside.get(i) else { break };
        // Detect a contiguous Classic/Shape tween chain `i ..= j` inside the range.
        if first_k.tween.is_some() {
            let mut j = i;
            while j + 1 < inside.len() {
                let (Some(cur), Some(nxt)) = (inside.get(j), inside.get(j + 1)) else { break };
                if cur.tween.is_some() && nxt.start == cur.end() {
                    j += 1;
                    if nxt.tween.is_none() {
                        break;
                    }
                } else {
                    break;
                }
            }
            if j > i {
                let chain = inside.get(i..=j).unwrap_or_default();
                let run_end = chain.last().map_or(span_end, Keyframe::end);
                let new_run_start = from + (span_end - run_end);
                // Chain has `m + 1` pose keyframes and `m` tween intervals `D_0 .. D_{m-1}`.
                let m = chain.len() - 1;
                let mut cur_start = new_run_start;
                for step_idx in 0..=m {
                    let Some(pose_src) = chain.get(m - step_idx) else { continue };
                    let (dur, tween, hints) = if step_idx < m {
                        let Some(seg_src) = chain.get(m - 1 - step_idx) else { continue };
                        let rev_tw = seg_src.tween.as_ref().map(reverse_tween);
                        let rev_hints = seg_src
                            .hints
                            .iter()
                            .map(|h| anicraft_doc::ShapeHint { start: h.end, end: h.start })
                            .collect();
                        (seg_src.duration.max(1), rev_tw, rev_hints)
                    } else {
                        let tail_dur = chain.last().map_or(1, |k| k.duration.max(1));
                        (tail_dur, None, Vec::new())
                    };
                    let mut nk = Keyframe::new(cur_start, dur);
                    nk.elements = pose_src.elements.clone();
                    nk.pose = pose_src.pose.clone();
                    nk.face_pose = pose_src.face_pose;
                    nk.label = pose_src.label.clone();
                    nk.script = pose_src.script.clone();
                    nk.sound = pose_src.sound.clone();
                    nk.tween = tween;
                    nk.hints = hints;
                    reversed_inside.push(Arc::new(nk));
                    cur_start = cur_start.saturating_add(dur);
                }
                i = j + 1;
                continue;
            }
        }

        // Standalone FBF span or MotionTween span: mirror `[k.start .. k.end())` within `[from .. span_end)`.
        let mut nk = first_k.clone();
        nk.tween = None;
        let k_end = nk.end().min(span_end);
        let dur = k_end.saturating_sub(nk.start).max(1);
        nk.start = from + (span_end - k_end);
        nk.duration = dur;
        if let Some(m) = &nk.motion {
            nk.motion = Some(reverse_motion_tween(m, dur));
        }
        reversed_inside.push(Arc::new(nk));
        i += 1;
    }

    reversed_inside.sort_by_key(|k| k.start);
    let mut all = before;
    all.extend(reversed_inside);
    all.extend(after);
    layer.keyframes = all;
    layer.normalize();
    Ok(true)
}

fn reverse_tween(tw: &Tween) -> Tween {
    Tween {
        easing: reverse_easing(&tw.easing),
        kind: tw.kind,
        orient: tw.orient,
        groups: tw.groups.iter().map(|(g, e)| (*g, reverse_easing(e))).collect(),
        keep_area: tw.keep_area,
    }
}

fn reverse_easing(e: &Easing) -> Easing {
    if *e == Easing::Linear {
        return Easing::Linear;
    }
    if let Some((fam, dir)) = e.family() {
        let rev_dir = match dir {
            EaseDir::In => EaseDir::Out,
            EaseDir::Out => EaseDir::In,
            EaseDir::InOut => EaseDir::InOut,
        };
        return Easing::preset(fam, rev_dir);
    }
    match e {
        Easing::Classic(s) => Easing::Classic(if s.is_finite() { (-s).clamp(-100.0, 100.0) } else { 0.0 }),
        Easing::CubicBezier(x1, y1, x2, y2) => Easing::CubicBezier(
            (1.0 - x2).clamp(0.0, 1.0),
            1.0 - y2,
            (1.0 - x1).clamp(0.0, 1.0),
            1.0 - y1,
        ),
        Easing::Custom(c) => {
            let pts: Vec<EasePoint> = c
                .points()
                .iter()
                .rev()
                .map(|pt| EasePoint {
                    at: Vec2::new((1.0f32 - pt.at.x).clamp(0.0, 1.0), 1.0f32 - pt.at.y),
                    h_in: Vec2::new(-pt.out.x, -pt.out.y),
                    out: Vec2::new(-pt.h_in.x, -pt.h_in.y),
                    smooth: pt.smooth,
                })
                .collect();
            Easing::Custom(Arc::new(CustomEase::new(pts)))
        }
        Easing::Sampled(s) => {
            let vals: Vec<f32> = s.values().iter().rev().map(|v| 1.0 - *v).collect();
            Easing::Sampled(Arc::new(SampledEase::new(s.name(), s.plugin(), s.ease(), vals)))
        }
        other => other.clone(),
    }
}

fn reverse_motion_tween(m: &MotionTween, duration: u32) -> MotionTween {
    let last = duration.saturating_sub(1);
    let tracks: Vec<Track> = m
        .tracks
        .iter()
        .map(|t| {
            let n = t.keys.len();
            let mut keys: Vec<PropertyKey> = Vec::with_capacity(n);
            for idx in 0..n {
                let src_idx = n - 1 - idx;
                let Some(src_k) = t.keys.get(src_idx) else { continue };
                let ease = if idx + 1 < n {
                    t.keys
                        .get(src_idx.saturating_sub(1))
                        .map_or(Easing::Linear, |prev| reverse_easing(&prev.ease))
                } else {
                    Easing::Linear
                };
                keys.push(PropertyKey {
                    frame: last.saturating_sub(src_k.frame.min(last)),
                    value: src_k.value,
                    ease,
                });
            }
            Track { property: t.property, keys }
        })
        .collect();

    let path = m.path.as_ref().and_then(reverse_cubic_path);
    let anchor_count = path.as_ref().map_or(0, |p| p.cmds.len());
    let pins = m
        .pins
        .iter()
        .filter_map(|pin| {
            let rev_anchor = anchor_count.checked_sub(1)?.checked_sub(pin.anchor as usize)?;
            Some(anicraft_doc::Pin {
                frame: last.saturating_sub(pin.frame.min(last)),
                anchor: rev_anchor as u32,
            })
        })
        .rev()
        .collect();

    let mut out = MotionTween {
        target: m.target,
        tracks,
        path,
        orient: m.orient,
        pins,
        keep_area: m.keep_area,
    };
    out.sanitize();
    out
}

fn reverse_cubic_path(path: &Path) -> Option<Path> {
    let norm = motion_path(path)?;
    let Some(PathCmd::MoveTo(first)) = norm.cmds.first().copied() else { return None };
    let mut segs: Vec<(Vec2, Vec2, Vec2, Vec2)> = Vec::new();
    let mut cur = first;
    for cmd in norm.cmds.iter().skip(1) {
        if let PathCmd::CubicTo(c1, c2, end) = *cmd {
            segs.push((cur, c1, c2, end));
            cur = end;
        }
    }
    let (_, _, _, last_end) = *segs.last()?;
    let mut cmds = Vec::with_capacity(segs.len() + 1);
    cmds.push(PathCmd::MoveTo(last_end));
    for (p0, c1, c2, _) in segs.into_iter().rev() {
        cmds.push(PathCmd::CubicTo(c2, c1, p0));
    }
    motion_path(&Path { cmds })
}

// ---------------------------------------------------------------------------
// 3. timeline.distribute_to_layers (Ctrl+Shift+D)
// ---------------------------------------------------------------------------

fn distribute_to_layers(s: &mut Session, p: &Value) -> Result<Value> {
    let src_lid = resolve_distribute_layer(s, p)?;
    let frame = s.frame;
    let (src_idx, src_parent, kf_start, kf_dur, picked, kept_any) = {
        let tl = s.timeline();
        let src_idx = tl
            .layers
            .iter()
            .position(|l| l.id == src_lid)
            .ok_or_else(|| EngineError::State(format!("no layer with id {}", src_lid.0)))?;
        let Some(src_layer) = tl.layers.get(src_idx) else {
            return Err(EngineError::State("no such layer".into()));
        };
        check_distributable_layer(src_layer)?;
        let kf = src_layer
            .keyframe_at(frame)
            .ok_or_else(|| EngineError::State(format!("frame {} is empty on layer '{}'", frame + 1, src_layer.name)))?;
        let sel: HashSet<ElementId> = s.selection.on(src_lid).into_iter().collect();
        let chosen: Vec<Arc<Element>> = if !sel.is_empty() {
            kf.elements.iter().filter(|e| sel.contains(&e.id)).cloned().collect()
        } else if kf.elements.len() >= 2 {
            kf.elements.clone()
        } else {
            return Err(EngineError::State(
                "select objects to distribute, or have at least two objects on the active layer".into(),
            ));
        };
        if chosen.is_empty() {
            return Err(EngineError::State("no matching objects on the active frame to distribute".into()));
        }
        if tl.layers.len() + chosen.len() > MAX_LAYERS {
            return Err(EngineError::State(format!("too many layers (limit {MAX_LAYERS})")));
        }
        let kept_any = chosen.len() < kf.elements.len();
        (src_idx, src_layer.parent, kf.start, kf.duration, chosen, kept_any)
    };

    let mut used_names: Vec<String> = s.timeline().layers.iter().map(|l| l.name.clone()).collect();
    let distributed_ids: HashSet<ElementId> = picked.iter().map(|e| e.id).collect();
    // `picked` is back-to-front (`0` = backmost, `len - 1` = frontmost).
    // In `Timeline::layers`, index `0` is topmost (drawn last / in front), so we build new layers
    // in reverse (`frontmost` first, `backmost` last) and insert them at `src_idx`!
    let base_color_idx = s.timeline().layers.len();
    let mut new_layers: Vec<Arc<Layer>> = Vec::with_capacity(picked.len());
    let mut new_layer_ids: Vec<u64> = Vec::with_capacity(picked.len());
    let mut new_sel: Vec<(LayerId, ElementId)> = Vec::with_capacity(picked.len());

    // Name layers in natural element order, then reverse for top-to-bottom insertion.
    let mut named_elements: Vec<(String, Arc<Element>)> = Vec::with_capacity(picked.len());
    for el in picked {
        let lname = next_distributed_layer_name(&s.doc, &mut used_names, &el);
        named_elements.push((lname, el));
    }

    for (k_ord, (lname, el)) in named_elements.into_iter().rev().enumerate() {
        let lid = LayerId(s.doc.alloc_id());
        let mut nl = Layer::new(lid, lname);
        nl.parent = src_parent;
        let c_hex = LAYER_COLORS.get((base_color_idx + k_ord) % LAYER_COLORS.len()).copied().unwrap_or(0x4fa3ff);
        nl.color = anicraft_doc::Rgba::hex(c_hex);
        let mut kfs: Vec<Arc<Keyframe>> = Vec::new();
        if kf_start > 0 {
            kfs.push(Arc::new(Keyframe::new(0, kf_start)));
        }
        let mut active_kf = Keyframe::new(kf_start, kf_dur.max(1));
        active_kf.elements = vec![el.clone()];
        kfs.push(Arc::new(active_kf));
        nl.keyframes = kfs;
        nl.normalize();
        new_layer_ids.push(lid.0);
        new_sel.push((lid, el.id));
        new_layers.push(Arc::new(nl));
    }

    {
        let tl = crate::editing::edited_mut(&mut s.doc, s.scene, &s.editing)?;
        if let Some(src_layer) = tl.layer_mut(src_lid)
            && let Some(ki) = src_layer.keyframe_index_at(frame)
            && let Some(kf) = src_layer.keyframe_mut(ki)
        {
            kf.elements.retain(|e| !distributed_ids.contains(&e.id));
            if kf.elements.is_empty() {
                kf.tween = None;
                kf.motion = None;
            } else if kf.motion.as_ref().is_some_and(|m| distributed_ids.contains(&m.target)) {
                kf.motion = None;
            }
        }
        let insert_at = src_idx.min(tl.layers.len());
        tl.layers.splice(insert_at..insert_at, new_layers);
        tl.repair_masks();
    }

    if let Some(&(top_lid, _)) = new_sel.first() {
        s.layer = top_lid;
    }
    s.selection.clear();
    for (lid, eid) in &new_sel {
        s.selection.insert(*lid, *eid);
    }

    Ok(json!({
        "layers": new_layer_ids,
        "distributed": new_sel.len(),
        "source_layer": src_lid.0,
        "source_has_remaining": kept_any,
    }))
}

fn resolve_distribute_layer(s: &Session, p: &Value) -> Result<LayerId> {
    if let Some(id) = opt_u64(p, "layer")? {
        return Ok(LayerId(id));
    }
    if !s.selection.on(s.layer).is_empty() {
        return Ok(s.layer);
    }
    if let Some(&first_sel_layer) = s.selected_layers().first() {
        return Ok(first_sel_layer);
    }
    Ok(s.layer)
}

fn check_distributable_layer(layer: &Layer) -> Result<()> {
    match layer.kind {
        LayerKind::Folder => Err(EngineError::State("a folder holds layers, not artwork".into())),
        LayerKind::Camera => Err(EngineError::State("the camera layer holds the camera, not artwork".into())),
        LayerKind::Armature => Err(EngineError::State("an armature layer's parts belong to its rig".into())),
        _ => Ok(()),
    }
}

fn next_distributed_layer_name(doc: &anicraft_doc::Document, used: &mut Vec<String>, el: &Element) -> String {
    let raw_base = if !el.name.trim().is_empty() {
        el.name.trim().to_string()
    } else {
        match &el.kind {
            ElementKind::Symbol { symbol, .. } => doc
                .symbol(*symbol)
                .map(|sym| sym.name.trim().to_string())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| "Symbol".into()),
            ElementKind::Text(t) => {
                let snippet: String = t
                    .text
                    .chars()
                    .filter(|c| !c.is_control())
                    .collect::<String>()
                    .trim()
                    .chars()
                    .take(24)
                    .collect();
                if snippet.is_empty() { "Text".into() } else { snippet }
            }
            ElementKind::Shape(sh) => match &sh.fill {
                Some(Paint::Bitmap { bitmap, .. }) => doc
                    .bitmap(*bitmap)
                    .map(|b| b.name.trim().to_string())
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| "Bitmap".into()),
                _ => "Shape".into(),
            },
            ElementKind::Group { .. } => "Group".into(),
            ElementKind::Camera => "Layer".into(),
        }
    };
    let base: String = raw_base.chars().filter(|c| !c.is_control()).take(64).collect();
    let base = if base.trim().is_empty() { "Layer".to_string() } else { base.trim().to_string() };
    if !used.iter().any(|u| u.eq_ignore_ascii_case(&base)) {
        used.push(base.clone());
        return base;
    }
    let mut n = 2usize;
    loop {
        let candidate = format!("{base} {n}");
        if !used.iter().any(|u| u.eq_ignore_ascii_case(&candidate)) {
            used.push(candidate.clone());
            return candidate;
        }
        n = n.saturating_add(1);
    }
}

// ---------------------------------------------------------------------------
// 4. timeline.distribute_to_keyframes
// ---------------------------------------------------------------------------

fn distribute_to_keyframes(s: &mut Session, p: &Value) -> Result<Value> {
    let step = opt_u32(p, "step")?.unwrap_or(1);
    if !(1..=60).contains(&step) {
        return Err(EngineError::Param("param 'step' must be between 1 and 60".into()));
    }
    let lid = resolve_distribute_layer(s, p)?;
    let playhead = s.frame;
    let start = opt_u32(p, "start")?.unwrap_or(playhead);
    if start > MAX_FRAME {
        return Err(EngineError::Param(format!("param 'start' must be at most {MAX_FRAME}")));
    }

    let picked: Vec<Arc<Element>> = {
        let tl = s.timeline();
        let layer = tl
            .layer(lid)
            .ok_or_else(|| EngineError::State(format!("no layer with id {}", lid.0)))?;
        check_distributable_layer(layer)?;
        let kf = layer
            .keyframe_at(playhead)
            .ok_or_else(|| EngineError::State(format!("frame {} is empty on layer '{}'", playhead + 1, layer.name)))?;
        let sel: HashSet<ElementId> = s.selection.on(lid).into_iter().collect();
        if !sel.is_empty() {
            kf.elements.iter().filter(|e| sel.contains(&e.id)).cloned().collect()
        } else if kf.elements.len() >= 2 {
            kf.elements.clone()
        } else {
            return Err(EngineError::State(
                "select objects to distribute, or have at least two objects on the active layer".into(),
            ));
        }
    };
    if picked.is_empty() {
        return Err(EngineError::State("no matching objects on the active frame to distribute".into()));
    }
    if picked.len() > MAX_KEYS {
        return Err(EngineError::State(format!("at most {MAX_KEYS} keyframes can be created at once")));
    }

    let count = picked.len() as u32;
    let total_span = count
        .checked_mul(step)
        .ok_or_else(|| EngineError::Param("frame span overflows timeline limits".into()))?;
    let range_end = start
        .checked_add(total_span)
        .filter(|&end| end <= MAX_FRAME + 1)
        .ok_or_else(|| EngineError::Param(format!("distributed keyframes would exceed frame {MAX_FRAME}")))?;

    let distributed_ids: HashSet<ElementId> = picked.iter().map(|e| e.id).collect();
    let layer = s.layer_mut(Some(lid))?;

    // Remove distributed items from the source keyframe first.
    if let Some(src_ki) = layer.keyframe_index_at(playhead)
        && let Some(kf) = layer.keyframe_mut(src_ki)
    {
        kf.elements.retain(|e| !distributed_ids.contains(&e.id));
        if kf.elements.is_empty() {
            kf.tween = None;
            kf.motion = None;
        } else if kf.motion.as_ref().is_some_and(|m| distributed_ids.contains(&m.target)) {
            kf.motion = None;
        }
    }

    // Preserve content before `start` and after `range_end`.
    if start > 0 && layer.keyframe_starting_at(start).is_none() {
        if layer.keyframe_index_at(start).is_some() {
            insert_keyframe_at(layer, start, true)?;
        } else if layer.end_frame() < start {
            let gap_start = layer.end_frame();
            layer.keyframes.push(Arc::new(Keyframe::new(gap_start, start - gap_start)));
            layer.normalize();
        }
    }
    if range_end <= MAX_FRAME
        && range_end < layer.end_frame()
        && layer.keyframe_starting_at(range_end).is_none()
        && layer.keyframe_index_at(range_end).is_some()
    {
        insert_keyframe_at(layer, range_end, true)?;
    }

    layer.keyframes.retain(|k| k.end() <= start || k.start >= range_end);
    for (idx, el) in picked.iter().enumerate() {
        let f = start + (idx as u32) * step;
        let mut nk = Keyframe::new(f, step);
        nk.elements = vec![el.clone()];
        layer.keyframes.push(Arc::new(nk));
    }
    layer.normalize();

    s.layer = lid;
    s.selection.clear();
    if let Some(kf) = s.timeline().layer(lid).and_then(|l| l.keyframe_at(s.frame))
        && let Some(first_el) = kf.elements.first()
    {
        s.selection.insert(lid, first_el.id);
    }

    Ok(json!({
        "layer": lid.0,
        "start": start,
        "step": step,
        "keyframes": count,
        "end": range_end.saturating_sub(1),
    }))
}

// ---------------------------------------------------------------------------
// 5. timeline.convert_to_frame_by_frame
// ---------------------------------------------------------------------------

fn convert_to_frame_by_frame(s: &mut Session, p: &Value) -> Result<Value> {
    let step = opt_u32(p, "step")?.unwrap_or(1);
    if !(1..=24).contains(&step) {
        return Err(EngineError::Param("param 'step' must be between 1 and 24 (1 = On 1s, 2 = On 2s, 3 = On 3s)".into()));
    }
    let lid = opt_u64(p, "layer")?.map_or(s.layer, LayerId);
    let fps = s.doc.stage.fps;

    let (from, to, baked) = {
        let tl = s.timeline();
        let layer = tl
            .layer(lid)
            .ok_or_else(|| EngineError::State(format!("no layer with id {}", lid.0)))?;
        match layer.kind {
            LayerKind::Folder => return Err(EngineError::State("a folder holds layers, not frames".into())),
            LayerKind::Camera => return Err(EngineError::State("the camera layer holds camera keys, not artwork".into())),
            _ => {}
        }
        let (default_from, default_to) = default_bake_range(layer, s.frame);
        let from = opt_u32(p, "from")?.unwrap_or(default_from);
        let to = opt_u32(p, "to")?.unwrap_or_else(|| default_to.max(from));
        if to < from || to > MAX_FRAME {
            return Err(EngineError::Param(format!("frames must be in order and within {MAX_FRAME}")));
        }
        if to - from > MAX_RANGE_SPAN {
            return Err(EngineError::Param(format!("at most {MAX_RANGE_SPAN} frames can be converted at once")));
        }

        let guide_layer = layer
            .parent
            .and_then(|pid| tl.layer(pid))
            .filter(|g| g.kind == LayerKind::Guide);
        let is_armature = layer.kind == LayerKind::Armature && layer.armature.as_ref().is_some_and(|a| !a.is_empty());

        let mut baked_keys: Vec<Arc<Keyframe>> = Vec::new();
        let mut f = from;
        while f <= to {
            let dur = (to - f + 1).min(step).max(1);
            let walk = guide_layer.and_then(|g| guide_walk(g, f));
            let mut nk = Keyframe::new(f, dur);
            if is_armature {
                // Keep the armature intact by baking root elements + evaluated pose per step.
                nk.elements = anicraft_doc::evaluate_layer_elements(layer, f)
                    .map(|(_, c)| c.into_owned())
                    .unwrap_or_default();
                nk.pose = if layer.armature.as_ref().is_some_and(|a| a.has_springs()) {
                    anicraft_doc::spring::sprung_pose(layer, f, fps).unwrap_or_default()
                } else {
                    anicraft_doc::armature::pose_at(layer, f).unwrap_or_default()
                };
                let fp = anicraft_doc::autorig::face_pose_at(layer, f);
                nk.face_pose = (!fp.is_default()).then_some(fp);
            } else {
                nk.elements = evaluate_shown(layer, f, walk.as_ref(), fps)
                    .map(|(_, c)| c.into_owned())
                    .unwrap_or_default();
            }
            // Carry over metadata (label, comment, script, sound) from a keyframe starting on `[f .. f + dur)`.
            for orig in &layer.keyframes {
                if orig.start >= f && orig.start < f + dur {
                    if nk.label.is_empty() && !orig.label.is_empty() {
                        nk.label = orig.label.clone();
                    }
                    if nk.script.is_none() && orig.script.is_some() {
                        nk.script = orig.script.clone();
                    }
                    if nk.sound.is_none() && orig.sound.is_some() && orig.start == f {
                        nk.sound = orig.sound.clone();
                    }
                }
            }
            // Explicitly clear tween/motion/hints so every baked keyframe is standalone!
            nk.tween = None;
            nk.motion = None;
            nk.hints = Vec::new();
            baked_keys.push(Arc::new(nk));
            match f.checked_add(step) {
                Some(next_f) => f = next_f,
                None => break,
            }
        }
        (from, to, baked_keys)
    };

    let span_end = to.saturating_add(1);
    let layer = s.layer_mut(Some(lid))?;
    if from > 0 && layer.keyframe_starting_at(from).is_none() {
        if layer.keyframe_index_at(from).is_some() {
            insert_keyframe_at(layer, from, true)?;
        } else if layer.end_frame() < from {
            let gap_start = layer.end_frame();
            layer.keyframes.push(Arc::new(Keyframe::new(gap_start, from - gap_start)));
            layer.normalize();
        }
    }
    if span_end <= MAX_FRAME
        && span_end < layer.end_frame()
        && layer.keyframe_starting_at(span_end).is_none()
        && layer.keyframe_index_at(span_end).is_some()
    {
        insert_keyframe_at(layer, span_end, true)?;
    }
    // Clear any classic/shape tween immediately before `from` whose end keyframe just got baked
    // only if `from` broke it? No — `from` now has a keyframe holding the exact evaluated pose at
    // `from`, so a classic tween ending at `from` continues to land smoothly on `from`!
    layer.keyframes.retain(|k| k.end() <= from || k.start >= span_end);
    let baked_len = baked.len();
    layer.keyframes.extend(baked);
    layer.normalize();

    Ok(json!({
        "layer": lid.0,
        "from": from,
        "to": to,
        "step": step,
        "keyframes": baked_len,
    }))
}

fn default_bake_range(layer: &Layer, playhead: u32) -> (u32, u32) {
    if let Some(idx) = layer.keyframe_index_at(playhead)
        && let Some(kf) = layer.keyframes.get(idx)
    {
        if kf.motion.is_some() {
            return (kf.start, kf.end().saturating_sub(1));
        }
        if kf.tween.is_some() {
            let end_f = layer
                .keyframes
                .get(idx + 1)
                .filter(|nxt| nxt.start == kf.end())
                .map_or_else(|| kf.end().saturating_sub(1), |nxt| nxt.start);
            return (kf.start, end_f);
        }
    }
    (0, layer.end_frame().saturating_sub(1))
}

// ---------------------------------------------------------------------------
// 6. timeline.copy_motion & timeline.paste_motion + motion.preset_* (P-041)
// ---------------------------------------------------------------------------

fn copy_motion(s: &mut Session, p: &Value) -> Result<Value> {
    let (lid_opt, frame) = target(s, p)?;
    let lid = lid_opt.unwrap_or(s.layer);
    let tl = s.timeline();
    let layer = tl
        .layer(lid)
        .ok_or_else(|| EngineError::State(format!("no layer with id {}", lid.0)))?;
    check_distributable_layer(layer)?;
    let kf_idx = layer
        .keyframe_index_at(frame)
        .ok_or_else(|| EngineError::State(format!("frame {} is empty on layer '{}'", frame + 1, layer.name)))?;
    let preferred = s.selection.on(lid).into_iter().next();
    let clip = MotionClip::from_keyframe(layer, kf_idx, preferred)
        .ok_or_else(|| EngineError::State("there is no object on this frame to copy motion from".into()))?;
    let out = json!({
        "copied": true,
        "layer": lid.0,
        "duration": clip.duration,
        "tracks": clip.tracks.len(),
        "has_trajectory": clip.path.is_some(),
        "filters": clip.filters.len(),
    });
    s.motion_clipboard = Some(clip);
    Ok(out)
}

fn paste_motion(s: &mut Session, p: &Value) -> Result<Value> {
    let clip = s
        .motion_clipboard
        .clone()
        .ok_or_else(|| EngineError::State("copy motion first with Copy Motion".into()))?;
    let res = apply_motion_clip_to_session(s, p, &clip)?;
    Ok(json!({
        "pasted": true,
        "layer": res.layer.0,
        "keyframe": res.keyframe_start,
        "duration": res.duration,
        "element": res.element.0,
        "tracks": res.tracks,
    }))
}

fn motion_preset_list(s: &mut Session, _: &Value) -> Result<Value> {
    let builtin: Vec<Value> = builtin_motion_presets()
        .into_iter()
        .map(|b| {
            json!({
                "name": b.id,
                "label": b.label,
                "description": b.description,
                "builtin": true,
                "duration": b.clip.duration,
                "tracks": b.clip.tracks.len(),
                "has_filters": !b.clip.filters.is_empty(),
            })
        })
        .collect();
    let custom: Vec<Value> = s
        .doc
        .motion_presets
        .iter()
        .map(|mp| {
            json!({
                "name": mp.name,
                "label": mp.name,
                "builtin": false,
                "duration": mp.clip.duration,
                "tracks": mp.clip.tracks.len(),
                "has_filters": !mp.clip.filters.is_empty(),
            })
        })
        .collect();
    let mut all = Vec::with_capacity(builtin.len() + custom.len());
    all.extend(builtin.iter().cloned());
    all.extend(custom.iter().cloned());
    Ok(json!({
        "presets": all,
        "builtin": builtin,
        "custom": custom,
    }))
}

fn motion_preset_apply(s: &mut Session, p: &Value) -> Result<Value> {
    let wanted = str_req(p, "preset")?;
    let (resolved_name, is_builtin, clip) = if let Some(b) = find_builtin_preset(wanted) {
        (b.id.to_string(), true, b.clip)
    } else if let Some(u) = s
        .doc
        .motion_presets
        .iter()
        .find(|mp| mp.name.eq_ignore_ascii_case(wanted.trim()))
    {
        (u.name.clone(), false, u.clip.clone())
    } else {
        return Err(EngineError::Param(format!("unknown motion preset '{wanted}'")));
    };

    let res = apply_motion_clip_to_session(s, p, &clip)?;
    Ok(json!({
        "preset": resolved_name,
        "builtin": is_builtin,
        "layer": res.layer.0,
        "keyframe": res.keyframe_start,
        "duration": res.duration,
        "element": res.element.0,
        "tracks": res.tracks,
    }))
}

fn motion_preset_save(s: &mut Session, p: &Value) -> Result<Value> {
    let raw_name = str_req(p, "name")?;
    let name = preset_name(raw_name).ok_or_else(|| EngineError::Param("a motion preset needs a non-empty name".into()))?;
    if find_builtin_preset(&name).is_some() {
        return Err(EngineError::Param(format!("'{name}' is a built-in studio preset; choose a custom name")));
    }

    let (lid_opt, frame) = target(s, p)?;
    let lid = lid_opt.unwrap_or(s.layer);
    let clip = {
        let tl = s.timeline();
        let layer = tl
            .layer(lid)
            .ok_or_else(|| EngineError::State(format!("no layer with id {}", lid.0)))?;
        check_distributable_layer(layer)?;
        let kf_idx = layer
            .keyframe_index_at(frame)
            .ok_or_else(|| EngineError::State(format!("frame {} is empty on layer '{}'", frame + 1, layer.name)))?;
        let preferred = s.selection.on(lid).into_iter().next();
        MotionClip::from_keyframe(layer, kf_idx, preferred)
            .ok_or_else(|| EngineError::State("there is no object on this frame to save as a motion preset".into()))?
    };
    if clip.tracks.is_empty() && clip.path.is_none() {
        return Err(EngineError::State("this span has no motion curves to save as a preset; create a tween first".into()));
    }

    let tracks_len = clip.tracks.len();
    let dur = clip.duration;
    let mut list = (*s.doc.motion_presets).clone();
    if let Some(existing) = list.iter_mut().find(|mp| mp.name.to_lowercase() == name.to_lowercase()) {
        existing.name = name.clone();
        existing.clip = clip;
    } else {
        if list.len() >= MAX_MOTION_PRESETS {
            return Err(EngineError::State(format!("a document holds at most {MAX_MOTION_PRESETS} saved motion presets")));
        }
        list.push(MotionPreset { name: name.clone(), clip });
    }
    s.doc.motion_presets = Arc::new(sanitized_motion_presets(&list));

    Ok(json!({
        "name": name,
        "duration": dur,
        "tracks": tracks_len,
        "count": s.doc.motion_presets.len(),
    }))
}

fn motion_preset_delete(s: &mut Session, p: &Value) -> Result<Value> {
    let raw = str_req(p, "name")?.trim();
    if raw.is_empty() {
        return Err(EngineError::Param("param 'name' cannot be empty".into()));
    }
    if find_builtin_preset(raw).is_some() {
        return Err(EngineError::State(format!("'{raw}' is a built-in studio preset and cannot be deleted")));
    }
    let pos = s
        .doc
        .motion_presets
        .iter()
        .position(|mp| mp.name.to_lowercase() == raw.to_lowercase())
        .ok_or_else(|| EngineError::State(format!("no saved motion preset named '{raw}'")))?;
    let mut list = (*s.doc.motion_presets).clone();
    let removed = list.remove(pos);
    s.doc.motion_presets = Arc::new(sanitized_motion_presets(&list));
    Ok(json!({
        "deleted": removed.name,
        "count": s.doc.motion_presets.len(),
    }))
}

struct AppliedMotion {
    layer: LayerId,
    keyframe_start: u32,
    duration: u32,
    element: ElementId,
    tracks: usize,
}

fn apply_motion_clip_to_session(s: &mut Session, p: &Value, clip: &MotionClip) -> Result<AppliedMotion> {
    let req_dur = opt_u32(p, "duration")?;
    if let Some(d) = req_dur
        && (d == 0 || d > MAX_FRAME)
    {
        return Err(EngineError::Param(format!("param 'duration' must be between 1 and {MAX_FRAME}")));
    }

    let lid = match opt_u64(p, "layer")? {
        Some(id) => LayerId(id),
        None => resolve_distribute_layer(s, p)?,
    };
    let frame = opt_u32(p, "frame")?.unwrap_or(s.frame);
    if frame > MAX_FRAME {
        return Err(EngineError::Param(format!("param 'frame' must be at most {MAX_FRAME}")));
    }

    // Check target layer & keyframe before mutating.
    let (kf_idx, target_idx, is_last_kf, cur_dur, had_tween, kf_start) = {
        let tl = s.timeline();
        let layer = tl
            .layer(lid)
            .ok_or_else(|| EngineError::State(format!("no layer with id {}", lid.0)))?;
        check_distributable_layer(layer)?;
        let kf_idx = layer
            .keyframe_index_at(frame)
            .ok_or_else(|| EngineError::State(format!("frame {} is empty on layer '{}'", frame + 1, layer.name)))?;
        let kf = layer.keyframes.get(kf_idx).ok_or_else(|| EngineError::State("no keyframe there".into()))?;
        if kf.elements.is_empty() {
            return Err(EngineError::State("place or select an object on this frame before applying motion".into()));
        }
        let sel = s.selection.on(lid);
        let t_idx = sel
            .first()
            .and_then(|id| kf.elements.iter().position(|e| e.id == *id))
            .or_else(|| kf.motion.as_ref().and_then(|m| m.target_index(&kf.elements)))
            .unwrap_or(0);
        let is_last = kf_idx + 1 == layer.keyframes.len();
        let had_tw = kf.motion.is_some() || kf.tween.is_some();
        (kf_idx, t_idx, is_last, kf.duration, had_tw, kf.start)
    };

    let max_avail_dur = MAX_FRAME.saturating_sub(kf_start).saturating_add(1).max(1);
    let desired_dur = match req_dur {
        Some(d) => d.min(max_avail_dur),
        None if is_last_kf && !had_tween => clip.duration.max(cur_dur).min(max_avail_dur),
        None if is_last_kf => clip.duration.min(max_avail_dur),
        None if cur_dur == 1 => clip.duration.min(max_avail_dur),
        None => cur_dur.min(max_avail_dur),
    };

    // If the clip uses filter tracks (such as `pulse_glow`), a Shape or Graphic symbol has no
    // filter list of its own: keep Shape/Graphic working via non-filter tracks, while MovieClip,
    // Button and Text also receive the animated filters automatically inside `apply_to_element`.
    let layer = s.layer_mut(Some(lid))?;
    if desired_dur > cur_dur && !is_last_kf {
        let delta = i64::from(desired_dur - cur_dur);
        let max_existing = layer.end_frame();
        if u64::from(max_existing) + (delta as u64) <= u64::from(MAX_FRAME) + 1 {
            shift_after(layer, kf_start + cur_dur, delta);
        }
    }

    let final_span_dur = if !is_last_kf {
        let next_start = layer.keyframes.get(kf_idx + 1).map_or(MAX_FRAME + 1, |nk| nk.start);
        desired_dur.min(next_start.saturating_sub(kf_start)).max(1)
    } else {
        desired_dur.max(1)
    };

    let kf = layer.keyframe_mut(kf_idx).ok_or_else(|| EngineError::State("no keyframe there".into()))?;
    let base_el = kf.elements.get(target_idx).cloned().ok_or_else(|| EngineError::State("target object missing".into()))?;
    let (updated_el, motion, _) = clip.apply_to_element(&base_el, Some(final_span_dur));
    let el_id = updated_el.id;
    let tracks_count = motion.tracks.len();
    if let Some(slot) = kf.elements.get_mut(target_idx) {
        *slot = Arc::new(updated_el);
    }
    kf.duration = final_span_dur;
    kf.tween = None;
    kf.motion = if motion.tracks.is_empty() && motion.path.is_none() {
        None
    } else {
        Some(motion)
    };
    if kf.motion.is_none()
        && let Some(tw) = &clip.classic_easing
    {
        kf.tween = Some(tw.clone());
    }
    layer.normalize();

    // Ensure evaluation of edited timeline never panics and update selection onto target.
    let _ = evaluate_timeline(&s.doc, s.timeline(), kf_start);
    s.layer = lid;
    s.select_only([el_id]);

    Ok(AppliedMotion {
        layer: lid,
        keyframe_start: kf_start,
        duration: final_span_dur,
        element: el_id,
        tracks: tracks_count,
    })
}
