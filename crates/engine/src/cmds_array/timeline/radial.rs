//! Convert a signed Radial Array into one native Motion Tween.

mod properties;
mod spiral;

use super::super::{ArrayEaseMode, ArrayEasing, ArrayPropertyStop, CLOSED_SWEEP_LIMIT, RadialDistribution, keep_pivot, posed_element};
use crate::{EngineError, Result};
use anicraft_doc::{Easing, Element, Keyframe, MotionTween, Property, Props};
use anicraft_geom::{Path, PathCmd, Vec2};
use std::sync::Arc;

/// At most 30 degrees per cubic. With tangent handles of length
/// (4/3) * tan(step/4) * radius, radial error is below 4e-7 times the
/// radius before the motion path's f32 coordinate rounding.
fn circle_path(radial: RadialDistribution) -> Path {
    let segments = (radial.sweep.abs() / 30.0).ceil().max(1.0) as u32;
    let step = f64::from(radial.sweep) / f64::from(segments);
    let handle = (4.0 / 3.0) * (step.to_radians() / 4.0).tan();
    let control = |i: u32, handle: f64| {
        // Reduce travel as in the shared Circle geometry, including at the seam.
        let angle = f64::from(radial.start) + (step * f64::from(i)) % 360.0;
        let (sin, cos) = angle.to_radians().sin_cos();
        let [cx, cy] = radial.centre.map(f64::from);
        let r = f64::from(radial.radius);
        Vec2::new((cx + r * (cos - handle * sin)) as f32, (cy + r * (sin + handle * cos)) as f32)
    };
    let mut cmds = vec![PathCmd::MoveTo(radial.point(0.0))];
    for i in 0..segments {
        let end = radial.point(f64::from(i + 1) / f64::from(segments));
        cmds.push(PathCmd::CubicTo(control(i, handle), control(i + 1, -handle), end));
    }
    Path { cmds }
}

pub(super) fn keyframe(
    source: &Keyframe,
    original: &Element,
    radial: RadialDistribution,
    seed: &ArrayPropertyStop,
    stops: &[ArrayPropertyStop],
    easing: &ArrayEasing,
    duration: u32,
) -> Result<Keyframe> {
    let spiral = radial.needs_angular_path();
    if radial.closed_loop && radial.sweep.abs() > CLOSED_SWEEP_LIMIT {
        return Err(EngineError::Param("Circle Timeline Animation supports sweeps of up to one revolution (360 degrees).".into()));
    }
    if radial.sweep == 0.0 || (!spiral && radial.radius == 0.0) {
        return Err(EngineError::Param("Radial Timeline Animation needs a nonzero sweep and a path of some length.".into()));
    }
    let eased_distribution = easing.eases_distribution(stops);
    let turn = if radial.rotate { radial.sweep } else { 0.0 };
    if !Property::Rotation.range().contains(&turn) {
        return Err(EngineError::Param("Radial Rotate exceeds the native rotation range.".into()));
    }
    let spiral_path = spiral.then(|| spiral::path(radial)).transpose()?;
    let circle;
    let path = if let Some(spiral) = &spiral_path {
        &spiral.path
    } else {
        circle = circle_path(radial);
        &circle
    };
    let basis = original.transform.decompose();
    let pose = |stop: &ArrayPropertyStop| {
        let mut base = original.transform;
        let at = radial.point(0.0);
        base.m[4] = at.x;
        base.m[5] = at.y;
        let adjusted = stop.source_transform(original.transform, basis);
        let transform = keep_pivot(base, adjusted, original.pivot());
        posed_element(original, transform, Some(stop.clone()), Some(seed), false)
    };
    let element = pose(stops.first().unwrap_or(seed));
    if !element.transform.is_finite() {
        return Err(EngineError::Param("Radial Timeline Animation needs a finite transform.".into()));
    }

    let base = Props::of(&element);
    let mut motion = MotionTween::new(element.id);
    motion.preserve_path_affine();
    let last = duration - 1;
    // X/Y keys anchor the original path. Distribution timing uses normalized
    // authored stop positions, independent of rounding the property keys.
    if !motion.set_path(path, &base, last) {
        return Err(EngineError::Param("The Radial path is too small or its control points exceed the native motion path range.".into()));
    }
    if spiral || eased_distribution {
        let segments = if !eased_distribution {
            vec![(0.0, Easing::Linear)]
        } else {
            match easing.mode {
                ArrayEaseMode::Global => vec![(0.0, easing.preset.clone())],
                ArrayEaseMode::PerSegment => super::property_times(stops)
                    .into_iter()
                    .map(|time| time as f32)
                    .take_while(|&time| time < 1.0)
                    .map(|time| {
                        let next = stops.partition_point(|stop| stop.offset <= time);
                        (time, easing.distribution_segment(stops, next).clone())
                    })
                    .collect(),
            }
        };
        let valid = if let Some(spiral) = &spiral_path {
            let map = spiral.distance_map().ok_or_else(|| EngineError::Param("The Spiral exceeds the native motion timing range.".into()))?;
            motion.set_path_distance_map(segments, map, turn)
        } else {
            motion.set_path_timing(segments, turn)
        };
        if !valid {
            return Err(EngineError::Param("Radial Distribution needs monotonic easing. Back, Bounce and Elastic overshoot or reverse along the path.".into()));
        }
    }
    properties::write(&mut motion, &element, radial, stops, easing, source.start..source.start + duration, pose)?;
    let mut key = source.clone();
    key.duration = duration;
    key.elements = vec![Arc::new(element)];
    key.motion = Some(motion);
    Ok(key)
}
