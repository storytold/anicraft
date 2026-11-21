//! Timeline and layer commands.

use crate::params::*;
use crate::{CommandSpec, EngineError, MAX_FRAME, Result, Session};
use anicraft_doc::{ElementId, Keyframe, Layer, LayerId, LayerKind, Tween, TweenKind, evaluate_layer_elements};
use serde_json::{Value, json};
use std::sync::Arc;

pub fn specs() -> Vec<CommandSpec> {
    let c = |id, label, menu, shortcut, params, mutates, run| CommandSpec { id, label, menu, shortcut, params, mutates, run };
    vec![
        c("timeline.insert_frame", "Frame", "Insert/Timeline", Some("F5"), "{layer? | layers?: [..] | all_layers?: true (every layer with frames at or after the frame, as F5 with no frame selection; the active layer when none has), frame?, count?, advance?: true (the playhead moves onto the last frame inserted, as the key does in the editor)}", true, insert_frame as crate::RunFn),
        c("timeline.remove_frame", "Remove Frame", "Insert/Timeline", Some("Shift+F5"), "{layer? | layers?: [..] | all_layers?: true (every layer with frames at or after the frame, as Shift+F5 with no frame selection: the layers stay in step), frame?, count?}", true, remove_frame),
        c("timeline.insert_keyframe", "Keyframe", "Insert/Timeline", Some("F6"), "{layer? | layers?: [..], frame?, count? (every frame of the range becomes a keyframe), advance?: true (on a keyframe the new key goes on the next frame, and the playhead moves onto it, as the key does in the editor)}", true, insert_keyframe),
        c("timeline.insert_blank_keyframe", "Blank Keyframe", "Insert/Timeline", Some("F7"), "{layer? | layers?: [..], frame?, count? (every frame of the range becomes a blank keyframe; keyframes already there are kept), advance?: true (on a keyframe the blank key goes on the next frame instead of clearing it, and the playhead moves onto it)}", true, insert_blank_keyframe),
        c("timeline.clear_keyframe", "Clear Keyframe", "Insert/Timeline", Some("Shift+F6"), "{layer? | layers?: [..] | all_layers?: true (every layer with a keyframe there, as Shift+F6 with no frame selection), frame?, count?}", true, clear_keyframe),
        c("timeline.move_keyframes", "Move Keyframes", "", None, "{layer? | layers?: [..], from, to?, by, layer_by? (rows down; negative = up)}", true, move_keyframes),
        c("timeline.move_spans", "Move Spans", "", None, "{layer? | layers?: [..], from, to?, by, layer_by? (rows down; negative = up), copy?: true (Alt-drag: the frames stay where they were too)}: the spans starting in from..=to move whole, keeping their lengths and tweens (as dragging a tween span in Animate), replacing what they land on; the frames they leave are empty", true, move_spans),
        c("timeline.create_motion_tween", "Create Motion Tween", "Insert", None, "{layer?, frame?} (several objects on the key become one graphic symbol; a one-frame span grows to a second)", true, crate::cmds_motion::create),
        c("timeline.create_tween", "Create Classic Tween", "Insert", None, "{layer?, frame?, easing? (as timeline.set_easing takes it)}", true, create_tween),
        c("timeline.create_shape_tween", "Create Shape Tween", "Insert", None, "{layer?, frame?, easing? (as timeline.set_easing takes it)}", true, create_shape_tween),
        c("timeline.remove_tween", "Remove Tween", "Insert", None, "{layer?, frame?} (a motion tween leaves its object as it is on the first frame)", true, remove_tween),
        c("timeline.set_easing", "Set Tween Easing", "", None, "{easing: a preset name (Linear, QuadIn … ElasticInOut) | {classic: -100..100} | {bezier: [x1, y1, x2, y2]} | {custom: {points: [..]} | [[x, y], ..]} | {saved: name} | {plugin: id, ease: id} (an ease plug-in's; the tween keeps its sampled curve), group?: all|position|rotation|scale|color|filters (classic tweens: that property group's own ease), layer?, frame?}", true, set_easing),
        c("timeline.set_orient", "Orient to Path", "", None, "{orient: bool, layer?, frame?}", true, set_orient),
        c(
            "timeline.set_keep_area",
            "Keep Area",
            "",
            None,
            "{keep_area: bool, layer?, frame?}: squash and stretch that keeps the object's area (ADR 0115). The span's tween (classic, shape or motion) moves scale X and Y geometrically between keys instead of in straight lines, so keys of one area keep it on every frame; on a motion tween a scale axis without keys of its own follows the other inversely. Off by default",
            true,
            set_keep_area,
        ),
        c("timeline.set_label", "Frame Label", "", None, "{label, layer?, frame?}", true, set_label),
        c("timeline.goto", "Go to Frame", "", None, "{frame}", false, goto),
        c("timeline.next_frame", "Next Frame", "Control", Some("."), "{}", false, |s, _| step(s, 1)),
        c("timeline.prev_frame", "Previous Frame", "Control", Some(","), "{}", false, |s, _| step(s, -1)),
        c("timeline.first_frame", "Go to First Frame", "Control", Some("Home"), "{}", false, |s, _| {
            s.set_frame(0);
            Ok(json!({"frame": 0}))
        }),
        c("timeline.last_frame", "Go to Last Frame", "Control", Some("End"), "{}", false, |s, _| {
            let f = s.frame_count().saturating_sub(1);
            s.set_frame(f);
            Ok(json!({"frame": f}))
        }),
        c("layer.new", "New Layer", "Insert/Timeline", None, "{name?}", true, new_layer),
        c("layer.new_folder", "New Folder", "Insert/Timeline", None, "{name?}", true, new_folder),
        c("layer.delete", "Delete Layer", "Modify/Timeline", None, "{layer? (a folder goes with its contents)}", true, delete_layer),
        c("layer.rename", "Rename Layer", "", None, "{name, layer?}", true, rename_layer),
        c("layer.toggle_visible", "Show/Hide Layer", "", None, "{layer?, on?: bool (default: the other way), all?: true (every layer but the camera, as the eye above the layers: hides them all when all show, else shows them all)}", true, |s, p| toggle(s, p, |l| &mut l.visible, true)),
        c("layer.toggle_lock", "Lock/Unlock Layer", "", None, "{layer?, on?: bool (default: the other way), all?: true (every layer but the camera, as the padlock above the layers: unlocks them all when all are locked, else locks them all)}", true, |s, p| toggle(s, p, |l| &mut l.locked, false)),
        c("layer.toggle_outline", "Outline Layer", "", None, "{layer?, on?: bool (default: the other way), all?: true (every layer but the camera, as the square above the layers: back to artwork when all are outlines, else all as outlines)}", true, |s, p| toggle(s, p, |l| &mut l.outline, false)),
        c("layer.select", "Select Layer", "", None, "{layer: id} or {index}, select?: bool (also select all of its artwork at the playhead, as a click on the layer's name does; nothing on a locked or hidden layer)", false, select_layer),
        c("layer.move_up", "Move Layer Up", "Modify/Timeline", None, "{layer?}", true, |s, p| move_layer(s, p, -1)),
        c("layer.move_down", "Move Layer Down", "Modify/Timeline", None, "{layer?}", true, |s, p| move_layer(s, p, 1)),
        c(
            "layer.move",
            "Move Layer To",
            "",
            None,
            "{layer?, before?: layer id | after?: layer id | into?: folder id} moves a layer anywhere in the stack, as a drag in the timeline does; a folder, mask or guide moves with everything in it. before / after: it lands just above that layer, or just below it and whatever it holds, beside it (in the same folder, or under the same mask or guide: normal layers only). into: it becomes the first layer in that folder. None of them: the bottom of the stack, outside every folder",
            true,
            move_layer_to,
        ),
        c("layer.toggle_mask", "Mask", "Modify/Timeline", None, "{layer?}", true, toggle_mask),
        c("layer.set_type", "Layer Type", "", None, "{type: normal|mask|masked|guide|guided, layer?}", true, set_layer_type),
        c("layer.add_motion_guide", "Add Classic Motion Guide", "Insert/Timeline", None, "{layer?}", true, add_motion_guide),
    ]
}

/// The mask command: turns a layer into a mask over the normal layer directly under it, or a
/// mask back into a normal layer (its masked layers become normal too).
fn toggle_mask(s: &mut Session, p: &Value) -> Result<Value> {
    let id = opt_u64(p, "layer")?.map_or(s.layer, LayerId);
    let is_mask = s.timeline().layer(id).ok_or_else(|| EngineError::State(format!("no layer with id {}", id.0)))?.kind == LayerKind::Mask;
    let ty = if is_mask { "normal" } else { "mask" };
    set_type(s, id, ty)?;
    if !is_mask {
        let tl = crate::editing::edited_mut(&mut s.doc, s.scene, &s.editing)?;
        // The normal layer directly below, in the same folder, becomes masked.
        let mask_parent = tl.layer(id).and_then(|m| m.parent);
        if let Some(i) = tl.layers.iter().position(|l| l.id == id)
            && let Some(below) = tl.layers.get_mut(i + 1)
            && below.kind == LayerKind::Normal
            && below.parent == mask_parent
        {
            Arc::make_mut(below).parent = Some(id);
        }
    }
    Ok(layer_type_json(s, id))
}

/// Add Classic Motion Guide: a guide layer above the layer, which the layer then follows.
/// Draw a line on the guide; the layer's classic tweens travel along it.
fn add_motion_guide(s: &mut Session, p: &Value) -> Result<Value> {
    let id = opt_u64(p, "layer")?.map_or(s.layer, LayerId);
    let tl = s.timeline();
    let i = tl.layers.iter().position(|l| l.id == id).ok_or_else(|| EngineError::State(format!("no layer with id {}", id.0)))?;
    let Some(l) = tl.layers.get(i) else { return Err(EngineError::State("no such layer".into())) };
    if l.kind != LayerKind::Normal {
        return Err(EngineError::State("a motion guide guides a normal layer".into()));
    }
    if tl.layers.len() >= 1000 {
        return Err(EngineError::State("too many layers (limit 1000)".into()));
    }
    // The guide sits where the layer was, beside it in its folder; a masked layer's guide
    // would break the mask, so it can't have one.
    if l.parent.and_then(|pid| tl.layer(pid)).is_some_and(|x| x.kind == LayerKind::Mask) {
        return Err(EngineError::State("a masked layer can't also follow a motion guide".into()));
    }
    let folder = tl.folder_of(i);
    let name: String = format!("Guide: {}", l.name).chars().take(128).collect();
    let gid = LayerId(s.doc.alloc_id());
    let mut guide = Layer::new(gid, name);
    guide.kind = LayerKind::Guide;
    guide.parent = folder;
    guide.color = anicraft_doc::Rgba::hex(0x9e9e9e);
    if let Some(k) = guide.keyframe_mut(0) {
        k.duration = s.frame_count().max(1);
    }
    let tl = crate::editing::edited_mut(&mut s.doc, s.scene, &s.editing)?;
    tl.layers.insert(i, Arc::new(guide));
    if let Some(l) = tl.layers.get_mut(i + 1) {
        Arc::make_mut(l).parent = Some(gid);
    }
    tl.repair_masks();
    s.layer = gid;
    s.selection.clear();
    Ok(json!({"guide": gid.0, "guided": id.0}))
}

/// Orient to Path: a guided tween turns with the guide as it bends.
fn set_orient(s: &mut Session, p: &Value) -> Result<Value> {
    let orient = opt_bool(p, "orient")?.ok_or_else(|| EngineError::Param("param 'orient' (true or false) is required".into()))?;
    let k = keyframe_mut(s, p)?;
    let t = k.tween.as_mut().ok_or_else(|| EngineError::State("this span has no tween; create one first".into()))?;
    t.orient = orient;
    Ok(json!({"orient": orient}))
}

/// Keep area (ADR 0115): the span's tween, of any kind, scales geometrically.
fn set_keep_area(s: &mut Session, p: &Value) -> Result<Value> {
    let on = opt_bool(p, "keep_area")?.ok_or_else(|| EngineError::Param("param 'keep_area' (true or false) is required".into()))?;
    let k = keyframe_mut(s, p)?;
    let kind = if let Some(m) = k.motion.as_mut() {
        m.keep_area = on;
        "motion"
    } else if let Some(t) = k.tween.as_mut() {
        t.keep_area = on;
        if t.kind == TweenKind::Shape { "shape" } else { "classic" }
    } else {
        return Err(EngineError::State("this span has no tween; create one first".into()));
    };
    Ok(json!({"keep_area": on, "tween": kind}))
}

fn set_layer_type(s: &mut Session, p: &Value) -> Result<Value> {
    let id = opt_u64(p, "layer")?.map_or(s.layer, LayerId);
    set_type(s, id, str_req(p, "type")?)?;
    Ok(layer_type_json(s, id))
}

pub(crate) fn set_type(s: &mut Session, id: LayerId, ty: &str) -> Result<()> {
    let tl = crate::editing::edited_mut(&mut s.doc, s.scene, &s.editing)?;
    let i = tl.layers.iter().position(|l| l.id == id).ok_or_else(|| EngineError::State(format!("no layer with id {}", id.0)))?;
    if tl.layers.get(i).is_some_and(|l| l.kind == LayerKind::Folder) {
        return Err(EngineError::State("a folder holds layers; it can't become another kind of layer".into()));
    }
    if tl.layers.get(i).is_some_and(|l| l.kind == LayerKind::Camera) {
        return Err(EngineError::State("the camera layer holds the camera; it can't become another kind of layer (Remove Camera takes it away)".into()));
    }
    if tl.layers.get(i).is_some_and(|l| l.kind == LayerKind::Armature) {
        return Err(EngineError::State("an armature layer holds its armature; it can't become another kind of layer (removing its bones frees the instances)".into()));
    }
    // Whatever the type, the layer stays in the folder it is in.
    let folder = tl.folder_of(i);
    let (kind, parent) = match ty {
        "normal" => (LayerKind::Normal, folder),
        "mask" => (LayerKind::Mask, folder),
        "guide" => (LayerKind::Guide, folder),
        "masked" => {
            let m = tl.mask_above(i).ok_or_else(|| EngineError::State("only a layer directly under a mask layer (or its masked layers) can be masked".into()))?;
            (LayerKind::Normal, Some(m))
        }
        "guided" => {
            let g = tl.guide_above(i).ok_or_else(|| EngineError::State("only a layer directly under a motion guide (or a layer it guides) can be guided".into()))?;
            (LayerKind::Normal, Some(g))
        }
        other => return Err(EngineError::Param(format!("unknown layer type '{other}' (normal, mask, masked, guide or guided)"))),
    };
    let l = tl.layer_mut(id).ok_or_else(|| EngineError::State(format!("no layer with id {}", id.0)))?;
    l.kind = kind;
    l.parent = parent;
    tl.repair_masks();
    Ok(())
}

pub(crate) fn layer_type_json(s: &Session, id: LayerId) -> Value {
    let Some(l) = s.timeline().layer(id) else { return json!({}) };
    let tl = s.timeline();
    let parent_kind = l.parent.and_then(|p| tl.layer(p)).map(|x| x.kind);
    let masked_by = l.parent.filter(|_| parent_kind == Some(LayerKind::Mask));
    let guided_by = l.parent.filter(|_| parent_kind == Some(LayerKind::Guide));
    let ty = match l.kind {
        LayerKind::Mask => "mask",
        LayerKind::Guide => "guide",
        LayerKind::Folder => "folder",
        LayerKind::Camera => "camera",
        LayerKind::Armature => "armature",
        LayerKind::Normal if masked_by.is_some() => "masked",
        LayerKind::Normal if guided_by.is_some() => "guided",
        LayerKind::Normal => "normal",
    };
    let folder = tl.layers.iter().position(|x| x.id == id).and_then(|i| tl.folder_of(i));
    json!({"layer": id.0, "type": ty, "masked_by": masked_by.map(|p| p.0), "guided_by": guided_by.map(|p| p.0), "folder": folder.map(|f| f.0)})
}

pub(crate) fn target(s: &Session, p: &Value) -> Result<(Option<LayerId>, u32)> {
    let layer = opt_u64(p, "layer")?.map(LayerId);
    let frame = opt_u32(p, "frame")?.unwrap_or(s.frame);
    if frame > MAX_FRAME {
        return Err(EngineError::Param(format!("param 'frame' must be at most {MAX_FRAME}")));
    }
    Ok((layer, frame))
}

/// Layers a range command acts on: `layers: [ids]`, else `layer`, else the active layer.
pub(crate) fn target_layers(s: &Session, p: &Value) -> Result<Vec<LayerId>> {
    let ids = match opt_u64_list(p, "layers")? {
        Some(v) if !v.is_empty() => v.into_iter().map(LayerId).collect(),
        Some(_) => return Err(EngineError::Param("param 'layers' must not be empty".into())),
        None => vec![opt_u64(p, "layer")?.map_or(s.layer, LayerId)],
    };
    let mut out: Vec<LayerId> = Vec::with_capacity(ids.len());
    let mut folders = 0;
    for id in ids {
        let Some(l) = s.timeline().layer(id) else { return Err(EngineError::State(format!("no layer with id {}", id.0))) };
        // Folders have no frames: a range across a folder row acts on the layers only.
        if l.kind == LayerKind::Folder {
            folders += 1;
            continue;
        }
        if !out.contains(&id) {
            out.push(id);
        }
    }
    if out.is_empty() && folders > 0 {
        return Err(EngineError::State("a folder holds layers, not frames; pick a layer inside it".into()));
    }
    Ok(out)
}

/// Frames a range command spans (`count`, default 1).
pub(crate) fn count_param(p: &Value) -> Result<u32> {
    let n = opt_u32(p, "count")?.unwrap_or(1);
    if !(1..=10_000).contains(&n) {
        return Err(EngineError::Param("param 'count' must be between 1 and 10,000".into()));
    }
    Ok(n)
}

/// Shifts keyframes starting at or after `from` by `delta` frames.
pub(crate) fn shift_after(layer: &mut Layer, from: u32, delta: i64) {
    for k in layer.keyframes.iter_mut().filter(|k| k.start >= from).map(Arc::make_mut) {
        k.start = (i64::from(k.start) + delta).clamp(0, i64::from(MAX_FRAME)) as u32;
    }
}

/// F5 over a range: `count` frames on each target layer. With `all_layers` (F5 with no frame
/// selection) every layer that has frames at or after `frame` grows, so the layers stay in step;
/// layers that end before it are left alone. With `advance` the playhead moves onto the last
/// frame inserted, as holding F5 walks along the timeline.
fn insert_frame(s: &mut Session, p: &Value) -> Result<Value> {
    let (_, frame) = target(s, p)?;
    let count = count_param(p)?;
    let all = opt_bool(p, "all_layers")?.unwrap_or(false);
    let ids = if all { layers_at_or_after(s, frame) } else { target_layers(s, p)? };
    let mut frames = 0;
    for &id in &ids {
        let layer = s.layer_mut(Some(id))?;
        if layer.kind == LayerKind::Folder {
            return Err(EngineError::State("a folder holds layers, not frames; pick a layer inside it".into()));
        }
        for _ in 0..count {
            if all && layer.keyframe_index_at(frame).is_none() && layer.end_frame() > frame {
                // A gap (or the frames before the first key) under the playhead: insert an
                // empty frame there, so what follows moves along with the other layers.
                insert_empty_frame_at(layer, frame)?;
            } else {
                insert_frame_at(layer, frame)?;
            }
        }
        frames = frames.max(layer.end_frame());
    }
    let mut out = json!({"frames": frames, "layers": ids.iter().map(|l| l.0).collect::<Vec<_>>()});
    if opt_bool(p, "advance")?.unwrap_or(false) {
        s.set_frame(frame.saturating_add(count).min(frames.saturating_sub(1)));
        out["playhead"] = json!(s.frame);
    }
    Ok(out)
}

/// The layers F5 with no frame selection extends: every layer of the timeline with frames at or
/// after `frame` (folders have none). When no layer reaches that far, the active layer, so F5
/// past the end still lengthens what you are working on.
pub(crate) fn layers_at_or_after(s: &Session, frame: u32) -> Vec<LayerId> {
    let tl = s.timeline();
    let ids: Vec<LayerId> = tl.layers.iter().filter(|l| l.kind != LayerKind::Folder && l.end_frame() > frame).map(|l| l.id).collect();
    if ids.is_empty() { vec![s.layer] } else { ids }
}

/// One empty frame at `frame`: everything from there on moves one frame later.
fn insert_empty_frame_at(layer: &mut Layer, frame: u32) -> Result<()> {
    if layer.end_frame() >= MAX_FRAME {
        return Err(EngineError::State("the timeline is at its maximum length".into()));
    }
    shift_after(layer, frame, 1);
    layer.normalize();
    Ok(())
}

pub(crate) fn insert_frame_at(layer: &mut Layer, frame: u32) -> Result<()> {
    if layer.end_frame() >= MAX_FRAME {
        return Err(EngineError::State("the timeline is at its maximum length".into()));
    }
    match layer.keyframe_index_at(frame) {
        Some(i) => {
            if let Some(k) = layer.keyframe_mut(i) {
                k.duration += 1;
            }
            shift_after(layer, frame + 1, 1);
        }
        None => match layer.keyframes.iter().rposition(|k| k.start <= frame) {
            // extend the previous keyframe up to and including `frame`
            Some(i) => {
                let next_start = layer.keyframes.get(i + 1).map(|k| k.start);
                if let Some(k) = layer.keyframe_mut(i) {
                    let want = frame + 1 - k.start;
                    k.duration = next_start.map_or(want, |n| want.min(n - k.start));
                }
            }
            None => layer.keyframes.insert(0, Arc::new(Keyframe::new(frame, 1))),
        },
    }
    layer.normalize();
    Ok(())
}

/// The layers Shift+F5 and Shift+F6 act on: with `all_layers` (the key with no frame selection,
/// #438) every layer with frames at or after `frame`, as F5 extends them; else the target layers.
pub(crate) fn removal_layers(s: &Session, p: &Value, frame: u32) -> Result<Vec<LayerId>> {
    if opt_bool(p, "all_layers")?.unwrap_or(false) { Ok(layers_at_or_after(s, frame)) } else { target_layers(s, p) }
}

/// Shift+F5 over a range: removes up to `count` frames on each target layer. Layers that run
/// out of frames stop early; it fails only when nothing could be removed. With `all_layers`
/// every layer that reaches the frame loses them, so the layers stay in step: a layer with no
/// frame there (a gap, or frames that start later) closes up by an empty frame.
pub(crate) fn remove_frame(s: &mut Session, p: &Value) -> Result<Value> {
    let (_, frame) = target(s, p)?;
    let count = count_param(p)?;
    let all = opt_bool(p, "all_layers")?.unwrap_or(false);
    let mut removed = 0u32;
    let mut first_err = None;
    for id in removal_layers(s, p, frame)? {
        let layer = s.layer_mut(Some(id))?;
        for _ in 0..count {
            if all && layer.keyframe_index_at(frame).is_none() {
                // An empty frame under the playhead: what follows moves one frame earlier.
                if layer.end_frame() <= frame + 1 {
                    break;
                }
                shift_after(layer, frame + 1, -1);
                layer.normalize();
                removed += 1;
                continue;
            }
            match remove_frame_at(layer, frame) {
                Ok(()) => removed += 1,
                Err(e) => {
                    first_err.get_or_insert(e);
                    break;
                }
            }
        }
    }
    match (removed, first_err) {
        (0, Some(e)) => Err(e),
        _ => {
            let frames = s.frame_count();
            let mut out = json!({"removed": removed, "frames": frames});
            // The last frames went from under the playhead: it steps back onto the new last
            // frame, so holding Shift+F5 keeps shortening the timeline as F5 lengthens it.
            if all && s.frame >= frames {
                s.set_frame(frames.saturating_sub(1));
                out["playhead"] = json!(s.frame);
            }
            Ok(out)
        }
    }
}

fn remove_frame_at(layer: &mut Layer, frame: u32) -> Result<()> {
    let i = layer.keyframe_index_at(frame).ok_or_else(|| EngineError::State(format!("frame {} is empty on this layer", frame + 1)))?;
    let dur = layer.keyframes.get(i).map_or(1, |k| k.duration);
    if dur > 1 {
        if let Some(k) = layer.keyframe_mut(i) {
            k.duration -= 1;
        }
    } else {
        if layer.keyframes.len() == 1 {
            return Err(EngineError::State("cannot remove the layer's only frame".into()));
        }
        layer.keyframes.remove(i);
    }
    shift_after(layer, frame + 1, -1);
    layer.normalize();
    Ok(())
}

/// Inserts a keyframe at `frame` (F6 when `copy`, F7 otherwise). Inside a tween the new key
/// captures the tweened state, so the motion is unchanged.
pub(crate) fn insert_keyframe_at(layer: &mut Layer, frame: u32, copy: bool) -> Result<()> {
    if frame > MAX_FRAME {
        return Err(EngineError::Param(format!("frame must be at most {MAX_FRAME}")));
    }
    if layer.keyframe_starting_at(frame).is_some() {
        if !copy {
            // F7 on a keyframe clears it
            if let Some(k) = layer.keyframe_starting_at(frame).and_then(|i| layer.keyframe_mut(i)) {
                k.elements.clear();
                k.tween = None;
                k.motion = None;
                k.pose = anicraft_doc::Pose::default();
            }
        }
        return Ok(());
    }
    let elements = if copy { evaluate_layer_elements(layer, frame).map(|(_, e)| e.into_owned()) } else { None };
    // On an armature layer a copy keeps the pose the frame shows (tweened between keys).
    let pose = if copy { anicraft_doc::armature::pose_at(layer, frame) } else { None };
    // Past the end of a motion tween, a copy starts from how its object looks at the end.
    let last_state = match layer.keyframes.iter().rposition(|k| k.start < frame) {
        Some(i) if copy && layer.keyframe_index_at(frame).is_none() => {
            layer.keyframes.get(i).filter(|k| k.motion.is_some()).and_then(|k| evaluate_layer_elements(layer, k.end() - 1)).map(|(_, e)| e.into_owned())
        }
        _ => None,
    };
    match layer.keyframe_index_at(frame) {
        Some(i) => {
            let Some(k) = layer.keyframe_mut(i) else { return Ok(()) };
            let end = k.end();
            let tween = if copy { k.tween.clone() } else { None };
            // A motion tween split in two keeps moving the same way in both halves.
            let motion = if copy {
                k.motion.as_ref().map(|m| {
                    let base = m.target_index(&k.elements).and_then(|t| k.elements.get(t)).map_or(anicraft_doc::Props([0.0; anicraft_doc::PROPERTIES]), |e| anicraft_doc::Props::of(e));
                    m.split_at(&base, frame - k.start)
                })
            } else {
                None
            };
            k.duration = frame - k.start;
            let mut nk = Keyframe::new(frame, end - frame);
            nk.elements = elements.unwrap_or_default();
            nk.tween = tween;
            nk.motion = motion;
            nk.pose = pose.unwrap_or_default();
            layer.keyframes.insert(i + 1, Arc::new(nk));
        }
        None => {
            // Past the end or in a gap: extend the previous key and copy its last state.
            let prev = layer.keyframes.iter().rposition(|k| k.start < frame);
            let mut nk = Keyframe::new(frame, 1);
            if let Some(i) = prev {
                let next_start = layer.keyframes.get(i + 1).map(|k| k.start);
                if let Some(k) = layer.keyframe_mut(i) {
                    if copy {
                        nk.elements = last_state.unwrap_or_else(|| k.elements.clone());
                        nk.pose = k.pose.clone();
                    }
                    k.duration = frame - k.start;
                }
                if let Some(n) = next_start {
                    nk.duration = n - frame;
                }
            }
            let pos = layer.keyframes.partition_point(|k| k.start < frame);
            layer.keyframes.insert(pos, Arc::new(nk));
        }
    }
    layer.normalize();
    Ok(())
}

/// F6. Over a range (`count`, as with frames selected in the editor) every frame in it becomes a
/// keyframe holding what that frame showed, as Animate's Convert to Keyframes does (#272).
fn insert_keyframe(s: &mut Session, p: &Value) -> Result<Value> {
    let frame = key_frame(s, p)?;
    let last = last_key_frame(frame, count_param(p)?);
    for id in target_layers(s, p)? {
        let layer = s.layer_mut(Some(id))?;
        for f in frame..=last {
            // Inside a motion tween F6 keys every property there; the span stays whole.
            if !crate::cmds_motion::key_all_at(layer, f) {
                insert_keyframe_at(layer, f, true)?;
            }
        }
    }
    advanced(s, p, frame)
}

/// F7. Over a range (`count`) every frame in it becomes a blank keyframe, ready for
/// frame-by-frame drawing (#272). Keyframes already in the range stay as they are: a range edit
/// makes the keys that are missing and never erases a drawing (one frame alone, without
/// `advance`, still clears the keyframe it is on).
fn insert_blank_keyframe(s: &mut Session, p: &Value) -> Result<Value> {
    let frame = key_frame(s, p)?;
    let last = last_key_frame(frame, count_param(p)?);
    for id in target_layers(s, p)? {
        let layer = s.layer_mut(Some(id))?;
        for f in frame..=last {
            if last > frame && layer.keyframe_starting_at(f).is_some() {
                continue;
            }
            insert_keyframe_at(layer, f, false)?;
        }
    }
    advanced(s, p, frame)
}

/// The last frame of a `count`-frame range starting at `frame`, kept inside the timeline.
fn last_key_frame(frame: u32, count: u32) -> u32 {
    frame.saturating_add(count.saturating_sub(1)).min(MAX_FRAME)
}

/// Where F6 / F7 put the key. With `advance`, a frame that is already a keyframe on every target
/// layer means "the next one": pressing F6 again after drawing makes the next drawing's key.
fn key_frame(s: &Session, p: &Value) -> Result<u32> {
    let (_, frame) = target(s, p)?;
    if !opt_bool(p, "advance")?.unwrap_or(false) {
        return Ok(frame);
    }
    let ids = target_layers(s, p)?;
    let tl = s.timeline();
    let on_keys = !ids.is_empty() && ids.iter().all(|id| tl.layer(*id).is_some_and(|l| l.keyframe_starting_at(frame).is_some()));
    if on_keys && frame < MAX_FRAME { Ok(frame + 1) } else { Ok(frame) }
}

/// The result of F6 / F7; with `advance` the playhead moves onto the new key.
fn advanced(s: &mut Session, p: &Value, frame: u32) -> Result<Value> {
    let mut out = json!({"keyframe": frame});
    if opt_bool(p, "advance")?.unwrap_or(false) {
        s.set_frame(frame);
        out["playhead"] = json!(frame);
    }
    Ok(out)
}

/// Shift+F6 over a range: clears every keyframe starting in `frame..frame + count` on each
/// target layer.
fn clear_keyframe(s: &mut Session, p: &Value) -> Result<Value> {
    let (_, frame) = target(s, p)?;
    let count = count_param(p)?;
    let mut cleared = 0;
    for id in removal_layers(s, p, frame)? {
        let layer = s.layer_mut(Some(id))?;
        let starts: Vec<u32> = layer.keyframes.iter().map(|k| k.start).filter(|&f| f >= frame && f - frame < count).collect();
        // Last first, so each removal merges into a span that is still there.
        for f in starts.into_iter().rev() {
            clear_keyframe_at(layer, f)?;
            cleared += 1;
        }
    }
    if cleared == 0 {
        return Err(EngineError::State(if count == 1 { format!("frame {} is not a keyframe", frame + 1) } else { "no keyframes in the selected frames".into() }));
    }
    Ok(json!({"cleared": cleared}))
}

/// Moves the keyframes starting in `from..=to` by `by` frames (dragging a frame selection).
/// Keyframes they land on are replaced; the span before a key moved right grows to fill the
/// frames it left, and the layer keeps its length (no frames are lost by moving left).
fn move_keyframes(s: &mut Session, p: &Value) -> Result<Value> {
    let from = opt_u32(p, "from")?.ok_or_else(|| EngineError::Param("param 'from' is required".into()))?;
    let to = opt_u32(p, "to")?.unwrap_or(from);
    let by = p.get("by").and_then(Value::as_i64).ok_or_else(|| EngineError::Param("param 'by' must be a whole number of frames".into()))?;
    if to < from || to > MAX_FRAME || by.unsigned_abs() > u64::from(MAX_FRAME) {
        return Err(EngineError::Param(format!("frames must be in order and within {MAX_FRAME}")));
    }
    let layer_by = match p.get("layer_by") {
        None | Some(Value::Null) => 0,
        Some(v) => v.as_i64().ok_or_else(|| EngineError::Param("param 'layer_by' must be a whole number of layers".into()))?,
    };
    if layer_by != 0 {
        return move_across_layers(s, p, from, to, by, layer_by, false);
    }
    let mut moved = 0;
    for id in target_layers(s, p)? {
        moved += move_keys(s.layer_mut(Some(id))?, from, to, by)?;
    }
    Ok(json!({"moved": moved}))
}

/// Moving whole spans (ADR 0083): the spans starting in `from..=to` keep their lengths, motion
/// and tweens, and land `by` frames later and `layer_by` rows down, replacing what was there.
/// `copy` (Alt-drag) leaves the originals where they were.
fn move_spans(s: &mut Session, p: &Value) -> Result<Value> {
    let from = opt_u32(p, "from")?.ok_or_else(|| EngineError::Param("param 'from' is required".into()))?;
    let to = opt_u32(p, "to")?.unwrap_or(from);
    let by = p.get("by").and_then(Value::as_i64).ok_or_else(|| EngineError::Param("param 'by' must be a whole number of frames".into()))?;
    if to < from || to > MAX_FRAME || by.unsigned_abs() > u64::from(MAX_FRAME) {
        return Err(EngineError::Param(format!("frames must be in order and within {MAX_FRAME}")));
    }
    let layer_by = match p.get("layer_by") {
        None | Some(Value::Null) => 0,
        Some(v) => v.as_i64().ok_or_else(|| EngineError::Param("param 'layer_by' must be a whole number of layers".into()))?,
    };
    let copy = opt_bool(p, "copy")?.unwrap_or(false);
    if by == 0 && layer_by == 0 {
        if copy {
            return Err(EngineError::State("the copy would land on the frames it copies: drag it somewhere else".into()));
        }
        return Ok(json!({"moved": 0}));
    }
    move_across_layers(s, p, from, to, by, layer_by, copy)
}

/// Dragging keyframes onto other layers (`layer_by` rows down, negative = up), or whole spans
/// along their own: the frames are lifted out of every source layer first (leaving blank
/// keyframes, as in Animate; a `copy` leaves them be), then placed on the target layers,
/// replacing what was there; frames after the landing range keep their content. Lifting
/// everything before placing anything makes overlapping row ranges (dragging two rows down by
/// one) come out right.
fn move_across_layers(s: &mut Session, p: &Value, from: u32, to: u32, by: i64, layer_by: i64, copy: bool) -> Result<Value> {
    let order: Vec<LayerId> = s.timeline().layers.iter().map(|l| l.id).collect();
    let mut rows: Vec<usize> = target_layers(s, p)?.iter().filter_map(|id| order.iter().position(|o| o == id)).collect();
    rows.sort_unstable();
    let mut plan = Vec::with_capacity(rows.len());
    for &r in &rows {
        let t = i64::try_from(r).unwrap_or(i64::MAX).checked_add(layer_by).filter(|t| *t >= 0 && (*t as usize) < order.len());
        let Some(t) = t else { return Err(EngineError::State("there is no layer there to move the frames to".into())) };
        let (Some(&src), Some(&dst)) = (order.get(r), order.get(t as usize)) else { continue };
        plan.push((src, dst));
    }
    // 1. Lift.
    let mut lifted: Vec<(LayerId, Vec<Keyframe>)> = Vec::new();
    for &(src, dst) in &plan {
        let layer = s.layer_mut(Some(src))?;
        let keys: Vec<Keyframe> = layer.keyframes.iter().filter(|k| k.start >= from && k.start <= to).map(|k| (**k).clone()).collect();
        if !copy
            && let (Some(first), Some(last)) = (keys.first().map(|k| k.start), keys.last().map(Keyframe::end))
        {
            layer.keyframes.retain(|k| !(k.start >= from && k.start <= to));
            // The frames stay; their content leaves.
            layer.keyframes.push(Arc::new(Keyframe::new(first, last - first)));
            layer.normalize();
        }
        lifted.push((dst, keys));
    }
    // 2. Place.
    let mut moved = 0;
    for (dst, keys) in lifted {
        let (Some(first), Some(last)) = (keys.first().map(|k| k.start), keys.last().map(Keyframe::end)) else { continue };
        let (a, b) = (i64::from(first) + by, i64::from(last) + by);
        if a < 0 || b > i64::from(MAX_FRAME) + 1 {
            return Err(EngineError::State(if a < 0 { "keyframes cannot move before frame 1".into() } else { "keyframes would move past the end of the timeline".into() }));
        }
        let (a, b) = (a as u32, b as u32);
        let layer = s.layer_mut(Some(dst))?;
        crate::cmds_camera::keys_fit(layer.kind, keys.iter())?;
        // Cut the target at both ends of the landing range so what lies outside it stays.
        if layer.keyframe_index_at(a).is_some_and(|i| layer.keyframes.get(i).is_some_and(|k| k.start < a)) {
            insert_keyframe_at(layer, a, true)?;
        }
        if b < layer.end_frame() && layer.keyframe_index_at(b).is_some() {
            insert_keyframe_at(layer, b, true)?;
        }
        layer.keyframes.retain(|k| !(k.start >= a && k.start < b));
        for mut k in keys {
            k.start = (i64::from(k.start) + by) as u32;
            layer.keyframes.push(Arc::new(k));
            moved += 1;
        }
        layer.normalize();
    }
    Ok(json!({"moved": moved}))
}

fn move_keys(layer: &mut Layer, from: u32, to: u32, by: i64) -> Result<usize> {
    if by == 0 {
        return Ok(0);
    }
    let end = layer.end_frame();
    let (mut moving, mut staying): (Vec<Arc<Keyframe>>, Vec<Arc<Keyframe>>) = std::mem::take(&mut layer.keyframes).into_iter().partition(|k| k.start >= from && k.start <= to);
    if moving.is_empty() {
        layer.keyframes = staying;
        return Ok(0);
    }
    let first = moving.first().map_or(0, |k| i64::from(k.start));
    let last_end = moving.last().map_or(0, |k| i64::from(k.end()));
    if first + by < 0 || last_end + by > i64::from(MAX_FRAME) + 1 {
        layer.keyframes = moving.into_iter().chain(staying).collect();
        layer.normalize();
        return Err(EngineError::State(if first + by < 0 { "keyframes cannot move before frame 1".into() } else { "keyframes would move past the end of the timeline".into() }));
    }
    for k in moving.iter_mut().map(Arc::make_mut) {
        k.start = (i64::from(k.start) + by) as u32;
    }
    let landed: Vec<u32> = moving.iter().map(|k| k.start).collect();
    staying.retain(|k| !landed.contains(&k.start));
    let n = moving.len();
    let new_end = end.max((last_end + by) as u32);
    let mut keys: Vec<Arc<Keyframe>> = staying.into_iter().chain(moving).collect();
    keys.sort_by_key(|k| k.start);
    // Each span runs to the next key; the last runs to the layer's end.
    let starts: Vec<u32> = keys.iter().map(|k| k.start).collect();
    for (i, k) in keys.iter_mut().enumerate() {
        let stop = starts.get(i + 1).copied().unwrap_or(new_end);
        let want = stop.saturating_sub(k.start).max(1);
        if k.duration != want {
            Arc::make_mut(k).duration = want;
        }
    }
    layer.keyframes = keys;
    layer.normalize();
    Ok(n)
}

fn clear_keyframe_at(layer: &mut Layer, frame: u32) -> Result<()> {
    let i = layer.keyframe_starting_at(frame).ok_or_else(|| EngineError::State(format!("frame {} is not a keyframe", frame + 1)))?;
    if i == 0 {
        if let Some(k) = layer.keyframe_mut(0) {
            k.elements.clear();
            k.tween = None;
            k.motion = None;
            k.hints.clear();
        }
    } else {
        let removed = layer.keyframes.remove(i);
        if let Some(prev) = layer.keyframe_mut(i - 1) {
            prev.duration = removed.end() - prev.start;
        }
    }
    layer.normalize();
    Ok(())
}

pub(crate) fn keyframe_mut<'a>(s: &'a mut Session, p: &Value) -> Result<&'a mut Keyframe> {
    let (lid, frame) = target(s, p)?;
    let layer = s.layer_mut(lid)?;
    let i = layer.keyframe_index_at(frame).ok_or_else(|| EngineError::State(format!("frame {} is empty on this layer", frame + 1)))?;
    layer.keyframe_mut(i).ok_or_else(|| EngineError::State("keyframe vanished".into()))
}

fn create_tween(s: &mut Session, p: &Value) -> Result<Value> {
    tween_of_kind(s, p, TweenKind::Classic)
}

/// Create Shape Tween: like a classic tween, and shapes of any structure morph into each other.
fn create_shape_tween(s: &mut Session, p: &Value) -> Result<Value> {
    tween_of_kind(s, p, TweenKind::Shape)
}

fn tween_of_kind(s: &mut Session, p: &Value, kind: TweenKind) -> Result<Value> {
    let given = opt_easing_in(s, p, "easing")?;
    let k = keyframe_mut(s, p)?;
    // Switching kinds keeps the easing the span already had, unless one is given; a classic
    // tween keeps its property groups' eases (a shape tween has one ease, as in Animate).
    let easing = given.unwrap_or_else(|| k.tween.as_ref().map(|t| t.easing.clone()).unwrap_or_default());
    let orient = k.tween.as_ref().is_some_and(|t| t.orient);
    let keep_area = k.tween.as_ref().is_some_and(|t| t.keep_area);
    let groups = if kind == TweenKind::Classic { k.tween.as_ref().map(|t| t.groups.clone()).unwrap_or_default() } else { Vec::new() };
    // It replaces a motion tween, keeping the object as it is on the first frame.
    crate::cmds_motion::bake_first_frame(k);
    let label = easing.label();
    k.tween = Some(Tween { easing, kind, orient, groups, keep_area });
    // Shape hints only mean something to a shape tween.
    if kind != TweenKind::Shape {
        k.hints.clear();
    }
    let kind = if kind == TweenKind::Shape { "shape" } else { "classic" };
    Ok(json!({"tween": k.start, "easing": label, "kind": kind}))
}

fn remove_tween(s: &mut Session, p: &Value) -> Result<Value> {
    let k = keyframe_mut(s, p)?;
    k.tween = None;
    k.hints.clear();
    crate::cmds_motion::bake_first_frame(k);
    Ok(json!({"removed": k.start}))
}

fn set_easing(s: &mut Session, p: &Value) -> Result<Value> {
    let easing = opt_easing_in(s, p, "easing")?.ok_or_else(|| EngineError::Param("param 'easing' is required".into()))?;
    let group = crate::cmds_ease::group(p)?;
    let k = keyframe_mut(s, p)?;
    let t = k.tween.as_mut().ok_or_else(|| EngineError::State("this span has no tween; create one first".into()))?;
    let label = easing.label();
    match group {
        Some(_) if t.kind == TweenKind::Shape => return Err(EngineError::State("a shape tween has one ease for everything; property groups ease separately in classic tweens".into())),
        Some(g) => t.set_ease(g, Some(easing)),
        None => t.easing = easing,
    }
    Ok(json!({"easing": label, "group": group.map_or("all", |g| g.key())}))
}

fn set_label(s: &mut Session, p: &Value) -> Result<Value> {
    let label = str_req(p, "label")?.chars().take(256).collect::<String>();
    let k = keyframe_mut(s, p)?;
    k.label = label;
    Ok(json!({"label": k.label}))
}

fn goto(s: &mut Session, p: &Value) -> Result<Value> {
    let f = opt_u32(p, "frame")?.ok_or_else(|| EngineError::Param("param 'frame' is required".into()))?;
    s.set_frame(f);
    Ok(json!({"frame": s.frame}))
}

fn step(s: &mut Session, d: i64) -> Result<Value> {
    let n = i64::from(s.frame_count().max(1));
    let f = (i64::from(s.frame) + d).rem_euclid(n);
    s.set_frame(f as u32);
    Ok(json!({"frame": s.frame}))
}

pub(crate) fn new_layer(s: &mut Session, p: &Value) -> Result<Value> {
    add_layer(s, p, LayerKind::Normal)
}

/// New Folder: an empty folder above the active layer, beside it (never inside a mask).
fn new_folder(s: &mut Session, p: &Value) -> Result<Value> {
    add_layer(s, p, LayerKind::Folder)
}

/// The colour the `n`th layer gets (its outline and timeline bar).
pub(crate) fn layer_color(n: usize) -> anicraft_doc::Rgba {
    // The first seven of the layer colours, in turn.
    const TURN: usize = 7;
    anicraft_doc::Rgba::hex(crate::LAYER_COLORS.get(n % TURN).copied().unwrap_or(0x4fa3ff))
}

fn add_layer(s: &mut Session, p: &Value, kind: LayerKind) -> Result<Value> {
    let tl = s.timeline();
    if tl.layers.len() >= 1000 {
        return Err(EngineError::State("too many layers (limit 1000)".into()));
    }
    let n = tl.layers.len() + 1;
    let folder = kind == LayerKind::Folder;
    let default_name = if folder { format!("Folder {}", tl.layers.iter().filter(|l| l.kind == LayerKind::Folder).count() + 1) } else { format!("Layer {n}") };
    let name = opt_str(p, "name")?.map(|x| x.chars().take(128).collect()).unwrap_or(default_name);
    let at = s.active_layer_index().unwrap_or(0);
    // It joins the active layer's parent: added above a masked layer it is masked too, and
    // added inside a folder it stays in the folder. A folder never goes inside a mask.
    let mut parent = tl.layers.get(at).and_then(|l| l.parent);
    if let Some(m) = parent.and_then(|pid| tl.layer(pid)).filter(|m| m.kind == LayerKind::Mask && folder) {
        parent = m.parent;
    }
    let id = LayerId(s.doc.alloc_id());
    let mut layer = Layer::new(id, name);
    layer.kind = kind;
    layer.parent = parent;
    if folder {
        // A folder has no frames of its own.
        layer.keyframes.clear();
    } else if let Some(k) = layer.keyframe_mut(0) {
        // Match the timeline length so the new layer spans the existing frames.
        k.duration = s.frame_count().max(1);
    }
    layer.color = layer_color(n);
    let tl = crate::editing::edited_mut(&mut s.doc, s.scene, &s.editing)?;
    tl.layers.insert(at, Arc::new(layer));
    tl.repair_masks();
    s.layer = id;
    s.selection.clear();
    Ok(json!({"layer": id.0}))
}

/// Deletes a layer; a folder goes with everything in it. A layer masked by a deleted mask
/// stays where it is, in the mask's folder.
fn delete_layer(s: &mut Session, p: &Value) -> Result<Value> {
    let id = opt_u64(p, "layer")?.map_or(s.layer, LayerId);
    let tl = s.timeline();
    let i = tl.layers.iter().position(|l| l.id == id).ok_or_else(|| EngineError::State(format!("no layer with id {}", id.0)))?;
    let is_folder = tl.layers.get(i).is_some_and(|l| l.kind == LayerKind::Folder);
    let range = if is_folder { tl.group_range(i) } else { i..i + 1 };
    let drawable_left = tl.layers.iter().enumerate().filter(|(j, l)| !range.contains(j) && !matches!(l.kind, LayerKind::Folder | LayerKind::Camera)).count();
    if drawable_left == 0 {
        return Err(EngineError::State("a document needs at least one layer to draw on".into()));
    }
    let up = tl.layers.get(i).and_then(|l| l.parent);
    let tl = crate::editing::edited_mut(&mut s.doc, s.scene, &s.editing)?;
    if !is_folder {
        for l in tl.layers.iter_mut().filter(|l| l.parent == Some(id)) {
            Arc::make_mut(l).parent = up;
        }
    }
    let removed: Vec<LayerId> = tl.layers.drain(range.clone()).map(|l| l.id).collect();
    tl.repair_masks();
    if removed.contains(&s.layer) {
        let j = range.start.min(s.timeline().layers.len().saturating_sub(1));
        s.layer = s.timeline().layers.get(j).map_or(LayerId(0), |l| l.id);
        s.selection.clear();
    }
    Ok(json!({"deleted": id.0, "layers": removed.len()}))
}

fn rename_layer(s: &mut Session, p: &Value) -> Result<Value> {
    let name: String = str_req(p, "name")?.chars().take(128).collect();
    if name.trim().is_empty() {
        return Err(EngineError::Param("layer name must not be empty".into()));
    }
    let lid = opt_u64(p, "layer")?.map(LayerId);
    let layer = s.layer_mut(lid)?;
    layer.name = name;
    Ok(json!({"name": layer.name}))
}

/// Show, lock or outline: one layer, or with `all` every layer but the camera (the buttons
/// above the layer names, as in Animate, #422). `rest` is the state layers start in (shown,
/// unlocked, artwork): while any layer is at rest `all` takes every layer away from it (hide,
/// lock, outline), and once none is it brings them all back, so one click gets them alike.
fn toggle(s: &mut Session, p: &Value, field: fn(&mut Layer) -> &mut bool, rest: bool) -> Result<Value> {
    let on = opt_bool(p, "on")?;
    if opt_bool(p, "all")? == Some(true) {
        let tl = crate::editing::edited_mut(&mut s.doc, s.scene, &s.editing)?;
        let read = |l: &Arc<Layer>| *field(&mut Layer::clone(l));
        let any_at_rest = tl.layers.iter().filter(|l| l.kind != LayerKind::Camera).any(|l| read(l) == rest);
        let to = on.unwrap_or(if any_at_rest { !rest } else { rest });
        let mut changed = 0;
        for l in tl.layers.iter_mut().filter(|l| l.kind != LayerKind::Camera) {
            // Only layers that change are copied (the rest stay shared with the undo history).
            if read(l) != to {
                *field(Arc::make_mut(l)) = to;
                changed += 1;
            }
        }
        return Ok(json!({"all": true, "on": to, "changed": changed}));
    }
    let lid = opt_u64(p, "layer")?.map(LayerId);
    let layer = s.layer_mut(lid)?;
    let f = field(layer);
    *f = on.unwrap_or(!*f);
    Ok(json!({"visible": layer.visible, "locked": layer.locked, "outline": layer.outline}))
}

fn select_layer(s: &mut Session, p: &Value) -> Result<Value> {
    let id = if let Some(id) = opt_u64(p, "layer")? {
        LayerId(id)
    } else if let Some(i) = opt_u64(p, "index")? {
        usize::try_from(i).ok().and_then(|i| s.timeline().layers.get(i)).map(|l| l.id).ok_or_else(|| EngineError::Param("layer index out of range".into()))?
    } else {
        return Err(EngineError::Param("pass {\"layer\": id} or {\"index\": n}".into()));
    };
    if s.timeline().layer(id).is_none() {
        return Err(EngineError::State(format!("no layer with id {}", id.0)));
    }
    // A click on a layer's name in Animate: the layer becomes active with all of its artwork at
    // the playhead selected (ADR 0044).
    if opt_bool(p, "select")? == Some(true) {
        s.layer = id;
        let ids: Vec<ElementId> = if s.selectable(id) { s.elements_on(id).iter().map(|e| e.id).collect() } else { Vec::new() };
        s.selection = crate::Selection::only(id, ids);
        return Ok(json!({"layer": id.0, "selected": s.selection.id_list()}));
    }
    if s.layer != id {
        s.layer = id;
        s.selection.clear();
    }
    Ok(json!({"layer": id.0}))
}

/// Moves a layer anywhere in the stack (a drag in the timeline, #407). A folder, mask or guide
/// goes with everything in it. The layer lands beside `before` (just above it) or `after` (just
/// below it and whatever it holds), with that layer's parent; first inside the folder `into`;
/// or at the bottom outside every folder.
fn move_layer_to(s: &mut Session, p: &Value) -> Result<Value> {
    let id = opt_u64(p, "layer")?.map_or(s.layer, LayerId);
    let before = opt_u64(p, "before")?.map(LayerId);
    let after = opt_u64(p, "after")?.map(LayerId);
    let into = opt_u64(p, "into")?.map(LayerId);
    if [before, after, into].iter().flatten().count() > 1 {
        return Err(EngineError::Param("pass one of before, after and into".into()));
    }
    let tl = crate::editing::edited_mut(&mut s.doc, s.scene, &s.editing)?;
    let pos = |tl: &anicraft_doc::Timeline, l: LayerId| tl.layers.iter().position(|x| x.id == l).ok_or_else(|| EngineError::State(format!("no layer with id {}", l.0)));
    let i = pos(tl, id)?;
    let block = tl.group_range(i);
    let (kind, was_parent) = tl.layers.get(i).map_or((LayerKind::Normal, None), |l| (l.kind, l.parent));
    if kind == LayerKind::Camera {
        return Err(EngineError::State("The camera layer stays on top".into()));
    }
    // Where it goes: the row it lands above, counted before it leaves (the row count: the
    // bottom), and its new parent.
    let (slot, parent) = if let Some(f) = into {
        let fi = pos(tl, f)?;
        if tl.layers.get(fi).is_none_or(|l| l.kind != LayerKind::Folder) {
            return Err(EngineError::State("Only a folder takes layers: pass a folder's id as into".into()));
        }
        if block.contains(&fi) {
            return Err(EngineError::State("A folder can't go inside itself".into()));
        }
        (fi + 1, Some(f))
    } else if let Some(beside) = before.or(after) {
        let bi = pos(tl, beside)?;
        if block.contains(&bi) {
            // Beside itself or something inside it: it is there already.
            return Ok(json!({"index": i, "parent": was_parent.map(|p| p.0)}));
        }
        let parent = tl.layers.get(bi).and_then(|l| l.parent);
        if kind != LayerKind::Normal && parent.and_then(|p| tl.layer(p)).is_some_and(|p| matches!(p.kind, LayerKind::Mask | LayerKind::Guide)) {
            return Err(EngineError::State("Only a normal layer can go under a mask or a guide".into()));
        }
        (if before.is_some() { bi } else { tl.group_range(bi).end }, parent)
    } else {
        (tl.layers.len(), None)
    };
    // Folders nest so deep and no deeper: refuse rather than flatten what was dragged.
    let head = tl.depth(i);
    let deepest = block.clone().map(|j| tl.depth(j).saturating_sub(head)).max().unwrap_or(0);
    let new_head = match parent {
        Some(p) => tl.depth(pos(tl, p)?) + 1,
        None => 0,
    };
    if new_head + deepest >= anicraft_doc::MAX_LAYER_DEPTH {
        return Err(EngineError::State(format!("Folders nest at most {} deep", anicraft_doc::MAX_LAYER_DEPTH)));
    }
    // A slot at the moved rows themselves is where they already are (only the parent changes).
    let slot = if block.contains(&slot) { block.end } else { slot };
    let at = if slot >= block.end { slot - block.len() } else { slot };
    let moved: Vec<Arc<Layer>> = tl.layers.drain(block).collect();
    let at = at.min(tl.layers.len());
    tl.layers.splice(at..at, moved);
    if let Some(l) = tl.layers.get_mut(at)
        && l.parent != parent
    {
        Arc::make_mut(l).parent = parent;
    }
    tl.repair_masks();
    let index = pos(tl, id)?;
    Ok(json!({"index": index, "parent": tl.layers.get(index).and_then(|l| l.parent).map(|p| p.0)}))
}

/// Moves a layer one step. A folder or mask moves with everything in it, past a whole sibling,
/// or out of its folder at either end. Any other layer swaps with its neighbour and then belongs
/// where it landed: moving it in under a folder or mask puts it inside, moving it out frees it.
fn move_layer(s: &mut Session, p: &Value, d: i64) -> Result<Value> {
    let id = opt_u64(p, "layer")?.map_or(s.layer, LayerId);
    let tl = crate::editing::edited_mut(&mut s.doc, s.scene, &s.editing)?;
    let i = tl.layers.iter().position(|l| l.id == id).ok_or_else(|| EngineError::State(format!("no layer with id {}", id.0)))?;
    let n = tl.layers.len();
    if tl.layers.get(i).is_some_and(|l| matches!(l.kind, LayerKind::Mask | LayerKind::Folder)) {
        // A folder or mask moves with everything in it, past one sibling (with its contents),
        // or out of its folder at either end.
        let block = tl.group_range(i);
        let parent = tl.layers.get(i).and_then(|l| l.parent);
        let parent_idx = parent.and_then(|p| tl.layers.iter().position(|l| l.id == p));
        let up_parent = parent.and_then(|p| tl.layer(p)).and_then(|l| l.parent);
        if d < 0 {
            let Some(a) = i.checked_sub(1) else { return Ok(json!({"index": i})) };
            if parent_idx == Some(a) {
                // First in its folder: out, above the folder.
                if let Some(l) = tl.layers.get_mut(i) {
                    Arc::make_mut(l).parent = up_parent;
                }
                let Some(span) = tl.layers.get_mut(a..block.end) else { return Ok(json!({"index": i})) };
                span.rotate_left(1);
                tl.repair_masks();
                return Ok(json!({"index": a}));
            }
            // The sibling above, with whatever it contains.
            let mut top = a;
            while let Some(p) = tl.layers.get(top).and_then(|l| l.parent).filter(|p| Some(*p) != parent) {
                match tl.layers.iter().position(|l| l.id == p) {
                    Some(t) if t < top => top = t,
                    _ => break,
                }
            }
            let Some(span) = tl.layers.get_mut(top..block.end) else { return Ok(json!({"index": i})) };
            span.rotate_left(i - top);
            tl.repair_masks();
            return Ok(json!({"index": top}));
        }
        if block.end < n && tl.layers.get(block.end).is_some_and(|l| l.parent == parent) {
            // Past the next sibling and its contents.
            let unit = tl.group_range(block.end);
            let Some(span) = tl.layers.get_mut(block.start..unit.end) else { return Ok(json!({"index": i})) };
            span.rotate_right(unit.len());
            tl.repair_masks();
            return Ok(json!({"index": block.start + unit.len()}));
        }
        if parent.is_some() {
            // Last in its folder: out, just below the folder's contents (where it already is).
            if let Some(l) = tl.layers.get_mut(i) {
                Arc::make_mut(l).parent = up_parent;
            }
            tl.repair_masks();
        }
        return Ok(json!({"index": i}));
    }
    let j = i as i64 + d;
    if j < 0 || j as usize >= n {
        return Ok(json!({"index": i}));
    }
    let j = j as usize;
    tl.layers.swap(i, j);
    // It belongs where it landed: in the folder or under the mask above it, or beside them.
    let kind = tl.layers.get(j).map_or(LayerKind::Normal, |l| l.kind);
    let join = tl.container_at(j, kind);
    if let Some(l) = tl.layers.get_mut(j)
        && l.parent != join
    {
        Arc::make_mut(l).parent = join;
    }
    tl.repair_masks();
    Ok(json!({"index": j}))
}
