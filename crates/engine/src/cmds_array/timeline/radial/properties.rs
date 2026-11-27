//! Compile Radial stops into ordinary native property tracks. X and Y belong to the path.

use super::super::{frame_offsets, overshoots, property_times};
use crate::cmds_array::{ArrayEaseMode, ArrayEasing, ArrayPropertyStop, RadialDistribution};
use crate::{EngineError, Result};
use anicraft_doc::{
    ColorEffect, CustomEase, EaseFamily, Easing, Element, FilterEntry, FilterParam, FilterProp, MAX_EASE_Y, MAX_FILTER_SLOTS, MotionTween, Property,
    Props, Rgba,
};
use anicraft_geom::Vec2;

pub(super) fn write(
    motion: &mut MotionTween,
    base: &Element,
    radial: RadialDistribution,
    authored: &[ArrayPropertyStop],
    easing: &ArrayEasing,
    span: std::ops::Range<u32>,
    pose: impl Fn(&ArrayPropertyStop) -> Element,
) -> Result<()> {
    let first = authored.first().ok_or_else(|| EngineError::Param("Radial needs property stops.".into()))?;
    let times = property_times(authored);
    let stops: Vec<_> = times.iter().map(|&t| ArrayPropertyStop::sample(authored.iter(), t as f32)).collect();
    let poses: Vec<_> = stops.iter().map(&pose).collect();
    let scales = authored.iter().any(|s| s.scale != first.scale);
    let rotation = authored.iter().any(|s| s.rotation != first.rotation);
    let colours = authored.iter().any(|s| s.alpha != first.alpha || s.color != first.color);
    let filters = authored.iter().map(&pose).any(|p| p.kind.filters() != base.kind.filters());
    let duration = span.end - span.start;
    let last = duration - 1;
    // Explicit path timing drives the Radial turn separately from authored Rotation keys.
    let keyed_turn = radial.rotate && !radial.needs_angular_path() && !easing.eases_distribution(authored);
    let sweep = if keyed_turn { radial.sweep } else { 0.0 };
    let props = Props::of(base);

    let pivot = base.pivot().unwrap_or(Vec2::ZERO);
    if pivot != Vec2::ZERO && (radial.rotate || rotation) {
        return Err(EngineError::Param(
            "Radial Motion Tween cannot rotate about an offset transformation point while keeping the registration point on the path.".into(),
        ));
    }
    if poses.iter().any(|p| p.transform.translation() != radial.point(0.0))
        || stops.iter().any(|s| (pivot.x != 0.0 && s.scale[0] != first.scale[0]) || (pivot.y != 0.0 && s.scale[1] != first.scale[1]))
    {
        return Err(EngineError::Param("These Radial transform stops shift the registration point around an offset transformation point.".into()));
    }

    if !(scales || rotation || colours || filters) {
        if keyed_turn {
            let angle = props.get(Property::Rotation);
            motion.set_key(Property::Rotation, 0, angle, angle);
            motion.set_key(Property::Rotation, last, angle + sweep, angle);
        }
        return Ok(());
    }

    let frames = frame_offsets(&times, authored, true, span.start, duration)?;
    let segments: Vec<_> = times
        .windows(2)
        .map(|pair| match pair {
            [a, b] => match easing.mode {
                ArrayEaseMode::Global => &easing.preset,
                ArrayEaseMode::PerSegment if (*b as f32) <= authored.last().map_or(1.0, |s| s.offset) => authored
                    .iter()
                    .find(|s| f64::from(s.offset) == *a)
                    .map_or(&Easing::Linear, |s| s.outbound.as_ref().unwrap_or_else(|| easing.segment_default(authored))),
                ArrayEaseMode::PerSegment => &Easing::Linear,
            },
            _ => &Easing::Linear,
        })
        .collect();
    let mut tracks = Tracks { motion, props, frames: &frames, segments: &segments, global: easing.mode == ArrayEaseMode::Global };

    if scales {
        let sign = first.scale[0].signum();
        // Native decomposition uses a positive X scale. Negating both authored axes
        // with the matching 180° base rotation keeps reflected artwork intact.
        let reconstructed = props.transform();
        if base.transform.m[..4].iter().zip(reconstructed.m).any(|(&a, b)| (a - b).abs() > 1e-5 * a.abs().max(1.0)) {
            return Err(EngineError::Param("This Radial transform cannot be represented by native scale tracks.".into()));
        }
        for (axis, property) in [Property::ScaleX, Property::ScaleY].into_iter().enumerate() {
            let values: Vec<_> = stops.iter().map(|s| s.scale.get(axis).copied().unwrap_or(1.0) * sign * 100.0).collect();
            let start = values.first().copied().unwrap_or(100.0);
            if values.iter().any(|&v| v.abs() < 0.01 || v.signum() != start.signum()) {
                return Err(EngineError::Param("Radial scale tracks cannot reach or cross zero.".into()));
            }
            tracks.channel(property, &values, easing.transform, false)?;
        }
    }
    if rotation {
        let angle = props.get(Property::Rotation);
        let own: Vec<_> = stops.iter().map(|s| s.rotation - first.rotation).collect();
        tracks.rotation(angle, &own, sweep, last, easing.transform)?;
    } else if keyed_turn {
        let angle = props.get(Property::Rotation);
        motion_keys(tracks.motion, Property::Rotation, angle, &[0, last], &[angle, angle + sweep], &[])?;
    }
    if colours {
        colour_tracks(&mut tracks, &stops, &poses, easing.colour_effect)?;
    }
    if filters {
        filter_tracks(&mut tracks, base.kind.filters().unwrap_or_default(), &poses, easing.filters)?;
    }
    Ok(())
}

struct Tracks<'a> {
    motion: &'a mut MotionTween,
    props: Props,
    frames: &'a [u32],
    segments: &'a [&'a Easing],
    global: bool,
}

impl Tracks<'_> {
    fn ease(&self, index: usize, enabled: bool, changing: bool) -> Result<&Easing> {
        if !enabled || !changing {
            return Ok(&Easing::Linear);
        }
        let ease = self.segments.get(index).copied().unwrap_or(&Easing::Linear);
        if self.global && self.segments.len() > 1 && *ease != Easing::Linear {
            return Err(EngineError::Param("Use Per Segment or Linear easing for changing Radial properties with interior stops.".into()));
        }
        Ok(ease)
    }

    fn channel(&mut self, property: Property, values: &[f32], enabled: bool, declare_style: bool) -> Result<()> {
        let Some(&first) = values.first() else { return Ok(()) };
        let base = self.props.get(property);
        if values.iter().all(|&value| value == first) {
            if declare_style {
                motion_keys(self.motion, property, base, &[0], &[first], &[])?;
            }
            return Ok(());
        }
        let mut curves = Vec::with_capacity(self.segments.len());
        for (i, pair) in values.windows(2).enumerate() {
            let ease = self.ease(i, enabled && !property.is_stepped(), pair.first() != pair.last())?;
            if overshoots(ease) {
                return Err(EngineError::Param(format!("Radial Motion Tween cannot preserve overshoot on {}.", property.label())));
            }
            curves.push(ease.clone());
        }
        motion_keys(self.motion, property, base, self.frames, values, &curves)
    }

    fn rotation(&mut self, angle: f32, own: &[f32], sweep: f32, last: u32, enabled: bool) -> Result<()> {
        let travel = |frame: u32| sweep * (frame as f32 / last as f32);
        let values: Vec<_> = own.iter().zip(self.frames).map(|(&v, &f)| angle + v + travel(f)).collect();
        let mut curves = Vec::with_capacity(self.segments.len());
        for (i, (own_pair, frame_pair)) in own.windows(2).zip(self.frames.windows(2)).enumerate() {
            let ([a, b], [start, end]) = (own_pair, frame_pair) else { continue };
            let ease = self.ease(i, enabled, a != b)?;
            let turn = travel(*end) - travel(*start);
            let keyed = b - a;
            let from = angle + a + travel(*start);
            let range = Property::Rotation.range();
            let curve = if *ease == Easing::Linear || keyed == 0.0 {
                Easing::Linear
            } else if turn == 0.0 {
                // Array's Back and Elastic stay inside this envelope.
                if overshoots(ease) && ![-1.0, 2.0].into_iter().all(|y| range.contains(&(from + keyed * y))) {
                    return Err(EngineError::Param("Radial rotation overshoot exceeds the native rotation range.".into()));
                }
                ease.clone()
            } else {
                combine_rotation(ease, turn, keyed, from)?
            };
            curves.push(curve);
        }
        motion_keys(self.motion, Property::Rotation, angle, self.frames, &values, &curves)
    }
}

fn motion_keys(motion: &mut MotionTween, property: Property, base: f32, frames: &[u32], values: &[f32], curves: &[Easing]) -> Result<()> {
    let range = property.range();
    if values.iter().any(|value| !range.contains(value)) {
        return Err(EngineError::Param(format!("Radial {} exceeds the native property range.", property.label())));
    }
    for (&frame, &value) in frames.iter().zip(values) {
        motion.set_key(property, frame, value, base);
    }
    if let Some(track) = motion.track_mut(property) {
        for (key, curve) in track.keys.iter_mut().zip(curves) {
            key.ease.clone_from(curve);
        }
    }
    Ok(())
}

fn combine_rotation(ease: &Easing, turn: f32, keyed: f32, from: f32) -> Result<Easing> {
    // A cubic's time coordinate is itself a Bézier. Taking a weighted sum of
    // time and the eased coordinate gives the exact native graph for turn*t + keyed*ease(t).
    let total = turn + keyed;
    let unsupported = || {
        EngineError::Param(
            "This eased rotation cannot be combined with Circle Rotate in one native curve. Use a compatible easing or turn off Rotate.".into(),
        )
    };
    if total == 0.0 || !matches!(ease.family(), Some((EaseFamily::Quad | EaseFamily::Cubic | EaseFamily::Back | EaseFamily::Bounce, _))) {
        return Err(unsupported());
    }
    let range = Property::Rotation.range();
    let mut points = ease.to_custom().points().to_vec();
    for point in &mut points {
        let mix = |p: Vec2| (turn * p.x + keyed * p.y) / total;
        let y = mix(point.at);
        let before = mix(point.h_in);
        let after = mix(point.out);
        if [y, y + before, y + after].iter().any(|&y| y.abs() > MAX_EASE_Y || !range.contains(&(from + y * total))) {
            return Err(unsupported());
        }
        point.at.y = y;
        point.h_in.y = before;
        point.out.y = after;
    }
    Ok(Easing::Custom(std::sync::Arc::new(CustomEase::new(points))))
}

fn colour_tracks(tracks: &mut Tracks<'_>, stops: &[ArrayPropertyStop], poses: &[Element], enabled: bool) -> Result<()> {
    let alpha: Vec<_> = stops.iter().map(|s| s.alpha * 100.0).collect();
    if stops.iter().all(|s| s.color == ColorEffect::None) {
        return tracks.channel(Property::Alpha, &alpha, enabled, true);
    }
    if stops.iter().all(|s| matches!(s.color, ColorEffect::None | ColorEffect::Brightness(_))) {
        let brightness: Vec<_> = stops
            .iter()
            .map(|s| match s.color {
                ColorEffect::Brightness(b) => b * 100.0,
                _ => 0.0,
            })
            .collect();
        tracks.channel(Property::Brightness, &brightness, enabled, true)?;
        if alpha.iter().any(|&a| a != 100.0) {
            tracks.channel(Property::Alpha, &alpha, enabled, true)?;
        }
        return Ok(());
    }
    let tint: Option<Vec<_>> = stops
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let tint_at = |i: usize| match stops.get(i)?.color {
                ColorEffect::Tint { color, .. } => Some(color),
                _ => None,
            };
            let (amount, color) = match s.color {
                ColorEffect::Tint { amount, color } => (amount, color),
                ColorEffect::None => {
                    let before = i.checked_sub(1).and_then(tint_at);
                    let after = tint_at(i + 1);
                    if before.zip(after).is_some_and(|(a, b)| a != b) {
                        return None;
                    }
                    (0.0, before.or(after).unwrap_or(Rgba::WHITE))
                }
                _ => return None,
            };
            Some([amount * 100.0, color.r * 255.0, color.g * 255.0, color.b * 255.0])
        })
        .collect();
    if let Some(tint) = tint {
        for (i, property) in [Property::Tint, Property::TintRed, Property::TintGreen, Property::TintBlue].into_iter().enumerate() {
            tracks.channel(property, &tint.iter().filter_map(|v| v.get(i).copied()).collect::<Vec<_>>(), enabled, true)?;
        }
        if alpha.iter().any(|&a| a != 100.0) {
            tracks.channel(Property::Alpha, &alpha, enabled, true)?;
        }
        return Ok(());
    }
    // Mixed styles use Advanced channels. Tint's colour*amount product and
    // Brightness crossing zero cannot be represented as a single Advanced segment.
    for (a, b) in stops.iter().zip(stops.iter().skip(1)) {
        let nonlinear = match (a.color, b.color) {
            (ColorEffect::Tint { color: ac, amount: aa }, ColorEffect::Tint { color: bc, amount: ba }) => ac != bc && aa != ba,
            (ColorEffect::Brightness(a), ColorEffect::Brightness(b)) => a * b < 0.0,
            _ => false,
        };
        if nonlinear {
            return Err(EngineError::Param("These mixed Colour Effect styles cannot share native Motion Tween tracks.".into()));
        }
    }
    let colours: Vec<_> = poses.iter().map(|pose| pose.color.transform()).collect();
    for (i, property) in Property::ADVANCED.into_iter().enumerate() {
        let values: Vec<_> = colours
            .iter()
            .filter_map(|colour| colour.mul.into_iter().map(|v| v * 100.0).chain(colour.add.into_iter().map(|v| v * 255.0)).nth(i))
            .collect();
        tracks.channel(property, &values, enabled, true)?;
    }
    Ok(())
}

fn filter_tracks(tracks: &mut Tracks<'_>, base: &[FilterEntry], poses: &[Element], enabled: bool) -> Result<()> {
    for (slot, entry) in base.iter().enumerate() {
        let snapshots: Vec<_> = poses.iter().filter_map(|p| p.kind.filters()?.get(slot)).collect();
        if snapshots.iter().all(|&f| f == entry) {
            continue;
        }
        let params = entry.filter.params();
        let without_tracks = |f: &FilterEntry| {
            let mut f = f.clone();
            for &p in &params {
                f.filter.set_param(p, 0.0);
            }
            f
        };
        if slot >= usize::from(MAX_FILTER_SLOTS)
            || snapshots.len() != poses.len()
            || snapshots.iter().any(|f| without_tracks(f) != without_tracks(entry))
        {
            return Err(EngineError::Param(format!(
                "Radial Motion Tween cannot animate changes to untracked settings in filter {} ({}), such as gradient stop positions or bevel type.",
                slot + 1,
                entry.filter.label()
            )));
        }
        for param in params {
            let mut values: Vec<_> = snapshots.iter().filter_map(|f| f.filter.param(param)).collect();
            if param == FilterParam::Angle {
                let mut previous = None;
                let mut unwrapped = 0.0;
                for value in &mut values {
                    let raw = *value;
                    let delta: f32 = previous.map_or(0.0, |prev| raw - prev);
                    unwrapped =
                        if previous.is_none() { raw } else { unwrapped + if delta.abs() > 180.0 { delta - delta.signum() * 360.0 } else { delta } };
                    previous = Some(raw);
                    *value = unwrapped;
                }
            }
            let property = Property::Filter(FilterProp { slot: slot as u8, param });
            tracks.channel(property, &values, enabled, false)?;
        }
    }
    Ok(())
}
