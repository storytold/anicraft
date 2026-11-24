//! Reusable motion clips and studio motion presets (`P-041`):
//!
//! * [`MotionClip`] captures relative transform curves (`X`/`Y` pixel offsets, `ScaleX`/`ScaleY`
//!   ratios in percent of base scale, `Rotation`/`Skew` degree offsets), colour effect curves,
//!   filter curves, motion trajectory + pins, base colour/filters, and optional classic-tween
//!   easing metadata for `timeline.copy_motion` / `timeline.paste_motion`.
//! * [`MotionPreset`] stores a named [`MotionClip`] on [`crate::Document`] alongside the 6
//!   built-in studio presets (`anime_snap_in`, `bounce_drop`, `elastic_pop`, `handheld_shake`,
//!   `pulse_glow`, `slide_overshoot`).

use crate::{
    ColorEffect, EaseGroup, Easing, Element, ElementId, Filter, FilterEntry, FilterParam,
    FilterProp, Layer, MAX_FILTER_SLOTS, MAX_FRAME, MotionTween, Pin, Property, PropertyKey, Props,
    Rgba, Track, Tween, TweenKind, motion_path,
};
use anicraft_geom::{Affine, Path};
use serde::{Deserialize, Serialize};

/// Maximum user-saved motion presets kept per document.
pub const MAX_MOTION_PRESETS: usize = 256;

/// Maximum length (in characters) of a motion preset's name.
pub const MAX_MOTION_PRESET_NAME: usize = 64;

/// Canonicalizes a motion preset name: trimmed, control characters stripped, up to
/// [`MAX_MOTION_PRESET_NAME`] characters; `None` if empty.
pub fn preset_name(name: &str) -> Option<String> {
    let n: String = name.trim().chars().filter(|c| !c.is_control()).take(MAX_MOTION_PRESET_NAME).collect();
    let n = n.trim().to_string();
    (!n.is_empty()).then_some(n)
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn clean_easing(e: Easing) -> Easing {
    match e {
        Easing::Classic(s) => Easing::Classic(if s.is_finite() { s.clamp(-100.0, 100.0) } else { 0.0 }),
        Easing::CubicBezier(x1, y1, x2, y2) => {
            let fx = |v: f32| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
            let fy = |v: f32| if v.is_finite() { v.clamp(-10.0, 10.0) } else { 0.0 };
            Easing::CubicBezier(fx(x1), fy(y1), fx(x2), fy(y2))
        }
        other => other,
    }
}

/// Portable, element-independent motion description captured by `timeline.copy_motion` or stored
/// in a [`MotionPreset`].
///
/// Transform tracks are stored relative to a neutral base at `(0, 0)`, scale `(100%, 100%)`,
/// rotation `0°`, skew `0°`, so pasting or applying a preset onto any object preserves its
/// resting stage position, scale and orientation while replaying the motion delta.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MotionClip {
    /// Span length in frames (`1..=MAX_FRAME`).
    pub duration: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tracks: Vec<Track>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<Path>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub orient: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pins: Vec<Pin>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub keep_area: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_effect: Option<ColorEffect>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filters: Vec<FilterEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classic_easing: Option<Tween>,
}

/// A user-saved motion preset stored in a [`crate::Document`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MotionPreset {
    pub name: String,
    pub clip: MotionClip,
}

/// Metadata + clip for one of the 6 built-in studio presets.
pub struct BuiltinMotionPreset {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub clip: MotionClip,
}

/// IDs of the 6 built-in studio motion presets.
pub const BUILTIN_PRESET_IDS: [&str; 6] = [
    "anime_snap_in",
    "bounce_drop",
    "elastic_pop",
    "handheld_shake",
    "pulse_glow",
    "slide_overshoot",
];

impl MotionClip {
    /// Empty static clip of `duration` frames.
    pub fn empty(duration: u32) -> Self {
        MotionClip {
            duration: duration.clamp(1, MAX_FRAME),
            tracks: Vec::new(),
            path: None,
            orient: false,
            pins: Vec::new(),
            keep_area: false,
            color_effect: None,
            filters: Vec::new(),
            classic_easing: None,
        }
    }

    /// Repairs hostile or out-of-range values loaded from a file. Idempotent.
    pub fn sanitize(&mut self) {
        self.duration = self.duration.clamp(1, MAX_FRAME);
        let mut tmp = MotionTween {
            target: ElementId(1),
            tracks: std::mem::take(&mut self.tracks),
            path: self.path.take(),
            orient: self.orient,
            pins: std::mem::take(&mut self.pins),
            keep_area: self.keep_area,
        };
        tmp.sanitize();
        let last = self.duration.saturating_sub(1);
        for t in &mut tmp.tracks {
            for k in &mut t.keys {
                if k.frame > last {
                    k.frame = last;
                }
            }
        }
        tmp.sanitize();
        self.tracks = tmp.tracks;
        self.path = tmp.path;
        self.pins = tmp.pins;
        self.color_effect = self.color_effect.take().map(|c| c.sanitized()).filter(|c| *c != ColorEffect::None);
        self.filters.truncate(usize::from(MAX_FILTER_SLOTS));
        if let Some(tw) = &mut self.classic_easing {
            tw.easing = clean_easing(tw.easing.clone());
            for (_, e) in &mut tw.groups {
                *e = clean_easing(e.clone());
            }
            tw.groups.dedup_by_key(|(g, _)| *g);
        }
    }

    pub fn sanitized(mut self) -> Self {
        self.sanitize();
        self
    }

    /// Captures motion from `layer`'s keyframe `kf_idx` (for `preferred_target`, or the tween's
    /// target / first element). Supports `MotionTween` spans, Classic/Shape tween spans, and
    /// single-keyframe element styles/filters.
    pub fn from_keyframe(layer: &Layer, kf_idx: usize, preferred_target: Option<ElementId>) -> Option<MotionClip> {
        let kf = layer.keyframes.get(kf_idx)?;
        if kf.elements.is_empty() {
            return None;
        }
        if let Some(m) = &kf.motion {
            let idx = m.target_index(&kf.elements)?;
            let base = kf.elements.get(idx)?;
            return Some(Self::from_motion_tween(base, m, kf.duration));
        }
        let target_idx = preferred_target
            .and_then(|id| kf.elements.iter().position(|e| e.id == id))
            .unwrap_or(0);
        let base = kf.elements.get(target_idx)?;
        if let Some(tween) = &kf.tween
            && let Some(next_kf) = layer.keyframes.get(kf_idx + 1)
            && next_kf.start == kf.end()
        {
            let next_el = next_kf
                .elements
                .iter()
                .find(|e| e.id == base.id)
                .or_else(|| next_kf.elements.get(target_idx))
                .or_else(|| next_kf.elements.first());
            if let Some(end_el) = next_el {
                return Some(Self::from_classic_tween(base, end_el, kf.duration, tween));
            }
        }
        let color_effect = (base.color != ColorEffect::None).then(|| base.color.sanitized());
        let filters = base.kind.filters().map_or_else(Vec::new, <[FilterEntry]>::to_vec);
        Some(
            MotionClip {
                duration: kf.duration.clamp(1, MAX_FRAME),
                tracks: Vec::new(),
                path: None,
                orient: false,
                pins: Vec::new(),
                keep_area: false,
                color_effect,
                filters,
                classic_easing: kf.tween.clone(),
            }
            .sanitized(),
        )
    }

    /// Captures a [`MotionTween`] relative to `base`.
    pub fn from_motion_tween(base: &Element, m: &MotionTween, duration: u32) -> MotionClip {
        let bp = Props::of(base);
        let (bx, by) = (bp.get(Property::X), bp.get(Property::Y));
        let (bsx, bsy) = (bp.get(Property::ScaleX), bp.get(Property::ScaleY));
        let (brot, bskew) = (bp.get(Property::Rotation), bp.get(Property::Skew));
        let duration = duration.clamp(1, MAX_FRAME);
        let last = duration.saturating_sub(1);
        let tracks: Vec<Track> = m
            .tracks
            .iter()
            .map(|t| {
                let keys = t
                    .keys
                    .iter()
                    .filter(|k| k.frame <= last)
                    .map(|k| {
                        let value = to_relative(t.property, k.value, bx, by, bsx, bsy, brot, bskew);
                        PropertyKey { frame: k.frame, value, ease: k.ease.clone() }
                    })
                    .collect();
                Track { property: t.property, keys }
            })
            .collect();
        let path = m
            .path
            .as_ref()
            .and_then(|p| motion_path(&p.transformed(&Affine::translate(-bx, -by))));
        let color_effect = (base.color != ColorEffect::None).then(|| base.color.sanitized());
        let filters = base.kind.filters().map_or_else(Vec::new, <[FilterEntry]>::to_vec);
        MotionClip {
            duration,
            tracks,
            path,
            orient: m.orient,
            pins: m.pins.clone(),
            keep_area: m.keep_area,
            color_effect,
            filters,
            classic_easing: None,
        }
        .sanitized()
    }

    /// Captures a Classic/Shape tween between `start_el` and `end_el` over `duration` frames.
    pub fn from_classic_tween(start_el: &Element, end_el: &Element, duration: u32, tween: &Tween) -> MotionClip {
        let p0 = Props::of(start_el);
        let p1 = Props::of(end_el);
        let dur = duration.clamp(1, MAX_FRAME);
        let end_frame = dur.max(2) - 1;
        let mut tracks = Vec::new();
        let add_pair = |tracks: &mut Vec<Track>, prop: Property, v0: f32, v1: f32, ease: Easing| {
            if (v1 - v0).abs() > 1e-4 && dur >= 2 {
                tracks.push(Track {
                    property: prop,
                    keys: vec![
                        PropertyKey { frame: 0, value: v0, ease },
                        PropertyKey { frame: end_frame, value: v1, ease: Easing::Linear },
                    ],
                });
            }
        };
        let pos_ease = tween.ease(EaseGroup::Position).clone();
        let scale_ease = tween.ease(EaseGroup::Scale).clone();
        let rot_ease = tween.ease(EaseGroup::Rotation).clone();
        let col_ease = tween.ease(EaseGroup::Color).clone();
        add_pair(&mut tracks, Property::X, 0.0, p1.get(Property::X) - p0.get(Property::X), pos_ease.clone());
        add_pair(&mut tracks, Property::Y, 0.0, p1.get(Property::Y) - p0.get(Property::Y), pos_ease);
        let ratio = |a: f32, b: f32| if a.abs() > 1e-3 { (b / a) * 100.0 } else { b };
        add_pair(&mut tracks, Property::ScaleX, 100.0, ratio(p0.get(Property::ScaleX), p1.get(Property::ScaleX)), scale_ease.clone());
        add_pair(&mut tracks, Property::ScaleY, 100.0, ratio(p0.get(Property::ScaleY), p1.get(Property::ScaleY)), scale_ease);
        let mut drot = (p1.get(Property::Rotation) - p0.get(Property::Rotation) + 180.0).rem_euclid(360.0) - 180.0;
        if !drot.is_finite() {
            drot = 0.0;
        }
        add_pair(&mut tracks, Property::Rotation, 0.0, drot, rot_ease.clone());
        add_pair(&mut tracks, Property::Skew, 0.0, p1.get(Property::Skew) - p0.get(Property::Skew), rot_ease);

        if start_el.color != end_el.color {
            match (start_el.color.style(), end_el.color.style()) {
                (s0, s1) if s0 == s1 || s0 == crate::ColorStyle::None || s1 == crate::ColorStyle::None => {
                    let active = if s0 != crate::ColorStyle::None { s0 } else { s1 };
                    for prop in Property::ALL.into_iter().filter(|p| p.style() == Some(active)) {
                        add_pair(&mut tracks, prop, p0.get(prop), p1.get(prop), col_ease.clone());
                    }
                }
                _ => {
                    for prop in Property::ADVANCED {
                        add_pair(&mut tracks, prop, p0.get(prop), p1.get(prop), col_ease.clone());
                    }
                }
            }
        }

        if let (Some(fa), Some(fb)) = (start_el.kind.filters(), end_el.kind.filters()) {
            let flt_ease = tween.ease(EaseGroup::Filters).clone();
            for (slot, (ea, eb)) in fa.iter().zip(fb.iter()).enumerate().take(usize::from(MAX_FILTER_SLOTS)) {
                if std::mem::discriminant(&ea.filter) == std::mem::discriminant(&eb.filter) {
                    for param in ea.filter.params() {
                        if let (Some(va), Some(vb)) = (ea.filter.param(param), eb.filter.param(param)) {
                            add_pair(&mut tracks, Property::Filter(FilterProp { slot: slot as u8, param }), va, vb, flt_ease.clone());
                        }
                    }
                }
            }
        }

        let color_effect = (start_el.color != ColorEffect::None).then(|| start_el.color.sanitized());
        let filters = start_el.kind.filters().map_or_else(Vec::new, <[FilterEntry]>::to_vec);
        MotionClip {
            duration: dur,
            tracks,
            path: None,
            orient: tween.orient,
            pins: Vec::new(),
            keep_area: tween.keep_area,
            color_effect,
            filters,
            classic_easing: Some(Tween { kind: TweenKind::Classic, ..tween.clone() }),
        }
        .sanitized()
    }

    /// Materializes this clip onto `base`, returning `(updated_base, motion_tween, span_duration)`.
    /// `target_duration` optionally stretches the motion curves from `self.duration` to the
    /// requested span length.
    pub fn apply_to_element(&self, base: &Element, target_duration: Option<u32>) -> (Element, MotionTween, u32) {
        let mut el = base.clone();
        if let Some(fx) = &self.color_effect
            && el.color == ColorEffect::None
        {
            el.color = *fx;
        }
        let mut filter_slot_offset: u8 = 0;
        if !self.filters.is_empty()
            && let Some(dst_filters) = el.kind.filters_mut()
        {
            if dst_filters.is_empty() {
                *dst_filters = self.filters.iter().take(usize::from(MAX_FILTER_SLOTS)).cloned().collect();
            } else {
                // Append missing filters if room remains so preset filter tracks resolve cleanly.
                let room = usize::from(MAX_FILTER_SLOTS).saturating_sub(dst_filters.len());
                if room > 0 {
                    filter_slot_offset = dst_filters.len() as u8;
                    dst_filters.extend(self.filters.iter().take(room).cloned());
                }
            }
        }

        let bp = Props::of(&el);
        let (bx, by) = (bp.get(Property::X), bp.get(Property::Y));
        let (bsx, bsy) = (bp.get(Property::ScaleX), bp.get(Property::ScaleY));
        let (brot, bskew) = (bp.get(Property::Rotation), bp.get(Property::Skew));

        let tracks: Vec<Track> = self
            .tracks
            .iter()
            .filter_map(|t| {
                let property = match t.property {
                    Property::Filter(fp) => {
                        let slot = fp.slot.saturating_add(filter_slot_offset);
                        if slot >= MAX_FILTER_SLOTS {
                            return None;
                        }
                        Property::Filter(FilterProp { slot, param: fp.param })
                    }
                    other => other,
                };
                let keys = t
                    .keys
                    .iter()
                    .map(|k| PropertyKey {
                        frame: k.frame,
                        value: from_relative(property, k.value, bx, by, bsx, bsy, brot, bskew),
                        ease: k.ease.clone(),
                    })
                    .collect();
                Some(Track { property, keys })
            })
            .collect();

        let path = self
            .path
            .as_ref()
            .and_then(|p| motion_path(&p.transformed(&Affine::translate(bx, by))));

        let mut motion = MotionTween {
            target: el.id,
            tracks,
            path,
            orient: self.orient,
            pins: self.pins.clone(),
            keep_area: self.keep_area,
        };
        motion.sanitize();

        let src_dur = self.duration.clamp(1, MAX_FRAME);
        let final_dur = target_duration.map_or(src_dur, |d| d.clamp(1, MAX_FRAME));
        if final_dur != src_dur && src_dur > 1 && final_dur > 1 {
            motion.stretch(src_dur, final_dur);
            motion.sanitize();
        }
        (el, motion, final_dur)
    }
}

#[allow(clippy::too_many_arguments)]
fn to_relative(p: Property, v: f32, bx: f32, by: f32, bsx: f32, bsy: f32, brot: f32, bskew: f32) -> f32 {
    match p {
        Property::X => v - bx,
        Property::Y => v - by,
        Property::ScaleX => if bsx.abs() > 1e-3 { (v / bsx) * 100.0 } else { v },
        Property::ScaleY => if bsy.abs() > 1e-3 { (v / bsy) * 100.0 } else { v },
        Property::Rotation => v - brot,
        Property::Skew => v - bskew,
        _ => v,
    }
}

#[allow(clippy::too_many_arguments)]
fn from_relative(p: Property, rel: f32, bx: f32, by: f32, bsx: f32, bsy: f32, brot: f32, bskew: f32) -> f32 {
    let raw = match p {
        Property::X => bx + rel,
        Property::Y => by + rel,
        Property::ScaleX => bsx * (rel / 100.0),
        Property::ScaleY => bsy * (rel / 100.0),
        Property::Rotation => brot + rel,
        Property::Skew => bskew + rel,
        _ => rel,
    };
    let r = p.range();
    if raw.is_finite() { raw.clamp(*r.start(), *r.end()) } else { 0.0f32.clamp(*r.start(), *r.end()) }
}

fn key(frame: u32, value: f32, ease: Easing) -> PropertyKey {
    PropertyKey { frame, value, ease }
}

fn track(property: Property, keys: Vec<PropertyKey>) -> Track {
    Track { property, keys }
}

/// Returns the 6 built-in studio motion presets.
pub fn builtin_motion_presets() -> Vec<BuiltinMotionPreset> {
    vec![
        BuiltinMotionPreset {
            id: "anime_snap_in",
            label: "Anime Snap In",
            description: "High-velocity horizontal snap entrance with squash-and-stretch settle",
            clip: MotionClip {
                duration: 14,
                tracks: vec![
                    track(Property::X, vec![key(0, -160.0, Easing::QuintOut), key(9, 6.0, Easing::QuadOut), key(13, 0.0, Easing::Linear)]),
                    track(Property::ScaleX, vec![key(0, 128.0, Easing::CubicOut), key(9, 94.0, Easing::SineInOut), key(13, 100.0, Easing::Linear)]),
                    track(Property::ScaleY, vec![key(0, 80.0, Easing::CubicOut), key(9, 106.0, Easing::SineInOut), key(13, 100.0, Easing::Linear)]),
                    track(Property::Alpha, vec![key(0, 0.0, Easing::QuadOut), key(4, 100.0, Easing::Linear)]),
                ],
                path: None,
                orient: false,
                pins: Vec::new(),
                keep_area: false,
                color_effect: None,
                filters: Vec::new(),
                classic_easing: None,
            }
            .sanitized(),
        },
        BuiltinMotionPreset {
            id: "bounce_drop",
            label: "Bounce Drop",
            description: "Gravity drop from above with multi-bounce settle and impact squash",
            clip: MotionClip {
                duration: 24,
                tracks: vec![
                    track(Property::Y, vec![key(0, -220.0, Easing::BounceOut), key(23, 0.0, Easing::Linear)]),
                    track(
                        Property::ScaleX,
                        vec![
                            key(0, 92.0, Easing::QuadIn),
                            key(8, 122.0, Easing::QuadOut),
                            key(14, 96.0, Easing::SineInOut),
                            key(23, 100.0, Easing::Linear),
                        ],
                    ),
                    track(
                        Property::ScaleY,
                        vec![
                            key(0, 110.0, Easing::QuadIn),
                            key(8, 80.0, Easing::QuadOut),
                            key(14, 104.0, Easing::SineInOut),
                            key(23, 100.0, Easing::Linear),
                        ],
                    ),
                    track(Property::Alpha, vec![key(0, 0.0, Easing::QuadOut), key(3, 100.0, Easing::Linear)]),
                ],
                path: None,
                orient: false,
                pins: Vec::new(),
                keep_area: false,
                color_effect: None,
                filters: Vec::new(),
                classic_easing: None,
            }
            .sanitized(),
        },
        BuiltinMotionPreset {
            id: "elastic_pop",
            label: "Elastic Pop",
            description: "Springy scale pop-in from centre with playful rotational settle",
            clip: MotionClip {
                duration: 20,
                tracks: vec![
                    track(Property::ScaleX, vec![key(0, 15.0, Easing::ElasticOut), key(19, 100.0, Easing::Linear)]),
                    track(Property::ScaleY, vec![key(0, 15.0, Easing::ElasticOut), key(19, 100.0, Easing::Linear)]),
                    track(Property::Rotation, vec![key(0, -8.0, Easing::ElasticOut), key(19, 0.0, Easing::Linear)]),
                    track(Property::Alpha, vec![key(0, 0.0, Easing::QuadOut), key(4, 100.0, Easing::Linear)]),
                ],
                path: None,
                orient: false,
                pins: Vec::new(),
                keep_area: false,
                color_effect: None,
                filters: Vec::new(),
                classic_easing: None,
            }
            .sanitized(),
        },
        BuiltinMotionPreset {
            id: "handheld_shake",
            label: "Handheld Shake",
            description: "Organic handheld camera/object jitter returning cleanly to rest",
            clip: MotionClip {
                duration: 24,
                tracks: vec![
                    track(
                        Property::X,
                        vec![
                            key(0, 0.0, Easing::SineInOut),
                            key(3, -6.0, Easing::SineInOut),
                            key(6, 5.0, Easing::SineInOut),
                            key(9, -4.0, Easing::SineInOut),
                            key(12, 6.0, Easing::SineInOut),
                            key(15, -3.0, Easing::SineInOut),
                            key(18, 4.0, Easing::SineInOut),
                            key(21, -2.0, Easing::SineInOut),
                            key(23, 0.0, Easing::Linear),
                        ],
                    ),
                    track(
                        Property::Y,
                        vec![
                            key(0, 0.0, Easing::SineInOut),
                            key(3, 4.0, Easing::SineInOut),
                            key(6, -5.0, Easing::SineInOut),
                            key(9, 3.0, Easing::SineInOut),
                            key(12, -4.0, Easing::SineInOut),
                            key(15, 5.0, Easing::SineInOut),
                            key(18, -3.0, Easing::SineInOut),
                            key(21, 2.0, Easing::SineInOut),
                            key(23, 0.0, Easing::Linear),
                        ],
                    ),
                    track(
                        Property::Rotation,
                        vec![
                            key(0, 0.0, Easing::SineInOut),
                            key(4, -1.8, Easing::SineInOut),
                            key(8, 1.5, Easing::SineInOut),
                            key(12, -1.4, Easing::SineInOut),
                            key(16, 1.2, Easing::SineInOut),
                            key(20, -0.7, Easing::SineInOut),
                            key(23, 0.0, Easing::Linear),
                        ],
                    ),
                ],
                path: None,
                orient: false,
                pins: Vec::new(),
                keep_area: false,
                color_effect: None,
                filters: Vec::new(),
                classic_easing: None,
            }
            .sanitized(),
        },
        BuiltinMotionPreset {
            id: "pulse_glow",
            label: "Pulse Glow",
            description: "Rhythmic luminous scale, brightness and outer glow pulse",
            clip: MotionClip {
                duration: 24,
                tracks: vec![
                    track(Property::ScaleX, vec![key(0, 100.0, Easing::SineInOut), key(12, 108.0, Easing::SineInOut), key(23, 100.0, Easing::Linear)]),
                    track(Property::ScaleY, vec![key(0, 100.0, Easing::SineInOut), key(12, 108.0, Easing::SineInOut), key(23, 100.0, Easing::Linear)]),
                    track(Property::Brightness, vec![key(0, 0.0, Easing::SineInOut), key(12, 32.0, Easing::SineInOut), key(23, 0.0, Easing::Linear)]),
                    track(
                        Property::Filter(FilterProp { slot: 0, param: FilterParam::BlurX }),
                        vec![key(0, 6.0, Easing::SineInOut), key(12, 22.0, Easing::SineInOut), key(23, 6.0, Easing::Linear)],
                    ),
                    track(
                        Property::Filter(FilterProp { slot: 0, param: FilterParam::BlurY }),
                        vec![key(0, 6.0, Easing::SineInOut), key(12, 22.0, Easing::SineInOut), key(23, 6.0, Easing::Linear)],
                    ),
                    track(
                        Property::Filter(FilterProp { slot: 0, param: FilterParam::Strength }),
                        vec![key(0, 80.0, Easing::SineInOut), key(12, 220.0, Easing::SineInOut), key(23, 80.0, Easing::Linear)],
                    ),
                ],
                path: None,
                orient: false,
                pins: Vec::new(),
                keep_area: false,
                color_effect: None,
                filters: vec![FilterEntry {
                    filter: Filter::Glow {
                        blur_x: 6.0,
                        blur_y: 6.0,
                        strength: 0.8,
                        quality: 2,
                        color: Rgba::hex(0x4fc3f7),
                        inner: false,
                        knockout: false,
                    },
                    enabled: true,
                }],
                classic_easing: None,
            }
            .sanitized(),
        },
        BuiltinMotionPreset {
            id: "slide_overshoot",
            label: "Slide Overshoot",
            description: "Smooth horizontal slide-in with BackOut overshoot and follow-through skew",
            clip: MotionClip {
                duration: 18,
                tracks: vec![
                    track(Property::X, vec![key(0, -200.0, Easing::BackOut), key(17, 0.0, Easing::Linear)]),
                    track(
                        Property::Skew,
                        vec![
                            key(0, -12.0, Easing::CubicOut),
                            key(12, 3.0, Easing::SineInOut),
                            key(17, 0.0, Easing::Linear),
                        ],
                    ),
                    track(Property::Alpha, vec![key(0, 0.0, Easing::QuadOut), key(5, 100.0, Easing::Linear)]),
                ],
                path: None,
                orient: false,
                pins: Vec::new(),
                keep_area: false,
                color_effect: None,
                filters: Vec::new(),
                classic_easing: None,
            }
            .sanitized(),
        },
    ]
}

/// Finds a built-in motion preset by ID or display label (case-insensitive, spaces/hyphens forgiving).
pub fn find_builtin_preset(query: &str) -> Option<BuiltinMotionPreset> {
    let norm = query.trim().to_ascii_lowercase().replace([' ', '-'], "_");
    builtin_motion_presets()
        .into_iter()
        .find(|p| p.id == norm || p.label.eq_ignore_ascii_case(query.trim()))
}

/// Sanitizes a loaded document's user motion presets: valid names, no built-in collisions or
/// case-insensitive duplicates, canonical clips, capped at [`MAX_MOTION_PRESETS`].
pub fn sanitized_motion_presets(list: &[MotionPreset]) -> Vec<MotionPreset> {
    let mut out: Vec<MotionPreset> = Vec::new();
    for item in list {
        if out.len() >= MAX_MOTION_PRESETS {
            break;
        }
        let Some(name) = preset_name(&item.name) else { continue };
        if find_builtin_preset(&name).is_some() {
            continue;
        }
        if out.iter().any(|p| p.name.to_lowercase() == name.to_lowercase()) {
            continue;
        }
        out.push(MotionPreset { name, clip: item.clip.clone().sanitized() });
    }
    out
}
