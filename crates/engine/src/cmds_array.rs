//! Baked symbol arrays and the editor's reversible preview of the same command.
//!
//! A count of N produces N total instances, including the original (ID preserved).
//! The first instance moves to the requested start location. Copies are ordinary
//! symbol instances, not a new document object, so existing editors and exports work.
//! Geometry edits that only move instances reuse the existing preview.

use crate::params::{opt_bool, opt_f32, opt_str, opt_u32};
use crate::{CommandSpec, EngineError, Result, Session};
use anicraft_doc::{ColorEffect, ColorStyle, Document, Easing, Element, ElementId, ElementKind, Filter, FilterEntry, FilterParam, GradientStop, LibraryEntry, LibraryItem, MAX_BLUR, REST_SNAP, SymbolId, pinned_stop};
use anicraft_geom::{Affine, Decomposed, Vec2};
use serde_json::{Value, json};
use std::cmp::Ordering;
use std::num::NonZeroU32;
use std::ops::{Range, RangeInclusive};
use std::sync::Arc;

mod timeline;
pub use timeline::array_timeline_default_duration;

pub const MAX_ARRAY_INSTANCES: u32 = 500;
pub const CLOSED_SWEEP_LIMIT: f32 = 360.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ArrayEaseMode {
    #[default]
    Global,
    PerSegment,
}

impl ArrayEaseMode {
    pub const fn key(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::PerSegment => "per_segment",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ArrayEasing {
    pub mode: ArrayEaseMode,
    pub preset: Easing,
    pub distribution: bool,
    pub transform: bool,
    pub colour_effect: bool,
    pub filters: bool,
}

impl Default for ArrayEasing {
    fn default() -> Self {
        Self {
            mode: ArrayEaseMode::Global, preset: Easing::Linear,
            distribution: true, transform: true, colour_effect: true, filters: true,
        }
    }
}

impl ArrayEasing {
    fn parse(params: &Value) -> Result<Self> {
        let defaults = Self::default();
        let Some(easing) = params.get("easing") else { return Ok(defaults) };
        if !easing.is_object() {
            return Err(EngineError::Param("easing must be an object".into()));
        }
        let mode = match opt_str(easing, "mode")? {
            None | Some("global") => ArrayEaseMode::Global,
            Some("per_segment") => ArrayEaseMode::PerSegment,
            Some(other) => return Err(EngineError::Param(format!("unknown Array easing mode '{other}'"))),
        };
        let preset = opt_named_ease(easing, "preset")?.unwrap_or(defaults.preset);
        Ok(Self {
            mode, preset,
            distribution: opt_bool(easing, "distribution")?.unwrap_or(defaults.distribution),
            transform: opt_bool(easing, "transform")?.unwrap_or(defaults.transform),
            colour_effect: opt_bool(easing, "colour_effect")?.unwrap_or(defaults.colour_effect),
            filters: opt_bool(easing, "filters")?.unwrap_or(defaults.filters),
        })
    }

    // A single Per Segment preset remains valid for command recordings without stop curves.
    fn segment_default(&self, stops: &[ArrayPropertyStop]) -> &Easing {
        if stops.iter().any(|stop| stop.outbound.is_some()) { &Easing::Linear } else { &self.preset }
    }

    fn eases_distribution(&self, stops: &[ArrayPropertyStop]) -> bool {
        self.distribution && match self.mode {
            ArrayEaseMode::Global => self.preset != Easing::Linear,
            ArrayEaseMode::PerSegment => *self.segment_default(stops) != Easing::Linear
                || stops.iter().take(stops.len().saturating_sub(1))
                    .any(|stop| stop.outbound.as_ref().is_some_and(|ease| *ease != Easing::Linear)),
        }
    }

    fn eased_position(&self, stops: &[ArrayPropertyStop], t: f64) -> f64 {
        let progress = t as f32;
        if self.mode == ArrayEaseMode::Global {
            return f64::from(self.preset.apply(progress));
        }
        // The path always has boundaries at 0 and 1, even when the authored stops don't.
        let next = stops.partition_point(|stop| stop.offset <= progress);
        let left = next.checked_sub(1).and_then(|i| stops.get(i));
        let right = stops.get(next);
        let a = left.map_or(0.0, |stop| stop.offset);
        let b = right.map_or(1.0, |stop| stop.offset);
        let span = b - a;
        if span <= f32::EPSILON || progress == a {
            return t;
        }
        let outbound = self.distribution_segment(stops, next);
        if *outbound == Easing::Linear { return t; }
        f64::from(a + span * outbound.apply((progress - a) / span))
    }

    fn distribution_segment<'a>(&'a self, stops: &'a [ArrayPropertyStop], next: usize) -> &'a Easing {
        next.checked_sub(1)
            .filter(|_| next < stops.len())
            .and_then(|i| stops.get(i))
            .and_then(|stop| stop.outbound.as_ref())
            .unwrap_or_else(|| self.segment_default(stops))
    }

    fn properties(&self, stops: &[ArrayPropertyStop], t: f32) -> ArrayPropertyStop {
        let linear = || ArrayPropertyStop::sample(stops.iter(), t);
        let filter_stops = stops.first().is_some_and(|stop| !stop.filters.is_empty());
        let mut sampled = if !(self.transform || self.colour_effect || self.filters && filter_stops)
            || (self.mode == ArrayEaseMode::Global && self.preset == Easing::Linear) {
            linear()
        } else {
            let mut eased = match self.mode {
                ArrayEaseMode::Global => ArrayPropertyStop::sample_global(stops, self.preset.apply(t)),
                ArrayEaseMode::PerSegment => ArrayPropertyStop::sample_segments(stops.iter(), t, Some(self.segment_default(stops))),
            };
            if !self.transform || !self.colour_effect || (!self.filters && filter_stops) {
                let base = linear();
                if !self.transform {
                    eased.scale = base.scale;
                    eased.rotation = base.rotation;
                }
                if !self.colour_effect {
                    eased.alpha = base.alpha;
                    eased.color = base.color;
                }
                if !self.filters {
                    eased.filters = base.filters;
                }
            }
            eased
        };
        // Discrete parameters follow the authored boundaries even when the ease reaches a
        // different segment, runs backwards, or overshoots the track.
        if filter_stops {
            let next = stops.partition_point(|stop| stop.offset < t);
            let held = stops.get(next).filter(|stop| stop.offset == t).or_else(|| stops.get(next.saturating_sub(1)));
            if let Some(held) = held {
                for (filter, source) in sampled.filters.iter_mut().zip(&held.filters) {
                    for param in FilterParam::STEPPED {
                        if let Some(value) = source.param(param) { filter.set_param(param, value); }
                    }
                    if let (Filter::Bevel { side, .. }, Filter::Bevel { side: held_side, .. })
                        | (Filter::GradientGlow { side, .. }, Filter::GradientGlow { side: held_side, .. })
                        | (Filter::GradientBevel { side, .. }, Filter::GradientBevel { side: held_side, .. }) = (filter, source)
                    {
                        *side = *held_side;
                    }
                }
            }
        }
        sampled
    }
}

fn opt_named_ease(p: &Value, key: &str) -> Result<Option<Easing>> {
    opt_str(p, key)?.map(|name| {
        Easing::from_name(name).filter(|ease| ease.name().is_some())
            .ok_or_else(|| EngineError::Param(format!("unknown Array easing preset '{name}'")))
    }).transpose()
}

/// The same continuous curve used for Array placement and the editor's stage guide.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RadialDistribution {
    pub centre: [f32; 2],
    pub radius: f32,
    pub start: f32,
    pub sweep: f32,
    pub closed_loop: bool,
    pub spiral_offset: f32,
    pub centre_drift: [f32; 2],
    pub rotate: bool,
}

impl RadialDistribution {
    pub fn sweep_limit(self) -> f32 {
        if self.closed_loop { CLOSED_SWEEP_LIMIT } else { f32::INFINITY }
    }

    fn travel(self, fraction: f64) -> f64 {
        let limit = self.sweep_limit();
        f64::from(self.sweep.clamp(-limit, limit)) * fraction
    }

    pub fn radius_at(self, fraction: f64) -> f64 {
        let turns = self.travel(fraction).abs() / f64::from(CLOSED_SWEEP_LIMIT);
        self.radius_after_turns(turns)
    }

    fn radius_after_turns(self, turns: f64) -> f64 {
        let offset = if self.closed_loop { 0.0 } else { f64::from(self.spiral_offset) };
        (f64::from(self.radius) + offset * turns).max(0.0)
    }

    /// Local centre after this many unsigned revolutions of an open distribution.
    pub fn centre_after_turns(self, turns: f64) -> [f64; 2] {
        let [x, y] = self.centre.map(f64::from);
        let [dx, dy] = if self.closed_loop { [0.0, 0.0] } else { self.centre_drift.map(f64::from) };
        [x + dx * turns, y + dy * turns]
    }

    fn needs_angular_path(self) -> bool {
        !self.closed_loop && (self.spiral_offset != 0.0 || self.centre_drift != [0.0; 2] || self.sweep.abs() > CLOSED_SWEEP_LIMIT)
    }

    pub fn point(self, fraction: f64) -> Vec2 {
        let [x, y] = self.point_f64(fraction);
        Vec2::new(x as f32, y as f32)
    }

    fn point_f64(self, fraction: f64) -> [f64; 2] {
        let travel = self.travel(fraction);
        let turns = travel.abs() / f64::from(CLOSED_SWEEP_LIMIT);
        // Reduce whole turns before adding the start angle so the seam is exact.
        let angle = f64::from(self.start) + travel % f64::from(CLOSED_SWEEP_LIMIT);
        let (sin, cos) = angle.to_radians().sin_cos();
        let r = self.radius_after_turns(turns);
        let [x, y] = self.centre_after_turns(turns);
        [x + r * cos, y + r * sin]
    }
}

/// Blur-only `array.create` recordings store the two sizes without a filter type.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyBlur {
    blur_x: f32,
    blur_y: f32,
}

fn blend_filter(from: &Filter, to: &Filter, t: f32) -> Filter {
    if from == to { return from.clone(); }
    let mut target = to.clone();
    if let (Filter::DropShadow { angle, .. }, Filter::DropShadow { angle: target_angle, .. })
        | (Filter::Bevel { angle, .. }, Filter::Bevel { angle: target_angle, .. })
        | (Filter::GradientGlow { angle, .. }, Filter::GradientGlow { angle: target_angle, .. })
        | (Filter::GradientBevel { angle, .. }, Filter::GradientBevel { angle: target_angle, .. }) = (from, &mut target)
    {
        let delta = *target_angle - *angle;
        // Both authored directions are normalized. Keep exact half turns in their
        // authored direction and unwrap the destination when the shorter route crosses zero.
        if delta.abs() > 180.0 { *target_angle -= delta.signum() * 360.0; }
    }
    let Some(mut blended) = from.lerp(&target, t) else { return from.clone() };
    if let (Filter::GradientGlow { stops, .. }, Filter::GradientGlow { stops: source, .. })
        | (Filter::GradientBevel { stops, .. }, Filter::GradientBevel { stops: source, .. }) = (&mut blended, from)
        && let Some(pin) = pinned_stop(from)
        && let Some(pin_index) = source.iter().position(|stop| stop.offset == pin)
    {
        // Constrain overshoot in source order, keeping each colour on its side of the pin.
        let mut previous = 0.0;
        for (index, stop) in stops.iter_mut().enumerate() {
            let range = ramp_slot_range(pin, index.cmp(&pin_index));
            stop.offset = stop.offset.max(previous).clamp(*range.start(), *range.end());
            previous = stop.offset;
        }
    }
    blended.sanitized()
}

fn ramp_slot_range(pin: f32, side: Ordering) -> RangeInclusive<f32> {
    match side {
        Ordering::Less => 0.0..=(pin - REST_SNAP).max(0.0),
        Ordering::Equal => pin..=pin,
        Ordering::Greater => (pin + REST_SNAP)..=1.0,
    }
}

fn ramp_matches_source(stops: &[GradientStop], source: &[GradientStop], pin: f32) -> bool {
    let Some(pin_index) = source.iter().position(|stop| stop.offset == pin) else { return false };
    stops.len() == source.len()
        && stops.iter().enumerate().all(|(index, stop)| ramp_slot_range(pin, index.cmp(&pin_index)).contains(&stop.offset))
        && stops.iter().zip(stops.iter().skip(1)).all(|(a, b)| a.offset <= b.offset)
}

fn supported_filters(stack: &[FilterEntry]) -> Vec<Filter> {
    stack.iter().filter(|entry| ArrayPropertyStop::supports_filter(&entry.filter))
        .map(|entry| entry.filter.sanitized()).collect()
}

/// One property snapshot on the array's 0..1 progression.
#[derive(Clone, Debug, PartialEq)]
pub struct ArrayPropertyStop {
    pub offset: f32,
    pub alpha: f32,
    /// The stop's colour style. The Advanced alpha multiplier lives in `alpha`.
    pub color: ColorEffect,
    pub scale: [f32; 2],
    pub rotation: f32,
    /// One entry per supported source filter, in stack order. On/off lives in the source.
    pub filters: Vec<Filter>,
    /// None denotes a stop from a command with no authored outbound preset.
    pub outbound: Option<Easing>,
}

impl ArrayPropertyStop {
    pub const LIMIT: usize = 15;
    pub const SCALE_LIMIT: f32 = 100.0;
    pub const ROTATION_LIMIT: f32 = 100_000.0;

    pub const fn new(offset: f32, alpha: f32) -> Self {
        Self { offset, alpha, color: ColorEffect::None, scale: [1.0; 2], rotation: 0.0, filters: Vec::new(), outbound: None }
    }

    pub fn supports_filter(filter: &Filter) -> bool {
        matches!(filter, Filter::Blur { .. } | Filter::DropShadow { .. } | Filter::Glow { .. } | Filter::Bevel { .. }
            | Filter::GradientGlow { .. } | Filter::GradientBevel { .. } | Filter::AdjustColor { .. })
    }

    /// Authored values as shown in Properties, apart from the distribution's position.
    pub fn from_source(source: &Element) -> Self {
        let basis = source.transform.decompose();
        Self {
            color: stop_color_style(source.color),
            scale: [basis.sx, basis.sy],
            rotation: basis.rotation.to_degrees(),
            filters: supported_filters(source.kind.filters().unwrap_or_default()),
            ..Self::new(0.0, source.color.alpha())
        }
    }

    /// Hold the endpoint value outside the first or last stop, as gradients do.
    /// Both the editor and command pass in stops sorted by position.
    pub fn sample<'a>(stops: impl IntoIterator<Item = &'a Self>, t: f32) -> Self {
        Self::sample_segments(stops, t, None)
    }

    fn sample_segments<'a>(stops: impl IntoIterator<Item = &'a Self>, t: f32, segment_default: Option<&Easing>) -> Self {
        let mut stops = stops.into_iter();
        let Some(mut a) = stops.next() else { return Self::new(t, 1.0) };
        if t <= a.offset { return Self { offset: t, ..a.clone() }; }
        for b in stops {
            if t <= b.offset {
                let span = b.offset - a.offset;
                if span <= f32::EPSILON || t == b.offset { return Self { offset: t, ..b.clone() }; }
                let ease = segment_default.map_or(&Easing::Linear, |default| a.outbound.as_ref().unwrap_or(default));
                return a.blend(b, t, ease.apply((t - a.offset) / span));
            }
            a = b;
        }
        Self { offset: t, ..a.clone() }
    }

    fn sample_global(stops: &[Self], t: f32) -> Self {
        if (0.0..=1.0).contains(&t) {
            return Self::sample(stops.iter(), t);
        }
        // Continue from the value at the edge of the track using the nearest usable
        // stop span. A held margin inside the track keeps its original value.
        let (boundary, end, other) = if t < 0.0 {
            let Some(end) = stops.first() else { return Self::new(t, 1.0) };
            (0.0, end, stops.iter().find(|stop| stop.offset - end.offset > f32::EPSILON))
        } else {
            // At a coincident endpoint, match the value that sampling exactly 1 returns.
            let end = stops.get(stops.partition_point(|stop| stop.offset < 1.0)).or_else(|| stops.last());
            let Some(end) = end else { return Self::new(t, 1.0) };
            (1.0, end, stops.iter().rev().find(|stop| end.offset - stop.offset > f32::EPSILON))
        };
        other.map_or_else(|| Self { offset: t, ..end.clone() }, |other| {
            end.blend(other, t, (t - boundary) / (other.offset - end.offset))
        })
    }

    fn blend(&self, other: &Self, t: f32, f: f32) -> Self {
        let blend = |a, b| a + (b - a) * f;
        let scale = |a: f32, b: f32| {
            // Imported instances can start larger than the normal editing range.
            let limit = Self::SCALE_LIMIT.max(a.abs()).max(b.abs());
            blend(a, b).clamp(-limit, limit)
        };
        Self {
            offset: t,
            alpha: blend(self.alpha, other.alpha).clamp(alpha_min(self.color).min(alpha_min(other.color)), 1.0),
            color: self.color.lerp(&other.color, f).sanitized(),
            scale: [scale(self.scale[0], other.scale[0]), scale(self.scale[1], other.scale[1])],
            rotation: blend(self.rotation, other.rotation),
            filters: self.filters.iter().zip(&other.filters).map(|(a, b)| blend_filter(a, b, f)).collect(),
            outbound: None,
        }
    }

    fn color_effect(&self) -> ColorEffect {
        match self.color {
            ColorEffect::None => ColorEffect::opacity(self.alpha),
            color if self.alpha == 1.0 => color,
            color => color.with_alpha(self.alpha),
        }
    }

    /// Compatibility with Array recordings that composed a relative effect over the source.
    fn apply_color(&self, source: ColorEffect) -> ColorEffect {
        let effect = self.color_effect();
        if effect.transform().is_identity() && source != ColorEffect::None {
            source
        } else {
            effect.nested(&source)
        }
    }

    fn apply_transform(&self, base: Affine, pivot: Option<Vec2>) -> Affine {
        let [sx, sy] = self.scale;
        // Wrap after interpolation so full turns affect the intermediate copies.
        let degrees = self.rotation % CLOSED_SWEEP_LIMIT;
        if [sx, sy] == [1.0; 2] && degrees == 0.0 { return base; }
        let adjusted = Affine::rotate(degrees.to_radians()).mul(&base.mul(&Affine::scale(sx, sy)));
        keep_pivot(base, adjusted, pivot)
    }

    fn source_transform(&self, mut original: Affine, basis: Decomposed) -> Affine {
        let original_scale = [basis.sx, basis.sy];
        let degrees = (self.rotation - basis.rotation.to_degrees()) % CLOSED_SWEEP_LIMIT;
        if self.scale != original_scale {
            let unit = Decomposed { sx: 1.0, sy: 1.0, ..basis }.compose();
            for (i, (from, to)) in original_scale.into_iter().zip(self.scale).enumerate() {
                if from == to { continue; }
                let idx = 2 * i;
                for j in idx..idx + 2 {
                    // Keep the original direction and shear. A collapsed axis needs a basis
                    // direction from the decomposition before it can be given a scale.
                    original.m[j] = if from == 0.0 { unit.m[j] * to } else { original.m[j] / from * to };
                }
            }
        }
        if degrees == 0.0 { original } else { Affine::rotate(degrees.to_radians()).mul(&original) }
    }
}

fn keep_pivot(base: Affine, mut adjusted: Affine, pivot: Option<Vec2>) -> Affine {
    let shift = pivot.map_or(Vec2::ZERO, |p| base.apply_vector(p) - adjusted.apply_vector(p));
    adjusted.m[4] = base.m[4] + shift.x;
    adjusted.m[5] = base.m[5] + shift.y;
    adjusted
}

fn stop_color_style(color: ColorEffect) -> ColorEffect {
    match color {
        ColorEffect::Alpha(_) => ColorEffect::None,
        ColorEffect::Advanced(_) => color.with_alpha(1.0),
        color => color,
    }
}

fn alpha_min(color: ColorEffect) -> f32 {
    if matches!(color, ColorEffect::Advanced(_)) { *anicraft_doc::ADVANCED_MUL.start() } else { 0.0 }
}

impl Session {
    pub fn array_preview_active(&self) -> bool {
        self.array_preview
    }

    /// Shared by geometry-only previews with the same instances, source artwork and stacking.
    pub fn array_translation_generation(&self) -> Option<crate::StateId> {
        self.array_positions.as_ref().map(|preview| preview.generation)
    }

    pub(super) fn require_no_array_preview(&self) -> Result<()> {
        if self.array_preview {
            return Err(EngineError::State("commit or cancel the Array preview first".into()));
        }
        Ok(())
    }

    pub(super) fn require_spatial_array_frame(&self) -> Result<()> {
        if self.active_layer().and_then(|layer| layer.keyframe_at(self.frame))
            .is_some_and(|key| key.motion.is_some() && self.frame > key.start)
        {
            return Err(EngineError::State("Move the playhead to the start of the Motion Tween or outside it to create a Spatial Array.".into()));
        }
        Ok(())
    }

    /// Start a preview from the original artwork. Reuse the instances for position-only edits.
    pub fn preview_array(&mut self, params: &Value) -> Result<()> {
        self.require_spatial_array_frame()?;
        let shared = if let Some(mut object) = params.as_object().filter(|p| p.contains_key("create_unique_symbols")).cloned() {
            opt_bool(params, "create_unique_symbols")?;
            object.remove("create_unique_symbols");
            Some(Value::Object(object))
        } else {
            None
        };
        let params = shared.as_ref().unwrap_or(params);
        if self.array_preview {
            if let Some(mut preview) = self.array_positions.take()
                && preview.move_to(self, params)
            {
                self.array_positions = Some(preview);
                return Ok(());
            }
            self.restore_gesture();
        } else {
            if self.in_gesture() || self.live_cut().is_some() {
                return Err(EngineError::State("finish the current edit before starting an array".into()));
            }
            self.begin_gesture("Array");
        }
        let result = self.execute_command("array.create", params).map(|_| ());
        if result.is_ok() {
            self.array_preview = true;
            self.array_positions = PositionPreview::new(self, params);
        } else if !self.array_preview {
            self.end_gesture();
        }
        result
    }

    /// Bake the current Array settings with unique definitions in the same undo step.
    pub fn commit_unique_array_preview(&mut self, params: &Value) -> Result<()> {
        if !self.array_preview {
            return Err(EngineError::State("no active Array preview".into()));
        }
        let mut params = params.as_object().cloned()
            .ok_or_else(|| EngineError::Param("Array settings must be an object".into()))?;
        params.insert("create_unique_symbols".into(), Value::Bool(true));

        self.commit_array_output("array.create", &Value::Object(params))
    }

    /// Turn the active Linear preview into one native timeline animation.
    pub fn commit_array_timeline_preview(&mut self, params: &Value) -> Result<()> {
        self.commit_array_output("array.create_timeline", params)
    }

    fn commit_array_output(&mut self, command: &str, params: &Value) -> Result<()> {
        if !self.array_preview {
            return Err(EngineError::State("no active Array preview".into()));
        }
        // Replay once from the same source as the preview. Keep the transient
        // pose and recording if commit fails so the artist can edit or cancel.
        let preview = self.snapshot("Array");
        let recording = self.rec.clone();
        self.restore_gesture();
        if let Err(error) = self.execute_command(command, params) {
            self.restore(preview);
            self.rec = recording;
            return Err(error);
        }
        self.array_positions = None;
        self.finish_array_preview(true);
        Ok(())
    }

    pub fn finish_array_preview(&mut self, commit: bool) {
        if !std::mem::take(&mut self.array_preview) {
            return;
        }
        let preview = self.array_positions.take();
        if !commit {
            self.revert_gesture();
        } else if let Some(preview) = preview {
            // Record the current parameters once, including moves made without replaying the
            // command. Replaying history still uses the public Array command.
            self.rec.truncate(self.gesture_rec);
            self.rec.push(crate::history::Recorded::new("array.create", &preview.params));
        }
        self.end_gesture();
    }
}

/// An existing stretch of baked preview instances whose topology stays unchanged on a move.
/// Tweens with keyed motion and linked layers keep the general command path.
pub(super) struct PositionPreview {
    generation: crate::StateId,
    key: usize,
    instances: Range<usize>,
    reversed: bool,
    eased_progress: Option<Vec<f64>>,
    grid_mirrors: [bool; 2],
    params: Value,
}

const LINEAR_COORDS: [&str; 4] = ["start_x", "start_y", "dx", "dy"];
const RADIAL_COORDS: [&str; 8] = ["center_x", "center_y", "radius", "start_angle", "sweep_angle", "spiral_offset", "drift_x", "drift_y"];

const GRID_COORDS: [&str; 4] = ["start_x", "start_y", "spacing_x", "spacing_y"];

fn explicit_positions(p: &Value) -> Option<(Layout, &'static [&'static str])> {
    match p.get("mode")?.as_str()? {
        "linear" => {
            let [start_x, start_y, dx, dy] = LINEAR_COORDS.map(|key| opt_f32(p, key).ok().flatten());
            Some((Layout::Linear { start_x: start_x?, start_y: start_y?, dx: dx?, dy: dy? }, &LINEAR_COORDS))
        }
        "grid" if GRID_COORDS.iter().all(|key| p.get(*key).is_some_and(Value::is_number)) => {
            Some((Layout::Grid(GridDistribution::parse(p, Vec2::ZERO).ok()?), &GRID_COORDS))
        }
        "radial" if ["radius", "start_angle"].iter().all(|key| p.get(*key).is_some_and(Value::is_number)) => {
            let radial = parse_radial(p, Vec2::ZERO).ok()?;
            (!radial.rotate).then_some((Layout::Radial(radial), &RADIAL_COORDS))
        }
        _ => None,
    }
}

impl PositionPreview {
    fn new(s: &Session, p: &Value) -> Option<Self> {
        let (layout, _) = explicit_positions(p)?;
        let easing = ArrayEasing::parse(p).ok()?;
        let tl = s.timeline();
        if tl.has_rig() { return None; }
        let layer = tl.layer(s.layer)?;
        let key = layer.keyframe_index_at(s.frame)?;
        let kf = layer.keyframes.get(key)?;
        if kf.motion.is_some() { return None; }
        let count = match layout {
            Layout::Grid(grid) => grid.rows * grid.columns,
            _ => opt_u32(p, "count").ok()?.unwrap_or(10),
        } as usize;
        let first = kf.elements.iter().position(|e| s.selection.contains(s.layer, e.id))?;
        let instances = first..first + count;
        let reversed = p.get("stacking").and_then(Value::as_str) == Some("front_to_back");
        let copies = kf.elements.get(instances.clone())?;
        // Grid positions always refer to registration, and its stops ease properties alone.
        let grid = matches!(layout, Layout::Grid(_));
        let custom_pivot = !grid && copies.first()?.pivot().is_some() && p.get("property_stops").is_some();
        let source = if custom_pivot && opt_bool(p, "source_values").ok()?.unwrap_or(false) {
            let base = s.gesture.as_ref()?;
            let (layer, id) = base.selection.first()?;
            let layer = crate::editing::edited(&base.doc, base.scene, &base.editing).layer(layer)?;
            let (_, elements) = anicraft_doc::evaluate_layer_elements(layer, base.frame)?;
            Some(ArrayPropertyStop::from_source(elements.iter().find(|element| element.id == id)?))
        } else {
            None
        };
        let stops = if grid { Vec::new() } else { parse_stops(p, source.as_ref(), None).ok()?.unwrap_or_default() };
        // A transformed custom pivot can offset the copy from its layout position.
        // Replay from the source so those offsets stay exact after a move.
        let (scale, rotation) = source.as_ref().map_or(([1.0; 2], 0.0), |source| (source.scale, source.rotation));
        if custom_pivot && stops.iter().any(|stop| stop.scale != scale || stop.rotation != rotation) {
            return None;
        }
        let eased_progress = (!grid && easing.eases_distribution(&stops)).then(|| {
            let count = count as u32;
            (0..count).map(|i| easing.eased_position(&stops, layout.progress(i, count))).collect()
        });
        let ids = copies.iter().map(|e| e.id);
        let in_order = if reversed { ids.rev().eq(s.selection.ids()) } else { ids.eq(s.selection.ids()) };
        in_order.then(|| Self { generation: crate::history::fresh_state(), key, instances, reversed, eased_progress, grid_mirrors: layout.grid_mirrors(), params: p.clone() })
    }

    fn move_to(&mut self, s: &mut Session, params: &Value) -> bool {
        let (Some(old), Some(new), Some((layout, coords))) = (self.params.as_object(), params.as_object(), explicit_positions(params)) else { return false };
        let position_param = |key: &str| coords.contains(&key) || (key == "stagger" && matches!(layout, Layout::Grid(_)));
        if old.len() != new.len()
            || old.iter().any(|(key, v)| !position_param(key) && new.get(key) != Some(v))
            || new.iter().any(|(key, v)| !position_param(key) && old.get(key) != Some(v))
        {
            return false;
        }
        // A reflection changes the cached footprint. Replay once at the crossing
        // to recompute the basis and give the stage a fresh tile generation.
        if layout.grid_mirrors() != self.grid_mirrors { return false; }
        if old == new { return true; }
        let count = self.instances.len() as u32;
        let place = |i, fraction| layout.place(Affine::IDENTITY, i, count, fraction).translation();
        // Check every eased sample before changing anything. Back Out can exceed both the
        // requested span and the nominal last sample, so checking endpoints isn't enough.
        // Grid offsets can peak between endpoints. Its bounded input coordinates and
        // 500-instance cap keep every sample finite, including accumulated offsets.
        let eased_positions = if let Some(progress) = &self.eased_progress {
            let positions: Vec<_> = progress.iter().enumerate().map(|(i, &t)| place(i as u32, Some(t))).collect();
            if !positions.iter().all(|at| at.x.is_finite() && at.y.is_finite()) { return false; }
            Some(positions)
        } else {
            if ![0, count.saturating_sub(1)].into_iter().all(|i| {
                let at = place(i, None);
                at.x.is_finite() && at.y.is_finite()
            }) { return false; }
            None
        };
        let Some(instances) = s.layer_mut(None).ok().and_then(|layer| layer.keyframe_mut(self.key))
            .and_then(|kf| kf.elements.get_mut(self.instances.clone())) else { return false };
        let mut changed = false;
        for (i, el) in instances.iter_mut().enumerate() {
            let i = if self.reversed { count - 1 - i as u32 } else { i as u32 };
            let at = eased_positions.as_ref().and_then(|positions| positions.get(i as usize)).copied()
                .unwrap_or_else(|| place(i, None));
            if at != el.transform.translation() {
                let xf = &mut Arc::make_mut(el).transform;
                xf.m[4] = at.x;
                xf.m[5] = at.y;
                changed = true;
            }
        }
        self.params = params.clone();
        if changed {
            s.state = crate::history::fresh_state();
            s.revision += 1;
            s.dl_cache.replace(None);
        }
        true
    }
}

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: "array.create",
        label: "Create Array",
        menu: "",
        shortcut: None,
        params: "{mode: linear|radial|grid, count: 1..500 (linear and radial, includes original), create_unique_symbols?: bool, dx?, dy?, start_x?, start_y? (linear), center_x?, center_y?, radius?, start_angle?, sweep_angle?, closed_loop?, spiral_offset?, drift_x?, drift_y?, rotate? (radial), rows?, columns?, start_x?, start_y?, spacing_x?, spacing_y?, flip_on_reverse?, property_distribution?: x|xy|y, stagger?: {accumulate?: bool, rows?: {enabled?: bool, every?: positive integer, x?: number, y?: number}, columns?: {enabled?: bool, every?: positive integer, x?: number, y?: number}} (grid), stacking?: back_to_front|front_to_back, source_values?: bool, property_stops?: [{offset: 0..1, alpha?: 0..1 (-1..1 for Advanced), color_effect?: document Colour Effect object, filters?: [{type: blur|drop_shadow|glow|bevel|gradient_glow|gradient_bevel|adjust_color, ...values}, ...], blurs?: [{blur_x: 0..255, blur_y: 0..255}, ...] (legacy alternative), scale_x?: -100..100, scale_y?: -100..100, rotation?: -100000..100000, outbound_ease?: named ease}, ...] (2..15), alpha_stops?: [{offset: 0..1, alpha: 0..1}, ...] (2..15, legacy alternative), easing?: {mode?: global|per_segment, preset?: named ease (e.g. Linear, QuadIn, BackOut, ElasticInOut), distribution?, transform?, colour_effect?, filters?}}. Filter lists match the source's supported entries in stack order. Omitted or empty lists use the source values. Legacy blurs lists follow only the Blur entries. Copies preserve source filter types, order, enabled states, Blur quality and disabled Adjust Color values. Drop Shadow, Glow, Bevel, Gradient Glow and Gradient Bevel quality and switches, including filter type settings, hold until the next authored stop without easing. Drop Shadow, Bevel, Gradient Glow and Gradient Bevel angles take the shortest arc. An omitted stops field inherits the source ramp. Authored ramps keep the source stop count and order, with a transparent pin at 0% for Gradient Glow or 50% for Gradient Bevel. Adjust Color values interpolate directly, including signed Hue in degrees. Legacy filter lists may omit all entries of Gradient Bevel or Adjust Color to keep their source values. Easing defaults to Global Linear with all four families enabled. In Per Segment mode each stop eases toward the next using outbound_ease (default Linear). If no stop supplies outbound_ease, preset supplies the legacy shared segment curve. With source_values enabled, scale, rotation and colour are authored instance values and omitted properties use the source values. Otherwise, scale and rotation are relative, colour effects compose over the source, and older calls without color_effect keep absolute alpha. Alpha is the stop effect's alpha multiplier. Radial sweep defaults to 360 degrees. Closed loop defaults to true, uses count intervals and limits sweep to one turn. Open mode uses count minus one intervals. spiral_offset changes radius down to zero and drift_x/drift_y move the centre. All three rates default to zero and are measured in pixels per turn. Grid defaults to 3 rows and 3 columns with 60 px spacing on both axes. Rows times columns must be 1..500 (includes original). Grid places registration points in row-major order, even with negative spacing. Flip on Reverse defaults to true and reflects artwork around its registration point along each occupied axis with negative spacing. Grid uses an evenly spaced base lattice. Grid property distribution defaults to xy (row-major order). With x, properties and their easing repeat across rows. With y, they repeat across columns. A one-instance sampling axis uses the start of the progression. Grid stagger rows and columns default to disabled with every=2 and stage offsets of (50, 0) and (0, 50) px respectively. Offsets add at intersections. With accumulate off, every Nth row or column gets one offset. With accumulate on, each qualifying row or column adds an offset that persists on subsequent rows or columns. The first row and first column never add an offset, including when every=1. Stagger leaves the base direction and property sampling intact. Requires exactly one selected symbol instance on the active layer. Moves the original to position 1, bakes the copies and selects them all. With create_unique_symbols (default false), each instance references a separate copy of the original definition in its Library folder. Names continue the highest numbered suffix in the same naming family. Instance properties and shared nested Library references are preserved.",
        mutates: true,
        run: create,
    }, timeline::spec()]
}

fn parse_stops(p: &Value, source: Option<&ArrayPropertyStop>, stack: Option<&[FilterEntry]>) -> Result<Option<Vec<ArrayPropertyStop>>> {
    let (field, value, legacy) = match (p.get("property_stops"), p.get("alpha_stops")) {
        (Some(_), Some(_)) => return Err(EngineError::Param("use property_stops or alpha_stops".into())),
        (Some(v), None) => ("property_stops", v, false),
        (None, Some(v)) => ("alpha_stops", v, true),
        (None, None) => return Ok(None),
    };
    let stops = value.as_array().ok_or_else(|| EngineError::Param(format!("{field} must be an array")))?;
    if !(2..=ArrayPropertyStop::LIMIT).contains(&stops.len()) {
        return Err(EngineError::Param(format!("{field} needs 2..={} stops", ArrayPropertyStop::LIMIT)));
    }
    let source_scale = source.map_or([1.0; 2], |source| source.scale);
    let [limit_x, limit_y] = source_scale.map(|value| ArrayPropertyStop::SCALE_LIMIT.max(value.abs()));
    let defaults = stack.filter(|_| !legacy && stops.iter().any(|v| v.get("filters").is_some() || v.get("blurs").is_some()))
        .map(supported_filters);
    let mut out = Vec::with_capacity(stops.len());
    for v in stops {
        let number = |key: &str, min: f32, max: f32, default: Option<f32>| -> Result<f32> {
            if v.get(key).is_none() && let Some(default) = default { return Ok(default); }
            let n = v.get(key).and_then(Value::as_f64).ok_or_else(|| EngineError::Param(format!("{field} entries need numeric '{key}'")))?;
            if !(f64::from(min)..=f64::from(max)).contains(&n) {
                return Err(EngineError::Param(format!("{field} '{key}' must be in {min}..={max}")));
            }
            Ok(n as f32)
        };
        let offset = number("offset", 0.0, 1.0, None)?;
        let color = match v.get("color_effect").filter(|_| !legacy) {
            None => source.map_or(ColorEffect::None, |source| source.color_effect()),
            Some(raw) => {
                if !raw.is_object() || opt_str(raw, "style")?.is_some_and(|s| ColorStyle::from_key(s).is_none()) {
                    return Err(EngineError::Param("color_effect must be a supported Colour Effect object".into()));
                }
                serde_json::from_value::<ColorEffect>(raw.clone())
                    .map_err(|e| EngineError::Param(format!("invalid color_effect: {e}")))?
            }
        };
        let alpha = number("alpha", alpha_min(color), 1.0, (!legacy).then(|| color.alpha()))?;
        let mut stop = ArrayPropertyStop::new(offset, alpha);
        if !legacy {
            stop.color = stop_color_style(color);
            let rotation = ArrayPropertyStop::ROTATION_LIMIT;
            stop.scale = [number("scale_x", -limit_x, limit_x, Some(source_scale[0]))?, number("scale_y", -limit_y, limit_y, Some(source_scale[1]))?];
            stop.rotation = number("rotation", -rotation, rotation, Some(source.map_or(0.0, |source| source.rotation)))?;
            stop.outbound = opt_named_ease(v, "outbound_ease")?;
            if let Some(defaults) = &defaults {
                stop.filters = parse_stop_filters(v, defaults)?;
            }
        }
        out.push(stop);
    }
    out.sort_by(|a, b| a.offset.total_cmp(&b.offset));
    Ok(Some(out))
}

fn parse_stop_filters(stop: &Value, defaults: &[Filter]) -> Result<Vec<Filter>> {
    let legacy = match (stop.get("filters"), stop.get("blurs")) {
        (Some(_), Some(_)) => return Err(EngineError::Param("use filters or blurs in an Array stop".into())),
        (Some(list), None) => {
            let entries = crate::cmds_filter::parse_entries(list, defaults.len())?;
            if entries.is_empty() { return Ok(defaults.to_vec()); }
            let stack_error = || EngineError::Param("Array stop filters must match the supported source filters in stack order".into());
            // Earlier commands can omit entire filter families added in later milestones.
            let has_bevel_ramps = entries.iter().any(|entry| matches!(entry.filter, Filter::GradientBevel { .. }));
            let has_adjust_color = entries.iter().any(|entry| matches!(entry.filter, Filter::AdjustColor { .. }));
            let mut entries = entries.into_iter().zip(list.as_array().into_iter().flatten());
            let filters = defaults.iter().map(|source| {
                if (!has_bevel_ramps && matches!(source, Filter::GradientBevel { .. }))
                    || (!has_adjust_color && matches!(source, Filter::AdjustColor { .. })) {
                    return Ok(source.clone());
                }
                let (mut entry, raw) = entries.next().ok_or_else(stack_error)?;
                if std::mem::discriminant(&entry.filter) != std::mem::discriminant(source) {
                    return Err(stack_error());
                }
                if let (Filter::GradientGlow { stops, .. }, Filter::GradientGlow { stops: source_ramp, .. })
                    | (Filter::GradientBevel { stops, .. }, Filter::GradientBevel { stops: source_ramp, .. }) = (&mut entry.filter, source)
                    && let Some(pin) = pinned_stop(source)
                {
                    if raw.get("stops").is_none() {
                        stops.clone_from(source_ramp);
                    } else if !ramp_matches_source(stops, source_ramp, pin) {
                        return Err(EngineError::Param(format!("Array {} ramps must retain the source stop count and order with the pin at {}%", source.label(), pin * 100.0)));
                    }
                }
                Ok(entry.filter.sanitized())
            }).collect::<Result<Vec<_>>>()?;
            if entries.next().is_some() { return Err(stack_error()); }
            return Ok(filters);
        }
        (None, Some(blurs)) => blurs,
        (None, None) => return Ok(defaults.to_vec()),
    };
    let blurs: Vec<LegacyBlur> = serde_json::from_value(legacy.clone())
        .map_err(|e| EngineError::Param(format!("invalid Array blurs: {e}")))?;
    let mut filters = defaults.to_vec();
    if blurs.is_empty() { return Ok(filters); }
    let axes = filters.iter_mut().filter_map(|filter| match filter {
        Filter::Blur { blur_x, blur_y, .. } => Some((blur_x, blur_y)),
        _ => None,
    });
    if blurs.len() != defaults.iter().filter(|filter| matches!(filter, Filter::Blur { .. })).count() {
        return Err(EngineError::Param("Array stop needs one set of Blur sizes per source Blur filter".into()));
    }
    for ((x, y), blur) in axes.zip(blurs) {
        if ![blur.blur_x, blur.blur_y].into_iter().all(|v| (0.0..=MAX_BLUR).contains(&v)) {
            return Err(EngineError::Param(format!("Array Blur sizes must be in 0..={MAX_BLUR}")));
        }
        *x = blur.blur_x;
        *y = blur.blur_y;
    }
    Ok(filters)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GridPropertyDistribution {
    X,
    #[default]
    Both,
    Y,
}

impl GridPropertyDistribution {
    pub const fn key(self) -> &'static str {
        match self {
            Self::X => "x",
            Self::Both => "xy",
            Self::Y => "y",
        }
    }

    fn parse(params: &Value) -> Result<Self> {
        match opt_str(params, "property_distribution")? {
            Some("x") => Ok(Self::X),
            Some("y") => Ok(Self::Y),
            None | Some("xy") => Ok(Self::Both),
            Some(_) => Err(EngineError::Param("grid property_distribution must be x, xy or y".into())),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridStaggerAxis {
    pub enabled: bool,
    pub every: NonZeroU32,
    pub offset: [f32; 2],
}

impl GridStaggerAxis {
    fn parse(value: Option<&Value>, defaults: Self) -> Result<Self> {
        let Some(value) = value else { return Ok(defaults) };
        if !value.is_object() {
            return Err(EngineError::Param("stagger rows and columns must be objects".into()));
        }
        let every = opt_u32(value, "every")?.unwrap_or(defaults.every.get());
        let every = NonZeroU32::new(every)
            .ok_or_else(|| EngineError::Param("stagger every must be a positive integer".into()))?;
        Ok(Self {
            enabled: opt_bool(value, "enabled")?.unwrap_or(defaults.enabled),
            every,
            offset: [
                opt_f32(value, "x")?.unwrap_or(defaults.offset[0]),
                opt_f32(value, "y")?.unwrap_or(defaults.offset[1]),
            ],
        })
    }

    fn displacement(self, index: u32, accumulate: bool) -> Vec2 {
        if !self.enabled || index == 0 { return Vec2::ZERO; }
        let every = self.every.get();
        let number = index + 1;
        let increments = if accumulate {
            number / every - u32::from(every == 1)
        } else {
            u32::from(number.is_multiple_of(every))
        };
        let [x, y] = self.offset;
        Vec2::new(x, y) * increments as f32
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridStagger {
    pub rows: GridStaggerAxis,
    pub columns: GridStaggerAxis,
    pub accumulate: bool,
}

impl Default for GridStagger {
    fn default() -> Self {
        let rows = GridStaggerAxis { enabled: false, every: NonZeroU32::MIN.saturating_add(1), offset: [50.0, 0.0] };
        Self { rows, columns: GridStaggerAxis { offset: [0.0, 50.0], ..rows }, accumulate: false }
    }
}

impl GridStagger {
    fn parse(params: &Value) -> Result<Self> {
        let defaults = Self::default();
        let Some(stagger) = params.get("stagger") else { return Ok(defaults) };
        if !stagger.is_object() {
            return Err(EngineError::Param("grid stagger must be an object".into()));
        }
        Ok(Self {
            rows: GridStaggerAxis::parse(stagger.get("rows"), defaults.rows)?,
            columns: GridStaggerAxis::parse(stagger.get("columns"), defaults.columns)?,
            accumulate: opt_bool(stagger, "accumulate")?.unwrap_or(defaults.accumulate),
        })
    }
}

#[derive(Clone, Copy, Debug)]
struct GridDistribution {
    rows: u32,
    columns: u32,
    start: Vec2,
    spacing: Vec2,
    flip_on_reverse: bool,
    property_distribution: GridPropertyDistribution,
    stagger: GridStagger,
}

impl GridDistribution {
    fn parse(params: &Value, origin: Vec2) -> Result<Self> {
        let rows = opt_u32(params, "rows")?.unwrap_or(3);
        let columns = opt_u32(params, "columns")?.unwrap_or(3);
        if rows == 0 || columns == 0 || rows.saturating_mul(columns) > MAX_ARRAY_INSTANCES {
            return Err(EngineError::Param(format!(
                "grid rows and columns must be positive and total at most {MAX_ARRAY_INSTANCES} instances"
            )));
        }
        Ok(Self {
            rows,
            columns,
            start: Vec2::new(
                opt_f32(params, "start_x")?.unwrap_or(origin.x),
                opt_f32(params, "start_y")?.unwrap_or(origin.y),
            ),
            spacing: Vec2::new(
                opt_f32(params, "spacing_x")?.unwrap_or(60.0),
                opt_f32(params, "spacing_y")?.unwrap_or(60.0),
            ),
            flip_on_reverse: opt_bool(params, "flip_on_reverse")?.unwrap_or(true),
            property_distribution: GridPropertyDistribution::parse(params)?,
            stagger: GridStagger::parse(params)?,
        })
    }

    fn point(self, index: u32) -> Vec2 {
        let column = index % self.columns;
        let row = index / self.columns;
        let base = self.start + Vec2::new(column as f32 * self.spacing.x, row as f32 * self.spacing.y);
        if !self.stagger.rows.enabled && !self.stagger.columns.enabled {
            return base;
        }
        base + self.stagger.rows.displacement(row, self.stagger.accumulate)
            + self.stagger.columns.displacement(column, self.stagger.accumulate)
    }

    fn mirrors(self) -> [bool; 2] {
        [
            self.flip_on_reverse && self.columns > 1 && self.spacing.x < 0.0,
            self.flip_on_reverse && self.rows > 1 && self.spacing.y < 0.0,
        ]
    }

    fn orient(self, mut transform: Affine) -> Affine {
        // Reflect the authored artwork in stage axes, after its stop transform.
        let [flip_x, flip_y] = self.mirrors();
        if flip_x {
            transform.m[0] = -transform.m[0];
            transform.m[2] = -transform.m[2];
        }
        if flip_y {
            transform.m[1] = -transform.m[1];
            transform.m[3] = -transform.m[3];
        }
        transform
    }
}

#[derive(Clone, Copy, Debug)]
enum Layout {
    Linear { start_x: f32, start_y: f32, dx: f32, dy: f32 },
    Radial(RadialDistribution),
    Grid(GridDistribution),
}

impl Layout {
    fn grid_mirrors(self) -> [bool; 2] {
        match self {
            Self::Grid(grid) => grid.mirrors(),
            _ => [false; 2],
        }
    }

    fn progress(self, index: u32, count: u32) -> f64 {
        if let Self::Grid(grid) = self {
            let (axis_index, axis_count) = match grid.property_distribution {
                GridPropertyDistribution::X => (index % grid.columns, grid.columns),
                GridPropertyDistribution::Y => (index / grid.columns, grid.rows),
                GridPropertyDistribution::Both => (index, count),
            };
            return f64::from(axis_index) / f64::from(axis_count.saturating_sub(1).max(1));
        }
        let closed_loop = matches!(self, Layout::Radial(radial) if radial.closed_loop);
        let divisor = count.saturating_sub(u32::from(!closed_loop)).max(1);
        f64::from(index) / f64::from(divisor)
    }

    fn place(self, source: Affine, index: u32, count: u32, eased: Option<f64>) -> Affine {
        match self {
            Layout::Linear { start_x, start_y, dx, dy } => {
                let mut m = source;
                let step = eased.map_or(index as f32, |t| (t * f64::from(count.saturating_sub(1))) as f32);
                m.m[4] = start_x + step * dx;
                m.m[5] = start_y + step * dy;
                m
            }
            Layout::Radial(radial) => {
                let t = eased.unwrap_or_else(|| self.progress(index, count));
                // Rotate the basis by the signed travel, then set the registration point.
                let mut m = if radial.rotate { Affine::rotate(radial.travel(t).to_radians() as f32).mul(&source) } else { source };
                let at = radial.point(t);
                m.m[4] = at.x;
                m.m[5] = at.y;
                m
            }
            Layout::Grid(grid) => {
                let at = grid.point(index);
                let mut m = source;
                m.m[4] = at.x;
                m.m[5] = at.y;
                m
            }
        }
    }
}

fn parse_radial(p: &Value, original: Vec2) -> Result<RadialDistribution> {
    let x = opt_f32(p, "center_x")?.ok_or_else(|| EngineError::Param("radial requires center_x".into()))?;
    let y = opt_f32(p, "center_y")?.ok_or_else(|| EngineError::Param("radial requires center_y".into()))?;
    let delta = original - Vec2::new(x, y);
    let radius = opt_f32(p, "radius")?.unwrap_or_else(|| delta.x.hypot(delta.y));
    if !radius.is_finite() || radius < 0.0 {
        return Err(EngineError::Param("radius must be finite and >= 0".into()));
    }
    Ok(RadialDistribution {
        centre: [x, y], radius,
        start: opt_f32(p, "start_angle")?.unwrap_or_else(|| delta.y.atan2(delta.x).to_degrees()),
        sweep: opt_f32(p, "sweep_angle")?.unwrap_or(CLOSED_SWEEP_LIMIT),
        closed_loop: opt_bool(p, "closed_loop")?.unwrap_or(true),
        spiral_offset: opt_f32(p, "spiral_offset")?.unwrap_or(0.0),
        centre_drift: [opt_f32(p, "drift_x")?.unwrap_or(0.0), opt_f32(p, "drift_y")?.unwrap_or(0.0)],
        rotate: opt_bool(p, "rotate")?.unwrap_or(false),
    })
}

fn numbered_symbol_name(name: &str) -> Option<(&str, &str)> {
    let (stem, number) = name.rsplit_once('_')?;
    (!number.is_empty() && number.bytes().all(|digit| digit.is_ascii_digit()))
        .then_some((stem, number.trim_start_matches('0')))
}

fn unique_symbol_names(doc: &Document, id: SymbolId, count: u32) -> Result<Vec<String>> {
    let source = doc.symbol(id)
        .ok_or_else(|| EngineError::State(format!("no symbol with id {}", id.0)))?;
    let name = source.name.trim();
    let stem = numbered_symbol_name(name).map_or(name, |(stem, _)| stem);
    let family = stem.to_lowercase();
    let parent = doc.parent_of(LibraryEntry::Item(LibraryItem::Symbol(id)));
    let last = doc.library_children(parent).into_iter()
        .filter_map(|entry| numbered_symbol_name(doc.entry_name(entry)?))
        .filter(|(prefix, _)| prefix.to_lowercase() == family)
        .map(|(_, number)| number)
        .max_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)))
        .unwrap_or_default();

    // Decimal digits avoid integer overflow even for imported, very large suffixes.
    // Starting above every sibling's suffix reserves the entire sequence at once.
    let mut suffix = last.as_bytes().to_vec();
    (0..count).map(|_| {
        let mut carry = true;
        for digit in suffix.iter_mut().rev() {
            if *digit < b'9' {
                *digit += 1;
                carry = false;
                break;
            }
            *digit = b'0';
        }
        if carry { suffix.insert(0, b'1'); }
        let number: String = suffix.iter().copied().map(char::from).collect();
        anicraft_doc::library::clean_name(&format!("{stem}_{number}"))
            .map_err(|error| EngineError::Param(error.to_string()))
    }).collect()
}

fn create(s: &mut Session, p: &Value) -> Result<Value> {
    s.require_spatial_array_frame()?;
    let create_unique = opt_bool(p, "create_unique_symbols")?.unwrap_or(false);
    let mode = opt_str(p, "mode")?.unwrap_or("linear");
    let count = if mode == "grid" {
        None
    } else {
        let count = opt_u32(p, "count")?.unwrap_or(10);
        if !(1..=MAX_ARRAY_INSTANCES).contains(&count) {
            return Err(EngineError::Param(format!("count must be in 1..={MAX_ARRAY_INSTANCES} (including original)")));
        }
        Some(count)
    };
    let front_to_back = match opt_str(p, "stacking")? {
        None | Some("back_to_front") => false,
        Some("front_to_back") => true,
        Some(other) => return Err(EngineError::Param(format!("unknown stacking '{other}'"))),
    };
    let source_values = opt_bool(p, "source_values")?.unwrap_or(false) && p.get("property_stops").is_some();
    let easing = ArrayEasing::parse(p)?;
    // Old Array command recordings used an absolute alpha multiplier. The full colour
    // controls use a relative effect, even when every stop is set to the neutral style.
    let compose_colour = p.get("property_stops").and_then(Value::as_array)
        .is_some_and(|stops| stops.iter().any(|stop| stop.get("color_effect").is_some()));
    let Some((_, id)) = s.selection.first().filter(|(layer, _)| *layer == s.layer && s.selection.len() == 1) else {
        return Err(EngineError::State("select exactly one symbol instance on the active layer".into()));
    };
    let original = s.editable_keyframe()?.elements.iter().find(|e| e.id == id)
        .map(|e| (**e).clone())
        .ok_or_else(|| EngineError::State("selected instance is not in the editable keyframe".into()))?;
    let ElementKind::Symbol { symbol: source_symbol, .. } = &original.kind else {
        return Err(EngineError::State("the initial Array implementation requires a symbol instance".into()));
    };
    let seed = source_values.then(|| ArrayPropertyStop::from_source(&original));
    let basis = source_values.then(|| original.transform.decompose());
    let stops = parse_stops(p, seed.as_ref(), Some(original.kind.filters().unwrap_or_default()))?;
    let [original_x, original_y] = [original.transform.m[4], original.transform.m[5]];
    let layout = match mode {
        "linear" => Layout::Linear {
            start_x: opt_f32(p, "start_x")?.unwrap_or(original_x),
            start_y: opt_f32(p, "start_y")?.unwrap_or(original_y),
            dx: opt_f32(p, "dx")?.unwrap_or(30.0),
            dy: opt_f32(p, "dy")?.unwrap_or(0.0),
        },
        "radial" => Layout::Radial(parse_radial(p, original.transform.translation())?),
        "grid" => Layout::Grid(GridDistribution::parse(p, original.transform.translation())?),
        other => return Err(EngineError::Param(format!("unknown array mode '{other}' (linear, radial, grid)"))),
    };
    let count = match layout {
        Layout::Grid(grid) => grid.rows * grid.columns,
        _ => count.unwrap_or(10),
    };
    // Grid holds the symbol registration point on the lattice, even when a
    // custom transformation point was authored for other editing operations.
    let grid = matches!(layout, Layout::Grid(_));
    let pivot = if grid { None } else { original.pivot() };
    let eased_distribution = !grid && easing.eases_distribution(stops.as_deref().unwrap_or_default());

    let mut copies: Vec<Arc<Element>> = Vec::with_capacity(count as usize);
    let mut ids: Vec<ElementId> = Vec::with_capacity(count as usize);
    for i in 0..count {
        let t = layout.progress(i, count);
        let eased = eased_distribution.then(|| easing.eased_position(stops.as_deref().unwrap_or_default(), t));
        let base = layout.place(original.transform, i, count, eased);
        let stop = stops.as_deref().map(|stops| easing.properties(stops, t as f32));
        let transform = match (&stop, basis) {
            (Some(stop), Some(basis)) => {
                let adjusted = layout.place(stop.source_transform(original.transform, basis), i, count, eased);
                keep_pivot(base, adjusted, pivot)
            }
            (Some(stop), None) => stop.apply_transform(base, pivot),
            (None, _) => base,
        };
        let transform = match layout {
            Layout::Grid(grid) => grid.orient(transform),
            _ => transform,
        };
        if !transform.is_finite() {
            return Err(EngineError::Param("array positions exceeded finite coordinate range".into()));
        }
        let mut el = posed_element(&original, transform, stop, seed.as_ref(), compose_colour);
        // Instance #1 is the original and keeps its ID.
        if i > 0 { el.id = ElementId(s.doc.alloc_id()); }
        ids.push(el.id);
        copies.push(Arc::new(el));
    }

    if create_unique {
        let names = unique_symbol_names(&s.doc, *source_symbol, count)?;
        // Duplicate places each definition immediately after its source. Walk
        // backwards so the Library and the logical Array display the same sequence.
        for (copy, name) in copies.iter_mut().zip(names).rev() {
            let ElementKind::Symbol { symbol, .. } = &mut Arc::make_mut(copy).kind else {
                return Err(EngineError::State("array copy is not a symbol instance".into()));
            };
            *symbol = s.doc.duplicate_symbol(*source_symbol, name)
                .ok_or_else(|| EngineError::State("array source symbol no longer exists".into()))?;
        }
    }
    // Preserve the source's position relative to unrelated artwork. Copies stack beside it.
    // Only the relative order among the array copies is controlled by "stacking".
    if front_to_back { copies.reverse(); }
    let kf = s.editable_keyframe()?;
    let at = kf.elements.iter().position(|e| e.id == id)
        .ok_or_else(|| EngineError::State("original disappeared from the keyframe".into()))?;
    kf.elements.remove(at);
    kf.elements.splice(at..at, copies);
    s.select_only(ids.iter().copied());
    Ok(json!({"count": count, "mode": mode, "instances": ids.iter().map(|id| id.0).collect::<Vec<_>>()}))
}

// Preview and both outputs use the same source-property and filter-slot rules.
fn posed_element(
    original: &Element,
    transform: Affine,
    stop: Option<ArrayPropertyStop>,
    seed: Option<&ArrayPropertyStop>,
    compose_colour: bool,
) -> Element {
    let mut el = original.clone();
    el.transform = transform;
    if let Some(stop) = stop {
        el.color = if let Some(seed) = seed {
            if (stop.color, stop.alpha) == (seed.color, seed.alpha) { original.color } else { stop.color_effect() }
        } else if compose_colour {
            stop.apply_color(original.color)
        } else {
            original.color.with_alpha(stop.alpha)
        };
        if !stop.filters.is_empty() && let Some(stack) = el.kind.filters_mut() {
            let supported = stack.iter_mut().filter(|entry| ArrayPropertyStop::supports_filter(&entry.filter));
            for (entry, filter) in supported.zip(stop.filters) {
                if !entry.enabled && matches!(entry.filter, Filter::AdjustColor { .. }) { continue; }
                // The Blur stop still controls sizes alone. Preserve its source quality,
                // including values from older imported documents.
                match (&mut entry.filter, filter) {
                    (Filter::Blur { blur_x, blur_y, .. }, Filter::Blur { blur_x: x, blur_y: y, .. }) => {
                        (*blur_x, *blur_y) = (x, y);
                    }
                    (source, filter) => *source = filter,
                }
            }
        }
    }
    el
}

#[cfg(test)]
mod tests {
    use super::*;
    use anicraft_doc::{ColorTransform, Rgba};

    fn st(p: f32, alpha: f32) -> ArrayPropertyStop { ArrayPropertyStop::new(p, alpha) }

    #[test]
    fn adjust_color_eases_each_signed_parameter_without_wrapping_hue() {
        let filter = |[brightness, contrast, saturation, hue]: [f32; 4]| Filter::AdjustColor { brightness, contrast, saturation, hue };
        let start = [-100.0, -60.0, 40.0, -180.0];
        let end = [100.0, 100.0, -100.0, 180.0];
        let make = |offset, values, outbound| ArrayPropertyStop {
            filters: vec![filter(values)], outbound: Some(outbound), ..st(offset, 1.0)
        };
        let mut stops = [make(0.0, start, Easing::QuadIn), make(0.5, end, Easing::QuadOut), make(1.0, start, Easing::Linear)];
        let global = ArrayEasing { preset: Easing::QuadIn, ..Default::default() };
        let segment = ArrayEasing { mode: ArrayEaseMode::PerSegment, ..global.clone() };
        let linear = ArrayEasing { filters: false, ..segment.clone() };
        for (easing, values) in [
            (&global, [-75.0, -40.0, 22.5, -135.0]),
            (&segment, [-50.0, -20.0, 5.0, -90.0]),
            (&linear, [0.0, 20.0, -30.0, 0.0]),
        ] {
            assert_eq!(easing.properties(&stops, 0.25).filters, [filter(values)]);
        }
        assert_eq!(segment.properties(&stops, 0.5).filters, [filter(end)]);
        assert_eq!(segment.properties(&stops, 0.75).filters, [filter([-50.0, -20.0, 5.0, -90.0])]);
        assert_eq!(linear.properties(&stops, 0.375).filters[0].param(FilterParam::Hue), Some(90.0));
        stops[0].outbound = Some(Easing::BackOut);
        assert_eq!(segment.properties(&stops, 0.25).filters, [filter(end)]);
        let overshoot = ArrayEasing { preset: Easing::BackOut, ..global };
        let sample = overshoot.properties(&stops, 0.5);
        let [Filter::AdjustColor { brightness: -100.0, contrast, saturation, hue: -180.0 }] = sample.filters.as_slice() else { panic!() };
        assert!((-100.0..-60.0).contains(contrast));
        assert!((40.0..=100.0).contains(saturation) && *saturation > 40.0);
    }

    #[test]
    fn adjust_color_arrays_keep_the_stack_and_compose_with_instance_colour() {
        let mut s = Session::default();
        s.execute("draw.rect", &json!({"x": 40, "y": 40, "w": 32, "h": 32, "fill": "#4478b0", "stroke": null})).unwrap();
        s.execute("modify.convert_to_symbol", &json!({"name": "Filtered", "kind": "movie_clip"})).unwrap();
        s.execute("element.set_color_effect", &json!({"green_multiplier": 50, "alpha_multiplier": 50})).unwrap();
        let source_colour = s.selected_elements()[0].1.color;
        let mut filters = vec![json!({"type": "blur", "quality": 3}), json!({"type": "drop_shadow", "quality": 3}),
            json!({"type": "glow", "quality": 3}), json!({"type": "bevel", "quality": 3}),
            json!({"type": "gradient_glow", "quality": 3}), json!({"type": "gradient_bevel", "quality": 3})];
        let adjust = |brightness, contrast, saturation, hue, enabled| json!({"type": "adjust_color", "brightness": brightness,
            "contrast": contrast, "saturation": saturation, "hue": hue, "enabled": enabled});
        filters.insert(1, adjust(-12, 4, 16, -160, true));
        filters.extend([adjust(8, 12, -28, 40, true), adjust(-7, 10, 15, 30, false)]);
        s.execute("element.set_filters", &json!({"filters": filters})).unwrap();
        let source_doc = s.doc.clone();
        let source_selection = s.selection.clone();
        let source_stack = s.selected_elements()[0].1.kind.filters().unwrap().to_vec();
        let mut edited = filters.clone();
        edited[1] = adjust(32, 24, -32, 120, false);
        edited[7] = adjust(-16, -24, 48, -100, true);
        edited[8] = adjust(100, -100, 100, -180, true);
        let colour = ColorEffect::Brightness(0.4);
        let params = json!({"count": 3, "start_x": 56, "start_y": 56, "dx": 120,
            "property_stops": [{"offset": 0}, {"offset": 1, "filters": edited, "color_effect": colour}]});
        let render = |s: &Session| anicraft_render::render_frame(&s.doc, 0, 1.0).unwrap().to_rgba8();
        s.preview_array(&params).unwrap();
        let copies = s.selected_elements();
        assert_eq!(copies[0].1.kind.filters().unwrap(), source_stack);
        for copy in &copies {
            let stack = copy.1.kind.filters().unwrap();
            assert!(anicraft_doc::filters_pair_up(stack, &source_stack));
            assert!(stack.iter().zip(&source_stack).all(|(entry, source)| entry.enabled == source.enabled));
            assert_eq!(stack[8], source_stack[8]);
        }
        let middle = copies[1].1.kind.filters().unwrap();
        assert_eq!(middle[1].filter, Filter::AdjustColor { brightness: 10.0, contrast: 14.0, saturation: -8.0, hue: -20.0 });
        assert_eq!(middle[7].filter, Filter::AdjustColor { brightness: -4.0, contrast: -6.0, saturation: 10.0, hue: -30.0 });
        assert_eq!(copies[2].1.color, colour.nested(&source_colour));
        let preview = render(&s);
        s.finish_array_preview(false);
        assert_eq!(s.doc, source_doc);
        assert_eq!(s.selection, source_selection);
        let mut colour_only = params.clone();
        colour_only["property_stops"][1]["filters"].as_array_mut().unwrap().retain(|entry| entry["type"] != "adjust_color");
        s.preview_array(&colour_only).unwrap();
        assert_eq!(s.selected_elements()[2].1.kind.filters().unwrap(), source_stack, "legacy lists keep source Adjust Color values");
        assert_ne!(render(&s), preview);
        s.finish_array_preview(false);
        let mut filters_only = params.clone();
        filters_only["property_stops"][1].as_object_mut().unwrap().remove("color_effect");
        s.preview_array(&filters_only).unwrap();
        assert_ne!(render(&s), preview);
        s.finish_array_preview(false);
        s.preview_array(&params).unwrap();
        s.finish_array_preview(true);
        assert_eq!(render(&s), preview);
        s.undo().unwrap();
        assert_eq!(s.doc, source_doc);
        s.redo().unwrap();
        assert_eq!(render(&s), preview);
    }

    #[test]
    fn independent_stops_can_fade_out_then_back_in() {
        let stops = [st(0.0, 1.0), st(0.5, 0.0), st(1.0, 1.0)];
        assert!((ArrayPropertyStop::sample(&stops, 0.25).alpha - 0.5).abs() < 1e-6);
        assert!((ArrayPropertyStop::sample(&stops, 0.75).alpha - 0.5).abs() < 1e-6);
        assert_eq!(ArrayPropertyStop::sample(&stops, 0.5).alpha, 0.0);
    }

    #[test]
    fn blur_stops_sample_both_axes_with_independent_easing_and_bounds() {
        let blur = |blur_x, blur_y| Filter::Blur { blur_x, blur_y, quality: 2 };
        let make = |offset, x, y| ArrayPropertyStop {
            filters: vec![blur(x, y), blur(y, x)],
            ..st(offset, offset)
        };
        let stops = [
            ArrayPropertyStop { outbound: Some(Easing::QuadIn), ..make(0.0, 0.0, 32.0) },
            make(0.5, 16.0, 16.0),
            make(1.0, 32.0, 0.0),
        ];
        let global = ArrayEasing { preset: Easing::QuadIn, transform: false, colour_effect: false, ..ArrayEasing::default() };
        let segment = ArrayEasing { mode: ArrayEaseMode::PerSegment, ..global.clone() };
        for (easing, x) in [(&global, 2.0), (&segment, 4.0)] {
            let sample = easing.properties(&stops, 0.25);
            assert_eq!(sample.filters, [blur(x, 32.0 - x), blur(32.0 - x, x)]);
            assert_eq!(sample.alpha, 0.25);
            let disabled = ArrayEasing { filters: false, ..easing.clone() };
            assert_eq!(disabled.properties(&stops, 0.25).filters, ArrayPropertyStop::sample(&stops, 0.25).filters);
        }
        assert_eq!(segment.properties(&stops, 0.5).filters, stops[1].filters);
        let mut extremes = [make(0.0, 0.0, MAX_BLUR), make(1.0, MAX_BLUR, 0.0)];
        extremes[0].outbound = Some(Easing::BackOut);
        for mode in [ArrayEaseMode::Global, ArrayEaseMode::PerSegment] {
            let easing = ArrayEasing { mode, preset: Easing::BackOut, ..global.clone() };
            assert_eq!(easing.properties(&extremes, 0.625).filters, extremes[1].filters);
        }
    }

    #[test]
    fn shadow_glow_ease_numbers_and_colours_while_switches_hold_at_stops() {
        let make = |offset, end: bool| {
            let (blur_x, blur_y, strength, quality, angle, distance, color) = if end {
                (30.0, 18.0, 6.0, 3, 10.0, 36.0, Rgba::new(0.0, 0.0, 1.0, 0.75))
            } else {
                (6.0, 2.0, 2.0, 1, 350.0, 4.0, Rgba::new(1.0, 0.0, 0.0, 0.25))
            };
            ArrayPropertyStop {
                filters: vec![
                    Filter::DropShadow { blur_x, blur_y, strength, quality, angle, distance, color, inner: end, knockout: end, hide_object: end },
                    Filter::Glow { blur_x, blur_y, strength, quality, color, inner: end, knockout: end },
                ],
                outbound: Some(Easing::QuadIn),
                ..st(offset, 1.0)
            }
        };
        let mut stops = [make(0.0, false), make(0.5, true), make(1.0, false)];
        let global = ArrayEasing { preset: Easing::QuadIn, ..Default::default() };
        let segment = ArrayEasing { mode: ArrayEaseMode::PerSegment, ..global.clone() };
        let linear = ArrayEasing { filters: false, ..global.clone() };
        for (easing, f) in [(&global, 0.125), (&segment, 0.25), (&linear, 0.5)] {
            let sampled = easing.properties(&stops, 0.25);
            let blur_x = 6.0 + 24.0 * f;
            let blur_y = 2.0 + 16.0 * f;
            let strength = 2.0 + 4.0 * f;
            let color = Rgba::new(1.0 - f, 0.0, f, 0.25 + 0.5 * f);
            assert_eq!(sampled.filters, [
                Filter::DropShadow { blur_x, blur_y, strength, quality: 1, angle: (350.0 + 20.0 * f) % 360.0, distance: 4.0 + 32.0 * f, color, inner: false, knockout: false, hide_object: false },
                Filter::Glow { blur_x, blur_y, strength, quality: 1, color, inner: false, knockout: false },
            ]);
        }
        let ahead = ArrayEasing { preset: Easing::QuadOut, ..global.clone() };
        for (easing, at, held) in [(&ahead, 0.375, 0), (&global, 0.5, 1), (&global, 0.625, 1), (&segment, 0.5, 1), (&segment, 1.0, 2)] {
            let sampled = easing.properties(&stops, at);
            for (f, expected) in sampled.filters.iter().zip(&stops[held].filters) {
                for p in FilterParam::STEPPED { assert_eq!(f.param(p), expected.param(p)); }
            }
        }
        let reverse = ArrayPropertyStop::sample(&stops[1..], 0.625);
        assert_eq!(reverse.filters[0].param(FilterParam::Angle), Some(5.0));

        stops[0].outbound = Some(Easing::BackOut);
        let overshot = segment.properties(&stops, 0.25);
        assert!(overshot.filters[0].param(FilterParam::BlurX) > Some(30.0));
        let c = overshot.filters[0].color(anicraft_doc::FilterColor::Color).unwrap();
        assert_eq!([c.r, c.g, c.b], [0.0, 0.0, 1.0]);
        assert!(c.a > 0.75 && c.a <= 1.0);
        for (f, expected) in overshot.filters.iter().zip(&stops[0].filters) {
            for p in FilterParam::STEPPED { assert_eq!(f.param(p), expected.param(p)); }
        }
    }

    #[test]
    fn gradient_glow_eases_rgba_and_positions_and_holds_discrete_settings() {
        use anicraft_doc::{FilterSide, GradientStop};
        let make = |offset, end: bool| {
            let stops = if end {
                [(0.0, [1.0, 0.0, 0.0, 0.0]), (0.5, [0.25, 0.75, 1.0, 0.625]), (0.5, [1.0, 0.25, 0.75, 0.75])]
            } else {
                [(0.0, [0.0, 0.0, 1.0, 0.0]), (0.125, [0.75, 0.25, 0.0, 0.125]), (0.875, [0.0, 0.75, 0.25, 0.25])]
            };
            ArrayPropertyStop {
                outbound: Some(Easing::QuadIn),
                filters: vec![Filter::GradientGlow {
                    stops: stops.into_iter().map(|(offset, [r, g, b, a])| GradientStop { offset, color: Rgba::new(r, g, b, a) }).collect(),
                    blur_x: if end { 20.0 } else { 4.0 }, blur_y: if end { 30.0 } else { 6.0 },
                    angle: if end { 10.0 } else { 350.0 }, distance: if end { 24.0 } else { -8.0 },
                    strength: if end { 6.0 } else { 2.0 }, quality: if end { 3 } else { 1 },
                    side: if end { FilterSide::Full } else { FilterSide::Outer }, knockout: end,
                }],
                ..st(offset, 1.0)
            }
        };
        let mut stops = [make(0.0, false), make(0.5, true), make(1.0, false)];
        let global = ArrayEasing { preset: Easing::QuadIn, ..Default::default() };
        let segment = ArrayEasing { mode: ArrayEaseMode::PerSegment, ..global.clone() };
        let linear = ArrayEasing { filters: false, ..global.clone() };
        for (ease, f) in [(&global, 0.125), (&segment, 0.25), (&linear, 0.5)] {
            let sampled = ease.properties(&stops, 0.25);
            let [Filter::GradientGlow { stops: ramp, blur_x, blur_y, angle, distance, strength, quality, side, knockout }] = sampled.filters.as_slice() else { panic!() };
            assert_eq!((*blur_x, *blur_y, *angle, *distance, *strength), (4.0 + 16.0 * f, 6.0 + 24.0 * f, (350.0 + 20.0 * f) % 360.0, -8.0 + 32.0 * f, 2.0 + 4.0 * f));
            assert_eq!((*quality, *side, *knockout), (1, FilterSide::Outer, false));
            assert_eq!(ramp.iter().map(|stop| stop.offset).collect::<Vec<_>>(), [0.0, 0.125 + 0.375 * f, 0.875 - 0.375 * f]);
            assert_eq!(ramp[0].color, Rgba::new(f, 0.0, 1.0 - f, 0.0));
            assert_eq!(ramp[1].color, Rgba::new(0.75 - 0.5 * f, 0.25 + 0.5 * f, f, 0.125 + 0.5 * f));
            assert_eq!(ramp[2].color, Rgba::new(f, 0.75 - 0.5 * f, 0.25 + 0.5 * f, 0.25 + 0.5 * f));
        }
        assert_eq!(segment.properties(&stops, 0.5).filters, stops[1].filters);
        assert!(matches!(global.properties(&stops, 0.5).filters[0], Filter::GradientGlow { quality: 3, side: FilterSide::Full, knockout: true, .. }));
        assert_eq!(linear.properties(&stops, 0.75).filters[0].param(FilterParam::Angle), Some(0.0));

        stops[0].outbound = Some(Easing::BackOut);
        let sampled = segment.properties(&stops, 0.25);
        let [Filter::GradientGlow { stops: ramp, quality: 1, side: FilterSide::Outer, knockout: false, .. }] = sampled.filters.as_slice() else { panic!() };
        assert_eq!(ramp.len(), 3);
        assert_eq!((ramp[0].offset, ramp[0].color.a), (0.0, 0.0));
        assert!(ramp[1].offset > 0.5 && ramp[1].offset == ramp[2].offset);
        assert!(ramp[1].color.r < ramp[2].color.r && ramp[1].color.a < ramp[2].color.a);
    }

    #[test]
    fn gradient_glow_arrays_preserve_ramp_topology_and_bake_the_rendered_preview() {
        let mut s = Session::default();
        s.execute("draw.rect", &json!({"x": 48, "y": 48, "w": 32, "h": 32, "fill": "#778899", "stroke": null})).unwrap();
        s.execute("modify.convert_to_symbol", &json!({"name": "Box", "kind": "movie_clip"})).unwrap();
        let source_ramp = json!([
            {"offset": 0, "color": "#ffffff00"}, {"offset": 0.25, "color": "#12cc2180"}, {"offset": 1, "color": "#122cddff"},
        ]);
        let edited_ramp = json!([
            {"offset": 0, "color": "#ffc810ff"}, {"offset": 0.75, "color": "#dd2c1270"}, {"offset": 1, "color": "#40de34a0"},
        ]);
        let blur = json!({"type": "blur", "blur_x": 3, "blur_y": 5, "quality": 3});
        let bevel = json!({"type": "bevel", "quality": 3});
        s.execute("element.set_filters", &json!({"filters": [
            blur, {"type": "gradient_glow", "quality": 3, "blur_x": 16, "blur_y": 11, "stops": source_ramp, "distance": 18},
            {"type": "gradient_bevel", "distance": 4, "enabled": false}, bevel,
            {"type": "gradient_glow", "enabled": false},
        ]})).unwrap();
        let base = s.doc.clone();
        let source = s.selected_elements()[0].1.kind.filters().unwrap().to_vec();
        let stops = |f: &FilterEntry| match &f.filter {
            Filter::GradientGlow { stops, .. } => stops.clone(),
            _ => panic!("expected Gradient Glow"),
        };
        assert_eq!(stops(&source[4]), anicraft_doc::default_glow_ramp());
        let p = json!({"count": 3, "start_x": 64, "start_y": 70, "dx": 100,
            "property_stops": [{"offset": 0}, {"offset": 1, "filters": [blur,
                {"type": "gradient_glow", "quality": 3, "blur_x": 16, "blur_y": 11, "stops": edited_ramp, "distance": 18, "enabled": false},
                bevel, {"type": "gradient_glow", "strength": 3, "enabled": true}]}]});
        for replacement in [json!([]), json!([source_ramp[0], source_ramp[2], source_ramp[1]]),
            json!([{"offset": 0.2, "color": "#ffffff00"}, source_ramp[1], source_ramp[2]]),
            json!([source_ramp[0], {"offset": 0, "color": "#aaee2288"}, source_ramp[2]]),
            json!([source_ramp[0], source_ramp[1], source_ramp[2], {"offset": 1}]),
        ] {
            let mut invalid = p.clone();
            invalid["property_stops"][1]["filters"][1]["stops"] = replacement;
            assert!(s.preview_array(&invalid).is_err());
            assert_eq!(s.doc, base);
        }
        let mut inherited = p.clone();
        inherited["property_stops"][1]["filters"][1].as_object_mut().unwrap().remove("stops");
        s.preview_array(&inherited).unwrap();
        assert_eq!(stops(&s.selected_elements()[2].1.kind.filters().unwrap()[1]), stops(&source[1]));
        let render = |s: &Session| anicraft_render::render_frame(&s.doc, 0, 1.0).unwrap().data;
        let baseline = render(&s);
        s.finish_array_preview(false);
        assert_eq!(s.doc, base);
        s.preview_array(&p).unwrap();
        let copies = s.selected_elements();
        assert_eq!(copies[0].1.kind.filters().unwrap(), source);
        for copy in &copies {
            let stack = copy.1.kind.filters().unwrap();
            assert!(anicraft_doc::filters_pair_up(stack, &source));
            for index in [0, 2, 3] { assert_eq!(stack[index], source[index]); }
            assert!(stack.iter().zip(&source).all(|(entry, original)| entry.enabled == original.enabled));
            assert_eq!(stops(&stack[4]), stops(&source[4]));
        }
        let middle = stops(&copies[1].1.kind.filters().unwrap()[1]);
        assert_eq!(middle.len(), 3);
        assert_eq!((middle[0].offset, middle[0].color.a), (0.0, 0.0));
        assert_eq!(middle[1].offset, 0.5);
        assert_eq!(copies[1].1.kind.filters().unwrap()[4].filter.param(FilterParam::Strength), Some(200.0));
        let preview = render(&s);
        assert_ne!(preview, baseline);
        s.finish_array_preview(true);
        assert_eq!(render(&s), preview);
        s.undo().unwrap();
        assert_eq!(s.doc, base);
        s.redo().unwrap();
        assert_eq!(render(&s), preview);
    }

    #[test]
    fn gradient_bevel_eases_both_halves_but_keeps_the_midpoint_and_steps() {
        use anicraft_doc::FilterSide;
        let make = |offset, end| {
            let (left, right) = if end { (0.484375, 0.515625) } else { (0.015625, 0.984375) };
            let (l, pin, r) = if end {
                (Rgba::new(0.25, 0.75, 1.0, 0.625), Rgba::hex(0xff0000), Rgba::new(1.0, 0.25, 0.75, 0.75))
            } else {
                (Rgba::new(0.75, 0.25, 0.0, 0.125), Rgba::hex(0x0000ff), Rgba::new(0.0, 0.75, 0.25, 0.25))
            };
            ArrayPropertyStop {
                outbound: Some(if end { Easing::QuadOut } else { Easing::QuadIn }),
                filters: vec![Filter::GradientBevel {
                    stops: [(0.0, Rgba::WHITE), (left, l), (0.5, pin.with_alpha(0.0)), (right, r), (1.0, Rgba::BLACK)]
                        .into_iter().map(|(offset, color)| GradientStop { offset, color }).collect(),
                    blur_x: if end { 20.0 } else { 4.0 }, blur_y: if end { 30.0 } else { 6.0 },
                    angle: if end { 10.0 } else { 350.0 }, distance: if end { 24.0 } else { -8.0 },
                    strength: if end { 6.0 } else { 2.0 }, quality: if end { 3 } else { 1 },
                    side: if end { FilterSide::Full } else { FilterSide::Inner }, knockout: end,
                }],
                ..st(offset, 1.0)
            }
        };
        let mut stops = [make(0.0, false), make(0.5, true), make(1.0, false)];
        let global = ArrayEasing { preset: Easing::QuadIn, ..Default::default() };
        let segment = ArrayEasing { mode: ArrayEaseMode::PerSegment, ..global.clone() };
        let linear = ArrayEasing { filters: false, ..global.clone() };
        for (ease, f) in [(&global, 0.125), (&segment, 0.25), (&linear, 0.5)] {
            let sample = ease.properties(&stops, 0.25);
            let [Filter::GradientBevel { stops: ramp, blur_x, blur_y, angle, distance, strength, quality, side, knockout }] = sample.filters.as_slice() else { panic!() };
            assert_eq!((*blur_x, *blur_y, *angle, *distance, *strength), (4.0 + 16.0 * f, 6.0 + 24.0 * f, (350.0 + 20.0 * f) % 360.0, -8.0 + 32.0 * f, 2.0 + 4.0 * f));
            assert_eq!((*quality, *side, *knockout), (1, FilterSide::Inner, false));
            assert_eq!(ramp.iter().map(|stop| stop.offset).collect::<Vec<_>>(), [0.0, 0.015625 + 0.46875 * f, 0.5, 0.984375 - 0.46875 * f, 1.0]);
            assert_eq!(ramp[1].color, Rgba::new(0.75 - 0.5 * f, 0.25 + 0.5 * f, f, 0.125 + 0.5 * f));
            assert_eq!(ramp[2].color, Rgba::new(f, 0.0, 1.0 - f, 0.0));
            assert_eq!(ramp[3].color, Rgba::new(f, 0.75 - 0.5 * f, 0.25 + 0.5 * f, 0.25 + 0.5 * f));
        }
        assert_eq!(segment.properties(&stops, 0.5).filters, stops[1].filters);
        assert!(matches!(global.properties(&stops, 0.5).filters[0], Filter::GradientBevel { quality: 3, side: FilterSide::Full, knockout: true, .. }));
        assert_eq!(segment.properties(&stops, 0.75).filters[0].param(FilterParam::Angle), Some(355.0));
        stops[0].outbound = Some(Easing::BackOut);
        let sample = segment.properties(&stops, 0.25);
        let [Filter::GradientBevel { stops: ramp, quality: 1, side: FilterSide::Inner, knockout: false, .. }] = sample.filters.as_slice() else { panic!() };
        assert_eq!(ramp.iter().map(|stop| stop.offset).collect::<Vec<_>>(), [0.0, 0.5 - REST_SNAP, 0.5, 0.5 + REST_SNAP, 1.0]);
        assert_eq!(ramp[2].color.a, 0.0);
        assert!(ramp[1].color.b > ramp[3].color.b && ramp[1].color.a < ramp[3].color.a);
    }

    #[test]
    fn gradient_bevel_array_preserves_duplicate_ramps_and_bakes_the_rendered_preview() {
        let mut s = Session::default();
        s.execute("draw.rect", &json!({"x": 40, "y": 40, "w": 32, "h": 32, "fill": "#778899", "stroke": null})).unwrap();
        s.execute("modify.convert_to_symbol", &json!({"name": "Box", "kind": "movie_clip"})).unwrap();
        let source_ramp = json!([
            {"offset": 0.125, "color": "#fcdf9280"}, {"offset": 0.35, "color": "#ffda43a0"},
            {"offset": 0.5, "color": "#00000000"}, {"offset": 0.75, "color": "#5e451c80"},
        ]);
        let edited_ramp = json!([
            {"offset": 0.375, "color": "#31a2f7c0"}, {"offset": 0.4, "color": "#37d9d580"},
            {"offset": 0.5, "color": "#ffffff80"}, {"offset": 0.95, "color": "#821030ff"},
        ]);
        let glow = json!({"type": "gradient_glow", "quality": 3});
        let bevel = |ramp| json!({"type": "gradient_bevel", "quality": 3, "side": "full", "stops": ramp});
        s.execute("element.set_filters", &json!({"filters": [
            bevel(&source_ramp), {"type": "adjust_color", "brightness": 11, "enabled": false},
            glow, {"type": "gradient_bevel", "enabled": false},
        ]})).unwrap();
        let source_doc = s.doc.clone();
        let source = s.selected_elements()[0].1.kind.filters().unwrap().to_vec();
        let ramp = |entry: &FilterEntry| match &entry.filter {
            Filter::GradientBevel { stops, .. } => stops.clone(),
            _ => panic!("expected Gradient Bevel"),
        };
        assert_eq!(ramp(&source[3]), anicraft_doc::default_bevel_ramp());
        let params = json!({"count": 3, "start_x": 56, "start_y": 56, "dx": 100,
            "property_stops": [{"offset": 0}, {"offset": 1, "filters": [bevel(&edited_ramp), glow,
                {"type": "gradient_bevel", "strength": 3, "enabled": true}]}]});
        for (slot, offset) in [(0, -0.1), (1, 0.5), (1, 0.55), (1, 0.5 - REST_SNAP * 0.5), (2, 0.49), (3, 0.4), (3, 1.1)] {
            let mut invalid = params.clone();
            invalid["property_stops"][1]["filters"][0]["stops"][slot]["offset"] = json!(offset);
            assert!(s.preview_array(&invalid).is_err(), "accepted slot {slot} at {offset}");
            assert_eq!(s.doc, source_doc);
        }
        let mut invalid = params.clone();
        invalid["property_stops"][1]["filters"][0]["stops"].as_array_mut().unwrap().pop();
        assert!(s.preview_array(&invalid).is_err());
        let render = |s: &Session| anicraft_render::render_frame(&s.doc, 0, 1.0).unwrap().data;
        let initial = render(&s);
        s.preview_array(&params).unwrap();
        let copies = s.selected_elements();
        assert_eq!(copies[0].1.kind.filters().unwrap(), source);
        for copy in &copies {
            let stack = copy.1.kind.filters().unwrap();
            assert!(anicraft_doc::filters_pair_up(stack, &source));
            assert!(stack.iter().zip(&source).all(|(entry, original)| entry.enabled == original.enabled));
            assert_eq!(stack[1..3], source[1..3]);
            assert_eq!(ramp(&stack[3]), ramp(&source[3]));
        }
        let middle = copies[1].1.kind.filters().unwrap();
        let middle_ramp = ramp(&middle[0]);
        assert_eq!(middle_ramp.len(), 4);
        assert_eq!((middle_ramp[0].offset, middle_ramp[2].offset, middle_ramp[2].color.a), (0.25, 0.5, 0.0));
        assert_eq!(middle[3].filter.param(FilterParam::Strength), Some(200.0));
        let preview = render(&s);
        assert_ne!(preview, initial);
        s.finish_array_preview(false);
        assert_eq!(s.doc, source_doc);
        s.preview_array(&params).unwrap();
        s.finish_array_preview(true);
        assert_eq!(render(&s), preview);
        s.undo().unwrap();
        assert_eq!(s.doc, source_doc);
        s.redo().unwrap();
        assert_eq!(render(&s), preview);
    }

    #[test]
    fn bevel_eases_both_rgba_colours_and_all_numbers_but_holds_options() {
        use anicraft_doc::{FilterColor, FilterSide};
        let start = Filter::Bevel {
            blur_x: 4.0, blur_y: 6.0, strength: 2.0, angle: 350.0, distance: -8.0,
            shadow_color: Rgba::new(0.75, 0.25, 0.0, 0.125), highlight_color: Rgba::new(0.0, 0.25, 0.75, 1.0),
            quality: 1, side: FilterSide::Inner, knockout: false,
        };
        let middle = Filter::Bevel {
            blur_x: 20.0, blur_y: 30.0, strength: 6.0, angle: 10.0, distance: 24.0,
            shadow_color: Rgba::new(0.25, 0.75, 1.0, 0.625), highlight_color: Rgba::new(1.0, 0.75, 0.25, 0.25),
            quality: 3, side: FilterSide::Outer, knockout: true,
        };
        let mut end = start.clone();
        if let Filter::Bevel { side, .. } = &mut end { *side = FilterSide::Full; }
        let stop = |offset, filter| ArrayPropertyStop { filters: vec![filter], outbound: Some(Easing::QuadIn), ..st(offset, 1.0) };
        let mut stops = [stop(0.0, start.clone()), stop(0.5, middle), stop(1.0, end)];
        let global = ArrayEasing { preset: Easing::QuadIn, ..Default::default() };
        let segment = ArrayEasing { mode: ArrayEaseMode::PerSegment, ..global.clone() };
        let linear = ArrayEasing { filters: false, ..global.clone() };
        for (ease, f) in [(&global, 0.125), (&segment, 0.25), (&linear, 0.5)] {
            assert_eq!(ease.properties(&stops, 0.25).filters, [Filter::Bevel {
                blur_x: 4.0 + 16.0 * f, blur_y: 6.0 + 24.0 * f, strength: 2.0 + 4.0 * f,
                angle: (350.0 + 20.0 * f) % 360.0, distance: -8.0 + 32.0 * f,
                shadow_color: Rgba::new(0.75 - 0.5 * f, 0.25 + 0.5 * f, f, 0.125 + 0.5 * f),
                highlight_color: Rgba::new(f, 0.25 + 0.5 * f, 0.75 - 0.5 * f, 1.0 - 0.75 * f),
                quality: 1, side: FilterSide::Inner, knockout: false,
            }]);
        }
        let ahead = ArrayEasing { preset: Easing::QuadOut, ..global.clone() };
        for (ease, at, side, quality, knockout) in [
            (&ahead, 0.375, FilterSide::Inner, 1, false),
            (&global, 0.5, FilterSide::Outer, 3, true),
            (&segment, 0.75, FilterSide::Outer, 3, true),
            (&global, 1.0, FilterSide::Full, 1, false),
        ] {
            let sample = ease.properties(&stops, at);
            assert!(matches!(sample.filters.as_slice(), [Filter::Bevel { side: s, quality: q, knockout: k, .. }]
                if (*s, *q, *k) == (side, quality, knockout)));
        }
        let reverse = ArrayPropertyStop::sample(&stops[1..], 0.75);
        assert_eq!(reverse.filters[0].param(FilterParam::Angle), Some(0.0));
        stops[0].outbound = Some(Easing::BackOut);
        let overshot = segment.properties(&stops, 0.25);
        assert_eq!(overshot.filters[0].color(FilterColor::Highlight).unwrap().r, 1.0);
        assert!(overshot.filters[0].param(FilterParam::Distance) > Some(24.0));
        assert!(matches!(overshot.filters[0], Filter::Bevel { side: FilterSide::Inner, quality: 1, knockout: false, .. }));
    }

    #[test]
    fn bevel_arrays_keep_duplicate_entries_and_bake_the_rendered_preview() {
        use anicraft_doc::FilterSide;
        let mut s = Session::default();
        s.execute("draw.rect", &json!({"x": 48, "y": 48, "w": 32, "h": 32, "fill": "#778899", "stroke": null})).unwrap();
        s.execute("modify.convert_to_symbol", &json!({"name": "Box", "kind": "movie_clip"})).unwrap();
        let authored = json!([
            {"type": "blur", "blur_x": 3, "blur_y": 5, "quality": 3},
            {"type": "bevel", "angle": 350, "quality": 3, "shadow_color": "#341b35", "highlight_color": "#d0edff"},
            {"type": "adjust_color", "brightness": 12, "enabled": false},
            {"type": "drop_shadow", "quality": 3}, {"type": "glow", "quality": 3},
            {"type": "bevel", "distance": 21, "quality": 3, "enabled": false},
        ]);
        s.execute("element.set_filters", &json!({"filters": authored})).unwrap();
        let base = s.doc.clone();
        let (id, source) = s.selected_elements().first().map(|(_, element)| (element.id, element.kind.filters().unwrap().to_vec())).unwrap();
        let mut stop_filters = authored.as_array().unwrap().clone();
        stop_filters.remove(2);
        stop_filters[1] = json!({"type": "bevel", "angle": 10, "side": "full", "knockout": true, "distance": 13, "quality": 3, "shadow_color": "#e61513", "highlight_color": "#41fb57", "enabled": false});
        stop_filters[4]["distance"] = json!(31);
        stop_filters[4]["enabled"] = json!(true);
        let render = |s: &Session| anicraft_render::render_frame(&s.doc, 0, 1.0).unwrap().data;
        for mode in ["linear", "radial"] {
            let mut p = json!({"mode": mode, "count": 3, "start_x": 64, "start_y": 70, "dx": 100,
                "center_x": 180, "center_y": 170, "radius": 48, "closed_loop": false, "sweep_angle": -480,
                "spiral_offset": 12, "drift_x": 16, "drift_y": 8, "property_stops": [{"offset": 0}, {"offset": 1}]});
            s.preview_array(&p).unwrap();
            let uniform = render(&s);
            p["property_stops"][1]["filters"] = json!(stop_filters);
            s.preview_array(&p).unwrap();
            let copies = s.selected_elements();
            assert_eq!(copies[0].1.kind.filters().unwrap(), source);
            for (i, (_, copy)) in copies.iter().enumerate().skip(1) {
                let stack = copy.kind.filters().unwrap();
                assert!(anicraft_doc::filters_pair_up(stack, &source));
                assert!(stack.iter().zip(&source).all(|(entry, original)| entry.enabled == original.enabled));
                for unchanged in [0, 2, 3, 4] { assert_eq!(stack[unchanged], source[unchanged]); }
                assert_eq!(stack[5].filter.param(FilterParam::Distance), Some(21.0 + i as f32 * 5.0));
            }
            assert!(matches!(copies[2].1.kind.filters().unwrap()[1].filter, Filter::Bevel { side: FilterSide::Full, knockout: true, .. }));
            let preview = render(&s);
            assert_ne!(preview, uniform, "edited Bevel must change real pixels");
            assert_eq!(s.committed_document().0, &base);
            s.finish_array_preview(false);
            assert_eq!(s.doc, base);
            s.preview_array(&p).unwrap();
            s.finish_array_preview(true);
            assert_eq!(render(&s), preview);
            s.undo().unwrap();
            assert_eq!(s.doc, base);
            s.select_only([id]);
        }
    }

    #[test]
    fn shadow_glow_arrays_bake_mixed_stacks_in_order_and_keep_preview_pixels() {
        let mut s = Session::default();
        s.execute("draw.rect", &json!({"x": 56, "y": 56, "w": 16, "h": 16, "fill": "#36425b", "stroke": null})).unwrap();
        s.execute("modify.convert_to_symbol", &json!({"name": "Box", "kind": "movie_clip"})).unwrap();
        s.execute("element.set_filters", &json!({"filters": [
            {"type": "drop_shadow", "angle": 350, "distance": 32, "blur_x": 6, "blur_y": 5, "color": "#dc2020"},
            {"type": "adjust_color", "brightness": -9, "contrast": 22, "enabled": false},
            {"type": "glow", "blur_x": 18, "blur_y": 18, "color": "#00aa88"},
            {"type": "blur", "blur_x": 0, "blur_y": 0, "quality": 2},
            {"type": "drop_shadow", "angle": 90, "distance": 8, "enabled": false},
            {"type": "glow", "blur_x": 4, "blur_y": 6, "color": "#2244dd", "enabled": false},
        ]})).unwrap();
        let base = s.doc.clone();
        let source = s.selected_elements()[0].1.kind.filters().unwrap().to_vec();
        let mut p = json!({"count": 3, "start_x": 64, "start_y": 64, "dx": 120});
        let last = json!([
            {"type": "drop_shadow", "angle": 10, "distance": 52, "blur_x": 18, "blur_y": 12, "color": "#5a22ff", "quality": 3, "hide_object": true, "enabled": false},
            {"type": "glow", "blur_x": 34, "blur_y": 10, "color": "#0044ff", "knockout": true, "enabled": false},
            {"type": "blur", "blur_x": 14, "blur_y": 4, "quality": 3},
            {"type": "drop_shadow", "angle": 180, "distance": 14, "inner": true, "enabled": true},
            {"type": "glow", "blur_x": 8, "blur_y": 22, "color": "#ffff00", "inner": true},
        ]);
        p["property_stops"] = json!([{"offset": 0}, {"offset": 1, "filters": last}]);
        let mut invalid = p.clone();
        invalid["property_stops"][1]["filters"][1] = json!({"type": "bevel"});
        assert!(s.preview_array(&invalid).is_err());
        invalid = p.clone();
        invalid["property_stops"][1]["blurs"] = json!([]);
        assert!(s.preview_array(&invalid).is_err());
        assert_eq!(s.doc, base);
        s.preview_array(&p).unwrap();
        let copies = s.selected_elements();
        assert_eq!(copies[0].1.kind.filters().unwrap(), source);
        let middle = copies[1].1.kind.filters().unwrap();
        let last = copies[2].1.kind.filters().unwrap();
        for stack in [middle, last] {
            assert!(anicraft_doc::filters_pair_up(stack, &source));
            assert!(stack.iter().zip(&source).all(|(entry, authored)| entry.enabled == authored.enabled));
            assert_eq!(stack[1], source[1]);
            assert_eq!(stack[3].filter.param(FilterParam::Quality), Some(2.0));
        }
        assert_eq!(middle[0].filter.param(FilterParam::Angle), Some(0.0));
        assert_eq!(middle[0].filter.param(FilterParam::Distance), Some(42.0));
        assert_eq!(middle[4].filter.param(FilterParam::Distance), Some(11.0));
        assert_eq!(middle[2].filter.param(FilterParam::BlurX), Some(26.0));
        assert_eq!(middle[5].filter.param(FilterParam::BlurX), Some(6.0));
        assert_eq!(last[0].filter.param(FilterParam::HideObject), Some(1.0));
        let preview = anicraft_render::render_frame(&s.doc, 0, 1.0).unwrap();
        let shadow = preview.pixel(96, 58).unwrap();
        assert!(shadow[0] > shadow[1], "the red shadow draws beyond the source bounds");
        let glow = preview.pixel(64, 53).unwrap();
        assert!(glow[1] > glow[0], "the green glow draws around the source");
        assert_eq!(s.committed_document().0, &base);
        s.finish_array_preview(true);
        let baked = anicraft_render::render_frame(&s.doc, 0, 1.0).unwrap();
        assert_eq!(baked.data, preview.data);
        s.undo().unwrap();
        assert_eq!(s.doc, base);
    }

    #[test]
    fn blur_array_preview_and_commit_keep_the_source_stack_and_render_anisotropically() {
        let mut s = Session::default();
        s.execute("draw.rect", &json!({"x": 60, "y": 60, "w": 16, "h": 16, "fill": "#ff0000", "stroke": null})).unwrap();
        s.execute("modify.convert_to_symbol", &json!({"name": "Box"})).unwrap();
        s.execute("symbol.set_kind", &json!({"kind": "movie_clip"})).unwrap();
        let base_params = json!({"count": 3, "start_x": 68, "start_y": 68, "dx": 64});
        let mut without_blur = base_params.clone();
        without_blur["property_stops"] = json!([{"offset": 0, "blurs": [{"blur_x": 0, "blur_y": 0}]}, {"offset": 1}]);
        let bare = s.doc.clone();
        assert!(s.preview_array(&without_blur).is_err());
        assert_eq!(s.doc, bare);
        s.execute("element.set_filters", &json!({"filters": [
            {"type": "blur", "blur_x": 0, "blur_y": 0, "quality": 2},
            {"type": "adjust_color", "brightness": -15, "contrast": 30},
            {"type": "blur", "blur_x": 11, "blur_y": 19, "quality": 3, "enabled": false},
        ]})).unwrap();
        let source = s.selected_elements()[0].1.kind.filters().unwrap().to_vec();
        let base = s.doc.clone();
        let selection = s.selection.clone();
        let steps = s.history_len();
        let render = |s: &Session| anicraft_render::render_frame(&s.doc, 0, 1.0).unwrap();
        assert!(s.preview_array(&without_blur).is_err(), "requires both Blur entries");
        for mode in ["linear", "radial"] {
            let mut p = base_params.clone();
            p["mode"] = json!(mode);
            p["center_x"] = json!(140);
            p["center_y"] = json!(150);
            p["radius"] = json!(55);
            p["start_angle"] = json!(0);
            p["sweep_angle"] = json!(-450);
            p["closed_loop"] = json!(false);
            p["spiral_offset"] = json!(20);
            p["drift_x"] = json!(55);
            p["drift_y"] = json!(-10);
            // An omitted stop inherits the source values.
            p["property_stops"] = json!([{"offset": 0}, {"offset": 1, "blurs": [
                {"blur_x": 32, "blur_y": 0}, {"blur_x": 21, "blur_y": 35},
            ]}]);
            s.preview_array(&p).unwrap();
            assert_eq!(s.committed_document().0, &base);
            for ((_, element), [x1, x2, y2]) in s.selected_elements().iter().zip([
                [0.0, 11.0, 19.0], [16.0, 16.0, 27.0], [32.0, 21.0, 35.0],
            ]) {
                assert_eq!(element.kind.filters().unwrap(), [
                    Filter::Blur { blur_x: x1, blur_y: 0.0, quality: 2 }.into(),
                    source[1].clone(),
                    FilterEntry { filter: Filter::Blur { blur_x: x2, blur_y: y2, quality: 3 }, enabled: false },
                ]);
            }
            let (_, last) = s.selected_elements().pop().unwrap();
            let at = last.transform.translation();
            let preview = render(&s);
            let coords = [at.x.round() as u32, at.y.round() as u32];
            assert_ne!(preview.pixel(coords[0] + 11, coords[1]), Some([1.0; 4]), "horizontal blur extends the edge");
            assert_eq!(preview.pixel(coords[0], coords[1] + 11), Some([1.0; 4]), "vertical blur and the disabled filter do not draw");
            s.finish_array_preview(false);
            assert_eq!(s.doc, base);
            assert_eq!(s.selection, selection);
            assert_eq!(s.history_len(), steps);

            s.preview_array(&p).unwrap();
            s.finish_array_preview(true);
            assert_eq!(s.history_len(), steps + 1);
            assert_eq!(render(&s).data, preview.data);
            s.undo().unwrap();
            assert_eq!(s.doc, base);
            assert_eq!(s.selection, selection);
        }
    }

    #[test]
    fn source_seeded_stops_keep_the_source_appearance_and_edit_absolute_values() {
        let mut s = radial_source();
        let id = s.selection.ids().next().unwrap();
        let source = s.editable_keyframe().unwrap().elements.iter_mut().find(|e| e.id == id).unwrap();
        let source = Arc::make_mut(source);
        let pivot = Vec2::new(4.0, -3.0);
        assert!(source.set_pivot(Some(pivot)));
        source.transform = Decomposed { sx: 1.5, sy: -0.75, rotation: 0.75, skew: 0.25, ..Default::default() }.compose();
        source.color = ColorEffect::Advanced(ColorTransform { mul: [0.9, 0.7, 0.5, 0.75], add: [0.1, 0.0, 0.4, 0.125] });
        let source = source.clone();
        let seeded = ArrayPropertyStop::from_source(&source);
        assert_eq!(seeded.color_effect(), source.color);
        let base = s.doc.clone();
        let select = s.selection.clone();
        for (mode, rotate) in [("linear", false), ("radial", true)] {
            let mut params = json!({
                "mode": mode, "count": 3, "start_x": 50, "start_y": 80, "dx": 25, "dy": 0,
                "center_x": 90, "center_y": 95, "radius": 20, "start_angle": 18,
                "closed_loop": false, "sweep_angle": -555, "spiral_offset": 20, "drift_x": 35, "drift_y": 15, "rotate": rotate,
            });
            let mut unchanged = Session::new(base.clone());
            unchanged.selection = select.clone();
            unchanged.execute("array.create", &params).unwrap();
            let copies = unchanged.selected_elements();
            params["source_values"] = json!(true);
            params["property_stops"] = json!([{"offset": 0}, {"offset": 1}]);
            s.preview_array(&params).unwrap();
            assert_eq!(s.doc, unchanged.doc);
            if mode == "linear" { assert!(s.array_translation_generation().is_some()); }

            let colour = ColorEffect::Tint { color: Rgba::hex(0x5566cc), amount: 0.5 };
            params["property_stops"][1] = json!({
                "offset": 1, "scale_x": 3.0, "rotation": seeded.rotation + 108.0,
                "color_effect": colour, "alpha": 0.5,
            });
            s.preview_array(&params).unwrap();
            let (_, last) = s.selected_elements().pop().unwrap();
            let baseline = copies.last().unwrap().1.transform;
            assert!(last.transform.apply(pivot).distance(baseline.apply(pivot)) < 1e-4);
            let expected = Affine::rotate(108.0_f32.to_radians()).apply_vector(baseline.apply_vector(Vec2::new(1.0, 0.0))) * 2.0;
            assert!(last.transform.apply_vector(Vec2::new(1.0, 0.0)).distance(expected) < 1e-5);
            assert_eq!(last.color, colour.with_alpha(0.5));
            s.finish_array_preview(false);
            assert_eq!(s.doc, base);
        }
    }

    #[test]
    fn colour_stops_interpolate_signed_brightness_tint_and_mixed_styles() {
        let dark = ArrayPropertyStop { color: ColorEffect::Brightness(-0.5), ..st(0.0, 0.5) };
        let light = ArrayPropertyStop { color: ColorEffect::Brightness(0.5), ..st(1.0, 1.0) };
        let halfway = ArrayPropertyStop::sample([&dark, &light], 0.5);
        assert_eq!(halfway.color, ColorEffect::Brightness(0.0));
        assert_eq!(halfway.alpha, 0.75);

        let red = ArrayPropertyStop { color: ColorEffect::Tint { color: Rgba::hex(0xff0000), amount: 0.25 }, ..st(0.0, 0.25) };
        let blue = ArrayPropertyStop { color: ColorEffect::Tint { color: Rgba::hex(0x0000ff), amount: 0.75 }, ..st(1.0, 0.75) };
        assert_eq!(ArrayPropertyStop::sample([&red, &blue], 0.5).color,
            ColorEffect::Tint { color: Rgba::hex(0xff0000).lerp(Rgba::hex(0x0000ff), 0.5), amount: 0.5 });
        assert_eq!(ArrayPropertyStop::sample([&st(0.0, 1.0), &blue], 0.5).color,
            ColorEffect::Tint { color: Rgba::hex(0x0000ff), amount: 0.375 });
        let mixed = ArrayPropertyStop::sample([&red, &light], 0.5);
        assert_eq!(mixed.color, ColorEffect::Advanced(ColorTransform {
            mul: [0.625, 0.625, 0.625, 1.0], add: [0.375, 0.25, 0.25, 0.0],
        }));
        assert_eq!(mixed.apply_color(ColorEffect::None).alpha(), 0.625);
    }

    #[test]
    fn colour_easing_samples_all_advanced_channels_and_bounds_overshoot() {
        let end = ArrayPropertyStop {
            color: ColorEffect::Advanced(ColorTransform {
                mul: [-1.0, 0.5, 0.0, 1.0], add: [1.0, -1.0, 0.5, 0.25],
            }),
            ..st(0.5, -1.0)
        };
        let start = ArrayPropertyStop { outbound: Some(Easing::QuadIn), ..st(0.0, 1.0) };
        let stops = [start, end.clone(), ArrayPropertyStop { offset: 1.0, ..end.clone() }];
        let global = ArrayEasing { preset: Easing::QuadIn, ..ArrayEasing::default() };
        let segment = ArrayEasing { mode: ArrayEaseMode::PerSegment, ..global.clone() };
        for (easing, fraction) in [(&global, 0.125), (&segment, 0.25)] {
            let actual = easing.properties(&stops, 0.25).apply_color(ColorEffect::None).transform();
            assert_eq!(actual, ColorTransform {
                mul: [1.0 - 2.0 * fraction, 1.0 - 0.5 * fraction, 1.0 - fraction, 1.0 - 2.0 * fraction],
                add: [fraction, -fraction, 0.5 * fraction, 0.25 * fraction],
            });
            let disabled = ArrayEasing { colour_effect: false, ..easing.clone() };
            let actual = disabled.properties(&stops, 0.25).apply_color(ColorEffect::None).transform();
            assert_eq!(actual.mul, [0.0, 0.75, 0.5, 0.0]);
            assert_eq!(actual.add, [0.5, -0.5, 0.25, 0.125]);
        }

        let back = ArrayEasing { preset: Easing::BackOut, ..global };
        let mut stops = [stops[0].clone(), ArrayPropertyStop { offset: 1.0, ..end }];
        stops[0].outbound = Some(Easing::BackOut);
        for easing in [&back, &ArrayEasing { mode: ArrayEaseMode::PerSegment, ..back.clone() }] {
            let t = easing.properties(&stops, 0.625).apply_color(ColorEffect::None).transform();
            assert_eq!(t.mul[0], -1.0);
            assert_eq!(t.mul[3], -1.0);
            assert_eq!(&t.add[..2], &[1.0, -1.0]);
            assert!(t.add[3] > 0.25);
        }
        let tint = |offset, color, amount| ArrayPropertyStop {
            color: ColorEffect::Tint { color: Rgba::hex(color), amount }, ..st(offset, 1.0)
        };
        let stops = [tint(0.0, 0xff0000, 0.0), tint(1.0, 0x0040ff, 1.0)];
        let ColorEffect::Tint { color, amount } = back.properties(&stops, 0.625).color else { panic!("expected Tint"); };
        assert_eq!(amount, 1.0);
        assert_eq!(color.r, 0.0);
        assert_eq!(color.b, 1.0);
        assert!(color.g > Rgba::hex(0x0040ff).g);
    }

    #[test]
    fn colour_preview_and_commit_compose_with_source_and_render_in_both_layouts() {
        let mut original = Session::default();
        original.execute("draw.rect", &json!({"x": 15, "y": 15, "w": 10, "h": 10, "fill": "#336699", "stroke": null})).unwrap();
        original.execute("modify.convert_to_symbol", &json!({"name": "Source"})).unwrap();
        let source = ColorEffect::Advanced(ColorTransform { mul: [1.0, 0.5, 1.0, 0.5], add: [0.0; 4] });
        original.execute("element.set_color_effect", &json!({"green_multiplier": 50, "alpha_multiplier": 50})).unwrap();
        let base = original.doc.clone();
        let selection = original.selection.clone();
        let bake = |s: &Session| anicraft_render::render_frame(&s.doc, 0, 1.0).unwrap();
        let pixel = |s: &Session, at: [f32; 2]| {
            let pm = bake(s);
            let i = ((at[1] as u32 * pm.width + at[0] as u32) * 4) as usize;
            <[u8; 4]>::try_from(&pm.to_rgba8()[i..i + 4]).unwrap()
        };
        let last = ColorEffect::Advanced(ColorTransform { mul: [0.0, 0.0, 0.0, 0.25], add: [0.0, 0.0, 1.0, 0.375] });
        for (mode, positions) in [("linear", [[20.0, 20.0], [60.0, 20.0], [100.0, 20.0]]),
            ("radial", [[180.0, 100.0], [105.0, 105.0], [230.0, 110.0]])]
        {
            let params = json!({
                "mode": mode, "count": 3, "start_x": 20, "start_y": 20, "dx": 40,
                "center_x": 140, "center_y": 100, "radius": 40, "start_angle": 0,
                "sweep_angle": -360, "closed_loop": false, "spiral_offset": 20, "drift_x": 30, "drift_y": 10,
                "property_stops": [
                    {"offset": 0, "color_effect": ColorEffect::None},
                    {"offset": 0.5, "alpha": 0.5, "color_effect": ColorEffect::Brightness(0.5)},
                    {"offset": 1, "alpha": 0.5, "color_effect": last},
                ],
            });
            let mut s = Session::new(base.clone());
            s.selection = selection.clone();
            let mut draft = params.clone();
            draft["property_stops"][2].as_object_mut().unwrap().remove("alpha");
            s.preview_array(&draft).unwrap();
            assert_eq!(s.selected_elements()[2].1.color, last.nested(&source));
            s.finish_array_preview(false);
            assert_eq!(s.doc, base);
            assert_eq!(s.selection, selection);
            assert_eq!(s.history_len(), 0);

            s.preview_array(&params).unwrap();
            let elements = s.selected_elements();
            assert_eq!(elements[0].1.color, source);
            assert_eq!(elements[1].1.color, ColorEffect::Brightness(0.5).with_alpha(0.5).nested(&source));
            assert_eq!(elements[2].1.color, last.with_alpha(0.5).nested(&source));
            for ((_, el), [x, y]) in elements.iter().zip(positions) {
                assert_eq!(el.transform.translation(), Vec2::new(x, y));
            }
            // Alpha = 0.5 * 0.5 + 0.375. Blue over the default white stage is (96, 96, 255).
            let colour = pixel(&s, positions[2]);
            assert!(colour.iter().zip([96u8, 96, 255, 255]).all(|(a, b)| a.abs_diff(b) <= 1), "{colour:?}");
            let preview = bake(&s).to_rgba8();
            s.finish_array_preview(true);
            assert_eq!(s.history_len(), 1);
            assert_eq!(bake(&s).to_rgba8(), preview);

            let (_, recorded) = s.history.steps().last().unwrap();
            let [recorded] = recorded.unwrap() else { panic!("expected one recorded Array command") };
            assert_eq!(recorded.cmd, "array.create");
            let replay_params = serde_json::from_str(&recorded.params.to_string()).unwrap();
            let mut replay = Session::new(base.clone());
            replay.selection = selection.clone();
            replay.execute(&recorded.cmd, &replay_params).unwrap();
            assert_eq!(replay.doc, s.doc);

            s.undo().unwrap();
            assert_eq!(s.doc, base);
        }
        original.execute("array.create", &json!({"count": 1, "property_stops": [
            {"offset": 0, "alpha": 1}, {"offset": 1, "alpha": 1}
        ]})).unwrap();
        assert_eq!(original.selected_elements()[0].1.color, source.with_alpha(1.0));
    }

    #[test]
    fn array_easing_accepts_the_shared_named_catalogue_and_command_names() {
        for preset in Easing::named() {
            let name = crate::params::easing_json(&preset);
            let easing = ArrayEasing::parse(&json!({"easing": {"preset": name}})).unwrap();
            assert_eq!(easing.preset, preset);
            for t in [0.0, 0.25, 0.75, 1.0] {
                assert_eq!(easing.eased_position(&[], t), f64::from(preset.apply(t as f32)));
            }
        }
        let alias = ArrayEasing::parse(&json!({"easing": {"preset": "elastic-in-out"}})).unwrap();
        assert_eq!(alias.preset, Easing::ElasticInOut);
    }

    #[test]
    fn array_easing_modes_and_families_sample_the_authored_stops_independently() {
        let stops = [
            st(0.0, 0.2),
            ArrayPropertyStop { scale: [2.0, 0.5], rotation: 360.0, ..st(0.5, 0.6) },
            ArrayPropertyStop { scale: [3.0, 0.25], rotation: 720.0, ..st(1.0, 0.4) },
        ];
        let easing = ArrayEasing { preset: Easing::QuadIn, ..ArrayEasing::default() };
        let per_segment = ArrayEasing { mode: ArrayEaseMode::PerSegment, ..easing.clone() };
        for (easing, at, angle) in [(&easing, 0.0625, 45.0), (&per_segment, 0.125, 90.0)] {
            assert_eq!(easing.eased_position(&stops, 0.25), at);
            let eased = easing.properties(&stops, 0.25);
            assert_eq!(eased.rotation, angle);
            assert_eq!(eased.scale, [1.0 + at as f32 * 2.0, 1.0 - at as f32]);
            let linear = ArrayPropertyStop::sample(&stops, 0.25);
            let colour_only = ArrayEasing { transform: false, ..easing.clone() }.properties(&stops, 0.25);
            assert_eq!(colour_only.scale, linear.scale);
            assert_eq!(colour_only.rotation, linear.rotation);
            assert_eq!(colour_only.alpha, eased.alpha);
            let transform_only = ArrayEasing { colour_effect: false, ..easing.clone() }.properties(&stops, 0.25);
            assert_eq!(transform_only.scale, eased.scale);
            assert_eq!(transform_only.alpha, linear.alpha);
        }
        assert_eq!(per_segment.properties(&stops, 0.5), stops[1]);
        assert_eq!(per_segment.eased_position(&stops, 0.5), 0.5);
        assert_eq!(per_segment.properties(&stops, 1.0), stops[2]);
        assert_ne!(easing.properties(&stops, 0.5), stops[1]);
        let interior = [st(0.25, 0.2), st(0.75, 0.8)];
        assert_eq!(per_segment.eased_position(&interior, 0.125), 0.0625);
        assert_eq!(per_segment.eased_position(&interior, 0.875), 0.8125);
        assert_eq!(per_segment.properties(&interior, 0.125).alpha, 0.2);
        assert_eq!(per_segment.properties(&interior, 0.875).alpha, 0.8);
    }

    #[test]
    fn array_back_overshoots_using_the_current_segment_and_handles_coincident_stops() {
        let stops = [
            st(0.0, 0.0),
            ArrayPropertyStop { scale: [1.5, 0.5], rotation: 360.0, ..st(0.5, 1.0) },
            ArrayPropertyStop { scale: [2.0, 0.25], rotation: 720.0, ..st(1.0, 0.0) },
        ];
        let global = ArrayEasing { preset: Easing::BackOut, ..ArrayEasing::default() };
        let per_segment = ArrayEasing { mode: ArrayEaseMode::PerSegment, ..global.clone() };
        assert!(global.eased_position(&stops, 0.625) > 1.0);
        let overshoot = global.properties(&stops, 0.625);
        assert!(overshoot.rotation > 720.0 && overshoot.scale[0] > 2.0 && overshoot.scale[1] < 0.25);
        assert_eq!(overshoot.alpha, 0.0);
        let local = per_segment.properties(&stops, 0.3125);
        assert!(per_segment.eased_position(&stops, 0.3125) > 0.5);
        assert!(local.rotation > 360.0 && local.scale[0] > 1.5 && local.scale[1] < 0.5);
        assert_eq!(local.alpha, 1.0);

        let end = ArrayPropertyStop { rotation: -90.0, ..st(1.0, 0.25) };
        let near = ArrayPropertyStop { offset: 1.0 - f32::EPSILON / 2.0, ..end.clone() };
        let crowded = [stops[0].clone(), stops[1].clone(), near, stops[2].clone(), end.clone()];
        assert_eq!(ArrayPropertyStop::sample_global(&crowded, 1.0).rotation, 720.0);
        assert!(ArrayPropertyStop::sample_global(&crowded, 1.001).rotation > 720.0);
        let coincident = [end, stops[2].clone()];
        assert_eq!(per_segment.properties(&coincident, 0.95).rotation, -90.0);
        assert_eq!(global.properties(&coincident, 0.625).rotation, -90.0);

        let back_in = ArrayEasing { preset: Easing::BackIn, ..global };
        assert!(back_in.eased_position(&stops, 0.375) < 0.0);
        let before_start = back_in.properties(&stops, 0.375);
        assert!(before_start.rotation < 0.0 && before_start.scale[0] < 1.0);
        assert_eq!(before_start.alpha, 0.0);
        let local = ArrayEasing { mode: ArrayEaseMode::PerSegment, ..back_in };
        assert!(local.eased_position(&stops, 0.6875) < 0.5);
        assert!(local.properties(&stops, 0.6875).rotation < 360.0);
    }

    #[test]
    fn authored_outbound_presets_are_independent_and_follow_stop_order() {
        let params = json!({
            "easing": {"mode": "per_segment", "preset": "BackOut"},
            "property_stops": [
                {"offset": 1, "rotation": 720, "alpha": 1, "outbound_ease": "ElasticIn"},
                {"offset": 0.5, "rotation": 360, "alpha": 0.5, "outbound_ease": "QuadOut", "scale_y": 2},
                {"offset": 0, "rotation": 0, "alpha": 0, "outbound_ease": "QuadIn"},
            ],
        });
        let stops = parse_stops(&params, None, None).unwrap().unwrap();
        let easing = ArrayEasing::parse(&params).unwrap();
        assert!(easing.eases_distribution(&stops));
        for (input, output, rotation, scale_y) in [(0.25, 0.125, 90.0, 1.25), (0.75, 0.875, 630.0, 1.25)] {
            assert_eq!(easing.eased_position(&stops, input), output);
            let props = easing.properties(&stops, input as f32);
            assert_eq!((props.alpha, props.rotation, props.scale[1]), (output as f32, rotation, scale_y));
        }
        assert_eq!(easing.eased_position(&stops, 0.5), 0.5);
        let mut inset = stops;
        inset[2].offset = 0.875;
        assert_eq!(easing.eased_position(&inset, 0.9375), 0.9375, "the terminal margin uses Linear");
        assert_eq!(easing.properties(&inset, 0.9375).rotation, 720.0);
        let global = ArrayEasing { mode: ArrayEaseMode::Global, ..easing };
        assert!(global.eased_position(&inset, 0.5) > 1.0);
    }

    #[test]
    fn array_easing_bakes_the_same_path_as_the_position_preview() {
        for mode in ["linear", "radial"] {
            for closed in [false, true] {
                let mut s = radial_source();
                let base = s.doc.clone();
                let source = s.selection.clone();
                let mut params = json!({
                    "mode": mode, "count": 9, "stacking": "front_to_back",
                    "start_x": 20, "start_y": 40, "dx": 12, "dy": 3,
                    "center_x": 140, "center_y": 100, "radius": 75, "start_angle": 20,
                    "closed_loop": closed, "sweep_angle": -720, "spiral_offset": -20, "drift_x": 35, "drift_y": -10,
                    "easing": {"mode": "global", "preset": "BackOut", "transform": false, "colour_effect": false},
                    "property_stops": [{"offset": 0.0, "alpha": 1}, {"offset": 0.5, "alpha": 0.5}, {"offset": 1, "alpha": 0}],
                });
                s.preview_array(&params).unwrap();
                let generation = s.array_translation_generation().unwrap();
                if mode == "linear" { params["dx"] = json!(20); } else { params["center_x"] = json!(150); }
                s.preview_array(&params).unwrap();
                assert_eq!(s.array_translation_generation(), Some(generation));

                let mut expected = Session::new(base);
                expected.selection = source;
                expected.execute("array.create", &params).unwrap();
                assert!(s.doc == expected.doc && s.selection == expected.selection);
                let copies = s.selected_elements();
                let t = 5.0 / if mode == "radial" && closed { 9.0 } else { 8.0 };
                let position = f64::from(Easing::BackOut.apply(t));
                assert!(position > 1.0);
                let at = if mode == "radial" {
                    parse_radial(&params, Vec2::ZERO).unwrap().point(position)
                } else {
                    Vec2::new(20.0 + 20.0 * (8.0 * position) as f32, 40.0 + 3.0 * (8.0 * position) as f32)
                };
                let copy = &copies[copies.len() - 1 - 5].1;
                assert_eq!(copy.transform.translation(), at);
                assert!((copy.color.alpha() - (1.0 - t)).abs() < 1e-6);
                s.finish_array_preview(true);
                let (_, recorded) = s.history.steps().last().unwrap();
                assert_eq!(recorded.unwrap(), &[crate::history::Recorded::new("array.create", &params)]);
            }
        }
    }

    #[test]
    fn array_easing_validates_parameters_and_handles_one_instance() {
        let mut s = radial_source();
        let original = s.doc.clone();
        for invalid in [
            json!([]), json!({"preset": "unknown"}), json!({"preset": "smooth"}), json!({"preset": {"classic": 50}}),
            json!({"mode": false}), json!({"distribution": 1}),
        ] {
            assert!(s.preview_array(&json!({"easing": invalid})).is_err());
            assert!(s.doc == original && !s.array_preview_active());
        }
        let params = json!({"mode": "radial", "count": 1, "center_x": 50, "center_y": 20, "radius": 20,
            "start_angle": 0, "sweep_angle": -1080, "closed_loop": false, "spiral_offset": -120, "drift_x": -30, "rotate": true,
            "easing": {"mode": "per_segment", "preset": "BackOut"},
            "property_stops": [{"offset": 0.0, "scale_x": 2, "alpha": 0.5}, {"offset": 1.0, "alpha": 0, "rotation": 720}],
        });
        s.execute("array.create", &params).unwrap();
        let copies = s.selected_elements();
        assert_eq!(copies.len(), 1);
        assert_eq!(copies[0].1.transform.translation(), Vec2::new(70.0, 20.0));
        assert_eq!(copies[0].1.transform.m[0], 2.0);
        assert_eq!(copies[0].1.color.alpha(), 0.5);
    }

    #[test]
    fn array_segment_easing_keeps_spiral_geometry_and_composes_with_instance_rotation() {
        for distribution in [false, true] {
            let mut s = radial_source();
            let params = json!({"mode": "radial", "count": 5, "center_x": 75, "center_y": 250, "radius": 120,
                "start_angle": 20, "sweep_angle": -720, "closed_loop": false, "spiral_offset": -30,
                "drift_x": 60, "drift_y": -16, "rotate": true,
                "easing": {"mode": "per_segment", "preset": "QuadIn", "distribution": distribution, "colour_effect": false},
                "property_stops": [
                    {"offset": 0, "rotation": 0, "alpha": 1},
                    {"offset": 0.5, "rotation": 360, "alpha": 0.5, "scale_x": 2},
                    {"offset": 1, "rotation": 720, "alpha": 0},
                ],
            });
            let path = parse_radial(&params, Vec2::ZERO).unwrap();
            s.execute("array.create", &params).unwrap();
            let copies = s.selected_elements();
            let at = if distribution { 0.125 } else { 0.25 };
            let xf = copies[1].1.transform;
            assert_eq!(xf.translation(), path.point(at));
            let (sin, cos) = (90.0_f32 - 720.0 * at as f32).to_radians().sin_cos();
            assert!(xf.apply_vector(Vec2::new(1.0, 0.0)).distance(Vec2::new(cos, sin) * 1.25) < 1e-5);
            assert_eq!(copies[1].1.color.alpha(), 0.75);
            assert_eq!(copies[2].1.transform.translation(), path.point(0.5));
            assert_eq!(copies[4].1.transform.translation(), path.point(1.0));
        }
    }

    #[test]
    fn transform_stops_bake_relative_scale_and_two_full_turns_with_alpha() {
        let mut s = radial_source();
        let id = s.selection.ids().next().unwrap();
        let source = s.editable_keyframe().unwrap().elements.iter_mut().find(|e| e.id == id).unwrap();
        let source = Arc::make_mut(source);
        source.transform = Affine::rotate(30.0_f32.to_radians()).mul(&Affine::scale(2.0, 0.5));
        let alpha = anicraft_doc::ColorEffect::Alpha(0.8);
        source.color = alpha;
        s.execute("array.create", &json!({"count": 9, "start_x": 25, "start_y": 60, "dx": 40,
            "property_stops": [{"offset": 0, "alpha": 1}, {"offset": 1, "alpha": 0.25, "scale_x": 2, "scale_y": 0.5, "rotation": 720}]})).unwrap();
        assert_eq!(s.selection.len(), 9);
        for (i, (_, copy)) in s.selected_elements().into_iter().enumerate() {
            let t = i as f32 / 8.0;
            let (sin, cos) = (30.0 + i as f32 * 90.0).to_radians().sin_cos();
            let xf = copy.transform;
            assert_eq!(xf.translation(), Vec2::new(25.0 + i as f32 * 40.0, 60.0));
            assert!(xf.apply_vector(Vec2::new(1.0, 0.0)).distance(Vec2::new(cos, sin) * (2.0 * (1.0 + t))) < 1e-5);
            assert!(xf.apply_vector(Vec2::new(0.0, 1.0)).distance(Vec2::new(-sin, cos) * (0.5 * (1.0 - 0.5 * t))) < 1e-5);
            assert_eq!(copy.color, alpha.with_alpha(1.0 - 0.75 * t));
        }
    }

    #[test]
    fn transform_stops_follow_radial_progression_and_add_to_rotate_instances_about_the_pivot() {
        let mut s = radial_source();
        let id = s.selection.ids().next().unwrap();
        let source = s.editable_keyframe().unwrap().elements.iter_mut().find(|e| e.id == id).unwrap();
        let source = Arc::make_mut(source);
        let pivot = Vec2::new(4.0, -3.0);
        assert!(source.set_pivot(Some(pivot)));
        source.transform = Affine::rotate(15.0_f32.to_radians()).mul(&Affine::scale(1.5, 0.75));
        let basis = source.transform;
        for (closed_loop, rotate, sweep) in [(true, true, -360.0), (false, false, 630.0), (false, true, -630.0)] {
            let p = json!({"mode": "radial", "count": 5, "center_x": 210, "center_y": 80, "radius": 40, "start_angle": 12,
                "sweep_angle": sweep, "closed_loop": closed_loop, "rotate": rotate, "spiral_offset": 33, "drift_x": 24, "drift_y": -17,
                "property_stops": [{"offset": 0, "rotation": -45, "scale_x": 0.5}, {"offset": 1, "rotation": 675, "scale_x": 2.5, "scale_y": 2} ]});
            let radial = parse_radial(&p, Vec2::ZERO).unwrap();
            let copies = placed(&mut s, &p);
            assert_eq!(copies.len(), 5);
            for (i, xf) in copies.into_iter().enumerate() {
                let t = i as f64 / if closed_loop { 5.0 } else { 4.0 };
                let travel = if rotate { sweep * t } else { 0.0 };
                let base = Affine::rotate((travel as f32).to_radians()).mul(&basis);
                let anchor = radial.point(t) + base.apply_vector(pivot);
                assert!(xf.apply(pivot).distance(anchor) < 0.0001);
                let angle = (15.0 + travel - 45.0 + 720.0 * t).to_radians() as f32;
                let (sin, cos) = angle.sin_cos();
                let t = t as f32;
                assert!(xf.apply_vector(Vec2::new(1.0, 0.0)).distance(Vec2::new(cos, sin) * (1.5 * (0.5 + 2.0 * t))) < 1e-5);
                assert!(xf.apply_vector(Vec2::new(0.0, 1.0)).distance(Vec2::new(-sin, cos) * (0.75 * (1.0 + t))) < 1e-5);
            }
        }
        assert!(s.array_translation_generation().is_none());
        s.finish_array_preview(false);
    }

    fn radial_source() -> Session {
        let mut s = Session::default();
        s.execute("draw.rect", &json!({"x": 5, "y": 10, "w": 20, "h": 18})).unwrap();
        s.execute("modify.convert_to_symbol", &json!({"name": "Source"})).unwrap();
        s
    }

    fn placed(s: &mut Session, p: &Value) -> Vec<Affine> {
        s.preview_array(p).unwrap();
        s.selected_elements().into_iter().map(|(_, el)| el.transform).collect()
    }

    fn assert_polar(at: Affine, radius: f32, angle: f32) {
        let (sin, cos) = angle.to_radians().sin_cos();
        let expected = anicraft_geom::Vec2::new(radius * cos, radius * sin);
        assert!(at.translation().distance(expected) < 1e-3, "expected {expected:?}, got {:?}", at.translation());
    }

    #[test]
    fn closed_radial_defaults_to_one_turn_and_keeps_count_intervals_below_it() {
        let mut s = radial_source();
        let mut p = json!({"mode": "radial", "center_x": 0, "center_y": 0, "radius": 50, "start_angle": 0});
        for count in [1, 2, 4, 9] {
            p["count"] = json!(count);
            let copies = placed(&mut s, &p);
            assert_eq!(copies.len(), count);
            for (i, at) in copies.into_iter().enumerate() {
                assert_polar(at, 50.0, 360.0 * i as f32 / count as f32);
            }
        }
        // A spiral value remains available when switching back to open mode.
        p["count"] = json!(4);
        p["spiral_offset"] = json!(35);
        p["closed_loop"] = json!(true);
        for sweep in [358.0f32, 360.0, 362.0, -358.0, -360.0, -362.0, 720.0, -1080.0] {
            p["sweep_angle"] = json!(sweep);
            let copies = placed(&mut s, &p);
            assert_polar(copies[0], 50.0, 0.0);
            assert_polar(copies[3], 50.0, sweep.clamp(-360.0, 360.0) * 0.75);
        }
    }

    #[test]
    fn closed_radial_full_turn_ends_exactly_at_start_without_placing_a_seam_copy() {
        let mut s = radial_source();
        for sweep in [-360.0, 360.0] {
            let radial = RadialDistribution { centre: [0.0, 0.0], radius: 420.0, start: 0.0,
                sweep, closed_loop: true, spiral_offset: 0.0, centre_drift: [60.0, -10.0], rotate: true };
            assert_eq!(radial.point(0.0), radial.point(1.0));
            let p = json!({"mode": "radial", "center_x": 0, "center_y": 0, "radius": 420,
                "start_angle": 0, "sweep_angle": sweep, "count": 10, "closed_loop": true, "rotate": true,
                "drift_x": 60, "drift_y": -10});
            let copies = placed(&mut s, &p);
            assert_eq!(copies.len(), 10);
            assert_eq!(copies[0].translation(), radial.point(0.0));
            assert!(copies.iter().skip(1).all(|copy| copy.translation() != radial.point(0.0)));
        }
    }

    #[test]
    fn centre_drift_accumulates_independently_of_radius_and_sweep_direction() {
        let mut s = radial_source();
        for sweep in [-720.0_f32, 720.0] {
            for offset in [-40.0_f32, 0.0, 25.0] {
                let p = json!({"mode": "radial", "center_x": 10, "center_y": 15, "radius": 40,
                    "start_angle": 0, "sweep_angle": sweep, "count": 9, "closed_loop": false,
                    "spiral_offset": offset, "drift_x": 30, "drift_y": -12});
                let copies = placed(&mut s, &p);
                assert_eq!(copies.len(), 9);
                for (i, copy) in copies.into_iter().enumerate() {
                    let turns = i as f32 / 4.0;
                    let centre = Vec2::new(10.0 + 30.0 * turns, 15.0 - 12.0 * turns);
                    let radius = (40.0 + offset * turns).max(0.0);
                    let (sin, cos) = (sweep.signum() * turns * std::f32::consts::TAU).sin_cos();
                    assert!(copy.translation().distance(centre + Vec2::new(cos, sin) * radius) < 1e-4);
                }
            }
        }
    }

    #[test]
    fn open_radial_keeps_both_endpoints_through_positive_and_negative_turn_boundaries() {
        let mut s = radial_source();
        let mut p = json!({"mode": "radial", "closed_loop": false, "center_x": 0, "center_y": 0, "radius": 50, "start_angle": 23});
        for count in [1, 2, 5, 8, 17] {
            p["count"] = json!(count);
            for sweep in [358.0, 360.0, 362.0, -358.0, -360.0, -362.0, 720.0, 1080.0] {
                p["sweep_angle"] = json!(sweep);
                let copies = placed(&mut s, &p);
                assert_eq!(copies.len(), count);
                assert_polar(copies[0], 50.0, 23.0);
                let last = if count == 1 { 23.0 } else { 23.0 + sweep };
                assert_polar(*copies.last().unwrap(), 50.0, last);
                if count > 2 {
                    let middle = count / 2;
                    let fraction = middle as f32 / (count - 1) as f32;
                    assert_polar(copies[middle], 50.0, 23.0 + sweep * fraction);
                }
            }
        }
    }

    #[test]
    fn spiral_grows_or_shrinks_per_turn_in_both_directions_and_stops_at_the_centre() {
        let mut s = radial_source();
        let mut p = json!({"mode": "radial", "closed_loop": false, "center_x": 0, "center_y": 0,
            "radius": 40, "start_angle": 0, "count": 7});
        for sign in [-1.0, 1.0] {
            for turns in [2.0, 3.0] {
                for offset in [-20.0, 15.0] {
                    let sweep = sign * 360.0 * turns;
                    p["sweep_angle"] = json!(sweep);
                    p["spiral_offset"] = json!(offset);
                    for (i, at) in placed(&mut s, &p).into_iter().enumerate() {
                        let t = i as f32 / 6.0;
                        let expected = (40.0f32 + offset * turns * t).max(0.0);
                        assert!(at.is_finite());
                        assert_polar(at, expected, sweep * t);
                    }
                }
            }
        }
        p["spiral_offset"] = json!(-40);
        for sweep in [359.0f32, 360.0, 361.0] {
            p["sweep_angle"] = json!(sweep);
            let at = *placed(&mut s, &p).last().unwrap();
            assert_polar(at, (40.0 - 40.0 * sweep / 360.0).max(0.0), sweep);
        }
    }

    #[test]
    fn spiral_rotation_uses_signed_travel_without_moving_the_registration_point() {
        let mut s = radial_source();
        let mut p = json!({"mode": "radial", "closed_loop": false, "center_x": 0, "center_y": 0,
            "radius": 50, "start_angle": 0, "spiral_offset": 16, "count": 2});
        for sweep in [-810.0f32, 810.0] {
            p["sweep_angle"] = json!(sweep);
            p["rotate"] = json!(false);
            let fixed = placed(&mut s, &p)[1];
            p["rotate"] = json!(true);
            let turned = placed(&mut s, &p)[1];
            assert_eq!(turned.translation(), fixed.translation());
            assert!((turned.m[1] - sweep.signum()).abs() < 1e-5);
            assert!(turned.m[0].abs() < 1e-5);
        }
    }

    #[test]
    fn invalid_radial_topology_and_spiral_values_do_not_change_the_document() {
        let mut s = radial_source();
        let (doc, selection, steps) = (s.doc.clone(), s.selection.clone(), s.history_len());
        for (key, value) in [("closed_loop", json!("false")), ("spiral_offset", json!("ten")), ("spiral_offset", json!(1e30)),
            ("drift_x", json!("right")), ("drift_y", json!(1e30))] {
            let mut p = json!({"mode": "radial", "center_x": 0, "center_y": 0, "closed_loop": false});
            p[key] = value;
            assert!(s.execute("array.create", &p).is_err());
            assert!(s.doc == doc && s.selection == selection && s.history_len() == steps);
        }
    }

    #[test]
    fn nonrotating_spiral_edits_reuse_the_preview_and_commit_exactly_the_last_geometry() {
        let mut s = radial_source();
        let base = s.doc.clone();
        let source = s.selection.clone();
        let mut p = json!({"mode": "radial", "closed_loop": false, "center_x": 200, "center_y": 50,
            "radius": 38, "start_angle": 15, "sweep_angle": 358, "spiral_offset": 15, "count": 20, "drift_x": 0, "drift_y": 0,
            "stacking": "front_to_back", "alpha_stops": [{"offset": 0, "alpha": 1}, {"offset": 1, "alpha": 0.2}]});
        s.preview_array(&p).unwrap();
        let generation = s.array_translation_generation();
        assert!(generation.is_some());
        let ids = s.selection.clone();
        let first = s.selected_elements().last().unwrap().1.clone();
        p["sweep_angle"] = json!(721);
        p["spiral_offset"] = json!(-30);
        p["drift_x"] = json!(46);
        p["drift_y"] = json!(-20);
        s.preview_array(&p).unwrap();
        assert_eq!(s.array_translation_generation(), generation);
        assert_eq!(s.selection, ids);
        assert!(Arc::ptr_eq(&first, &s.selected_elements().last().unwrap().1));
        s.finish_array_preview(true);

        let mut expected = Session::new(base);
        expected.selection = source;
        expected.execute("array.create", &p).unwrap();
        assert!(s.doc == expected.doc && s.selection == expected.selection);
        let (_, command) = s.history.steps().last().unwrap();
        assert_eq!(command.unwrap(), &[crate::history::Recorded::new("array.create", &p)]);
    }

    #[test]
    fn grid_property_distribution_reuses_the_full_stop_sampler() {
        let mut s = radial_source();
        s.execute("symbol.set_kind", &json!({"kind": "movie_clip"})).unwrap();
        s.execute("element.set_filters", &json!({"filters": [{"type": "blur", "blur_x": 2, "blur_y": 3}]})).unwrap();
        let source = s.doc.clone();
        let selection = s.selection.clone();
        let stops: Vec<_> = [0.0, 0.5, 1.0].into_iter().map(|t| json!({
            "offset": t, "alpha": 1.0 - t * 0.5, "scale_x": 1.0 + t,
            "scale_y": 1.0 + 2.0 * t, "rotation": 45.0 * t,
            "color_effect": ColorEffect::Brightness(t),
            "filters": [{"type": "blur", "blur_x": 2.0 + 8.0 * t, "blur_y": 3}],
            "outbound_ease": "QuadIn",
        })).collect();
        let mut p = json!({"mode": "grid", "rows": 3, "columns": 4, "start_x": 120, "start_y": 100,
            "spacing_x": -25, "spacing_y": -30, "easing": {"mode": "per_segment"},
            "source_values": true, "property_stops": stops});
        s.preview_array(&p).unwrap();
        let legacy = s.doc.clone();
        p["property_distribution"] = json!("xy");
        s.preview_array(&p).unwrap();
        assert_eq!(s.doc, legacy);

        for (rows, columns) in [(3, 4), (1, 4), (3, 1), (1, 1)] {
            p["rows"] = json!(rows);
            p["columns"] = json!(columns);
            for flip in [false, true] {
                p["flip_on_reverse"] = json!(flip);
                for mode in ["x", "y", "xy"] {
                    p["property_distribution"] = json!(mode);
                    s.preview_array(&p).unwrap();
                    for (i, (_, copy)) in s.selected_elements().into_iter().enumerate() {
                        let (x, y) = (i % columns, i / columns);
                        let (step, count) = match mode {
                            "x" => (x, columns),
                            "y" => (y, rows),
                            _ => (i, rows * columns),
                        };
                        let progress = step as f32 / (count - 1).max(1) as f32;
                        let start = if progress < 0.5 { 0.0 } else { 0.5 };
                        let t = start + ((progress - start) * 2.0).powi(2) * 0.5;
                        let close = |a: f32, b: f32| assert!((a - b).abs() < 0.0001, "{mode} {i}: {a} != {b}");
                        let at = copy.transform.translation();
                        assert_eq!(at, Vec2::new(120.0 - 25.0 * x as f32, 100.0 - 30.0 * y as f32));
                        let angle = (45.0 * t).to_radians();
                        let (sin, cos) = angle.sin_cos();
                        close(copy.transform.m[0], cos * (1.0 + t) * if flip && columns > 1 { -1.0 } else { 1.0 });
                        close(copy.transform.m[1], sin * (1.0 + t) * if flip && rows > 1 { -1.0 } else { 1.0 });
                        close(copy.color.alpha(), 1.0 - t * 0.5);
                        let color = ColorEffect::Brightness(t).with_alpha(1.0 - t * 0.5);
                        let expected = color.transform();
                        let actual = copy.color.transform();
                        for (a, b) in actual.mul.into_iter().chain(actual.add).zip(expected.mul.into_iter().chain(expected.add)) {
                            close(a, b);
                        }
                        let [entry] = copy.kind.filters().unwrap() else { panic!("expected one filter") };
                        let Filter::Blur { blur_x, blur_y, .. } = entry.filter else { panic!("expected blur") };
                        close(blur_x, 2.0 + 8.0 * t);
                        assert_eq!(blur_y, 3.0);
                    }
                }
            }
        }

        p["rows"] = json!(3);
        p["columns"] = json!(4);
        p["property_distribution"] = json!("y");
        s.preview_array(&p).unwrap();
        let preview = s.doc.clone();
        s.finish_array_preview(true);
        assert_eq!(s.doc, preview);
        s.undo().unwrap();
        assert_eq!(s.doc, source);
        assert_eq!(s.selection, selection);
        s.execute("array.create", &p).unwrap();
        assert_eq!(s.doc, preview);
    }

    fn instance_symbol(element: &Element) -> SymbolId {
        let ElementKind::Symbol { symbol, .. } = element.kind else { panic!("expected a symbol") };
        symbol
    }

    #[test]
    fn array_unique_commit_preserves_the_evaluated_preview_in_every_layout() {
        let mut s = radial_source();
        let source = instance_symbol(&s.selected_elements()[0].1);
        s.execute("symbol.set_kind", &json!({"kind": "movie_clip"})).unwrap();
        s.execute("element.set_filters", &json!({"filters": [
            {"type": "drop_shadow", "distance": 4, "angle": 20, "blur_x": 2, "blur_y": 2, "color": "#446688"},
            {"type": "blur", "blur_x": 1, "blur_y": 1, "enabled": false}
        ]})).unwrap();
        let original = s.selection.ids().next().unwrap();
        let base = s.doc.clone();
        let selection = s.selection.clone();
        let steps = s.history_len();
        let stops = json!([
            {"offset": 0, "alpha": 0.95, "scale_x": 1, "scale_y": 1, "rotation": 0,
                "color_effect": ColorEffect::Brightness(0.1),
                "filters": [{"type": "drop_shadow", "distance": 4, "angle": 20, "blur_x": 2, "blur_y": 2, "color": "#446688"},
                    {"type": "blur", "blur_x": 1, "blur_y": 1}]},
            {"offset": 1, "alpha": 0.45, "scale_x": 1.75, "scale_y": 0.5, "rotation": 50,
                "color_effect": ColorEffect::Brightness(-0.25),
                "filters": [{"type": "drop_shadow", "distance": 17, "angle": 70, "blur_x": 8, "blur_y": 6, "color": "#cc6633"},
                    {"type": "blur", "blur_x": 9, "blur_y": 9}]}
        ]);
        let mut p = json!({"mode": "linear", "count": 4, "start_x": 100, "start_y": 100, "dx": -25, "dy": 10,
            "center_x": 150, "center_y": 150, "radius": 50, "start_angle": 45,
            "rows": 2, "columns": 2, "spacing_x": -50, "spacing_y": -45, "property_distribution": "y",
            "stagger": {"rows": {"enabled": true, "x": 15}},
            "property_stops": stops, "source_values": true, "stacking": "front_to_back"});
        let render = |session: &Session| anicraft_render::render_frame(&session.doc, 0, 0.35).unwrap().data;
        for (mode, closed) in [("linear", true), ("radial", true), ("radial", false), ("grid", true)] {
            p["mode"] = json!(mode);
            p["closed_loop"] = json!(closed);
            p["spiral_offset"] = json!(if closed { 0 } else { 14 });
            p["rotate"] = json!(!closed);
            p["create_unique_symbols"] = json!(false);
            s.preview_array(&p).unwrap();
            let shared_preview = s.doc.clone();
            let shared = s.selected_elements();
            let pixels = render(&s);
            s.finish_array_preview(true);
            assert_eq!(s.doc, shared_preview);
            assert_eq!(s.doc.library, base.library);
            assert!(shared.iter().all(|(_, el)| instance_symbol(el) == source));
            s.undo().unwrap();
            assert_eq!(s.doc, base);

            p["create_unique_symbols"] = json!(true);
            s.preview_array(&p).unwrap();
            assert_eq!(s.doc, shared_preview);
            assert_eq!(s.doc.library, base.library);
            s.finish_array_preview(false);
            assert_eq!(s.doc, base);
            assert_eq!(s.selection, selection);

            s.preview_array(&p).unwrap();
            s.commit_unique_array_preview(&p).unwrap();
            assert_eq!(s.history_len(), steps + 1);
            assert_eq!(s.selection.ids().next(), Some(original));
            assert_eq!(s.doc.symbol(source), base.symbol(source));
            assert_eq!(s.doc.library.len(), base.library.len() + 4);
            assert_eq!(render(&s), pixels);
            for (i, ((_, before), (_, after))) in shared.iter().rev().zip(s.selected_elements().into_iter().rev()).enumerate() {
                let id = instance_symbol(&after);
                let definition = s.doc.symbol(id).unwrap();
                assert_eq!(definition.name, format!("Source_{}", i + 1));
                assert_eq!(definition.kind, base.symbol(source).unwrap().kind);
                let mut normalized = (*after).clone();
                let ElementKind::Symbol { symbol, .. } = &mut normalized.kind else { panic!("expected a symbol") };
                *symbol = source;
                assert_eq!(&normalized, &**before);
                let stack = after.kind.filters().unwrap();
                let [shadow, blur] = stack else { panic!("expected one shadow and one blur") };
                assert!(matches!(shadow.filter, Filter::DropShadow { distance, .. } if (4.0..=17.0).contains(&distance)));
                assert!(shadow.enabled && !blur.enabled);
            }
            let baked = s.doc.clone();
            let (_, recorded) = s.history.steps().last().unwrap();
            let [recorded] = recorded.unwrap() else { panic!("expected one Array command") };
            assert_eq!(recorded.cmd, "array.create");
            assert_eq!(recorded.params["create_unique_symbols"], true);
            let mut replay = Session::new(base.clone());
            replay.selection = selection.clone();
            replay.execute(&recorded.cmd, &recorded.params).unwrap();
            assert_eq!(replay.doc, baked);
            s.undo().unwrap();
            assert_eq!(s.doc, base);
            assert_eq!(s.selection, selection);
            s.redo().unwrap();
            assert_eq!(s.doc, baked);
            s.undo().unwrap();
        }
    }

    #[test]
    fn array_unique_definitions_keep_independent_timelines_and_shared_nested_symbols() {
        let mut s = radial_source();
        let source = instance_symbol(&s.selected_elements()[0].1);
        let original = s.selection.ids().next().unwrap();
        let folder = s.doc.ensure_folder_path("Array Examples").unwrap();
        s.doc.place_entry(LibraryEntry::Item(LibraryItem::Symbol(source)), Some(folder));
        s.execute("library.rename", &json!({"symbol": source.0, "name": "Box_8"})).unwrap();
        for name in ["Box", "Box_1", "box_3", "bOx_09"] {
            s.execute("library.duplicate", &json!({"symbol": source.0, "name": name})).unwrap();
        }
        s.execute("draw.rect", &json!({"x": 8, "y": 8, "w": 5, "h": 8})).unwrap();
        let nested = s.execute("modify.convert_to_symbol", &json!({"name": "Nested"})).unwrap()["symbol"].as_u64().unwrap();
        s.execute("symbol.edit", &json!({"symbol": source.0})).unwrap();
        s.execute("library.place", &json!({"symbol": nested, "x": 10, "y": 15})).unwrap();
        s.execute("timeline.goto", &json!({"frame": 6})).unwrap();
        s.execute("timeline.insert_keyframe", &json!({})).unwrap();
        s.execute("edit.select_all", &json!({})).unwrap();
        s.execute("element.set_transform", &json!({"x": 30})).unwrap();
        s.execute("symbol.exit", &json!({})).unwrap();
        let other = s.execute("library.place", &json!({"symbol": source.0, "x": 200, "y": 220})).unwrap()["element"].as_u64().unwrap();
        let unrelated = s.elements_on(s.layer).into_iter().find(|el| el.id == ElementId(other)).unwrap();
        s.select_only([original]);
        let doc = s.doc.clone();
        let params = json!({"mode": "linear", "count": 3, "start_x": 55, "start_y": 70, "dx": 42, "dy": 0});
        s.preview_array(&params).unwrap();
        s.commit_unique_array_preview(&params).unwrap();
        let copies = s.selected_elements().into_iter().map(|(_, el)| instance_symbol(&el)).collect::<Vec<_>>();
        assert!(s.elements_on(s.layer).contains(&unrelated));
        assert_eq!(s.doc.library.len(), doc.library.len() + 3);
        let definition = s.doc.symbol(source).unwrap();
        assert_eq!(Some(definition), doc.symbol(source));
        assert_eq!(definition.timeline.layers[0].keyframes.len(), 2);
        let mut internal_ids = std::collections::HashSet::new();
        for (i, id) in std::iter::once(source).chain(copies.iter().copied()).enumerate() {
            let symbol = s.doc.symbol(id).unwrap();
            assert_eq!(symbol.name, format!("Box_{}", if i == 0 { 8 } else { i + 9 }));
            assert_eq!(s.doc.parent_of(LibraryEntry::Item(LibraryItem::Symbol(id))), Some(folder));
            let layer = &symbol.timeline.layers[0];
            assert!(internal_ids.insert(layer.id.0));
            assert_eq!(layer.keyframes.len(), 2);
            let members = &layer.keyframes[0].elements;
            let nested_instance = members.iter().find(|el| matches!(el.kind, ElementKind::Symbol { .. })).unwrap();
            assert_eq!(instance_symbol(nested_instance), SymbolId(nested));
            for el in members {
                assert!(internal_ids.insert(el.id.0));
            }
            assert_eq!(layer.keyframes[1].start, 6);
            assert!(layer.keyframes[1].elements.iter().any(|el| el.id == nested_instance.id));
        }
        let before_edit = s.doc.clone();
        s.execute("symbol.edit", &json!({"symbol": copies[1].0})).unwrap();
        s.execute("timeline.goto", &json!({"frame": 3})).unwrap();
        s.execute("timeline.insert_keyframe", &json!({})).unwrap();
        s.execute("draw.rect", &json!({"x": -8, "y": 6, "w": 7, "h": 8})).unwrap();
        assert_ne!(s.doc.symbol(copies[1]), before_edit.symbol(copies[1]));
        assert_eq!(s.doc.symbol(copies[1]).unwrap().timeline.layers[0].keyframes.len(), 3);
        for id in [source, copies[0], copies[2], SymbolId(nested)] {
            assert_eq!(s.doc.symbol(id), before_edit.symbol(id));
        }
    }

    #[test]
    fn array_unique_names_support_large_suffixes_and_the_instance_cap() {
        let mut s = radial_source();
        let source = instance_symbol(&s.selected_elements()[0].1);
        let original = s.selection.ids().next().unwrap();
        let params = json!({"mode": "linear", "count": 500, "start_x": 4, "start_y": 5, "dx": 3, "dy": 0});
        s.preview_array(&params).unwrap();
        let generation = s.array_translation_generation();
        let mut toggled = params.clone();
        toggled["create_unique_symbols"] = json!(true);
        s.preview_array(&toggled).unwrap();
        assert_eq!(s.array_translation_generation(), generation);
        assert_eq!(s.doc.library.len(), 1);
        s.commit_unique_array_preview(&params).unwrap();
        assert_eq!(s.selection.len(), 500);
        assert_eq!(s.selection.ids().next(), Some(original));
        assert_eq!(s.doc.library.len(), 501);
        assert_eq!(s.doc.library.get(500).unwrap().name, "Source_500");
        s.undo().unwrap();
        assert_eq!(s.doc.library.len(), 1);

        s.execute("library.duplicate", &json!({"symbol": source.0, "name": "sOuRcE_9999999999999999999999"})).unwrap();
        assert_eq!(unique_symbol_names(&s.doc, source, 2).unwrap(), ["Source_10000000000000000000000", "Source_10000000000000000000001"]);
        s.execute("library.rename", &json!({"symbol": source.0, "name": "b".repeat(256)})).unwrap();
        let base = s.doc.clone();
        let params = json!({"count": 2, "mode": "linear", "start_x": 55, "start_y": 42, "dx": 30, "dy": 0});
        s.preview_array(&params).unwrap();
        let preview = s.doc.clone();
        assert!(s.commit_unique_array_preview(&params).is_err());
        assert!(s.array_preview_active());
        assert_eq!(s.doc, preview);
        assert_eq!(s.committed_document().0, &base);
        s.finish_array_preview(false);
        assert_eq!(s.doc, base);
    }

    #[test]
    fn grid_stagger_repeats_or_accumulates_from_the_nth_position() {
        for (every, repeat, accumulate) in [
            (1, [0, 1, 1, 1, 1, 1], [0, 1, 2, 3, 4, 5]),
            (2, [0, 1, 0, 1, 0, 1], [0, 1, 1, 2, 2, 3]),
            (3, [0, 0, 1, 0, 0, 1], [0, 0, 1, 1, 1, 2]),
            (7, [0; 6], [0; 6]),
        ] {
            let axis = GridStaggerAxis { enabled: true, every: NonZeroU32::new(every).unwrap(), offset: [50.0, -20.0] };
            for (accumulate, expected) in [(false, repeat), (true, accumulate)] {
                for (index, increments) in expected.into_iter().enumerate() {
                    assert_eq!(
                        axis.displacement(index as u32, accumulate),
                        Vec2::new(50.0, -20.0) * increments as f32
                    );
                }
            }
        }
        let zero = GridStaggerAxis { enabled: true, offset: [0.0; 2], ..GridStagger::default().rows };
        assert_eq!(zero.displacement(5, true), Vec2::ZERO);
    }

    #[test]
    fn grid_stagger_moves_instances_without_changing_stop_sampling_or_mirroring() {
        let mut s = radial_source();
        s.execute("symbol.set_kind", &json!({"kind": "movie_clip"})).unwrap();
        s.execute("element.set_filters", &json!({"filters": [{"type": "blur", "blur_x": 2}]})).unwrap();
        let original = s.selection.ids().next().unwrap();
        let mut p = json!({"mode": "grid", "rows": 3, "columns": 4, "start_x": 120, "start_y": 100,
            "spacing_x": -25, "spacing_y": -30, "easing": {"mode": "per_segment"},
            "property_stops": [
                {"offset": 0, "alpha": 1, "scale_x": 1, "outbound_ease": "QuadIn", "filters": [{"type": "blur", "blur_x": 2}]},
                {"offset": 1, "alpha": 0.5, "scale_x": 2, "rotation": 45, "filters": [{"type": "blur", "blur_x": 8}]}
            ]
        });
        for mode in ["x", "xy", "y"] {
            p["property_distribution"] = json!(mode);
            for flip in [false, true] {
                p["flip_on_reverse"] = json!(flip);
                p.as_object_mut().unwrap().remove("stagger");
                s.preview_array(&p).unwrap();
                let legacy = s.doc.clone();
                p["stagger"] = json!({});
                s.preview_array(&p).unwrap();
                assert_eq!(s.doc, legacy);
                let plain = s.selected_elements();
                let generation = s.array_translation_generation().unwrap();
                p["stagger"] = json!({
                    "rows": {"enabled": true},
                    "columns": {"enabled": true, "every": 3, "x": 0, "y": -20},
                    "accumulate": true
                });
                s.preview_array(&p).unwrap();
                assert_eq!(s.array_translation_generation(), Some(generation));
                assert_eq!(s.selection.ids().next(), Some(original));
                for (i, ((_, before), (_, after))) in plain.iter().zip(s.selected_elements()).enumerate() {
                    let (row, column) = (i / 4, i % 4);
                    let row_x = if row > 0 { 50.0 } else { 0.0 };
                    let column_y = if column > 1 { -20.0 } else { 0.0 };
                    assert_eq!(after.id, before.id);
                    assert_eq!(after.transform.translation(), before.transform.translation() + Vec2::new(row_x, column_y));
                    assert_eq!(&after.transform.m[..4], &before.transform.m[..4]);
                    assert_eq!(after.color, before.color);
                    assert_eq!(after.kind, before.kind);
                }
                p["spacing_x"] = json!(-40);
                p["start_y"] = json!(130);
                s.preview_array(&p).unwrap();
                assert_eq!(s.array_translation_generation(), Some(generation));
                assert_eq!(s.selected_elements()[0].1.transform.translation(), Vec2::new(120.0, 130.0));
                assert_eq!(s.selected_elements()[10].1.transform.translation(), Vec2::new(90.0, 50.0));
            }
        }

        p["rows"] = json!(20);
        p["columns"] = json!(25);
        s.preview_array(&p).unwrap();
        assert_eq!(s.selection.len(), MAX_ARRAY_INSTANCES as usize);
        let preview = s.doc.clone();
        s.finish_array_preview(true);
        assert_eq!(s.doc, preview);
        s.undo().unwrap();
        s.execute("array.create", &p).unwrap();
        assert_eq!(s.doc, preview);
        s.undo().unwrap();
        let base = s.doc.clone();
        for invalid in [
            json!({"mode": "grid", "rows": 21, "columns": 25, "stagger": p["stagger"]}),
            json!({"mode": "grid", "stagger": []}),
            json!({"mode": "grid", "stagger": {"rows": true}}),
            json!({"mode": "grid", "stagger": {"rows": {"every": 0}}}),
            json!({"mode": "grid", "stagger": {"columns": {"every": 1.5}}}),
            json!({"mode": "grid", "stagger": {"rows": {"x": 1e30}}}),
        ] {
            assert!(s.execute("array.create", &invalid).is_err(), "{invalid}");
            assert_eq!(s.doc, base);
        }
    }

    #[test]
    fn grid_counts_and_signed_row_major_positions_include_the_original() {
        let mut s = radial_source();
        let original = s.selection.ids().next().unwrap();
        for (rows, columns, x, y) in [
            (1, 1, -12.0, -5.0), (1, 4, -12.0, 21.0), (5, 1, 9.0, -5.0),
            (5, 5, -12.0, -5.0), (20, 25, 0.0, 0.0),
        ] {
            let params = json!({"mode": "grid", "rows": rows, "columns": columns, "start_x": 40, "start_y": 30,
                "spacing_x": x, "spacing_y": y, "stacking": "front_to_back"});
            s.preview_array(&params).unwrap();
            let copies = s.selected_elements();
            assert_eq!(copies.len(), rows * columns);
            // The selected elements are returned in draw order. The generated
            // selection IDs retain logical order even with first-in-front stacking.
            assert_eq!(s.selection.ids().next(), Some(original));
            assert!(s.selection.ids().eq(copies.iter().rev().map(|(_, copy)| copy.id)));
            for (index, (_, copy)) in copies.iter().rev().enumerate() {
                assert_eq!(copy.transform.translation(),
                    Vec2::new(40.0 + (index % columns) as f32 * x, 30.0 + (index / columns) as f32 * y));
                assert_eq!(copy.transform.m[0], if columns > 1 && x < 0.0 { -1.0 } else { 1.0 });
                assert_eq!(copy.transform.m[3], if rows > 1 && y < 0.0 { -1.0 } else { 1.0 });
            }
        }
        s.finish_array_preview(false);
        let base = s.doc.clone();
        for params in [
            json!({"mode": "grid", "rows": 0}),
            json!({"mode": "grid", "columns": -1}),
            json!({"mode": "grid", "rows": 22, "columns": 23}),
            json!({"mode": "grid", "rows": u32::MAX, "columns": u32::MAX}),
            json!({"mode": "grid", "columns": 4, "spacing_x": 2e38}),
            json!({"mode": "grid", "flip_on_reverse": "yes"}),
        ] {
            assert!(s.execute("array.create", &params).is_err(), "{params}");
            assert_eq!(s.doc, base);
        }
    }

    #[test]
    fn grid_preview_and_commit_match_replay_when_columns_replace_stagger() {
        let mut s = radial_source();
        let mut replay = Session::new(s.doc.clone());
        replay.selection = s.selection.clone();
        let mut params = json!({"mode": "grid", "rows": 2, "start_x": 120, "start_y": 100,
            "spacing_x": 25, "spacing_y": 30, "stagger": {"rows": {"enabled": true}}});
        s.preview_array(&params).unwrap();

        params.as_object_mut().unwrap().remove("stagger");
        params["columns"] = json!(1);
        s.preview_array(&params).unwrap();
        let preview = s.doc.clone();
        let preview_count = s.selection.len();
        s.finish_array_preview(true);
        assert_eq!(s.doc, preview);

        let (_, recorded) = s.history.steps().last().unwrap();
        let [recorded] = recorded.unwrap() else { panic!("expected one Array command") };
        replay.execute(&recorded.cmd, &recorded.params).unwrap();
        assert_eq!(preview_count, replay.selection.len(), "Grid preview and replay instance counts differ");
        assert_eq!(preview, replay.doc);
    }

    #[test]
    fn grid_mirrors_asymmetric_artwork_around_its_registration_after_stop_transforms() {
        let mut s = Session::default();
        s.execute("draw.path", &json!({"d": "M 10 10 L 14 10 L 14 16 L 25 16 L 25 20 L 10 20 Z",
            "fill": "#eb5632", "stroke": null})).unwrap();
        s.execute("modify.convert_to_symbol", &json!({"name": "Asymmetric"})).unwrap();
        s.execute("symbol.set_kind", &json!({"kind": "movie_clip"})).unwrap();
        s.execute("element.set_filters", &json!({"filters": [{"type": "blur", "blur_x": 2, "blur_y": 3}]})).unwrap();
        let original = s.selection.ids().next().unwrap();
        let source = s.editable_keyframe().unwrap().elements.iter_mut().find(|e| e.id == original).unwrap();
        let source = Arc::make_mut(source);
        let custom_pivot = Vec2::new(3.0, 5.0);
        assert!(source.set_pivot(Some(custom_pivot)));
        source.transform = Decomposed { sx: 1.5, sy: 0.75, rotation: 0.3, skew: 0.2, ..Default::default() }.compose();
        let base = s.doc.clone();
        let selection = s.selection.clone();
        let steps = s.history_len();
        let mut p = json!({"mode": "grid", "rows": 2, "columns": 2, "start_x": 160, "start_y": 150,
            "spacing_x": -55, "spacing_y": -50, "source_values": true,
            "easing": {"mode": "per_segment"},
            "property_stops": [
                {"offset": 0, "alpha": 1, "scale_x": 1.25, "scale_y": 0.5, "rotation": 30, "outbound_ease": "QuadIn",
                    "filters": [{"type": "blur", "blur_x": 2}]},
                {"offset": 1, "alpha": 0.4, "scale_x": 0.75, "scale_y": 1.5, "rotation": 80,
                    "filters": [{"type": "blur", "blur_x": 8}]}
            ], "flip_on_reverse": false
        });
        s.preview_array(&p).unwrap();
        let unflipped = s.selected_elements();
        for (reverse_x, reverse_y) in [(false, false), (true, false), (false, true), (true, true)] {
            let x = if reverse_x { -55.0 } else { 55.0 };
            let y = if reverse_y { -50.0 } else { 50.0 };
            p["spacing_x"] = json!(x);
            p["spacing_y"] = json!(y);
            for flip in [false, true] {
                p["flip_on_reverse"] = json!(flip);
                s.preview_array(&p).unwrap();
                let (fx, fy) = (flip && reverse_x, flip && reverse_y);
                assert_eq!(s.committed_document().0, &base);
                let copies = s.selected_elements();
                assert_eq!(copies[0].1.id, original);
                for ((i, (_, copy)), (_, plain)) in copies.iter().enumerate().zip(&unflipped) {
                    let at = Vec2::new(160.0 + (i % 2) as f32 * x, 150.0 + (i / 2) as f32 * y);
                    assert_eq!(copy.transform.translation(), at);
                    assert_eq!(copy.pivot(), Some(custom_pivot));
                    for point in [Vec2::new(2.0, 0.0), Vec2::new(11.0, 7.0), custom_pivot] {
                        let local = plain.transform.apply_vector(point);
                        let expected = at + Vec2::new(if fx { -local.x } else { local.x }, if fy { -local.y } else { local.y });
                        assert!(copy.transform.apply(point).distance(expected) < 0.0001);
                    }
                    let t = (i as f32 / 3.0).powi(2);
                    assert!((copy.color.alpha() - (1.0 - 0.6 * t)).abs() < 1e-5);
                    let stack = copy.kind.filters().unwrap();
                    assert_eq!(Some(stack), plain.kind.filters());
                    assert!(matches!(stack[0].filter, Filter::Blur { blur_x, .. } if (blur_x - (2.0 + 6.0 * t)).abs() < 1e-5));
                }
            }
        }

        let per_segment = p["easing"].clone();
        p["easing"] = json!({"mode": "global", "preset": "QuadOut"});
        s.preview_array(&p).unwrap();
        for (i, (_, copy)) in s.selected_elements().into_iter().enumerate() {
            let linear = i as f32 / 3.0;
            let t = 1.0 - (1.0 - linear).powi(2);
            assert!((copy.color.alpha() - (1.0 - 0.6 * t)).abs() < 1e-5);
            assert_eq!(copy.transform.translation(), Vec2::new(160.0 - (i % 2) as f32 * 55.0, 150.0 - (i / 2) as f32 * 50.0));
        }
        p["easing"] = per_segment;
        s.preview_array(&p).unwrap();
        let generation = s.array_translation_generation().unwrap();
        let anchor = s.selected_elements()[0].1.clone();
        p["spacing_x"] = json!(-68);
        s.preview_array(&p).unwrap();
        assert_eq!(s.array_translation_generation(), Some(generation));
        assert!(Arc::ptr_eq(&anchor, &s.selected_elements()[0].1));
        p["spacing_x"] = json!(68);
        s.preview_array(&p).unwrap();
        let reflected = s.array_translation_generation().unwrap();
        assert_ne!(reflected, generation);
        p["spacing_y"] = json!(-62);
        s.preview_array(&p).unwrap();
        assert_eq!(s.array_translation_generation(), Some(reflected));
        let preview = s.doc.clone();
        let pixels = anicraft_render::render_frame(&s.doc, 0, 0.5).unwrap().data;
        s.finish_array_preview(true);
        assert_eq!(s.history_len(), steps + 1);
        assert_eq!(s.doc, preview);
        assert_eq!(anicraft_render::render_frame(&s.doc, 0, 0.5).unwrap().data, pixels);
        s.undo().unwrap();
        assert_eq!(s.doc, base);
        assert_eq!(s.selection, selection);
        s.execute("array.create", &p).unwrap();
        assert_eq!(s.doc, preview);
    }

    #[test]
    #[ignore = "local Grid preview timing against full command replay"]
    fn measure_grid_preview_resize() {
        for count in [3, 10] {
            for reuse in [false, true] {
                let mut s = Session::default();
                s.execute("draw.rect", &json!({"x": 5, "y": 10, "w": 20, "h": 18,
                    "fill": null, "stroke": "#000000", "stroke_width": 2})).unwrap();
                s.execute("modify.convert_to_symbol", &json!({"name": "Stroked rectangle"})).unwrap();
                let mut params = json!({"mode": "grid", "rows": count, "columns": count,
                    "start_x": 20, "start_y": 30, "spacing_x": 40, "spacing_y": 40,
                    "source_values": true, "property_stops": [{"offset": 0}, {"offset": 1}]});
                s.preview_array(&params).unwrap();
                let mut samples = Vec::new();
                for i in 0..320 {
                    params["spacing_x"] = json!(40.0 + f64::from(i % 60) * 0.1);
                    params["spacing_y"] = json!(40.0 + f64::from(i % 60) * 0.2);
                    if !reuse { s.array_positions = None; }
                    let start = std::time::Instant::now();
                    s.preview_array(&params).unwrap();
                    if i >= 20 { samples.push(start.elapsed().as_secs_f64() * 1_000.0); }
                }
                samples.sort_by(f64::total_cmp);
                eprintln!("Grid {count} x {count}, preview reuse {reuse}: median {:.4} ms",
                    samples[samples.len() / 2]);
            }
        }
    }

    #[test]
    fn linear_count_includes_original_and_hits_last_stop() {
        let l = Layout::Linear { start_x: 12.0, start_y: 18.0, dx: 10.0, dy: -2.0 };
        let a = l.place(Affine::IDENTITY, 0, 10, None);
        let b = l.place(Affine::IDENTITY, 9, 10, None);
        assert_eq!(a.translation().x, 12.0);
        assert_eq!(b.translation().x, 102.0);
        assert_eq!(l.progress(0, 10), 0.0);
        assert_eq!(l.progress(9, 10), 1.0);
    }

    #[test]
    fn command_creates_baked_instances_as_one_undoable_edit() {
        let mut s = Session::default();
        assert!(s.execute("draw.rect", &json!({"x": 10, "y": 10, "w": 24, "h": 14, "fill": "#ffcc00", "stroke": null})).is_ok());
        assert!(s.execute("modify.convert_to_symbol", &json!({"name": "Test petal"})).is_ok());
        assert_eq!(s.selection.len(), 1);
        let original = s.selection.ids().next();
        let before = s.history_len();
        let made = s.execute("array.create", &json!({
            "mode": "linear", "count": 10, "dx": 20, "dy": 0,
            "alpha_stops": [
                {"offset": 0.0, "alpha": 0.75},
                {"offset": 0.4, "alpha": 1.0},
                {"offset": 1.0, "alpha": 0.03}
            ]
        }));
        assert!(made.is_ok(), "{made:?}");
        assert_eq!(s.history_len(), before + 1, "baking an array is a single undo step");
        assert_eq!(s.selection.len(), 10, "count includes the original");
        assert_eq!(s.selection.ids().next(), original, "instance #1 keeps the source ID");
        let elements = s.selected_elements();
        assert_eq!(elements.len(), 10);
        let alphas: Vec<f32> = elements.iter().map(|(_, e)| e.color.alpha()).collect();
        assert!((alphas[0] - 0.75).abs() < 1e-4);
        assert!((alphas[9] - 0.03).abs() < 1e-4);
        assert!(s.execute("edit.undo", &json!({})).is_ok());
        assert_eq!(s.selected_elements().len(), 1, "undo restores the original single instance");
    }

    #[test]
    fn stop_positions_and_values_are_validated() {
        let p = json!({"alpha_stops": [{"offset": 1, "alpha": 0.03}, {"offset": 0, "alpha": 0.75}, {"offset": 0.4, "alpha": 1}]});
        let parsed = parse_stops(&p, None, None).ok().flatten().unwrap_or_default();
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0].alpha, 0.75);
        assert_eq!(parsed[1].offset, 0.4);
        assert!(parse_stops(&json!({"alpha_stops": [{"offset": 2, "alpha": 0.5}, {"offset": 1, "alpha": 0.4}]}), None, None).is_err());
        for (key, value) in [("alpha", json!(1.01)), ("scale_x", json!(101)), ("scale_y", json!("50%")), ("rotation", json!(1e10))] {
            let mut params = json!({"property_stops": [{"offset": 0}, {"offset": 1}]});
            params["property_stops"][0][key] = value;
            assert!(parse_stops(&params, None, None).is_err());
        }
    }

    #[test]
    fn preview_owns_its_gesture_and_recovery_sees_the_committed_document() {
        let mut s = Session::default();
        s.execute("draw.rect", &json!({"x": 10, "y": 10, "w": 24, "h": 14})).unwrap();
        s.execute("modify.convert_to_symbol", &json!({"name": "Source"})).unwrap();
        s.mark_saved();
        let (doc, selection, state, steps) = (s.doc.clone(), s.selection.clone(), s.state, s.history_len());

        s.begin_gesture("Other edit");
        assert!(s.preview_array(&json!({"count": 3})).is_err());
        assert!(s.in_gesture() && !s.array_preview_active());
        s.end_gesture();
        s.preview_array(&json!({"count": 3})).unwrap();
        let shown = s.doc.clone();
        s.end_gesture();
        s.revert_gesture();
        assert!(s.undo().is_err());
        assert!(s.redo().is_err());
        assert!(s.execute("edit.deselect", &json!({})).is_err());
        assert!(s.execute("file.save", &json!({})).is_err());
        assert!(s.doc == shown && s.array_preview_active() && s.in_gesture());
        assert_eq!(s.selection.len(), 3);
        assert_eq!(s.history_len(), steps);
        assert!(!s.is_dirty());
        assert!(s.committed_document() == (&doc, state));

        assert!(s.preview_array(&json!({"count": 0})).is_err());
        assert!(s.array_preview_active() && s.doc == doc && s.selection == selection);
        s.preview_array(&json!({"count": 2})).unwrap();
        s.finish_array_preview(false);
        assert!(s.doc == doc && s.selection == selection && !s.in_gesture());
        assert_eq!(s.history_len(), steps);
    }

    #[test]
    fn spatial_array_rejection_inside_a_motion_tween_preserves_earlier_frames() {
        let params = json!({"count": 3});
        for preview in [false, true] {
            let mut s = radial_source();
            s.execute("timeline.create_motion_tween", &json!({})).unwrap();
            s.set_frame(12);
            s.execute("element.translate", &json!({"dx": 40, "dy": 0})).unwrap();
            let earlier_artwork = |s: &Session| {
                anicraft_doc::evaluate_layer_elements(s.active_layer().unwrap(), 6).unwrap().1.into_owned()
            };
            let earlier = earlier_artwork(&s);
            let (doc, selection, frame, state) = (s.doc.clone(), s.selection.clone(), s.frame, s.state);
            let undo = (s.history_len(), s.can_undo(), s.can_redo());
            let result = if preview { s.preview_array(&params) } else { s.execute("array.create", &params).map(|_| ()) };

            assert_eq!(earlier_artwork(&s).len(), earlier.len(), "Array added copies before the playhead");
            assert_eq!(earlier_artwork(&s), earlier);
            assert!(matches!(result, Err(EngineError::State(_))));
            assert_eq!(s.doc, doc);
            assert_eq!(s.selection, selection);
            assert_eq!(s.frame, frame);
            assert_eq!(s.state, state);
            assert_eq!((s.history_len(), s.can_undo(), s.can_redo()), undo);
            assert!(!s.array_preview_active() && !s.in_gesture());
        }
    }

    #[test]
    fn a_rejected_interior_edit_leaves_the_array_preview_from_the_tween_start_intact() {
        let mut s = radial_source();
        s.execute("timeline.create_motion_tween", &json!({})).unwrap();
        let base = s.doc.clone();
        let end = s.active_layer().unwrap().keyframe_at(s.frame).unwrap().end();
        s.preview_array(&json!({"count": 3})).unwrap();
        assert_eq!(s.selection.len(), 3);

        // Public scrubbing freezes an active preview. Exercise the guard with an open
        // preview at the last frame still covered by the tween.
        s.frame = end - 1;
        let (doc, selection, state, revision) = (s.doc.clone(), s.selection.clone(), s.state, s.revision);
        let history_len = s.history_len();
        assert!(matches!(s.preview_array(&json!({"count": 2})), Err(EngineError::State(_))));
        assert_eq!(s.doc, doc);
        assert_eq!(s.selection, selection);
        assert_eq!(s.frame, end - 1);
        assert_eq!((s.state, s.revision, s.history_len()), (state, revision, history_len));
        assert!(s.array_preview_active() && s.in_gesture());
        s.finish_array_preview(false);
        assert_eq!(s.doc, base);
    }

    #[test]
    fn array_requires_the_entire_selection_to_be_one_instance_on_the_active_layer() {
        let mut s = Session::default();
        s.execute("draw.rect", &json!({"x": 10, "y": 10, "w": 24, "h": 14})).unwrap();
        s.execute("modify.convert_to_symbol", &json!({"name": "Source"})).unwrap();
        let (layer, source) = s.selection.first().unwrap();
        s.execute("layer.new", &json!({"name": "Others"})).unwrap();
        s.execute("draw.rect", &json!({"x": 50, "y": 20, "w": 10, "h": 10})).unwrap();
        let (other_layer, other) = s.selection.first().unwrap();
        s.selection.insert(layer, source);
        s.layer = layer;
        assert!(s.execute("array.create", &json!({})).is_err());
        assert!(s.selection.contains(other_layer, other));
        s.select_only([source]);
        assert!(s.execute("array.create", &json!({"count": 1})).is_ok());
        assert_eq!(s.selection.first(), Some((layer, source)));
    }

    #[test]
    fn moving_linear_preview_reuses_instances_and_commits_the_same_document_as_the_command() {
        let mut s = Session::default();
        s.execute("draw.rect", &json!({"x": 10, "y": 10, "w": 24, "h": 14})).unwrap();
        s.execute("modify.convert_to_symbol", &json!({"name": "Source"})).unwrap();
        let (base, selected, steps) = (s.doc.clone(), s.selection.clone(), s.history_len());
        let mut p = json!({"mode": "linear", "count": 5, "start_x": 50, "start_y": 40, "dx": 12, "dy": -5,
            "stacking": "front_to_back", "property_stops": [{"offset": 0, "alpha": 1, "scale_x": 0.6},
            {"offset": 1, "alpha": 0.2, "scale_x": 1.8, "scale_y": 1.5, "rotation": -540}]});
        s.preview_array(&p).unwrap();
        let generation = s.array_translation_generation().unwrap();
        let ids = s.selection.clone();
        let still = s.selected_elements().last().unwrap().1.clone();
        p["dx"] = json!(400);
        p["dy"] = json!(70);
        s.preview_array(&p).unwrap();
        assert_eq!(s.array_translation_generation(), Some(generation));
        assert_eq!(s.selection, ids);
        assert!(Arc::ptr_eq(&still, &s.selected_elements().last().unwrap().1));

        let mut expected = Session::new(base.clone());
        expected.selection = selected.clone();
        expected.execute("array.create", &p).unwrap();
        assert!(s.doc == expected.doc && s.selection == expected.selection);
        s.finish_array_preview(true);
        assert!(s.array_translation_generation().is_none());
        assert_eq!(s.history_len(), steps + 1);
        let (_, recorded) = s.history.steps().last().unwrap();
        assert_eq!(recorded.unwrap(), &[crate::history::Recorded::new("array.create", &p)]);
        s.undo().unwrap();
        assert!(s.doc == base && s.selection == selected);
    }
}
