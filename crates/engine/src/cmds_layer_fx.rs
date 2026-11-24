//! Layer-level and keyframe-level blend modes, colour effects and bitmap filters (P-027).
//!
//! Layers and individual keyframes can carry `blend`, `color_effect` and `filters` without
//! wrapping their artwork in a MovieClip first. Keyframe `color_effect` and `filters` interpolate
//! across Classic Tweens into the adjacent following keyframe.

use crate::cmds_draw::EffectEdit;
use crate::cmds_filter::parse_list;
use crate::params::*;
use crate::{CommandSpec, EngineError, Result, Session, color_effect_json};
use anicraft_doc::{BlendMode, ColorEffect, FilterEntry, Keyframe, Layer, LayerId, LayerKind};
use serde_json::{Value, json};

pub fn specs() -> Vec<CommandSpec> {
    let c = |id, label, params, mutates, run| CommandSpec { id, label, menu: "", shortcut: None, params, mutates, run };
    vec![
        c(
            "layer.set_effects",
            "Layer Effects",
            "{layer? (default: active layer), blend?: normal|layer|darken|multiply|lighten|screen|overlay|hardlight|add|subtract|difference|invert|alpha|erase, filters?: [{type: blur|drop_shadow|glow|bevel|gradient_glow|gradient_bevel|adjust_color, …values, enabled?: bool}], style?: none|brightness|tint|advanced|alpha, alpha? (%, 0..100), brightness? (%, -100..100), tint? (#rrggbb), tint_amount? (%, 0..100), red_multiplier?, green_multiplier?, blue_multiplier?, alpha_multiplier? (%, -100..100), red_offset?, green_offset?, blue_offset?, alpha_offset? (-255..255)} sets blend mode, filters and/or colour effect on a layer → {layer, blend, color_effect, filters}",
            true,
            set_layer_effects as crate::RunFn,
        ),
        c(
            "layer.effects",
            "Get Layer Effects",
            "{layer? (default: active layer)} reports the layer's blend mode, colour effect and filters → {layer, blend, color_effect, filters}",
            false,
            get_layer_effects,
        ),
        c(
            "timeline.set_frame_effects",
            "Frame Effects",
            "{layer? (default: active layer), frame? (default: playhead), blend?: normal|layer|darken|multiply|lighten|screen|overlay|hardlight|add|subtract|difference|invert|alpha|erase, filters?: [{type: blur|drop_shadow|glow|bevel|gradient_glow|gradient_bevel|adjust_color, …values, enabled?: bool}], style?: none|brightness|tint|advanced|alpha, alpha? (%, 0..100), brightness? (%, -100..100), tint? (#rrggbb), tint_amount? (%, 0..100), red_multiplier?, green_multiplier?, blue_multiplier?, alpha_multiplier? (%, -100..100), red_offset?, green_offset?, blue_offset?, alpha_offset? (-255..255)} sets blend mode, filters and/or colour effect on the keyframe covering `frame` → {layer, frame, keyframe, blend, color_effect, filters}",
            true,
            set_frame_effects,
        ),
        c(
            "timeline.frame_effects",
            "Get Frame Effects",
            "{layer? (default: active layer), frame? (default: playhead)} reports the covering keyframe's blend mode, colour effect and filters (plus evaluated tweened values at `frame`) → {layer, frame, keyframe, blend, color_effect, filters, evaluated}",
            false,
            get_frame_effects,
        ),
    ]
}

fn layer_id(s: &Session, p: &Value) -> Result<LayerId> {
    Ok(opt_u64(p, "layer")?.map(LayerId).unwrap_or(s.layer))
}

fn parse_blend_opt(p: &Value) -> Result<Option<BlendMode>> {
    let Some(key) = opt_str(p, "blend")? else {
        return Ok(None);
    };
    BlendMode::from_key(key).map(Some).ok_or_else(|| {
        let all: Vec<&str> = BlendMode::ALL.iter().map(|m| m.key()).collect();
        EngineError::Param(format!("unknown blend mode '{key}' ({})", all.join(", ")))
    })
}

type EffectsPatch = (Option<BlendMode>, Option<Vec<FilterEntry>>, Option<EffectEdit>);

fn parse_effects_patch(p: &Value) -> Result<EffectsPatch> {
    let blend = parse_blend_opt(p)?;
    let filters = match p.get("filters") {
        None | Some(Value::Null) => None,
        Some(v) => Some(parse_list(Some(v))?),
    };
    let fx = EffectEdit::parse(p)?;
    if blend.is_none() && filters.is_none() && fx.is_none() {
        return Err(EngineError::Param("give 'blend', 'filters', or a colour effect ('style' or its values)".into()));
    }
    Ok((blend, filters, fx))
}

fn check_layer_takes_effects(kind: LayerKind, name: &str) -> Result<()> {
    match kind {
        LayerKind::Normal | LayerKind::Armature => Ok(()),
        LayerKind::Folder => Err(EngineError::State(format!("'{name}' is a group container and draws nothing itself; set effects on a layer inside it"))),
        LayerKind::Camera => Err(EngineError::State(format!("'{name}' is the camera layer; set colour effects on the camera instead"))),
        LayerKind::Mask => Err(EngineError::State(format!("'{name}' is a mask layer; set effects on the masked layers instead"))),
        LayerKind::Guide => Err(EngineError::State(format!("'{name}' is a guide layer and isn't drawn in the movie"))),
    }
}

pub fn layer_effects_json(l: &Layer) -> Value {
    json!({
        "layer": l.id.0,
        "blend": l.blend.key(),
        "color_effect": color_effect_json(&l.color_effect),
        "filters": l.filters,
    })
}

fn frame_effects_json(lid: LayerId, frame: u32, k: &Keyframe, eval: (BlendMode, ColorEffect, Vec<FilterEntry>)) -> Value {
    json!({
        "layer": lid.0,
        "frame": frame,
        "keyframe": k.start,
        "blend": k.blend.key(),
        "color_effect": color_effect_json(&k.color_effect),
        "filters": k.filters,
        "evaluated": {
            "blend": eval.0.key(),
            "color_effect": color_effect_json(&eval.1),
            "filters": eval.2,
        }
    })
}

fn set_layer_effects(s: &mut Session, p: &Value) -> Result<Value> {
    let id = layer_id(s, p)?;
    let (blend, filters, fx) = parse_effects_patch(p)?;
    let (kind, name) = s
        .timeline()
        .layer(id)
        .map(|l| (l.kind, l.name.clone()))
        .ok_or_else(|| EngineError::State(format!("no layer with id {}", id.0)))?;
    check_layer_takes_effects(kind, &name)?;
    let l = s.layer_mut(Some(id))?;
    if let Some(b) = blend {
        l.blend = b;
    }
    if let Some(fs) = filters {
        l.filters = fs;
    }
    if let Some(edit) = fx {
        l.color_effect = edit.apply(&l.color_effect);
    }
    Ok(layer_effects_json(l))
}

fn get_layer_effects(s: &mut Session, p: &Value) -> Result<Value> {
    let id = layer_id(s, p)?;
    let l = s
        .timeline()
        .layer(id)
        .ok_or_else(|| EngineError::State(format!("no layer with id {}", id.0)))?;
    Ok(layer_effects_json(l))
}

fn set_frame_effects(s: &mut Session, p: &Value) -> Result<Value> {
    let id = layer_id(s, p)?;
    let frame = opt_u32(p, "frame")?.unwrap_or(s.frame);
    if frame > crate::MAX_FRAME {
        return Err(EngineError::Param(format!("param 'frame' must be at most {}", crate::MAX_FRAME)));
    }
    let (blend, filters, fx) = parse_effects_patch(p)?;
    {
        let tl = s.timeline();
        let l = tl.layer(id).ok_or_else(|| EngineError::State(format!("no layer with id {}", id.0)))?;
        check_layer_takes_effects(l.kind, &l.name)?;
        if l.locked || tl.locked_by_folder(id) {
            return Err(EngineError::State(format!("layer '{}' is locked", l.name)));
        }
        if l.keyframe_index_at(frame).is_none() {
            return Err(EngineError::State(format!("no keyframe covers frame {frame} on layer '{}'", l.name)));
        }
    }
    let l = s.layer_mut(Some(id))?;
    let idx = l
        .keyframe_index_at(frame)
        .ok_or_else(|| EngineError::State(format!("no keyframe covers frame {frame}")))?;
    if let Some(k) = l.keyframe_mut(idx) {
        if let Some(b) = blend {
            k.blend = b;
        }
        if let Some(fs) = filters {
            k.filters = fs;
        }
        if let Some(edit) = fx {
            k.color_effect = edit.apply(&k.color_effect);
        }
    }
    let eval = anicraft_doc::evaluate_keyframe_effects(l, frame).unwrap_or((BlendMode::Normal, ColorEffect::None, Vec::new()));
    let k = l.keyframes.get(idx).ok_or_else(|| EngineError::State(format!("no keyframe covers frame {frame}")))?;
    Ok(frame_effects_json(id, frame, k, eval))
}

fn get_frame_effects(s: &mut Session, p: &Value) -> Result<Value> {
    let id = layer_id(s, p)?;
    let frame = opt_u32(p, "frame")?.unwrap_or(s.frame);
    if frame > crate::MAX_FRAME {
        return Err(EngineError::Param(format!("param 'frame' must be at most {}", crate::MAX_FRAME)));
    }
    let l = s
        .timeline()
        .layer(id)
        .ok_or_else(|| EngineError::State(format!("no layer with id {}", id.0)))?;
    let k = l
        .keyframe_at(frame)
        .ok_or_else(|| EngineError::State(format!("no keyframe covers frame {frame} on layer '{}'", l.name)))?;
    let eval = anicraft_doc::evaluate_keyframe_effects(l, frame).unwrap_or((BlendMode::Normal, ColorEffect::None, Vec::new()));
    Ok(frame_effects_json(id, frame, k, eval))
}
