//! Convert an Array to native timeline spans on the source layer.

mod radial;

use super::{ArrayEaseMode, ArrayEasing, ArrayPropertyStop, MAX_ARRAY_INSTANCES, keep_pivot, parse_radial, parse_stops, posed_element};
use crate::params::{opt_bool, opt_f32, opt_str, opt_u32};
use crate::{CommandSpec, EngineError, MAX_FRAME, Result, Session};
use anicraft_doc::{ColorEffect, EaseGroup, Easing, Element, ElementKind, FilterParam, Keyframe, LayerKind, Tween, TweenKind};
use anicraft_geom::{Decomposed, Vec2};
use serde_json::{Value, json};
use std::sync::Arc;

pub(super) fn spec() -> CommandSpec {
    CommandSpec {
        id: "array.create_timeline",
        label: "Create Timeline Animation",
        menu: "",
        shortcut: None,
        params: "{duration: frames (at least 2), animation?: classic|hold (Linear, default classic) or motion (Radial, default motion), mode?: linear|radial (center_x, center_y, radius?, start_angle?, sweep_angle?, closed_loop?, spiral_offset?, drift_x?, drift_y?, rotate?), start_x?, start_y?, end_x?, end_y?, count?: 1..500 (Hold samples only), property_stops?, easing?}. Stops use the authored instance values from array.create with source_values enabled. Converts one Symbol exposure on the active layer at the playhead.",
        mutates: true,
        run: create,
    }
}

fn create(s: &mut Session, p: &Value) -> Result<Value> {
    let is_radial = match opt_str(p, "mode")?.unwrap_or("linear") {
        "linear" => false,
        "radial" => true,
        _ => return Err(EngineError::Param("Timeline Animation is available for Linear and Radial Arrays only.".into())),
    };
    if opt_bool(p, "create_unique_symbols")?.unwrap_or(false) {
        return Err(EngineError::Param("Create Unique Symbols is available for Spatial Array only.".into()));
    }
    if p.get("alpha_stops").is_some() {
        return Err(EngineError::Param("Timeline Animation uses authored property_stops.".into()));
    }
    let classic = match (is_radial, opt_str(p, "animation")?) {
        (false, None | Some("classic")) => true,
        (false, Some("hold")) | (true, None | Some("motion")) => false,
        (true, _) => return Err(EngineError::Param("Radial Timeline Animation uses Motion Tween.".into())),
        (false, _) => return Err(EngineError::Param("Choose Classic Tween or Hold Keyframes.".into())),
    };
    let start = s.frame;
    let duration = opt_u32(p, "duration")?.unwrap_or_else(|| crate::array_timeline_default_duration(s.doc.stage.fps, start));
    let end = start
        .checked_add(duration)
        .filter(|&end| duration >= 2 && end <= MAX_FRAME + 1)
        .ok_or_else(|| EngineError::Param(format!("Duration must be at least 2 frames and end by frame {}.", MAX_FRAME + 1)))?;
    if s.editing_group() {
        return Err(EngineError::State("Timeline Animation needs a scene or Symbol timeline. Close the group first.".into()));
    }
    let Some((layer_id, id)) = s.selection.first().filter(|&(layer, _)| layer == s.layer && s.selection.len() == 1) else {
        return Err(EngineError::State("Select one Symbol instance on the active layer.".into()));
    };
    let timeline = s.timeline();
    let layer = timeline.layer(layer_id).ok_or_else(|| EngineError::State("The source layer is unavailable.".into()))?;
    if layer.kind != LayerKind::Normal || layer.locked || timeline.locked_by_folder(layer_id) {
        return Err(EngineError::State("Timeline Animation needs an unlocked normal artwork layer.".into()));
    }
    if layer.parent.and_then(|id| timeline.layer(id)).is_some_and(|p| p.kind != LayerKind::Folder) {
        return Err(EngineError::State("Timeline Animation cannot use a masked or guided layer.".into()));
    }
    if layer.rig_parent.is_some() || timeline.layers.iter().any(|other| other.rig_parent == Some(layer_id)) {
        return Err(EngineError::State("Timeline Animation cannot change a layer linked by layer parenting.".into()));
    }
    let index =
        layer.keyframe_starting_at(start).ok_or_else(|| EngineError::State("Move the playhead to the start of the source keyframe.".into()))?;
    let source = layer.keyframes.get(index).ok_or_else(|| EngineError::State("The source keyframe is unavailable.".into()))?;
    if source.tween.is_some()
        || source.motion.is_some()
        || index.checked_sub(1).and_then(|i| layer.keyframes.get(i)).is_some_and(|key| key.end() == start && key.tween.is_some())
    {
        return Err(EngineError::State("The source keyframe is part of an existing tween.".into()));
    }
    let [original] = source.elements.as_slice() else {
        return Err(EngineError::State("The source keyframe must contain only the selected Symbol.".into()));
    };
    let ElementKind::Symbol { symbol, first_frame, loop_mode, .. } = original.kind else {
        return Err(EngineError::State("Timeline Animation needs a Symbol instance.".into()));
    };
    if original.id != id {
        return Err(EngineError::State("The selected Symbol is unavailable on the source keyframe.".into()));
    }
    if source.end() > end {
        return Err(EngineError::State("Increase the duration to cover the complete source exposure.".into()));
    }
    let library_symbol = s.doc.symbol(symbol).ok_or_else(|| EngineError::State("The source Symbol definition is unavailable.".into()))?;
    let graphic_length = (library_symbol.kind == anicraft_doc::SymbolKind::Graphic).then(|| library_symbol.timeline.frame_count());

    // Blank exposures may be consumed, but preserve a blank tail after the new animation.
    // Never consume an explicit tween, artwork, or frame metadata.
    let mut after = index + 1;
    let mut tail = None;
    while let Some(key) = layer.keyframes.get(after).filter(|key| key.start < end) {
        if !key.elements.is_empty()
            || key.tween.is_some()
            || key.motion.is_some()
            || !key.label.is_empty()
            || key.sound.is_some()
            || key.script.is_some()
            || !key.hints.is_empty()
            || !key.pose.is_empty()
        {
            return Err(EngineError::State(format!("The destination contains existing timeline content at frame {}.", key.start + 1)));
        }
        if key.end() > end {
            tail = Some(Arc::new(Keyframe::new(end, key.end() - end)));
        }
        after += 1;
    }

    let seed = ArrayPropertyStop::from_source(original);
    let stops = parse_stops(p, Some(&seed), original.kind.filters())?.unwrap_or_else(|| {
        let mut last = seed.clone();
        last.offset = 1.0;
        vec![seed.clone(), last]
    });
    let easing = ArrayEasing::parse(p)?;
    let origin = original.transform.translation();
    let keys = if is_radial {
        let radial = parse_radial(p, origin)?;
        vec![radial::keyframe(source, original, radial, &seed, &stops, &easing, duration)?]
    } else {
        let from = Vec2::new(opt_f32(p, "start_x")?.unwrap_or(origin.x), opt_f32(p, "start_y")?.unwrap_or(origin.y));
        let to = Vec2::new(opt_f32(p, "end_x")?.unwrap_or(from.x), opt_f32(p, "end_y")?.unwrap_or(from.y));
        let count = opt_u32(p, "count")?.unwrap_or(10);
        let times = if classic {
            classic_times(&stops, &easing, from != to)?
        } else {
            if !(1..=MAX_ARRAY_INSTANCES).contains(&count) {
                return Err(EngineError::Param(format!("Hold Keyframes needs 1 to {MAX_ARRAY_INSTANCES} Array samples.")));
            }
            (0..count).map(|i| f64::from(i) / f64::from((count - 1).max(1))).collect()
        };
        let offsets = frame_offsets(&times, &stops, classic, start, duration)?;

        let basis = original.transform.decompose();
        let distribution = easing.eases_distribution(&stops);
        let mut keys = Vec::with_capacity(times.len());
        for (i, (&t, &offset)) in times.iter().zip(&offsets).enumerate() {
            let progress = if distribution { easing.eased_position(&stops, t) } else { t };
            let position = if progress == 0.0 {
                from
            } else if progress == 1.0 {
                to
            } else {
                from.lerp(to, progress as f32)
            };
            let mut base = original.transform;
            base.m[4] = position.x;
            base.m[5] = position.y;
            let stop = easing.properties(&stops, t as f32);
            let adjusted = stop.source_transform(original.transform, basis);
            let transform = keep_pivot(base, adjusted, original.pivot());
            if !transform.is_finite() {
                return Err(EngineError::Param("Array positions or transforms exceeded the finite coordinate range.".into()));
            }
            let mut pose = posed_element(original, transform, Some(stop), Some(&seed), false);
            if let Some(len) = graphic_length
                && let ElementKind::Symbol { first_frame: first, .. } = &mut pose.kind
            {
                *first = loop_mode.local_frame(offset, first_frame, len);
            }
            let span = offsets.get(i + 1).copied().unwrap_or(duration) - offset;
            let mut key = if i == 0 { (**source).clone() } else { Keyframe::new(start + offset, span) };
            key.duration = span;
            key.elements = vec![Arc::new(pose)];
            if classic && let Some(&next) = times.get(i + 1) {
                key.tween = Some(classic_tween(&stops, &easing, t as f32, next as f32));
            }
            keys.push(key);
        }
        if classic {
            validate_classic(&keys, &times, &stops)?;
        }
        keys
    };
    let key_count = keys.len();
    let replacement = keys.into_iter().map(Arc::new).chain(tail);
    s.layer_mut(Some(layer_id))?.keyframes.splice(index..after, replacement);
    Ok(json!({"instance": id.0, "start": start, "duration": duration, "keyframes": key_count}))
}

fn frame_at(time: f64, duration: u32) -> u32 {
    (time * f64::from(duration - 1) + 0.5).floor() as u32
}

pub fn array_timeline_default_duration(fps: f32, start: u32) -> u32 {
    let available = (MAX_FRAME + 1).saturating_sub(start);
    (fps.round() as u32).clamp(2, available.max(2))
}

fn frame_offsets(times: &[f64], stops: &[ArrayPropertyStop], classic: bool, start: u32, duration: u32) -> Result<Vec<u32>> {
    let offsets: Vec<_> = times.iter().map(|&time| frame_at(time, duration)).collect();
    if offsets.windows(2).all(|pair| pair.first() != pair.last()) {
        return Ok(offsets);
    }

    let added_start = stops.first().is_some_and(|stop| stop.offset != 0.0);
    let mut groups = Vec::new();
    for group in offsets.chunk_by(|a, b| a == b) {
        let index = groups.last().map_or(0, |&(end, _)| end);
        groups.push((index + group.len(), group));
    }
    let mut messages = Vec::new();
    for (end, group) in groups.into_iter().filter(|(_, group)| group.len() > 1) {
        let entries: Vec<_> = (end - group.len()..end).filter_map(|i| {
            let time = times.get(i)?;
            let number = i.checked_sub(usize::from(added_start)).filter(|&n| n < stops.len());
            let label = if !classic {
                format!("Sample #{}", i + 1)
            } else if let Some(n) = number {
                format!("Stop #{}", n + 1)
            } else if i == 0 {
                "Start endpoint".into()
            } else {
                "End endpoint".into()
            };
            let percent = format!("{:.6}", *time as f32 * 100.0);
            Some(format!("{label} ({}%)", percent.trim_end_matches('0').trim_end_matches('.')))
        }).collect();
        if let Some(frame) = group.first() {
            messages.push(format!("Frame {}: {} map to the same frame.", start + frame + 1, entries.join(", ")));
        }
    }

    let coincident = times.windows(2).any(|pair| pair.first() == pair.last());
    // Rounding can separate a pair at one duration and bring it back together at the next.
    // Try each candidate within the timeline bound, skipping durations with too few frames.
    let suggested = if coincident {
        None
    } else {
        ((duration + 1).max(times.len() as u32)..=MAX_FRAME + 1 - start).find(|&candidate| {
            times.windows(2).all(|pair| match pair {
                [a, b] => frame_at(*a, candidate) != frame_at(*b, candidate),
                _ => true,
            })
        })
    };
    messages.push(if coincident {
        "Coincident property stops have identical positions. Increasing the duration cannot separate them.".into()
    } else if let Some(frames) = suggested {
        format!("Increase the animation duration to at least {frames} frames.")
    } else {
        "No longer duration fits these keys within the remaining timeline frames.".into()
    });
    Err(EngineError::ArrayDuration { message: messages.join("\n"), suggested })
}

fn property_times(stops: &[ArrayPropertyStop]) -> Vec<f64> {
    let mut times: Vec<_> = stops.iter().map(|stop| f64::from(stop.offset)).collect();
    if times.first() != Some(&0.0) { times.insert(0, 0.0); }
    if times.last() != Some(&1.0) { times.push(1.0); }
    times
}

fn classic_times(stops: &[ArrayPropertyStop], easing: &ArrayEasing, moving: bool) -> Result<Vec<f64>> {
    let times = property_times(stops);
    if times.len() > 2 && easing.mode == ArrayEaseMode::Global && easing.preset != Easing::Linear {
        let varies = stops.iter().zip(stops.iter().skip(1)).any(|(a, b)| {
            (easing.transform && (a.scale != b.scale || a.rotation != b.rotation))
                || (easing.colour_effect && (a.alpha != b.alpha || a.color != b.color))
                || (easing.filters && a.filters != b.filters)
        });
        if varies || (easing.distribution && moving) {
            return Err(EngineError::Param(
                "Classic Tween cannot preserve non-linear Global easing across interior property stops. Use Per Segment easing, Linear easing, or Hold Keyframes.".into()
            ));
        }
    }
    Ok(times)
}

fn classic_tween(stops: &[ArrayPropertyStop], easing: &ArrayEasing, from: f32, to: f32) -> Tween {
    let linear = &Easing::Linear;
    let selected = match easing.mode {
        ArrayEaseMode::Global => &easing.preset,
        ArrayEaseMode::PerSegment => {
            let left = stops.iter().find(|stop| stop.offset == from);
            let right = stops.iter().any(|stop| stop.offset == to);
            left.filter(|_| right).map_or(linear, |stop| stop.outbound.as_ref().unwrap_or_else(|| easing.segment_default(stops)))
        }
    };
    let mut tween = Tween::new(Easing::Linear, TweenKind::Classic);
    if selected != linear {
        for (group, enabled) in [
            (EaseGroup::Position, easing.distribution),
            (EaseGroup::Rotation, easing.transform),
            (EaseGroup::Scale, easing.transform),
            (EaseGroup::Color, easing.colour_effect),
            (EaseGroup::Filters, easing.filters),
        ] {
            if enabled {
                tween.set_ease(group, Some(selected.clone()));
            }
        }
    }
    tween
}

fn overshoots(ease: &Easing) -> bool {
    matches!(ease, Easing::BackIn | Easing::BackOut | Easing::BackInOut | Easing::ElasticIn | Easing::ElasticOut | Easing::ElasticInOut)
}

fn validate_classic(keys: &[Keyframe], times: &[f64], stops: &[ArrayPropertyStop]) -> Result<()> {
    for ((left, right), (&from, &to)) in keys.iter().zip(keys.iter().skip(1)).zip(times.iter().zip(times.iter().skip(1))) {
        let (Some(a), Some(b), Some(tween)) = (left.elements.first(), right.elements.first(), &left.tween) else {
            continue;
        };
        let a_stop = ArrayPropertyStop::sample(stops, from as f32);
        let b_stop = ArrayPropertyStop::sample(stops, to as f32);
        let (from, to) = (a.transform.decompose(), b.transform.decompose());
        let decomposes = |pose: &Element, decomposition: Decomposed| {
            pose.transform.m[..4]
                .iter()
                .zip(decomposition.compose().m)
                .all(|(&original, recomposed)| (original - recomposed).abs() <= 0.00001 * original.abs().max(1.0))
        };
        if !decomposes(a, from) || !decomposes(b, to) {
            return Err(EngineError::Param("Classic Tween cannot preserve this singular source transform. Use Hold Keyframes.".into()));
        }
        if a_stop.scale != b_stop.scale || a_stop.rotation != b_stop.rotation {
            validate_transform(&a_stop, &b_stop, from, to, tween)?;
        }
        if a_stop.color != b_stop.color || a_stop.alpha != b_stop.alpha {
            if overshoots(tween.ease(EaseGroup::Color)) {
                return Err(EngineError::Param(
                    "Classic Tween cannot preserve this Colour Effect overshoot. Change its easing or use Hold Keyframes.".into(),
                ));
            }
            if !classic_colour(&a_stop, &b_stop) {
                return Err(EngineError::Param(
                    "Classic Tween cannot preserve this combined Colour Effect and Alpha interpolation. Use Hold Keyframes.".into(),
                ));
            }
        }
        let a_filters = a.kind.filters().unwrap_or_default();
        let b_filters = b.kind.filters().unwrap_or_default();
        if a_filters != b_filters && overshoots(tween.ease(EaseGroup::Filters)) {
            return Err(EngineError::Param("Classic Tween cannot preserve this filter overshoot. Change its easing or use Hold Keyframes.".into()));
        }
        if a_filters
            .iter()
            .zip(b_filters)
            .any(|(a, b)| a.filter.param(FilterParam::Angle).zip(b.filter.param(FilterParam::Angle)).is_some_and(|(a, b)| (b - a).abs() > 180.0))
        {
            return Err(EngineError::Param(
                "Classic Tween cannot preserve a filter angle that wraps across the shortest arc. Add a property stop or use Hold Keyframes.".into(),
            ));
        }
    }
    Ok(())
}

fn validate_transform(a: &ArrayPropertyStop, b: &ArrayPropertyStop, from: Decomposed, to: Decomposed, tween: &Tween) -> Result<()> {
    if a.scale.into_iter().zip(b.scale).any(|(a, b)| a == 0.0 || b == 0.0 || a.signum() != b.signum()) {
        return Err(EngineError::Param("Classic Tween cannot preserve this collapsed or sign-changing scale. Use Hold Keyframes.".into()));
    }
    let authored_turn = b.rotation - a.rotation;
    let native_turn = (from.lerp(&to, 1.0).rotation - from.rotation).to_degrees();
    if (authored_turn - native_turn).abs() > 0.01 {
        return Err(EngineError::Param(
            "Classic Tween takes the shortest rotation arc. Add a property stop to preserve this turn, or use Hold Keyframes.".into(),
        ));
    }
    if a.scale != b.scale && overshoots(tween.ease(EaseGroup::Scale)) {
        return Err(EngineError::Param("Classic Tween cannot preserve this Transform overshoot. Change its easing or use Hold Keyframes.".into()));
    }
    Ok(())
}

fn classic_colour(a: &ArrayPropertyStop, b: &ArrayPropertyStop) -> bool {
    if a.alpha == 1.0 && b.alpha == 1.0 {
        return true;
    }
    // Applying Alpha converts Tint and Brightness to Advanced. Classic then blends their
    // channel transforms directly. This matches the authored blend only if the transform
    // is affine in progress: Tint multiplies colour by amount, Brightness has a kink at zero.
    match (a.color, b.color) {
        (ColorEffect::Tint { color: ac, amount: aa }, ColorEffect::Tint { color: bc, amount: ba }) => ac == bc || aa == ba,
        (ColorEffect::Brightness(a), ColorEffect::Brightness(b)) => a * b >= 0.0,
        _ => true,
    }
}

#[cfg(test)]
mod tests;
