//! Frame Picker commands (P-026): inspect symbol pose frames (`frame_picker.frames`) and pick
//! a symbol pose at the playhead with optional auto-keyframing (`frame_picker.pick`).

use crate::params::*;
use crate::{CommandSpec, EngineError, KeyEdit, MAX_FRAME, Result, Session};
use anicraft_doc::{
    Element, ElementId, ElementKind, LayerId, LayerKind, LoopMode, SymbolId, evaluate_layer_elements,
};
use serde_json::{Value, json};
use std::sync::Arc;

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
            "frame_picker.frames",
            "List Symbol Frames",
            "",
            None,
            "{element?, symbol?, layer?} → {symbol, name, frame_count, loop_mode, first_frame, frames: [{frame, label, non_empty}]}",
            false,
            frames as crate::RunFn,
        ),
        c(
            "frame_picker.pick",
            "Pick Symbol Frame",
            "",
            None,
            "{frame_index, loop?: single_frame|play_once|loop, auto_keyframe?: bool, element?, layer?} sets loop and first_frame on the symbol instance (optionally inserting a keyframe at the playhead) in one undo step",
            true,
            pick,
        ),
    ]
}

pub fn loop_mode_key(m: LoopMode) -> &'static str {
    match m {
        LoopMode::Loop => "loop",
        LoopMode::PlayOnce => "play_once",
        LoopMode::SingleFrame => "single_frame",
    }
}

pub fn parse_loop_mode(s: &str) -> Option<LoopMode> {
    match s.trim().to_ascii_lowercase().replace([' ', '-'], "_").as_str() {
        "loop" => Some(LoopMode::Loop),
        "play_once" | "playonce" | "once" => Some(LoopMode::PlayOnce),
        "single_frame" | "singleframe" | "single" => Some(LoopMode::SingleFrame),
        _ => None,
    }
}

/// Resolves a target symbol instance `(LayerId, Arc<Element>)` at the current playhead.
pub fn resolve_instance(
    s: &Session,
    elem_param: Option<ElementId>,
    layer_param: Option<LayerId>,
) -> Result<(LayerId, Arc<Element>)> {
    let tl = s.timeline();
    let find_on_layer = |lid: LayerId, eid: ElementId| -> Option<Arc<Element>> {
        let l = tl.layer(lid)?;
        if l.kind == LayerKind::Folder {
            return None;
        }
        let (_, els) = evaluate_layer_elements(l, s.frame)?;
        els.iter().find(|e| e.id == eid).cloned()
    };

    if let Some(eid) = elem_param {
        if let Some(lid) = layer_param {
            let l = tl
                .layer(lid)
                .ok_or_else(|| EngineError::State(format!("no layer with id {}", lid.0)))?;
            let el = find_on_layer(lid, eid).ok_or_else(|| {
                EngineError::State(format!(
                    "no symbol instance with id {} on layer '{}' at frame {}",
                    eid.0,
                    l.name,
                    s.frame + 1
                ))
            })?;
            return Ok((lid, el));
        }
        if let Some(el) = find_on_layer(s.layer, eid) {
            return Ok((s.layer, el));
        }
        for l in &tl.layers {
            if let Some(el) = find_on_layer(l.id, eid) {
                return Ok((l.id, el));
            }
        }
        return Err(EngineError::State(format!(
            "no symbol instance with id {} at frame {}",
            eid.0,
            s.frame + 1
        )));
    }

    // Check active selection first.
    let selected_syms: Vec<(LayerId, Arc<Element>)> = s
        .selected_elements()
        .into_iter()
        .filter(|(lid, e)| {
            layer_param.is_none_or(|want| want == *lid)
                && matches!(e.kind, ElementKind::Symbol { .. })
        })
        .collect();
    if let Some(first) = selected_syms.first() {
        return Ok(first.clone());
    }

    // Fallback: lone symbol instance on target/active layer at the playhead.
    let check_lid = layer_param.unwrap_or(s.layer);
    if let Some(l) = tl.layer(check_lid)
        && l.kind != LayerKind::Folder
        && let Some((_, els)) = evaluate_layer_elements(l, s.frame)
    {
        let syms: Vec<&Arc<Element>> = els
            .iter()
            .filter(|e| matches!(e.kind, ElementKind::Symbol { .. }))
            .collect();
        if let [only] = syms.as_slice() {
            return Ok((check_lid, (*only).clone()));
        }
    }

    Err(EngineError::State(
        "select a symbol instance on the stage (or pass 'element' or 'symbol')".into(),
    ))
}

fn frames(s: &mut Session, p: &Value) -> Result<Value> {
    let sym_param = opt_u64(p, "symbol")?.map(SymbolId);
    let elem_param = opt_u64(p, "element")?.map(ElementId);
    let layer_param = opt_u64(p, "layer")?.map(LayerId);

    let (sym_id, loop_mode, first_frame, elem_id, layer_id) = if let Some(sid) = sym_param {
        let inst = if elem_param.is_some() {
            resolve_instance(s, elem_param, layer_param).ok()
        } else {
            None
        };
        match inst {
            Some((lid, el)) => match &el.kind {
                ElementKind::Symbol { loop_mode, first_frame, .. } => {
                    (sid, *loop_mode, *first_frame, Some(el.id), Some(lid))
                }
                _ => (sid, LoopMode::SingleFrame, 0, None, None),
            },
            None => (sid, LoopMode::SingleFrame, 0, None, None),
        }
    } else {
        let (lid, el) = resolve_instance(s, elem_param, layer_param)?;
        match &el.kind {
            ElementKind::Symbol { symbol, loop_mode, first_frame, .. } => {
                (*symbol, *loop_mode, *first_frame, Some(el.id), Some(lid))
            }
            _ => {
                return Err(EngineError::State(
                    "Frame Picker works on symbol instances: convert the artwork to a Graphic symbol first (F8)".into(),
                ))
            }
        }
    };

    let sym = s
        .doc
        .symbol(sym_id)
        .ok_or_else(|| EngineError::State(format!("no symbol with id {}", sym_id.0)))?;
    let frame_count = sym.timeline.frame_count().max(1);
    let mut list = Vec::with_capacity((frame_count as usize).min(4096));
    for f in 0..frame_count {
        let label = sym
            .timeline
            .layers
            .iter()
            .find_map(|l| {
                l.keyframe_starting_at(f)
                    .and_then(|i| l.keyframes.get(i))
                    .map(|k| k.label.as_str())
                    .filter(|lb| !lb.is_empty())
            })
            .unwrap_or("");
        let non_empty = sym
            .timeline
            .layers
            .iter()
            .filter(|l| l.visible && !matches!(l.kind, LayerKind::Folder | LayerKind::Guide | LayerKind::Camera))
            .any(|l| evaluate_layer_elements(l, f).is_some_and(|(_, els)| !els.is_empty()));
        list.push(json!({
            "frame": f,
            "label": label,
            "non_empty": non_empty,
        }));
    }

    Ok(json!({
        "symbol": sym.id.0,
        "name": sym.name,
        "kind": crate::cmds_library::kind_key(sym.kind),
        "element": elem_id.map(|id| id.0),
        "layer": layer_id.map(|id| id.0),
        "frame_count": frame_count,
        "loop_mode": loop_mode_key(loop_mode),
        "first_frame": first_frame,
        "frames": list,
    }))
}

fn pick(s: &mut Session, p: &Value) -> Result<Value> {
    let frame_index = opt_u32(p, "frame_index")?
        .ok_or_else(|| EngineError::Param("param 'frame_index' (0-based symbol frame) is required".into()))?;
    if frame_index > MAX_FRAME {
        return Err(EngineError::Param(format!("param 'frame_index' must be at most {MAX_FRAME}")));
    }
    let loop_override = match opt_str(p, "loop")? {
        None => None,
        Some(raw) => Some(parse_loop_mode(raw).ok_or_else(|| {
            EngineError::Param(format!(
                "unknown loop mode '{}' (single_frame, play_once or loop)",
                raw.chars().take(40).collect::<String>()
            ))
        })?),
    };
    let auto_keyframe = opt_bool(p, "auto_keyframe")?.unwrap_or(false);
    let elem_param = opt_u64(p, "element")?.map(ElementId);
    let layer_param = opt_u64(p, "layer")?.map(LayerId);

    let (lid, el) = resolve_instance(s, elem_param, layer_param)?;
    let (sym_id, cur_loop) = match &el.kind {
        ElementKind::Symbol { symbol, loop_mode, .. } => (*symbol, *loop_mode),
        _ => {
            return Err(EngineError::State(
                "Frame Picker works on symbol instances: select a symbol instance first".into(),
            ))
        }
    };
    let sym_frames = s
        .doc
        .symbol(sym_id)
        .ok_or_else(|| EngineError::State(format!("no symbol with id {}", sym_id.0)))?
        .timeline
        .frame_count()
        .max(1);
    if frame_index >= sym_frames {
        return Err(EngineError::Param(format!(
            "frame_index {frame_index} is out of range: symbol has {sym_frames} frames (0 to {})",
            sym_frames - 1
        )));
    }

    let tl = s.timeline();
    let layer_ref = tl
        .layer(lid)
        .ok_or_else(|| EngineError::State(format!("no layer with id {}", lid.0)))?;
    if layer_ref.locked {
        return Err(EngineError::State(format!("layer '{}' is locked", layer_ref.name)));
    }
    if tl.locked_by_folder(lid) {
        return Err(EngineError::State(format!("layer '{}' is in a locked folder", layer_ref.name)));
    }
    let is_armature = layer_ref.kind == LayerKind::Armature;
    let was_keyframe = layer_ref.keyframe_starting_at(s.frame).is_some();

    let next_loop = loop_override.unwrap_or(cur_loop);
    let playhead = s.frame;

    s.with_layer(lid, |s| {
        if auto_keyframe && !was_keyframe {
            let layer = s.layer_mut(Some(lid))?;
            crate::cmds_timeline::insert_keyframe_at(layer, playhead, true)?;
        }
        if is_armature {
            if auto_keyframe {
                let kf = s.editable_keyframe_in(KeyEdit::Pose)?;
                let slot = kf
                    .elements
                    .iter_mut()
                    .find(|e| e.id == el.id)
                    .ok_or_else(|| EngineError::State("symbol instance not found on armature keyframe".into()))?;
                if let ElementKind::Symbol { loop_mode, first_frame, .. } = &mut Arc::make_mut(slot).kind {
                    *loop_mode = next_loop;
                    *first_frame = frame_index;
                }
            } else {
                let layer = s.layer_mut(Some(lid))?;
                let mut touched = 0usize;
                for k in &mut layer.keyframes {
                    for e in Arc::make_mut(k).elements_mut_where(|id| id == el.id) {
                        if let ElementKind::Symbol { loop_mode, first_frame, .. } = &mut e.kind {
                            *loop_mode = next_loop;
                            *first_frame = frame_index;
                            touched += 1;
                        }
                    }
                }
                if touched == 0 {
                    return Err(EngineError::State("symbol instance not found on armature layer".into()));
                }
            }
        } else {
            let kf = s.editable_keyframe()?;
            let slot = kf
                .elements
                .iter_mut()
                .find(|e| e.id == el.id)
                .ok_or_else(|| EngineError::State("symbol instance not found on keyframe".into()))?;
            if let ElementKind::Symbol { loop_mode, first_frame, .. } = &mut Arc::make_mut(slot).kind {
                *loop_mode = next_loop;
                *first_frame = frame_index;
            }
        }
        Ok(())
    })?;

    let now_keyframe = s
        .timeline()
        .layer(lid)
        .is_some_and(|l| l.keyframe_starting_at(playhead).is_some());

    Ok(json!({
        "element": el.id.0,
        "layer": lid.0,
        "symbol": sym_id.0,
        "frame": playhead,
        "frame_index": frame_index,
        "loop_mode": loop_mode_key(next_loop),
        "keyframe_created": !was_keyframe && now_keyframe,
    }))
}
