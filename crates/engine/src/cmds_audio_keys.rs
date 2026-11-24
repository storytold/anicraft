//! Convert Audio to Keyframes and auto lip-sync (ADR 0114): a sound on the timeline drives an
//! object on another layer.
//!
//! * `sound.to_keyframes` maps the sound's loudness on each frame (gated and smoothed) onto a
//!   property of an object: scale, alpha, a position or rotation offset, or a graphic
//!   symbol's first frame. It writes keyframes, or a motion tween's property curve.
//! * `sound.lip_sync` sorts each frame of speech into a small set of mouth shapes and writes
//!   keyframes that show the matching frame of a graphic symbol of mouth poses.
//! * `sound.analyse` reports what both would work from, without changing anything.
//!
//! The analysis is `anicraft_audio::analysis` (pure, bounded); this module finds the sound and
//! the object, and writes the keys. Each command is one undo step.

use crate::params::*;
use crate::{CommandSpec, EngineError, MAX_FRAME, Result, Session};
use anicraft_doc::audio::analysis::{self, Channel, LipOptions, MAX_ANALYSIS_FRAMES, MAX_HOLD, MAX_SMOOTHING, Span, Viseme, Viseme12};
use anicraft_doc::motion::MAX_KEYS;
use anicraft_doc::sound::frame_start;
use anicraft_doc::{Element, ElementId, ElementKind, FrameSound, Layer, LayerId, LayerKind, LoopMode, Property, Sound, SoundSync, Symbol, SymbolKind, own_value};
use anicraft_geom::{Affine, Vec2};
use serde_json::{Value, json};
use std::sync::Arc;

/// What the dialogs start from.
pub const DEFAULT_THRESHOLD: f32 = 0.05;
pub const DEFAULT_SMOOTHING: u32 = 1;
/// Most frames between keys.
pub const MAX_STEP: u32 = 240;

pub fn specs() -> Vec<CommandSpec> {
    let c = |id, label, menu, params, mutates, run| CommandSpec { id, label, menu, shortcut: None, params, mutates, run };
    vec![
        c(
            "sound.to_keyframes",
            "Convert Audio to Keyframes…",
            "Modify/Timeline",
            "{property: scale|scale_x|scale_y|alpha|x|y|rotation|first_frame, min, max (the property's value in silence and at the loudest frame: scale % of the object's size, alpha %, x and y pixels from where it is, rotation degrees from how it is turned, first_frame a 0-based frame of the graphic symbol), layer? (the layer that gets the keys; default the active layer), element? (the object on it; default the selected one, or the only one), sound_layer?, sound_frame? (the sound's keyframe; default the sound at the playhead), threshold? (0–0.99 of the loudest frame: quieter is silence; default 0.05), smoothing? (0–24 frames either side; default 1), channel?: both|left|right, step? (a key every n frames, 1–240; default 1), output?: auto|keyframes|motion (motion writes the property's curve in the motion tween on the span; auto picks it when the span has one), from?, to? (frames; default the frames the sound is heard on)} → {keys, from, to, output, property, layer, element}",
            true,
            to_keyframes as crate::RunFn,
        ),
        c(
            "sound.lip_sync",
            "Auto Lip-Sync…",
            "Modify/Timeline",
            "{layer? (the layer with the mouth; default the active layer), element? (a graphic symbol instance of mouth poses on it; default the selected one, or the only one), mouths?: {rest, open, wide, round, closed} or 12-viseme keys {neutral, ah, d, ee, f, l, m, oh, r, s, uh, woo}, mouths_12?: {neutral?, ah?, d?, ee?, f?, l?, m?, oh?, r?, s?, uh?, woo?} | true, mode?: visemes|visemes_12, sound_layer?, sound_frame? (the speech; default the sound at the playhead), threshold? (0–0.99 of the loudest frame: quieter is silence; default 0.08), hold? (fewest frames a mouth is held, 1–12; default 2), from?, to? (frames; default the frames the sound is heard on)} → {keys, from, to, layer, element, mouths, counts}",
            true,
            lip_sync,
        ),
        c(
            "sound.analyse",
            "Analyse Sound",
            "",
            "{what?: levels|visemes|visemes_12 (default levels), sound_layer?, sound_frame?, from?, to?, threshold?, smoothing?, channel? (levels), hold? (visemes)} → {sound, name, layer, keyframe, from, to, fps, levels (each frame's loudness, 0–1 of the loudest) and values (gated and smoothed), visemes (rest|open|wide|round|closed per frame), or visemes_12 (neutral|ah|d|ee|f|l|m|oh|r|s|uh|woo per frame)}",
            false,
            analyse,
        ),
    ]
}

/// A property Convert Audio to Keyframes drives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KeyProperty {
    /// Both scales together, percent of the object's own size.
    Scale,
    ScaleX,
    ScaleY,
    /// Opacity, percent.
    Alpha,
    /// Pixels from where the object is.
    X,
    Y,
    /// Degrees from how the object is turned.
    Rotation,
    /// The frame a graphic symbol instance shows (0-based), held as a single frame.
    FirstFrame,
}

impl KeyProperty {
    pub const ALL: [KeyProperty; 8] = [KeyProperty::Scale, KeyProperty::ScaleX, KeyProperty::ScaleY, KeyProperty::Alpha, KeyProperty::X, KeyProperty::Y, KeyProperty::Rotation, KeyProperty::FirstFrame];

    pub fn key(self) -> &'static str {
        match self {
            KeyProperty::Scale => "scale",
            KeyProperty::ScaleX => "scale_x",
            KeyProperty::ScaleY => "scale_y",
            KeyProperty::Alpha => "alpha",
            KeyProperty::X => "x",
            KeyProperty::Y => "y",
            KeyProperty::Rotation => "rotation",
            KeyProperty::FirstFrame => "first_frame",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            KeyProperty::Scale => "Scale",
            KeyProperty::ScaleX => "Scale X",
            KeyProperty::ScaleY => "Scale Y",
            KeyProperty::Alpha => "Alpha",
            KeyProperty::X => "X offset",
            KeyProperty::Y => "Y offset",
            KeyProperty::Rotation => "Rotation",
            KeyProperty::FirstFrame => "First frame",
        }
    }

    pub fn unit(self) -> &'static str {
        match self {
            KeyProperty::Scale | KeyProperty::ScaleX | KeyProperty::ScaleY | KeyProperty::Alpha => "%",
            KeyProperty::X | KeyProperty::Y => "px",
            KeyProperty::Rotation => "°",
            KeyProperty::FirstFrame => "",
        }
    }

    pub fn from_key(s: &str) -> Option<KeyProperty> {
        let s = s.trim().to_ascii_lowercase().replace([' ', '-'], "_");
        let s = match s.as_str() {
            "opacity" => "alpha",
            "scalex" => "scale_x",
            "scaley" => "scale_y",
            "rotate" => "rotation",
            "frame" | "firstframe" => "first_frame",
            other => other,
        };
        KeyProperty::ALL.into_iter().find(|p| p.key() == s)
    }

    /// The values `min` and `max` may take.
    pub fn range(self) -> std::ops::RangeInclusive<f32> {
        match self {
            KeyProperty::Scale | KeyProperty::ScaleX | KeyProperty::ScaleY => 1.0..=10_000.0,
            KeyProperty::Alpha => 0.0..=100.0,
            KeyProperty::X | KeyProperty::Y => -100_000.0..=100_000.0,
            KeyProperty::Rotation => -3_600.0..=3_600.0,
            KeyProperty::FirstFrame => 0.0..=MAX_FRAME as f32,
        }
    }

    /// What the dialog offers first: (in silence, at the loudest).
    pub fn defaults(self) -> (f32, f32) {
        match self {
            KeyProperty::Scale | KeyProperty::ScaleX | KeyProperty::ScaleY => (100.0, 150.0),
            KeyProperty::Alpha => (20.0, 100.0),
            KeyProperty::X => (0.0, 40.0),
            KeyProperty::Y => (0.0, -40.0),
            KeyProperty::Rotation => (0.0, 20.0),
            KeyProperty::FirstFrame => (0.0, 1.0),
        }
    }

    /// The motion tween tracks it writes (none for a symbol's first frame).
    fn tracks(self) -> &'static [Property] {
        match self {
            KeyProperty::Scale => &[Property::ScaleX, Property::ScaleY],
            KeyProperty::ScaleX => &[Property::ScaleX],
            KeyProperty::ScaleY => &[Property::ScaleY],
            KeyProperty::Alpha => &[Property::Alpha],
            KeyProperty::X => &[Property::X],
            KeyProperty::Y => &[Property::Y],
            KeyProperty::Rotation => &[Property::Rotation],
            KeyProperty::FirstFrame => &[],
        }
    }

    /// A track's value for the property's value `v`, on an object whose own value is `own`.
    fn track_value(self, own: f32, v: f32) -> f32 {
        match self {
            KeyProperty::Scale | KeyProperty::ScaleX | KeyProperty::ScaleY => own * v / 100.0,
            KeyProperty::Alpha | KeyProperty::FirstFrame => v,
            KeyProperty::X | KeyProperty::Y | KeyProperty::Rotation => own + v,
        }
    }
}

/// How the keys are written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    /// Keyframes on the layer, each holding the object with the value.
    Keyframes,
    /// Property keys in the motion tween on the span.
    Motion,
}

impl Output {
    pub fn key(self) -> &'static str {
        match self {
            Output::Keyframes => "keyframes",
            Output::Motion => "motion",
        }
    }
}

/// The sound a command reads: where it is, and the frames it is heard on.
#[derive(Clone, Debug)]
pub struct Source {
    pub layer: LayerId,
    /// Its keyframe's first frame.
    pub keyframe: u32,
    pub sound: Arc<Sound>,
    /// The frames analysed, first and last.
    pub from: u32,
    pub to: u32,
    span: Span,
    fs: FrameSound,
}

impl Source {
    /// Frames analysed.
    pub fn frames(&self) -> u32 {
        self.to - self.from + 1
    }
}

fn sounding(k: &anicraft_doc::Keyframe) -> Option<&FrameSound> {
    k.sound.as_ref().filter(|f| f.sync != SoundSync::Stop)
}

/// The keyframe with the sound to read, as `(layer, keyframe start)`: on `sound_layer` when
/// given, else the first layer sounding at the frame (a stream sound before the others), else
/// the timeline's first sound.
fn find_sound(s: &Session, p: &Value) -> Result<(LayerId, u32)> {
    let frame = opt_u32(p, "sound_frame")?.unwrap_or(s.frame).min(MAX_FRAME);
    let tl = s.timeline();
    // On a layer: the sound whose keyframe holds the frame, else the last one before it, else
    // its first.
    let on = |l: &Layer| {
        let at = l.keyframe_at(frame).filter(|k| sounding(k).is_some());
        let before = || l.keyframes.iter().rev().find(|k| k.start <= frame && sounding(k).is_some()).map(|k| &**k);
        let first = || l.keyframes.iter().find(|k| sounding(k).is_some()).map(|k| &**k);
        at.or_else(before).or_else(first).map(|k| (l.id, k.start))
    };
    if let Some(id) = opt_u64(p, "sound_layer")?.map(LayerId) {
        let l = tl.layer(id).ok_or_else(|| EngineError::State(format!("no layer with id {}", id.0)))?;
        return on(l).ok_or_else(|| EngineError::State(format!("layer '{}' has no sound on its keyframes", l.name)));
    }
    let here = |stream: bool| tl.layers.iter().find_map(|l| l.keyframe_at(frame).filter(|k| sounding(k).is_some_and(|f| !stream || f.sync == SoundSync::Stream)).map(|k| (l.id, k.start)));
    here(true)
        .or_else(|| here(false))
        .or_else(|| tl.layers.iter().find_map(|l| on(l)))
        .ok_or_else(|| EngineError::State("there is no sound on this timeline: import one (File › Import) or put one on a keyframe first".into()))
}

/// The sound the params name and the frames to read (`from`, `to`; default the frames it is
/// heard on from its keyframe, within the timeline).
pub fn source(s: &Session, p: &Value) -> Result<Source> {
    let (layer, keyframe) = find_sound(s, p)?;
    let tl = s.timeline();
    let k = tl.layer(layer).and_then(|l| l.keyframe_starting_at(keyframe).and_then(|i| l.keyframes.get(i))).ok_or_else(|| EngineError::State("the sound's keyframe went away".into()))?;
    let fs = sounding(k).ok_or_else(|| EngineError::State("the keyframe has no sound".into()))?;
    let sound = s.doc.sounds.iter().find(|x| x.id == fs.sound).cloned().ok_or_else(|| EngineError::State("the keyframe's sound is no longer in the library".into()))?;
    let fps = s.doc.stage.fps;
    let rate = sound.rate.max(1);
    // The timeline's last frame, at least the sound keyframe's own.
    let last = tl.frame_count().max(k.end()).saturating_sub(1).min(MAX_FRAME);
    let rest = u64::from(last - keyframe.min(last) + 1);
    let heard = fs.heard(sound.frames, frame_start(u64::from(k.duration), fps, rate), frame_start(rest, fps, rate));
    // Movie frames it is heard on (the last one may be partly silent).
    let per = (f64::from(rate) / f64::from(if fps.is_finite() && fps > 0.0 { fps } else { 24.0 })).max(1e-9);
    let heard_frames = ((heard as f64 / per).ceil() as u64).clamp(1, rest) as u32;
    let from = opt_u32(p, "from")?.unwrap_or(keyframe);
    let to = opt_u32(p, "to")?.unwrap_or(keyframe + heard_frames - 1);
    if from < keyframe {
        return Err(EngineError::Param(format!("'from' must not be before the sound's keyframe (frame {keyframe})")));
    }
    if to < from {
        return Err(EngineError::Param("'to' must not be before 'from'".into()));
    }
    if to > MAX_FRAME {
        return Err(EngineError::Param(format!("'to' must be at most {MAX_FRAME}")));
    }
    if (to - from) as usize >= MAX_ANALYSIS_FRAMES {
        return Err(EngineError::Param(format!("at most {MAX_ANALYSIS_FRAMES} frames are analysed at once")));
    }
    // A loop has no end of its own: it sounds for as long as its span (or the movie) lets it.
    let span = Span { fps, offset: from - keyframe, frames: to - from + 1, heard: if fs.looping { u64::MAX } else { heard } };
    Ok(Source { layer, keyframe, sound, from, to, span, fs: fs.clone() })
}

fn pcm(src: &Source) -> Result<Arc<anicraft_doc::audio::Pcm>> {
    let dec = src.sound.decoded().ok_or_else(|| EngineError::State(format!("'{}' can't be decoded", src.sound.name)))?;
    anicraft_doc::sound::trimmed_pcm(&dec.pcm, &src.fs).ok_or_else(|| EngineError::State(format!("'{}' has an empty trim range", src.sound.name)))
}

/// How the loudness becomes a value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LevelSettings {
    pub threshold: f32,
    pub smoothing: u32,
    pub channel: Channel,
}

impl Default for LevelSettings {
    fn default() -> Self {
        LevelSettings { threshold: DEFAULT_THRESHOLD, smoothing: DEFAULT_SMOOTHING, channel: Channel::Both }
    }
}

fn threshold_param(p: &Value, default: f32) -> Result<f32> {
    let t = opt_f32(p, "threshold")?.unwrap_or(default);
    if !(0.0..=0.99).contains(&t) {
        return Err(EngineError::Param("param 'threshold' must be between 0 and 0.99 (a share of the loudest frame)".into()));
    }
    Ok(t)
}

fn level_settings(p: &Value) -> Result<LevelSettings> {
    let smoothing = opt_u32(p, "smoothing")?.unwrap_or(DEFAULT_SMOOTHING);
    if smoothing > MAX_SMOOTHING {
        return Err(EngineError::Param(format!("param 'smoothing' must be between 0 and {MAX_SMOOTHING} frames")));
    }
    let channel = match opt_str(p, "channel")? {
        None => Channel::Both,
        Some(c) => Channel::from_key(c).ok_or_else(|| EngineError::Param("param 'channel' must be both, left or right".into()))?,
    };
    Ok(LevelSettings { threshold: threshold_param(p, DEFAULT_THRESHOLD)?, smoothing, channel })
}

fn lip_options(p: &Value) -> Result<LipOptions> {
    let hold = opt_u32(p, "hold")?.unwrap_or(LipOptions::default().hold);
    if !(1..=MAX_HOLD).contains(&hold) {
        return Err(EngineError::Param(format!("param 'hold' must be between 1 and {MAX_HOLD} frames")));
    }
    Ok(LipOptions { threshold: threshold_param(p, LipOptions::default().threshold)?, hold })
}

/// A sound's loudness frame by frame, as Convert Audio to Keyframes reads it.
#[derive(Clone, Debug, PartialEq)]
pub struct Levels {
    /// The first frame analysed.
    pub from: u32,
    /// Each frame's loudness as a share of the loudest (`0..=1`).
    pub raw: Vec<f32>,
    /// The same gated by the threshold and smoothed: what is mapped onto the property.
    pub shaped: Vec<f32>,
}

/// The loudness of `src` under `set`.
pub fn levels(src: &Source, set: &LevelSettings) -> Result<Levels> {
    let raw = analysis::normalised(&analysis::levels(&*pcm(src)?, &src.span, set.channel));
    let shaped = analysis::shape(&raw, set.threshold, set.smoothing);
    Ok(Levels { from: src.from, raw, shaped })
}

/// The mouth shape of each frame of `src`.
pub fn visemes(src: &Source, opts: &LipOptions) -> Result<Vec<Viseme>> {
    Ok(analysis::visemes(&*pcm(src)?, &src.span, opts))
}

/// The 12-viseme studio mouth shape of each frame of `src`.
pub fn visemes_12(src: &Source, opts: &LipOptions) -> Result<Vec<Viseme12>> {
    Ok(analysis::visemes_12(&*pcm(src)?, &src.span, opts))
}

/// The frames that get a key when `values` (one per frame) are held between keys: every
/// `step`-th frame whose value differs from the last key's by more than `eps`, the first always.
pub fn held_keys(values: &[f32], step: u32, eps: f32) -> Vec<(u32, f32)> {
    let mut out: Vec<(u32, f32)> = Vec::new();
    for (i, &v) in values.iter().enumerate().step_by(step.max(1) as usize) {
        if out.last().is_none_or(|(_, last)| (v - last).abs() > eps) {
            out.push((i as u32, v));
        }
    }
    out
}

/// The frames that get a key when the value travels in straight lines between keys: every
/// `step`-th frame, less the insides of flat stretches (their two ends stay, so a flat stretch
/// stays flat).
pub fn curve_keys(values: &[f32], step: u32, eps: f32) -> Vec<(u32, f32)> {
    let sampled: Vec<(u32, f32)> = values.iter().enumerate().step_by(step.max(1) as usize).map(|(i, &v)| (i as u32, v)).collect();
    let mut out: Vec<(u32, f32)> = Vec::new();
    for (i, &(f, v)) in sampled.iter().enumerate() {
        let moved = out.last().is_none_or(|(_, last)| (v - last).abs() > eps);
        let turns = sampled.get(i + 1).is_none_or(|(_, next)| (next - v).abs() > eps);
        if moved || turns {
            out.push((f, v));
        }
    }
    out
}

/// Differences smaller than this share of the range don't make a key.
const KEY_EPS: f32 = 0.002;

/// The keys a run writes for shaped levels `shaped` (one per frame): each as (frames from the
/// first one, the property's value). What the dialog's preview marks.
pub fn planned_keys(prop: KeyProperty, min: f32, max: f32, step: u32, output: Output, shaped: &[f32]) -> Vec<(u32, f32)> {
    let values: Vec<f32> = shaped.iter().map(|l| min + (max - min) * l).map(|v| if prop == KeyProperty::FirstFrame { v.round() } else { v }).collect();
    let eps = ((max - min).abs() * KEY_EPS).max(1e-6);
    match output {
        Output::Keyframes => held_keys(&values, step, eps),
        Output::Motion => curve_keys(&values, step, eps),
    }
}

/// The layer and object that get the keys: `layer` (default the active one) and on it
/// `element`, else the selected object, else the only one on the keyframe at `frame`.
pub fn target(s: &Session, p: &Value, frame: u32) -> Result<(LayerId, Arc<Element>)> {
    let lid = opt_u64(p, "layer")?.map_or(s.layer, LayerId);
    let layer = s.timeline().layer(lid).ok_or_else(|| EngineError::State(format!("no layer with id {}", lid.0)))?;
    match layer.kind {
        LayerKind::Folder => return Err(EngineError::State(format!("'{}' is a folder; pick the layer with the object to animate", layer.name))),
        LayerKind::Armature => return Err(EngineError::State(format!("'{}' is an armature layer: its poses place its objects, so sound can't key them yet", layer.name))),
        LayerKind::Camera => return Err(EngineError::State("the camera can't be driven by sound yet; pick a layer with artwork".into())),
        _ => {}
    }
    // A layer that ends before the frame shows nothing there, but its object is the one meant:
    // the keys carry it on over the sound.
    let k = layer.keyframe_at(frame).or_else(|| layer.keyframes.iter().rev().find(|k| k.start <= frame).map(|k| &**k)).ok_or_else(|| EngineError::State(format!("layer '{}' is empty at frame {}: put the object to animate there first", layer.name, frame + 1)))?;
    if k.elements.is_empty() {
        return Err(EngineError::State(format!("layer '{}' has nothing at frame {} to animate", layer.name, frame + 1)));
    }
    let pick = |id: ElementId| k.elements.iter().find(|e| e.id == id).cloned();
    if let Some(id) = opt_u64(p, "element")?.map(ElementId) {
        return pick(id).map(|e| (lid, e)).ok_or_else(|| EngineError::State(format!("no object with id {} on layer '{}' at frame {}", id.0, layer.name, frame + 1)));
    }
    let selected: Vec<Arc<Element>> = s.selection.on(lid).into_iter().filter_map(pick).collect();
    match (selected.as_slice(), k.elements.as_slice()) {
        ([one], _) | ([], [one]) => Ok((lid, one.clone())),
        _ => Err(EngineError::State(format!("layer '{}' has {} objects at frame {}: select the one to animate, or pass 'element'", layer.name, k.elements.len(), frame + 1))),
    }
}

/// Whether the target layer has a motion tween anywhere on `from..=to`.
fn motion_in(layer: &Layer, from: u32, to: u32) -> bool {
    layer.keyframes.iter().any(|k| k.motion.is_some() && k.start <= to && k.end() > from)
}

/// Writes held keys for object `id` on `layer`: a keyframe at each of `keys`' frames (made as
/// Insert Keyframe makes it, holding what the frame showed), then every keyframe on
/// `from..=to` gets the object as `apply` makes it from what it was and the value held there.
/// The frame after `to`, when the layer goes on, keeps what it showed. How many keyframes hold
/// the object changed.
fn write_held<T>(layer: &mut Layer, id: ElementId, from: u32, to: u32, keys: &[(u32, T)], apply: &dyn Fn(&Element, &T) -> Element) -> Result<usize> {
    if keys.len() > MAX_KEYS {
        return Err(EngineError::Param(format!("that is {} keyframes, more than {MAX_KEYS} at once: raise 'step' or the threshold, or shorten the range with 'from' and 'to'", keys.len())));
    }
    if motion_in(layer, from, to) {
        return Err(EngineError::State(format!("layer '{}' has a motion tween on these frames: keyframes would cut it up. Use the motion tween's curve (output: motion), or remove the tween first", layer.name)));
    }
    // First every keyframe, while each frame still shows what it showed.
    let after = to.checked_add(1).filter(|f| *f < layer.end_frame());
    for f in keys.iter().map(|(f, _)| *f).chain(after) {
        if layer.keyframe_index_at(f).is_none_or(|i| layer.keyframes.get(i).is_some_and(|k| k.start != f)) {
            crate::cmds_timeline::insert_keyframe_at(layer, f, true)?;
        }
    }
    // Then the values: each keyframe in the range holds the value of the last key at or
    // before it (keyframes that were there already included).
    let first = layer.keyframes.partition_point(|k| k.start < from);
    let count = layer.keyframes.iter().skip(first).take_while(|k| k.start <= to).count();
    let mut changed = 0;
    for i in first..first + count {
        let Some(k) = layer.keyframe_mut(i) else { continue };
        let Some(value) = keys.partition_point(|(f, _)| *f <= k.start).checked_sub(1).and_then(|j| keys.get(j)).map(|(_, v)| v) else { continue };
        if let Some(slot) = k.elements.iter_mut().find(|e| e.id == id) {
            let next = apply(slot, value);
            if next != **slot {
                *slot = Arc::new(next);
            }
            changed += 1;
        }
    }
    // The object is there for the whole of the sound: a layer that ended sooner goes on to
    // the last frame keyed.
    if layer.end_frame() <= to
        && let Some(i) = layer.keyframes.len().checked_sub(1)
        && let Some(k) = layer.keyframe_mut(i)
    {
        k.duration = (to - k.start).saturating_add(1);
    }
    if changed == 0 {
        return Err(EngineError::State("the object isn't on the layer's keyframes in that range".into()));
    }
    Ok(changed)
}

/// The point of `el`'s own space that scale and rotation keys hold still: its transformation
/// point when it has one, a symbol's registration point, the middle of anything else.
fn anchor(s: &Session, el: &Element) -> Vec2 {
    if let Some(p) = el.pivot() {
        return p;
    }
    if matches!(el.kind, ElementKind::Symbol { .. }) {
        return Vec2::ZERO;
    }
    let b = el.local_bounds(&s.doc);
    let c = b.center();
    if b.is_empty() || !c.is_finite() { Vec2::ZERO } else { c }
}

/// `base` with `prop` at value `v` (see [`KeyProperty`]), scaled and turned about `anchor`.
fn with_value(base: &Element, prop: KeyProperty, v: f32, anchor: Vec2) -> Element {
    let mut el = base.clone();
    let about_local = |m: Affine| base.transform.mul(&Affine::translate(anchor.x, anchor.y)).mul(&m).mul(&Affine::translate(-anchor.x, -anchor.y));
    let scale = |pct: f32| {
        let k = pct / 100.0;
        if k.abs() < 1e-4 { 1e-4 } else { k }
    };
    let t = match prop {
        KeyProperty::Scale => about_local(Affine::scale(scale(v), scale(v))),
        KeyProperty::ScaleX => about_local(Affine::scale(scale(v), 1.0)),
        KeyProperty::ScaleY => about_local(Affine::scale(1.0, scale(v))),
        KeyProperty::X => Affine::translate(v, 0.0).mul(&base.transform),
        KeyProperty::Y => Affine::translate(0.0, v).mul(&base.transform),
        KeyProperty::Rotation => {
            let at = base.transform.apply(anchor);
            Affine::translate(at.x, at.y).mul(&Affine::rotate(v.to_radians())).mul(&Affine::translate(-at.x, -at.y)).mul(&base.transform)
        }
        KeyProperty::Alpha => {
            el.color = base.color.with_alpha((v / 100.0).clamp(0.0, 1.0)).sanitized();
            base.transform
        }
        KeyProperty::FirstFrame => {
            show_frame(&mut el, v.round().clamp(0.0, MAX_FRAME as f32) as u32);
            base.transform
        }
    };
    if t.is_finite() {
        el.transform = t;
    }
    el
}

/// Makes a symbol instance show `frame` of its symbol and hold it.
fn show_frame(el: &mut Element, frame: u32) {
    if let ElementKind::Symbol { loop_mode, first_frame, .. } = &mut el.kind {
        *loop_mode = LoopMode::SingleFrame;
        *first_frame = frame.min(MAX_FRAME);
    }
}

/// The graphic symbol `el` is an instance of.
fn graphic<'a>(s: &'a Session, el: &Element, what: &str) -> Result<&'a Symbol> {
    let sym = match &el.kind {
        ElementKind::Symbol { symbol, .. } => s.doc.symbol(*symbol),
        _ => None,
    };
    sym.filter(|x| x.kind == SymbolKind::Graphic).ok_or_else(|| EngineError::State(format!("{what} needs an instance of a graphic symbol (one frame per pose): convert the artwork with Modify › Convert to Symbol, type Graphic")))
}

/// The settings of a `sound.to_keyframes` run, read and checked.
struct Plan {
    prop: KeyProperty,
    min: f32,
    max: f32,
    step: u32,
    output: Option<Output>,
}

fn plan(p: &Value) -> Result<Plan> {
    let all = || KeyProperty::ALL.map(KeyProperty::key).join(", ");
    let prop = match opt_str(p, "property")? {
        Some(k) => KeyProperty::from_key(k).ok_or_else(|| EngineError::Param(format!("unknown property '{k}' ({})", all())))?,
        None => return Err(EngineError::Param(format!("param 'property' is required ({})", all()))),
    };
    let r = prop.range();
    let bound = |key: &str| -> Result<f32> {
        let v = f32_req(p, key)?;
        if !r.contains(&v) {
            return Err(EngineError::Param(format!("param '{key}' must be between {} and {} for {}", r.start(), r.end(), prop.key())));
        }
        Ok(v)
    };
    let step = opt_u32(p, "step")?.unwrap_or(1);
    if !(1..=MAX_STEP).contains(&step) {
        return Err(EngineError::Param(format!("param 'step' must be between 1 and {MAX_STEP} frames")));
    }
    let output = match opt_str(p, "output")? {
        None | Some("auto") => None,
        Some("keyframes") => Some(Output::Keyframes),
        Some("motion") => Some(Output::Motion),
        Some(other) => return Err(EngineError::Param(format!("unknown output '{other}' (auto, keyframes or motion)"))),
    };
    Ok(Plan { prop, min: bound("min")?, max: bound("max")?, step, output })
}

/// Convert Audio to Keyframes.
fn to_keyframes(s: &mut Session, p: &Value) -> Result<Value> {
    let plan = plan(p)?;
    let src = source(s, p)?;
    let set = level_settings(p)?;
    let (lid, el) = target(s, p, src.from)?;
    if plan.prop == KeyProperty::FirstFrame {
        let frames = graphic(s, &el, "First frame")?.timeline.frame_count().max(1);
        if plan.min >= frames as f32 || plan.max >= frames as f32 {
            return Err(EngineError::Param(format!("the symbol has {frames} frames: 'min' and 'max' must be between 0 and {}", frames - 1)));
        }
    }
    let lv = levels(&src, &set)?;
    let layer = s.timeline().layer(lid).ok_or_else(|| EngineError::State("the layer went away".into()))?;
    let in_motion = layer.keyframe_at(src.from).is_some_and(|k| k.motion.is_some());
    let output = plan.output.unwrap_or(if in_motion && plan.prop != KeyProperty::FirstFrame { Output::Motion } else { Output::Keyframes });
    let (keys, to) = match output {
        Output::Keyframes => {
            let keys: Vec<(u32, f32)> = planned_keys(plan.prop, plan.min, plan.max, plan.step, output, &lv.shaped).into_iter().map(|(i, v)| (src.from + i, v)).collect();
            let at = anchor(s, &el);
            let prop = plan.prop;
            let layer = s.layer_mut(Some(lid))?;
            write_held(layer, el.id, src.from, src.to, &keys, &|base, v| with_value(base, prop, *v, at))?;
            (keys.len(), src.to)
        }
        Output::Motion => write_curve(s, lid, &el, &plan, &src, &lv.shaped)?,
    };
    Ok(json!({"keys": keys, "from": src.from, "to": to, "output": output.key(), "property": plan.prop.key(), "layer": lid.0, "element": el.id.0, "sound": src.sound.id.0}))
}

/// Writes shaped levels (one per frame from `src.from`) as the property's keys in the motion tween
/// on the span at `src.from`, making the tween when the span has none. The range is cut to the
/// span. The keys written (per track) and the last frame they reach.
fn write_curve(s: &mut Session, lid: LayerId, el: &Element, plan: &Plan, src: &Source, shaped: &[f32]) -> Result<(usize, u32)> {
    let tracks = plan.prop.tracks();
    if tracks.is_empty() {
        return Err(EngineError::Param("a symbol's first frame has no motion tween curve: use output: keyframes".into()));
    }
    let has = s.timeline().layer(lid).and_then(|l| l.keyframe_at(src.from)).is_some_and(|k| k.motion.is_some());
    if !has {
        let alone = s.timeline().layer(lid).and_then(|l| l.keyframe_at(src.from)).is_some_and(|k| k.elements.len() == 1);
        if !alone {
            return Err(EngineError::State("a motion tween moves one object: put the object on a layer of its own (or Create Motion Tween on it) first, or use output: keyframes".into()));
        }
        crate::cmds_motion::create(s, &json!({"layer": lid.0, "frame": src.from}))?;
    }
    let layer = s.layer_mut(Some(lid))?;
    let i = layer.keyframe_index_at(src.from).ok_or_else(|| EngineError::State("the span went away".into()))?;
    let kf = layer.keyframe_mut(i).ok_or_else(|| EngineError::State("the span went away".into()))?;
    let (start, last) = (kf.start, kf.duration.saturating_sub(1));
    let target = kf.motion.as_ref().and_then(|m| m.target_index(&kf.elements)).and_then(|t| kf.elements.get(t)).cloned().ok_or_else(|| EngineError::State("the motion tween's object is no longer on its keyframe".into()))?;
    if target.id != el.id {
        return Err(EngineError::State("the motion tween on this span moves another object: pick that one, or use output: keyframes on a layer without a motion tween".into()));
    }
    let m = kf.motion.as_mut().ok_or_else(|| EngineError::State("no motion tween here".into()))?;
    if m.path.is_some() && tracks.iter().any(|t| matches!(t, Property::X | Property::Y)) {
        return Err(EngineError::State("the motion tween follows a motion path, which places the object: clear the path first, or drive another property".into()));
    }
    // The range, cut to the span.
    let a = src.from - start;
    let b = (src.to - start).min(last);
    let len = (b - a + 1) as usize;
    let keys = planned_keys(plan.prop, plan.min, plan.max, plan.step, Output::Motion, shaped.get(..len).unwrap_or(shaped));
    if keys.len() > MAX_KEYS {
        return Err(EngineError::Param(format!("that is {} keys, more than {MAX_KEYS} on one curve: raise 'step' or the threshold, or shorten the range with 'from' and 'to'", keys.len())));
    }
    for &prop in tracks {
        let own = own_value(&target, prop).ok_or_else(|| EngineError::State("the motion tween's object has no such property".into()))?;
        let r = prop.range();
        // The curve's keys in the range give way to the new ones; what follows the range
        // starts from the object's own value again unless it has keys of its own there.
        let later = m.track(prop).is_some_and(|t| t.keys.iter().any(|k| k.frame > b));
        if let Some(t) = m.track_mut(prop) {
            t.keys.retain(|k| k.frame < a || k.frame > b);
        }
        for &(f, v) in &keys {
            m.set_key(prop, a + f, plan.prop.track_value(own, v).clamp(*r.start(), *r.end()), own);
        }
        if !later && b < last {
            m.set_key(prop, b + 1, own, own);
        }
    }
    Ok((keys.len(), start + b))
}

/// The names a mouth shape's frame goes by: the Lip-sync mouth chart template's labels first
/// (ADR 0065), then other common ones. Compared without case, spaces or punctuation.
fn mouth_names(v: Viseme) -> &'static [&'static str] {
    match v {
        Viseme::Rest => &["rest", "neutral", "silence", "sil", "idle", "x"],
        Viseme::Open => &["ai", "a", "ah", "aa", "open"],
        Viseme::Wide => &["e", "ee", "wide", "i", "cdgknrsthyz", "smile", "etc"],
        Viseme::Round => &["o", "oh", "round", "u", "oo", "wq", "w"],
        Viseme::Closed => &["mbp", "m", "closed", "bmp", "pbm", "b", "p"],
    }
}

fn plain(label: &str) -> String {
    label.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// The frame of `sym` labelled `label` (without case, spaces or punctuation), the first when
/// several are.
pub fn labelled_frame(sym: &Symbol, label: &str) -> Option<u32> {
    let want = plain(label);
    if want.is_empty() {
        return None;
    }
    sym.timeline.layers.iter().flat_map(|l| l.keyframes.iter()).filter(|k| plain(&k.label) == want).map(|k| k.start).min()
}

/// The frame of `sym` each mouth shape shows by default, in [`Viseme::ALL`]'s order: by the
/// symbol's frame labels; a shape without one borrows the nearest shape's frame (lips together
/// and rest stand in for each other, round and wide borrow open's), and a symbol without any
/// of the labels gives its first five frames in order.
pub fn default_mouths(sym: &Symbol) -> [u32; 5] {
    let last = sym.timeline.frame_count().max(1) - 1;
    let found = Viseme::ALL.map(|v| mouth_names(v).iter().find_map(|n| labelled_frame(sym, n)));
    if found.iter().all(Option::is_none) {
        return [0, 1, 2, 3, 4].map(|i: u32| i.min(last));
    }
    let [rest, open, wide, round, closed] = found;
    let rest_f = rest.or(closed).unwrap_or(0);
    let open_f = open.or(wide).or(round).unwrap_or(rest_f);
    [rest_f, open_f, wide.unwrap_or(open_f), round.unwrap_or(open_f), closed.or(rest).unwrap_or(rest_f)].map(|f| f.min(last))
}

/// Auto-matched frame label candidates for each of the 12 studio visemes (`Viseme12`).
fn mouth_names_12(v: Viseme12) -> &'static [&'static str] {
    match v {
        Viseme12::Neutral => &["neutral", "rest", "silence", "sil", "idle", "x"],
        Viseme12::Ah => &["ah", "ai", "a", "aa", "open"],
        Viseme12::D => &["d", "cdgknrsthyz", "dgknt", "t", "cons", "etc"],
        Viseme12::Ee => &["ee", "e", "i", "wide", "smile"],
        Viseme12::F => &["f", "fv", "v"],
        Viseme12::L => &["l", "th"],
        Viseme12::M => &["m", "mbp", "closed", "bmp", "pbm", "b", "p"],
        Viseme12::Oh => &["oh", "o", "round"],
        Viseme12::R => &["r", "er", "wq"],
        Viseme12::S => &["s", "cdgknrsthyz", "sz", "sh", "ch"],
        Viseme12::Uh => &["uh", "u", "schwa"],
        Viseme12::Woo => &["woo", "wq", "oo", "w", "q"],
    }
}

/// Default symbol frame indices for the 12 studio visemes, in [`Viseme12::ALL`] order.
pub fn default_mouths_12(sym: &Symbol) -> [u32; 12] {
    let last = sym.timeline.frame_count().max(1) - 1;
    let found = Viseme12::ALL.map(|v| mouth_names_12(v).iter().find_map(|n| labelled_frame(sym, n)));
    if found.iter().all(Option::is_none) {
        return [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11].map(|i: u32| i.min(last));
    }
    let [neutral, ah, d, ee, f, l, m, oh, r, s, uh, woo] = found;
    let neutral_f = neutral.or(m).unwrap_or(0);
    let m_f = m.or(neutral).unwrap_or(neutral_f);
    let ah_f = ah.or(uh).or(ee).or(oh).unwrap_or(neutral_f);
    let ee_f = ee.or(d).or(s).unwrap_or(ah_f);
    let oh_f = oh.or(woo).or(uh).unwrap_or(ah_f);
    let uh_f = uh.or(ah).or(oh).unwrap_or(ah_f);
    let woo_f = woo.or(oh).unwrap_or(oh_f);
    let d_f = d.or(s).or(ee).unwrap_or(ee_f);
    let s_f = s.or(d).or(ee).unwrap_or(ee_f);
    let f_f = f.or(m).unwrap_or(m_f);
    let l_f = l.or(d).unwrap_or(ah_f);
    let r_f = r.or(woo).or(oh).unwrap_or(oh_f);
    [neutral_f, ah_f, d_f, ee_f, f_f, l_f, m_f, oh_f, r_f, s_f, uh_f, woo_f].map(|fr| fr.min(last))
}

fn resolve_mouth_frame(sym: &Symbol, frames: u32, key: &str, v: &Value) -> Result<u32> {
    let frame = match v {
        Value::String(label) => labelled_frame(sym, label).ok_or_else(|| {
            EngineError::Param(format!(
                "'{}' has no frame labelled '{}'",
                sym.name,
                label.chars().take(60).collect::<String>()
            ))
        })?,
        other => other
            .as_u64()
            .and_then(|f| u32::try_from(f).ok())
            .ok_or_else(|| EngineError::Param(format!("mouth '{key}' must be a frame of the symbol (0-based) or a frame label")))?,
    };
    if frame >= frames {
        return Err(EngineError::Param(format!(
            "mouth '{key}': '{}' has {frames} frames (0 to {})",
            sym.name,
            frames - 1
        )));
    }
    Ok(frame)
}

fn viseme12_from_any_key(key: &str) -> Option<Viseme12> {
    if let Some(v12) = Viseme12::from_key(key) {
        return Some(v12);
    }
    match key.trim().to_ascii_lowercase().as_str() {
        "open" => Some(Viseme12::Ah),
        "wide" => Some(Viseme12::Ee),
        "round" => Some(Viseme12::Oh),
        "closed" => Some(Viseme12::M),
        _ => None,
    }
}

fn wants_12_visemes(p: &Value) -> Result<bool> {
    if let Some(mode) = opt_str(p, "mode")? {
        return match mode {
            "visemes_12" | "12" => Ok(true),
            "visemes" | "5" => Ok(false),
            other => Err(EngineError::Param(format!(
                "unknown lip-sync mode '{}' (visemes or visemes_12)",
                other.chars().take(40).collect::<String>()
            ))),
        };
    }
    if let Some(m12) = p.get("mouths_12")
        && !m12.is_null()
    {
        if let Some(b) = m12.as_bool() {
            return Ok(b);
        }
        return Ok(true);
    }
    if let Some(m) = p.get("mouths").and_then(Value::as_object) {
        // Trigger 12-viseme mode automatically when any 12-viseme-specific key is passed in `mouths`.
        if m.keys().any(|k| Viseme::from_key(k).is_none() && Viseme12::from_key(k).is_some()) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn mouths_12_param(sym: &Symbol, p: &Value) -> Result<[u32; 12]> {
    let mut out = default_mouths_12(sym);
    let frames = sym.timeline.frame_count().max(1);
    for param_name in ["mouths", "mouths_12"] {
        let Some(m) = p.get(param_name).filter(|m| !m.is_null()) else { continue };
        if param_name == "mouths_12" && m.is_boolean() {
            continue;
        }
        let map = m.as_object().ok_or_else(|| {
            EngineError::Param(format!(
                "param '{param_name}' must be an object {{neutral, ah, d, ee, f, l, m, oh, r, s, uh, woo}}, each a frame of the symbol (0-based) or a frame label"
            ))
        })?;
        for (key, v) in map {
            let shape = viseme12_from_any_key(key).ok_or_else(|| {
                EngineError::Param(format!(
                    "unknown mouth '{}' (neutral, ah, d, ee, f, l, m, oh, r, s, uh or woo)",
                    key.chars().take(40).collect::<String>()
                ))
            })?;
            let frame = resolve_mouth_frame(sym, frames, key, v)?;
            if let Some(slot) = out.get_mut(shape.index()) {
                *slot = frame;
            }
        }
    }
    Ok(out)
}

/// The frame of `sym` each mouth shape shows: `mouths` over the defaults.
fn mouths_param(sym: &Symbol, p: &Value) -> Result<[u32; 5]> {
    let mut out = default_mouths(sym);
    let frames = sym.timeline.frame_count().max(1);
    let Some(m) = p.get("mouths").filter(|m| !m.is_null()) else { return Ok(out) };
    let map = m.as_object().ok_or_else(|| EngineError::Param("param 'mouths' must be an object {rest, open, wide, round, closed}, each a frame of the symbol (0-based) or a frame label".into()))?;
    for (key, v) in map {
        let shape = Viseme::from_key(key).ok_or_else(|| EngineError::Param(format!("unknown mouth '{}' (rest, open, wide, round or closed)", key.chars().take(40).collect::<String>())))?;
        let frame = resolve_mouth_frame(sym, frames, key, v)?;
        if let Some(slot) = out.get_mut(shape.index()) {
            *slot = frame;
        }
    }
    Ok(out)
}

/// Auto Lip-Sync.
fn lip_sync(s: &mut Session, p: &Value) -> Result<Value> {
    let use_12 = wants_12_visemes(p)?;
    let src = source(s, p)?;
    let opts = lip_options(p)?;
    let (lid, el) = target(s, p, src.from)?;
    if use_12 {
        let mouths = mouths_12_param(graphic(s, &el, "Lip-sync")?, p)?;
        let shapes = visemes_12(&src, &opts)?;
        let mut keys: Vec<(u32, Viseme12)> = Vec::new();
        let mut counts = [0u32; 12];
        for (i, v) in shapes.iter().enumerate() {
            if keys.last().is_none_or(|(_, last)| last != v) {
                keys.push((src.from + i as u32, *v));
                if let Some(n) = counts.get_mut(v.index()) {
                    *n += 1;
                }
            }
        }
        let layer = s.layer_mut(Some(lid))?;
        write_held(layer, el.id, src.from, src.to, &keys, &|base, v| {
            let mut el = base.clone();
            show_frame(&mut el, mouths.get(v.index()).copied().unwrap_or(0));
            el
        })?;
        let named = |list: [u32; 12]| {
            Value::Object(
                Viseme12::ALL
                    .iter()
                    .zip(list)
                    .map(|(v, n)| (v.key().to_string(), json!(n)))
                    .collect(),
            )
        };
        return Ok(json!({
            "keys": keys.len(),
            "from": src.from,
            "to": src.to,
            "layer": lid.0,
            "element": el.id.0,
            "sound": src.sound.id.0,
            "mode": "visemes_12",
            "mouths": named(mouths),
            "mouths_12": named(mouths),
            "counts": named(counts),
        }));
    }
    let mouths = mouths_param(graphic(s, &el, "Lip-sync")?, p)?;
    let shapes = visemes(&src, &opts)?;
    // A key where the mouth changes.
    let mut keys: Vec<(u32, Viseme)> = Vec::new();
    let mut counts = [0u32; 5];
    for (i, v) in shapes.iter().enumerate() {
        if keys.last().is_none_or(|(_, last)| last != v) {
            keys.push((src.from + i as u32, *v));
            if let Some(n) = counts.get_mut(v.index()) {
                *n += 1;
            }
        }
    }
    let layer = s.layer_mut(Some(lid))?;
    write_held(layer, el.id, src.from, src.to, &keys, &|base, v| {
        let mut el = base.clone();
        show_frame(&mut el, mouths.get(v.index()).copied().unwrap_or(0));
        el
    })?;
    let named = |list: [u32; 5]| Value::Object(Viseme::ALL.iter().zip(list).map(|(v, n)| (v.key().to_string(), json!(n))).collect());
    Ok(json!({"keys": keys.len(), "from": src.from, "to": src.to, "layer": lid.0, "element": el.id.0, "sound": src.sound.id.0, "mouths": named(mouths), "counts": named(counts)}))
}

/// Analyse Sound: what the two commands above work from.
fn analyse(s: &mut Session, p: &Value) -> Result<Value> {
    let src = source(s, p)?;
    let mut out = json!({"sound": src.sound.id.0, "name": src.sound.name, "layer": src.layer.0, "keyframe": src.keyframe, "from": src.from, "to": src.to, "fps": s.doc.stage.fps});
    let round = |v: &f32| json!((f64::from(*v) * 10_000.0).round() / 10_000.0);
    match opt_str(p, "what")?.unwrap_or("levels") {
        "levels" => {
            let lv = levels(&src, &level_settings(p)?)?;
            out["levels"] = lv.raw.iter().map(round).collect();
            out["values"] = lv.shaped.iter().map(round).collect();
        }
        "visemes" => out["visemes"] = visemes(&src, &lip_options(p)?)?.iter().map(|v| json!(v.key())).collect(),
        "visemes_12" => out["visemes_12"] = visemes_12(&src, &lip_options(p)?)?.iter().map(|v| json!(v.key())).collect(),
        other => return Err(EngineError::Param(format!("unknown 'what' '{}' (levels or visemes, or visemes_12)", other.chars().take(40).collect::<String>()))),
    }
    Ok(out)
}
