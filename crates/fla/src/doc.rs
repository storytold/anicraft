//! A legacy document into the AniCraft model: scenes, layers, keyframes and their artwork,
//! symbol instances, tweens and the library (ADR 0096).
//!
//! Observed format (Flash 8–CS4 era; see ADR 0096 and the corpus notes): the `Contents`
//! stream lists the document — library items (stream name, label, item id, symbol type), the
//! stage, and one `CDocumentPage` record per scene (page stream name + scene label). Every page
//! stream (`P <n> <stamp>`) and library stream (`S <n> <stamp>`, `Symbol <n>`) holds one
//! timeline as an object graph ([`crate::archive`]): layers (stored bottom-first), their
//! keyframes, each keyframe's raw artwork and objects (symbol instances, drawing objects,
//! groups, texts, bitmaps). `Contents` also lists the library's folders, bitmaps and sounds
//! ([`crate::library`]); their data streams decode through `anicraft_flamedia`. The mapping
//! mirrors the XFL importer's, so a legacy file and its Animate twin open with the same
//! structure: folder layers hold no frames, raw shapes merge (ADR 0042), comments are reported,
//! classic and shape tweens with their eases and hints, frame sounds with their sync, repeats
//! and envelope, frame scripts kept as written (never run here), instance handlers reported.
//!
//! What isn't decoded yet — a movie clip's component settings, filters, bitmap fills — is left
//! out and counted in the report; a timeline that doesn't decode at all falls back to the
//! structure-only reading (layer names, a symbol's first shape).

use crate::archive::{self, Graph, InstanceClass, Node};
use crate::cfb::Compound;
use crate::library::{self, ItemId, MediaKind};
use crate::shape;
use crate::report::{ImportError, Report};
use crate::text::Faces;
use anicraft_doc::ColorTransform;
use anicraft_doc::{
    Bitmap, BitmapId, BlendMode, ColorEffect, CustomEase, Document, EaseGroup, Easing, Element, ElementId, ElementKind, EnvelopePoint, FrameScript, FrameSound, Keyframe,
    Language, Layer, LayerId, LayerKind, LibraryEntry, LibraryItem as DocItem, LoopMode, Paint, Rgba, Scene, Shape, ShapeHint, Sound, SoundEffect, SoundId, SoundSync, SourceScript,
    Stage, Symbol, SymbolId, SymbolKind, Timeline, Tween, TweenKind,
};
use anicraft_geom::{FillRule, Path, Vec2};
use anicraft_text::Fonts;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// The `Contents` stream's version byte on every corpus file of the Flash 8–CS4 era.
const CONTENTS_VERSION: u8 = 0x3F;

/// The 3-byte prefix of a stream-name string (`01 80 17`), then a one-byte length.
const STREAM_NAME_PREFIX: [u8; 2] = [0x01, 0x80];
/// The 3-byte marker before a named record's UTF-16 string (`ff fe ff`), then a length byte.
const STRING_MARKER: [u8; 3] = [0xFF, 0xFE, 0xFF];
/// The 9 bytes that introduce every layer record's trailing name.
const LAYER_ANCHOR: [u8; 9] = [0x00, 0x80, 0x00, 0x00, 0x00, 0x80, 0x00, 0x00, 0x0B];
/// Longest name read (layer, scene and library labels).
const MAX_NAME: usize = 64;
/// Latest frame kept (as the XFL importer).
const MAX_FRAME: u32 = 100_000;
/// Deepest group nesting kept.
const MAX_GROUP_DEPTH: usize = 32;
/// Most objects one document's mapping converts. An object can be used more than once (a
/// group listed twice, a group in a group listed twice, …), each use converted where it is:
/// the total is capped so a few objects can't stand for exponentially many.
const MAX_CONVERTED: usize = 1_000_000;

/// A scene as the document lists it: its page stream's name and the scene's label.
struct Page {
    stream: String,
    label: String,
}

/// A library item as `Contents` lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct LibraryItem {
    /// Its stream (`S 54 1522499933`, `Symbol 125`).
    pub stream: String,
    pub label: String,
    /// The id placements refer to it by.
    pub id: u32,
    /// 0 graphic, 1 button, 2 movie clip.
    pub kind: u8,
    /// The folder it sits in, when its entry says (`library::ROOT`: the library's root).
    pub parent: Option<ItemId>,
}

/// What the mapping needs while it builds timelines.
struct Ctx<'a> {
    doc: &'a mut Document,
    report: &'a mut Report,
    /// Library item id → symbol.
    symbols: &'a HashMap<u32, SymbolId>,
    /// Media item id → bitmap, and → sound.
    bitmaps: &'a HashMap<u16, BitmapId>,
    sounds: &'a HashMap<u16, SoundId>,
    /// The fonts texts are laid out with, and the faces looked up so far.
    fonts: &'a Fonts,
    faces: &'a mut Faces,
    /// The language frame scripts are written in.
    language: Language,
    /// Objects the document's mapping may still convert ([`MAX_CONVERTED`]).
    left: &'a mut usize,
}

/// Imports a legacy `.fla` into the AniCraft model.
pub fn import(data: &[u8]) -> Result<(Document, Report), ImportError> {
    import_within(data, MAX_CONVERTED)
}

/// [`import`] converting at most `max_converted` objects and shapes in place of
/// [`MAX_CONVERTED`] (tests try a small allowance).
pub(crate) fn import_within(data: &[u8], max_converted: usize) -> Result<(Document, Report), ImportError> {
    let file = Compound::new(data)?;
    let contents = file.stream("Contents")?;
    if contents.first() != Some(&CONTENTS_VERSION) {
        return Err(ImportError::UnsupportedVersion);
    }
    let mut report = Report::default();
    let pages = pages(&contents)?;
    let items = library_items(&contents);
    if !items.is_empty() {
        report.keep("library names");
    }

    let mut doc = Document::default();
    doc.scenes.clear();
    doc.library.clear();
    doc.stage = stage(&contents).unwrap_or_default();
    // The library's folders first (empty ones come across too, as in the XFL importer).
    let folder_list = library::folders(&contents);
    let folders = library::folder_paths(&folder_list);
    for f in &folder_list {
        if let Some(path) = folders.get(&f.id) {
            doc.ensure_folder_path(path);
        }
    }
    if !doc.library_tree.folders.is_empty() {
        report.keep("library folders");
    }
    let language = match library::script_version(&contents).as_deref() {
        Some("1" | "2") => Language::ActionScript1or2,
        // As the XFL importer: ActionScript 3.0 unless the file says otherwise.
        _ => Language::ActionScript3,
    };
    let (bitmaps, sounds) = media(&file, &contents, &folders, &mut doc, &mut report);
    // Symbols first, so instances can refer to any of them.
    let mut symbols: HashMap<u32, SymbolId> = HashMap::new();
    let mut ids = Vec::with_capacity(items.len());
    for item in &items {
        let id = SymbolId(doc.alloc_id());
        symbols.insert(item.id, id);
        ids.push(id);
    }
    let fonts = Fonts::get();
    let mut faces = Faces::new();
    let mut left = max_converted;
    for (item, &id) in items.iter().zip(&ids) {
        let timeline = match file.stream(&item.stream) {
            Ok(bytes) => {
                let mut ctx = Ctx { doc: &mut doc, report: &mut report, symbols: &symbols, bitmaps: &bitmaps, sounds: &sounds, fonts, faces: &mut faces, language, left: &mut left };
                library_timeline(&bytes, &item.label, &mut ctx)
            }
            Err(_) => {
                report.skip_named("library symbols whose stream is missing", item.label.clone());
                Timeline { layers: vec![Arc::new(Layer::new(LayerId(doc.alloc_id()), "Layer 1"))] }
            }
        };
        let kind = match item.kind {
            0 => SymbolKind::Graphic,
            1 => SymbolKind::Button,
            _ => SymbolKind::MovieClip,
        };
        doc.library.push(Arc::new(Symbol { id, name: item.label.clone(), kind, linkage: None, scale_grid: None, scale_grid_kept: None, timeline }));
        place_in_folder(&mut doc, &folders, item.parent, DocItem::Symbol(id));
        report.keep("library symbols");
    }
    for page in &pages {
        let mut timeline = Timeline::default();
        match file.stream(&page.stream) {
            Ok(bytes) => match archive::parse(&bytes) {
                Ok(graph) => {
                    let mut ctx = Ctx { doc: &mut doc, report: &mut report, symbols: &symbols, bitmaps: &bitmaps, sounds: &sounds, fonts, faces: &mut faces, language, left: &mut left };
                    timeline = build_timeline(&graph, &mut ctx);
                }
                Err(_) => {
                    // The structure-only reading: the layer names.
                    report.skip_named("scenes whose timeline didn't decode (layer names only)", page.label.clone());
                    for name in layer_names(&bytes).into_iter().rev() {
                        timeline.layers.push(Arc::new(Layer::new(LayerId(doc.alloc_id()), name)));
                    }
                }
            },
            Err(_) => report.skip_named("scenes whose page stream is missing", page.label.clone()),
        }
        if timeline.layers.is_empty() {
            timeline.layers.push(Arc::new(Layer::new(LayerId(doc.alloc_id()), "Layer 1")));
        }
        doc.scenes.push(Arc::new(Scene { name: page.label.clone(), timeline, guides: Vec::new() }));
    }
    if doc.scenes.is_empty() {
        let timeline = Timeline { layers: vec![Arc::new(Layer::new(LayerId(doc.alloc_id()), "Layer 1"))] };
        doc.scenes.push(Arc::new(Scene { name: "Scene 1".into(), timeline, guides: Vec::new() }));
        report.skip("scenes (no readable page record)");
    }
    doc.sanitize();
    Ok((doc, report))
}

/// Puts a library item in its folder (when its entry names one `Contents` lists).
fn place_in_folder(doc: &mut Document, folders: &HashMap<ItemId, String>, parent: Option<ItemId>, item: DocItem) {
    if let Some(path) = parent.and_then(|p| folders.get(&p))
        && let Some(f) = doc.ensure_folder_path(path)
    {
        doc.place_entry(LibraryEntry::Item(item), Some(f));
    }
}

/// The library's bitmaps and sounds (`Contents`' media items), their data decoded from their
/// streams the way the XFL importer decodes the same payloads in `bin/` (`anicraft_flamedia`):
/// a bitmap in Animate's own lossless layout, or a PNG or JPEG; a sound as raw 16-bit PCM of
/// its item's format and sample count (it comes across as a WAV), or a WAV, Ogg Vorbis or MP3
/// file. What doesn't decode is reported by name, with why.
fn media(file: &Compound, contents: &[u8], folders: &HashMap<ItemId, String>, doc: &mut Document, report: &mut Report) -> (HashMap<u16, BitmapId>, HashMap<u16, SoundId>) {
    let streams: HashSet<String> = file.entries().iter().filter(|e| e.kind == crate::cfb::Kind::Stream).map(|e| e.path.clone()).collect();
    let (mut bitmaps, mut sounds) = (HashMap::new(), HashMap::new());
    for item in library::media_items(contents, |s| streams.contains(s)) {
        if bitmaps.contains_key(&item.id) || sounds.contains_key(&item.id) {
            continue;
        }
        let shown = item.name.clone();
        let Ok(bytes) = file.stream(&item.stream) else {
            report.skip_named("media whose stream couldn't be read", shown);
            continue;
        };
        let kind = item.kind.unwrap_or(if anicraft_flamedia::is_bitmap(&bytes) { MediaKind::Bitmap } else { MediaKind::Sound });
        match kind {
            MediaKind::Bitmap => {
                let decoded = if anicraft_flamedia::is_bitmap(&bytes) {
                    anicraft_flamedia::decode_bitmap(&bytes).map_err(|e| e.to_string()).and_then(|px| {
                        Bitmap::from_rgba(BitmapId(0), item.name.clone(), px.width, px.height, px.rgba).ok_or_else(|| anicraft_flamedia::DatError::BadSize.to_string())
                    })
                } else {
                    Bitmap::from_image(BitmapId(0), item.name.clone(), bytes).map_err(|_| "not a format AniCraft reads".to_string())
                };
                match decoded {
                    Ok(mut b) => {
                        b.id = BitmapId(doc.alloc_id());
                        b.smoothing = true;
                        bitmaps.insert(item.id, b.id);
                        place_in_folder(doc, folders, Some(item.parent), DocItem::Bitmap(b.id));
                        doc.bitmaps.push(Arc::new(b));
                        report.keep("bitmaps");
                    }
                    Err(why) => report.skip_named("bitmaps in Animate's own format AniCraft can't decode", format!("{shown}: {why}")),
                }
            }
            MediaKind::Sound => {
                // Raw PCM when it is exactly the item's sample count of its format; otherwise a
                // sound file kept as it came.
                let mut raw_why = None;
                let mut sound = None;
                if let Some((format, count)) = item.sound {
                    match anicraft_flamedia::SoundFormat::from_legacy(format).and_then(|f| anicraft_flamedia::decode_sound(&bytes, f, Some(u64::from(count)))) {
                        Ok(pcm) => match pcm.to_wav().map(|wav| Sound::from_data(SoundId(0), item.name.clone(), wav)) {
                            Some(Ok(s)) => sound = Some(s),
                            Some(Err(e)) => raw_why = Some(e.to_string()),
                            None => raw_why = Some(anicraft_flamedia::DatError::TooLong.to_string()),
                        },
                        Err(anicraft_flamedia::DatError::LengthMismatch { .. }) => {}
                        Err(e) => raw_why = Some(e.to_string()),
                    }
                }
                let sound = match sound {
                    Some(s) => Ok(s),
                    None => Sound::from_data(SoundId(0), item.name.clone(), bytes).map_err(|e| raw_why.map_or_else(|| e.to_string(), |r| format!("Animate's own sound data ({r})"))),
                };
                match sound {
                    Ok(mut s) => {
                        s.id = SoundId(doc.alloc_id());
                        sounds.insert(item.id, s.id);
                        place_in_folder(doc, folders, Some(item.parent), DocItem::Sound(s.id));
                        doc.sounds.push(Arc::new(s));
                        report.keep("sounds");
                    }
                    Err(why) => report.skip_named("sounds in formats AniCraft can't decode", format!("{shown}: {why}")),
                }
            }
        }
    }
    (bitmaps, sounds)
}

/// A library item's timeline: its stream's object graph, or (when that doesn't decode) its
/// first shape record on one keyframe.
fn library_timeline(bytes: &[u8], label: &str, ctx: &mut Ctx) -> Timeline {
    if let Ok(graph) = archive::parse(bytes) {
        let tl = build_timeline(&graph, ctx);
        if !tl.layers.is_empty() {
            return tl;
        }
    }
    let (doc, report) = (&mut *ctx.doc, &mut *ctx.report);
    let shapes = shapes(bytes, report);
    let layer = Layer::new(LayerId(doc.alloc_id()), "artwork");
    if shapes.is_empty() {
        report.skip_named("library symbols whose timeline didn't decode", label.to_string());
        return Timeline { layers: vec![Arc::new(layer)] };
    }
    report.skip_named("library symbols whose timeline didn't decode (first drawing shown)", label.to_string());
    let elements = shapes.into_iter().map(|mut e| {
        e.id = ElementId(doc.alloc_id());
        Arc::new(e)
    }).collect();
    let keyframe = Keyframe { elements, ..Keyframe::new(0, 1) };
    Timeline { layers: vec![Arc::new(Layer { keyframes: vec![Arc::new(keyframe)], ..layer })] }
}

/// A timeline from a decoded graph: layers top-first, keyframes with their artwork and tweens.
fn build_timeline(graph: &Graph, ctx: &mut Ctx) -> Timeline {
    let order = graph.layers();
    let mut ids: HashMap<usize, LayerId> = HashMap::new();
    for &n in &order {
        ids.insert(n, LayerId(ctx.doc.alloc_id()));
    }
    let mut layers = Vec::with_capacity(order.len());
    for &n in order.iter().rev() {
        let (Some(Node::Layer(rec)), Some(&id)) = (graph.node(n), ids.get(&n)) else { continue };
        let mut layer = Layer::new(id, rec.name.chars().take(256).collect::<String>());
        layer.kind = match rec.kind {
            1 => LayerKind::Guide,
            3 => LayerKind::Folder,
            4 => LayerKind::Mask,
            _ => LayerKind::Normal,
        };
        layer.parent = rec.parent.and_then(|p| ids.get(&p).copied()).filter(|p| *p != id);
        layer.visible = !rec.hidden;
        layer.locked = rec.locked;
        layer.outline = rec.outline;
        let [r, g, b, a] = rec.color;
        layer.color = Rgba::new(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, a as f32 / 255.0);
        ctx.report.keep("layers");
        if layer.kind == LayerKind::Folder {
            // A folder holds layers, never frames.
            layer.keyframes.clear();
            layers.push(Arc::new(layer));
            continue;
        }
        let mut keys = Vec::new();
        let mut hints: HashMap<u32, Vec<archive::HintRec>> = HashMap::new();
        let mut start = 0u32;
        for &f in &rec.frames {
            let Some(Node::Frame(frame)) = graph.node(f) else {
                ctx.report.skip("keyframes that didn't decode");
                continue;
            };
            let duration = u32::from(frame.duration);
            let at = start.min(MAX_FRAME);
            start = start.saturating_add(duration);
            // A zero-length frame record holds nothing that shows (Animate drops it too).
            if duration == 0 {
                continue;
            }
            if !frame.hints.is_empty() {
                hints.insert(at, frame.hints.clone());
            }
            keys.push(Arc::new(keyframe(graph, frame, at, duration.clamp(1, MAX_FRAME), ctx)));
            ctx.report.keep("keyframes");
        }
        if !keys.is_empty() {
            layer.keyframes = keys;
        }
        layer.normalize();
        if !hints.is_empty() {
            pair_hints(&mut layer, &hints, ctx.report);
        }
        layers.push(Arc::new(layer));
    }
    let mut tl = Timeline { layers };
    tl.repair_masks();
    tl
}

fn keyframe(graph: &Graph, frame: &archive::FrameRec, start: u32, duration: u32, ctx: &mut Ctx) -> Keyframe {
    let mut k = Keyframe::new(start, duration);
    if !frame.label.is_empty() {
        // Label kind 1 is a comment (every corpus label); 0, a name, and 2, an anchor, are a
        // hypothesis (Flash's menu order) and both a label, as XFL's names and anchors are.
        if frame.label_kind == 1 {
            // A note to the animator that nothing can go to: no place for it yet.
            ctx.report.skip("frame comments");
        } else {
            k.label = frame.label.chars().take(256).collect();
        }
    }
    match frame.tween {
        1 | 2 => {
            let kind = if frame.tween == 2 { TweenKind::Shape } else { TweenKind::Classic };
            let (easing, groups) = easing(frame, ctx.report);
            let mut t = Tween { easing, kind, ..Tween::default() };
            for (g, e) in groups {
                t.set_ease(g, Some(e));
            }
            k.tween = Some(t);
        }
        0 => {}
        _ => ctx.report.skip("tweens of an unknown kind (kept as keyframes)"),
    }
    if !frame.script.trim().is_empty() {
        k.script = frame_script(&frame.script, ctx);
    }
    if frame.sound != 0 {
        k.sound = frame_sound(frame, ctx);
    }
    let mut elements = Vec::new();
    // The keyframe's raw artwork sits below its objects, its shapes merging (ADR 0042).
    match &frame.raw {
        Some(record) => push_shapes(record, true, ctx, &mut elements),
        None => {
            if let Some(why) = frame.raw_error {
                ctx.report.skip(shape_skip(why));
            }
        }
    }
    let mut groups = Vec::new();
    for &e in &frame.elements {
        element(graph, e, ctx, &mut elements, &mut groups);
    }
    k.elements = elements;
    k
}

/// A frame script, exactly as written, in the document's language: kept (ADR 0013 tier 1), as
/// the XFL importer keeps `<Actionscript>`; nothing here runs it.
fn frame_script(text: &str, ctx: &mut Ctx) -> Option<FrameScript> {
    let script = SourceScript::new(ctx.language, text);
    if script.check().is_err() {
        ctx.report.skip("frame scripts over 1 MiB (left out)");
        return None;
    }
    let what = format!("frame scripts in {}", ctx.language.label());
    if script.runs() {
        *ctx.report.runs.entry(what).or_default() += 1;
    } else {
        ctx.report.keep(&what);
    }
    Some(FrameScript::Source(Arc::new(script)))
}

/// The sound on a keyframe, as the XFL importer reads a frame's `soundName`: its sync (by
/// Flash's menu order, 0 event, 1 start, 2 stop, 3 stream; only stream is in the corpus),
/// repeats and envelope (time in samples at 44.1 kHz, levels `0..=32768`). In and out points
/// are reported (played whole), as there.
fn frame_sound(frame: &archive::FrameRec, ctx: &mut Ctx) -> Option<FrameSound> {
    let Some(&id) = ctx.sounds.get(&frame.sound) else {
        ctx.report.skip("frame sounds whose sound isn't in the file");
        return None;
    };
    let mut fs = FrameSound::new(id);
    fs.sync = match frame.sync {
        0 => SoundSync::Event,
        1 => SoundSync::Start,
        2 => SoundSync::Stop,
        3 => SoundSync::Stream,
        _ => {
            ctx.report.skip("sound syncs of an unknown kind (played as event)");
            SoundSync::Event
        }
    };
    fs.repeat = u32::from(frame.loops).clamp(1, anicraft_doc::sound::MAX_REPEAT);
    let points: Vec<EnvelopePoint> = frame
        .envelope
        .iter()
        .take(anicraft_doc::sound::MAX_ENVELOPE_POINTS)
        .map(|&(mark, left, right)| EnvelopePoint::new(f64::from(mark) / 44_100.0, f32::from(left) / 32_768.0, f32::from(right) / 32_768.0))
        .collect();
    // The format has no effect byte: an envelope plays as its points (as the XFL importer
    // reads an envelope without a preset name).
    if !points.is_empty() {
        fs.effect = SoundEffect::Custom(anicraft_doc::sound::clean_points(&points));
    }
    if frame.in_point != 0 || frame.out_point != archive::NO_OUT_POINT {
        ctx.report.skip("sound in and out points (played whole)");
    }
    Some(fs)
}

/// Gives each shape tween on `layer` the hints its start keyframe lists (each with its start
/// and its end point, twips), snapped onto the outlines when they sit within a couple of
/// pixels of one, as the XFL importer pairs them.
fn pair_hints(layer: &mut Layer, raw: &HashMap<u32, Vec<archive::HintRec>>, report: &mut Report) {
    const SNAP: f32 = 2.0;
    let snap = |els: &[Arc<Element>], p: Vec2| match anicraft_doc::nearest_outline(els, p) {
        Some((_, q, d)) if d <= SNAP => q,
        _ => p,
    };
    let px = |(x, y): (i32, i32)| Vec2::new(x as f32 / 20.0, y as f32 / 20.0);
    for i in 0..layer.keyframes.len() {
        let (Some(k), Some(next)) = (layer.keyframes.get(i), layer.keyframes.get(i + 1)) else { continue };
        let Some(list) = raw.get(&k.start) else { continue };
        if !layer.is_shape_tween(i) {
            continue;
        }
        if list.len() > anicraft_doc::MAX_SHAPE_HINTS {
            report.skip("shape hints past z");
        }
        let hints: Vec<ShapeHint> = list.iter().take(anicraft_doc::MAX_SHAPE_HINTS).map(|&(a, b)| ShapeHint { start: snap(&k.elements, px(a)), end: snap(&next.elements, px(b)) }).collect();
        if let Some(k) = layer.keyframes.get_mut(i) {
            Arc::make_mut(k).hints = hints;
        }
    }
}

/// A tween's ease (as the XFL importer reads it): the classic slider, or the custom curves —
/// one for everything, or one per property group.
fn easing(frame: &archive::FrameRec, report: &mut Report) -> (Easing, Vec<(EaseGroup, Easing)>) {
    let acc = (frame.ease as f32).clamp(-100.0, 100.0);
    let classic = if acc == 0.0 { Easing::Linear } else { Easing::Classic(acc) };
    if frame.eases.iter().all(Vec::is_empty) {
        return (classic, Vec::new());
    }
    const GROUPS: [EaseGroup; 5] = [EaseGroup::Position, EaseGroup::Rotation, EaseGroup::Scale, EaseGroup::Color, EaseGroup::Filters];
    if let Some(all) = frame.eases.get(5).filter(|c| !c.is_empty()) {
        return (curve(all, report).unwrap_or(classic), Vec::new());
    }
    let groups = GROUPS.iter().zip(&frame.eases).filter(|(_, c)| !c.is_empty()).filter_map(|(g, c)| curve(c, report).map(|e| (*g, e))).collect();
    (classic, groups)
}

/// One custom ease curve from its anchors (in handle, anchor, out handle), as the XFL
/// importer makes them: one segment is a cubic Bézier, more a custom ease.
fn curve(anchors: &[[f64; 6]], report: &mut Report) -> Option<Easing> {
    let mut pts: Vec<Vec2> = Vec::with_capacity(anchors.len() * 3);
    for (i, a) in anchors.iter().enumerate() {
        let p = |x: f64, y: f64| Vec2::new(x as f32, y as f32);
        if i > 0 {
            pts.push(p(a[0], a[1]));
        }
        pts.push(p(a[2], a[3]));
        if i + 1 < anchors.len() {
            pts.push(p(a[4], a[5]));
        }
    }
    if pts.iter().any(|p| !p.is_finite()) || pts.len() / 3 + 1 > anicraft_doc::MAX_EASE_POINTS {
        report.skip("custom ease curves with a broken point list (drawn with the simple ease)");
        return None;
    }
    let Some(c) = CustomEase::from_flat(&pts) else {
        report.skip("custom ease curves with a broken point list (drawn with the simple ease)");
        return None;
    };
    Some(match c.points() {
        [a, b] => Easing::CubicBezier(a.out.x, a.out.y, b.at.x + b.h_in.x, b.at.y + b.h_in.y),
        _ => Easing::Custom(Arc::new(c)),
    })
}

/// The AniCraft elements for one decoded object, pushed bottom-first. `groups` are the groups
/// being converted around it, outermost first.
fn element(graph: &Graph, n: usize, ctx: &mut Ctx, out: &mut Vec<Arc<Element>>, groups: &mut Vec<usize>) {
    if !spend(ctx) {
        return;
    }
    match graph.node(n) {
        Some(Node::Instance(inst)) => {
            if let Some(e) = instance(inst, ctx) {
                out.push(Arc::new(e));
            }
        }
        Some(Node::Text(t)) => {
            let id = ElementId(ctx.doc.alloc_id());
            if let Some(e) = t.to_element(id, ctx.fonts, ctx.faces, ctx.report) {
                out.push(Arc::new(e));
                ctx.report.keep("texts");
            }
        }
        Some(Node::Bitmap(b)) => {
            // As the XFL importer's bitmap instance: the image's rectangle, filled with it.
            let found = ctx.bitmaps.get(&b.item).and_then(|&id| ctx.doc.bitmap(id).map(|bm| (id, bm.width as f32, bm.height as f32)));
            let Some((bitmap, w, h)) = found else {
                ctx.report.skip("instances of missing bitmaps");
                return;
            };
            let shape = Shape { path: Arc::new(Path::rect(0.0, 0.0, w, h)), fill: Some(Paint::Bitmap { bitmap, repeat: false, matrix: anicraft_geom::Affine::IDENTITY }), stroke: None, fill_rule: FillRule::NonZero, merge: false, warp: None };
            out.push(Arc::new(Element { id: ElementId(ctx.doc.alloc_id()), name: String::new(), transform: b.matrix, color: ColorEffect::default(), kind: ElementKind::Shape(shape) }));
            ctx.report.keep("bitmap instances");
        }
        Some(Node::Shape(s)) => {
            let group = !s.drawing_object && (!s.members.is_empty() || s.flag & 1 == 0);
            if !group {
                if let Some(record) = &s.record {
                    push_shapes(record, !s.drawing_object, ctx, out);
                }
                return;
            }
            // A member that is the group itself, or a group around it: a loop, not artwork.
            if groups.contains(&n) {
                ctx.report.skip("groups that contain themselves (left out)");
                return;
            }
            if groups.len() >= MAX_GROUP_DEPTH {
                ctx.report.skip("groups nested more than 32 deep");
                return;
            }
            // A group (ADR 0073): its own body, then its members; their shapes merge only with
            // each other.
            let mut members = Vec::new();
            if let Some(record) = &s.record {
                push_shapes(record, true, ctx, &mut members);
            }
            groups.push(n);
            for &m in &s.members {
                element(graph, m, ctx, &mut members, groups);
            }
            groups.pop();
            if !members.is_empty() {
                let layer = LayerId(ctx.doc.alloc_id());
                let id = ElementId(ctx.doc.alloc_id());
                out.push(Arc::new(Element { id, name: String::new(), transform: anicraft_geom::Affine::IDENTITY, color: ColorEffect::default(), kind: ElementKind::group(layer, members) }));
            }
        }
        Some(Node::Skipped(class)) => {
            let what = match class.as_str() {
                "CPicShape" => "shapes whose edge records didn't decode",
                "CPicText" => "texts whose records didn't decode",
                "CPicBitmap" => "bitmap instances whose records didn't decode",
                "CPicVideo" => "videos (decode pending)",
                _ => "objects of a kind not decoded (left out)",
            };
            ctx.report.skip(what);
        }
        Some(Node::Morph | Node::List(_) | Node::Hint(..) | Node::Pending) | None => {}
        Some(_) => ctx.report.skip("objects of a kind not decoded (left out)"),
    }
}

/// The report kind for a shape that didn't decode.
fn shape_skip(why: shape::ShapeError) -> &'static str {
    match why {
        shape::ShapeError::FillKind(0x40..=0x43) => "shapes with bitmap fills (decode pending)",
        shape::ShapeError::FillKind(_) => "shapes with fills of an unknown kind",
        _ => "shapes whose edge records didn't decode",
    }
}

/// Spends one of the document's [`MAX_CONVERTED`] objects; false, and reported, once they are
/// spent.
fn spend(ctx: &mut Ctx) -> bool {
    let Some(left) = ctx.left.checked_sub(1) else {
        ctx.report.skip("objects past a million in one document (left out)");
        return false;
    };
    *ctx.left = left;
    true
}

/// A shape record's AniCraft shapes (one per fill, then per stroke), carried by its matrix.
/// Each shape is charged against the same allowance as the objects: one record may hold
/// thousands of fills and strokes and be placed on many keyframes.
fn push_shapes(record: &shape::ShapeRecord, merge: bool, ctx: &mut Ctx, out: &mut Vec<Arc<Element>>) {
    for what in &record.approximations {
        ctx.report.skip(what);
    }
    if *ctx.left == 0 {
        ctx.report.skip("objects past a million in one document (left out)");
        return;
    }
    for mut s in record.shapes() {
        if !spend(ctx) {
            break;
        }
        s.merge = merge;
        let id = ElementId(ctx.doc.alloc_id());
        out.push(Arc::new(Element { id, name: String::new(), transform: record.matrix, color: ColorEffect::default(), kind: ElementKind::Shape(s) }));
    }
}

/// A symbol instance (as the XFL importer builds `DOMSymbolInstance`).
fn instance(inst: &archive::InstanceRec, ctx: &mut Ctx) -> Option<Element> {
    let Some(&symbol) = ctx.symbols.get(&u32::from(inst.item)) else {
        ctx.report.skip("instances of missing symbols");
        return None;
    };
    // Graphics pick their frame by loop mode; movie clips and buttons play their own timeline.
    let loop_mode = match (inst.class, inst.loop_mode) {
        (InstanceClass::Graphic, 1) => LoopMode::PlayOnce,
        (InstanceClass::Graphic, 2) => LoopMode::SingleFrame,
        _ => LoopMode::Loop,
    };
    let blend = match inst.blend {
        2 => BlendMode::Layer,
        3 => BlendMode::Multiply,
        4 => BlendMode::Screen,
        5 => BlendMode::Lighten,
        6 => BlendMode::Darken,
        7 => BlendMode::Difference,
        8 => BlendMode::Add,
        9 => BlendMode::Subtract,
        10 => BlendMode::Invert,
        11 => BlendMode::Alpha,
        12 => BlendMode::Erase,
        13 => BlendMode::Overlay,
        14 => BlendMode::HardLight,
        _ => BlendMode::Normal,
    };
    if inst.filters != 0 {
        ctx.report.skip("filters and blend modes on filtered instances (left out)");
    } else if inst.extras_skipped {
        ctx.report.skip(match inst.class {
            InstanceClass::MovieClip => "movie clip component settings (left out)",
            _ => "button settings in a layout not decoded (left out)",
        });
    }
    // Older documents put handlers (`on (release) { … }`) on instances; only frame scripts
    // have a place yet (as in the XFL importer).
    if !inst.script.trim().is_empty() {
        ctx.report.skip("scripts on instances (left out; frame scripts are kept)");
    }
    let m = inst.matrix;
    // The stored point is the transformation point placed by the matrix: back into the
    // instance's own space. Kept only when the artist moved it off the registration point.
    let placed = Vec2::new(inst.point.0 as f32 / 20.0, inst.point.1 as f32 / 20.0);
    let pivot = m.inverse().map(|inv| inv.apply(placed)).filter(|p| p.is_finite() && (p.x.abs() > 0.025 || p.y.abs() > 0.025));
    let mut e = Element {
        id: ElementId(ctx.doc.alloc_id()),
        name: String::new(),
        transform: m,
        color: color_effect(&inst.color),
        kind: ElementKind::Symbol { symbol, loop_mode, first_frame: u32::from(inst.first_frame).min(MAX_FRAME), blend, filters: Vec::new(), events: Vec::new(), pivot },
    };
    let name = inst.name.trim();
    if !name.is_empty() {
        if anicraft_doc::actions::valid_instance_name(name) {
            e.name = name.to_string();
        } else {
            ctx.report.skip("instance names actions can't use (left out)");
        }
    }
    ctx.report.keep("symbol instances");
    Some(e)
}

/// An instance's colour effect by its style, as the XFL importer reads Animate's.
fn color_effect(c: &archive::ColorRec) -> ColorEffect {
    let mul = |i: usize| c.mul.get(i).map_or(1.0, |v| *v as f32 / 256.0);
    let add = |i: usize| c.add.get(i).map_or(0.0, |v| *v as f32 / 255.0);
    let alpha = mul(0);
    let pct = (c.percent as f32 / 100.0).clamp(-1.0, 1.0);
    let with_alpha = |fx: ColorEffect| if alpha != 1.0 { fx.with_alpha(alpha) } else { fx };
    let fx = match c.style {
        1 if pct != 0.0 => with_alpha(ColorEffect::Brightness(pct)),
        2 => {
            let [r, g, b, _] = c.tint;
            with_alpha(ColorEffect::Tint { color: Rgba::new(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0), amount: pct })
        }
        3 => {
            let t = ColorTransform { mul: [mul(1), mul(2), mul(3), mul(0)], add: [add(1), add(2), add(3), add(0)] };
            if t.is_identity() { ColorEffect::None } else { ColorEffect::Advanced(t) }
        }
        4 if alpha != 1.0 => ColorEffect::Alpha(alpha),
        _ => ColorEffect::None,
    };
    fx.sanitized()
}

/// The stage: its size, frame rate and colour, in the document properties `Contents` lists
/// before the `legacyLineSpacing` property: the stage rectangle (i32 left, right, top, bottom
/// in twips) 74 bytes before that property's name, the background (RGBA) 21 bytes before it
/// and the frame rate (16.16) 13 bytes before it. `None` when they don't make a stage.
pub fn stage(contents: &[u8]) -> Option<Stage> {
    let name: Vec<u8> = "legacyLineSpacing".encode_utf16().flat_map(u16::to_le_bytes).collect();
    let mut needle = vec![0xFF, 0xFE, 0xFF, 17];
    needle.extend_from_slice(&name);
    let at = find(contents, &needle)?;
    let i32_at = |o: usize| -> Option<i32> { let s = at.checked_sub(o)?; contents.get(s..s + 4).and_then(|b| b.try_into().ok()).map(i32::from_le_bytes) };
    let (left, right, top, bottom) = (i32_at(74)?, i32_at(70)?, i32_at(66)?, i32_at(62)?);
    let fps = i32_at(13)? as f32 / 65536.0;
    let bg = contents.get(at.checked_sub(21)?..at.checked_sub(17)?)?;
    // In i64: edges a whole i32 range apart don't overflow.
    let (w, h) = ((i64::from(right) - i64::from(left)) / 20, (i64::from(bottom) - i64::from(top)) / 20);
    let edge = 1..=i64::from(anicraft_doc::MAX_STAGE_EDGE);
    if !edge.contains(&w) || !edge.contains(&h) || !(1.0..=120.0).contains(&fps) {
        return None;
    }
    let (w, h) = (u32::try_from(w).ok()?, u32::try_from(h).ok()?);
    let c = |i: usize| bg.get(i).map_or(1.0, |v| *v as f32 / 255.0);
    Some(Stage { width: w, height: h, fps, background: Rgba::new(c(0), c(1), c(2), 1.0) })
}

/// The fallback for a library stream whose timeline doesn't decode: its first `CPicShape`
/// record (the object that names the class) as shape elements, one per fill style, then one
/// per stroke style, carried by the record's matrix. A record that doesn't decode is left
/// out whole and counted.
fn shapes(bytes: &[u8], report: &mut Report) -> Vec<Element> {
    let Some(name) = find(bytes, b"CPicShape") else { return Vec::new() };
    let after = name + b"CPicShape".len();
    let decoded = match shape::body_after_tag(bytes, after) {
        Some(body) => shape::decode(bytes, body),
        // A shape object that holds its artwork in a nested object right behind its own
        // first field (`04 00`, then the nested object's class tag; seen on a few grouped
        // shapes): that object's body.
        None => match (after..after + 16).find_map(|start| shape::body_after_tag(bytes, start).and_then(|body| shape::decode(bytes, body).ok())) {
            Some(record) => Ok(record),
            None => {
                report.skip("shapes (edge header not recognised yet)");
                return Vec::new();
            }
        },
    };
    let record = match decoded {
        Ok(record) => record,
        Err(shape::ShapeError::FillKind(0x40..=0x43)) => {
            report.skip("shapes with bitmap fills (decode pending)");
            return Vec::new();
        }
        Err(shape::ShapeError::FillKind(_)) => {
            report.skip("shapes with fills of an unknown kind");
            return Vec::new();
        }
        Err(_) => {
            report.skip("shapes whose edge records didn't decode");
            return Vec::new();
        }
    };
    for what in &record.approximations {
        report.skip(what);
    }
    let transform = record.matrix;
    record.shapes().into_iter().enumerate().map(|(i, s)| Element {
        id: ElementId(i as u64 + 1),
        name: String::new(),
        transform,
        color: anicraft_doc::ColorEffect::default(),
        kind: ElementKind::Shape(s),
    }).collect()
}

/// The document's scenes, from the `CDocumentPage` record(s): each holds the page's
/// stream name (`P 2 1522499960`, NUL-terminated UTF-16) then the scene's label behind
/// the `ff fe ff` marker.
fn pages(contents: &[u8]) -> Result<Vec<Page>, ImportError> {
    let mut out = Vec::new();
    let mut from = 0usize;
    while let Some(at) = contents.get(from..).and_then(|rest| find(rest, b"CDocumentPage")) {
        let at = from + at;
        let Some(name_end) = at.checked_add(b"CDocumentPage".len()) else { break };
        // The page record: a `17` marker and a byte of attributes, then the page's
        // stream name (UTF-16, NUL-terminated), then the scene's label behind the
        // `ff fe ff` marker.
        let rest = contents.get(name_end..).ok_or(ImportError::BadStructure("the file ends inside a page record"))?;
        if rest.first() != Some(&0x17) {
            from = name_end;
            continue;
        }
        let mut chars = Vec::new();
        let mut i = 2usize; // past the `17` marker byte and its attribute byte
        while i + 1 < rest.len() {
            if i + STRING_MARKER.len() <= rest.len() && rest[i..i + STRING_MARKER.len()] == STRING_MARKER {
                break; // the name ends at the next record's marker (no terminator)
            }
            if chars.len() > MAX_NAME {
                return Err(ImportError::BadStructure("a page record's stream name is too long"));
            }
            let c = u16::from_le_bytes([rest[i], rest[i + 1]]);
            if c == 0 {
                break;
            }
            chars.push(c);
            i += 2;
        }
        let stream = String::from_utf16(&chars).map_err(|_| ImportError::BadStructure("a page record's stream name is not text"))?;
        if stream.is_empty() || !stream.chars().all(|c| c.is_ascii_graphic() || c == ' ') {
            return Err(ImportError::BadStructure("a page record's stream name is not text"));
        }
        // The scene label: the marker string right after the stream name.
        let after_name = i;
        let (label, end) = rest.get(after_name..)
            .and_then(|tail| (0..tail.len().saturating_sub(4)).find_map(|j| after_marker(tail, j)))
            .ok_or(ImportError::BadStructure("a page record has no scene name"))?;
        out.push(Page { stream, label });
        // On from where the label ends as stored (its UTF-16 units, not its UTF-8 length).
        from = name_end + after_name + end;
    }
    Ok(out)
}

/// The library: stream-name strings (`01 80 17` prefix) each followed by a label string
/// (`ff fe ff`), e.g. `Symbol 125` → `UFO rays`, as (stream, label) pairs.
pub fn library(contents: &[u8], report: &mut Report) -> Vec<(String, String)> {
    let items = library_items(contents);
    if !items.is_empty() {
        report.keep("library names");
    }
    items.into_iter().map(|i| (i.stream, i.label)).collect()
}

/// The library's symbols: each a stream-name string (`01 80 17` prefix), its label (`ff fe ff`),
/// then a u32 item id (what placements refer to; not always the stream's number) and a symbol
/// type byte (0 graphic, 1 button, 2 movie clip).
pub fn library_items(contents: &[u8]) -> Vec<LibraryItem> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + STREAM_NAME_PREFIX.len() + 2 <= contents.len() {
        if contents.get(i..i + 2) == Some(&STREAM_NAME_PREFIX[..]) && contents.get(i + 2) == Some(&0x17)
            && let Some((name, from)) = marked_string(contents, i + 2, 0x17) {
                // The label follows within the same record: take the first string ahead.
                let to = (from + 256).min(contents.len());
                let mut found = false;
                for j in from..to {
                    // `j` is the marker's last byte; the length, then the UTF-16 units.
                    if let Some((label, end)) = after_marker(contents, j) {
                        let id = contents.get(end..end + 4).and_then(|b| b.try_into().ok()).map(u32::from_le_bytes);
                        let kind = contents.get(end + 4).copied();
                        i = end;
                        found = true;
                        if let (Some(id), Some(kind)) = (id, kind)
                            && (name.starts_with("S ") || name.starts_with("Symbol "))
                        {
                            let parent = library::symbol_parent(contents, end + 5);
                            out.push(LibraryItem { stream: name, label, id, kind, parent });
                        }
                        break;
                    }
                }
                // A name with no label ahead is no entry: on past it.
                if !found {
                    i += 1;
                }
                continue;
            }
        i += 1;
    }
    out
}

/// Layer names in a page stream: the constant layer anchor, the `ff fe ff` marker, then
/// the name. Other named records (a sound's file name inside a frame, for one) share the
/// anchor; they are told apart only by the record-level decode pass, so a document with
/// sounds may gain a layer named after the sound.
fn layer_names(bytes: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + LAYER_ANCHOR.len() + 4 <= bytes.len() {
        if bytes[i..i + LAYER_ANCHOR.len()] == LAYER_ANCHOR {
            // The marker's last byte sits two past the anchor's end.
            if let Some((name, end)) = after_marker(bytes, i + LAYER_ANCHOR.len() + 2) {
                out.push(name);
                i = end;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// A string behind the `ff fe ff` marker whose last byte is at `at`: a one-byte length, then
/// UTF-16. Returns the text when it is plausible, and where it ends (see [`marked_string`]).
fn after_marker(data: &[u8], at: usize) -> Option<(String, usize)> {
    marked_string(data, at, 0xFF)
}

/// A string whose 3rd marker byte is `third` (`ff fe ff` or `01 80 17`): `data[at]` is
/// the third marker byte, `data[at + 1]` the length, then UTF-16. Returns the text and the
/// offset just past its units as stored (not `at` plus the text's UTF-8 length, which differs
/// outside ASCII).
fn marked_string(data: &[u8], at: usize, third: u8) -> Option<(String, usize)> {
    if data.get(at) != Some(&third) {
        return None;
    }
    let len = *data.get(at.checked_add(1)?)? as usize;
    if len == 0 || len > MAX_NAME {
        return None;
    }
    let start = at.checked_add(2)?;
    let end = start.checked_add(2 * len)?;
    let chars: Vec<u16> = data.get(start..end)?.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
    let text = String::from_utf16(&chars).ok()?;
    (text.chars().all(|c| !c.is_control())).then_some((text, end))
}

/// Finds `needle` in `data` (small, bounded scans; the streams are capped by the reader).
fn find(data: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || data.len() < needle.len() {
        return None;
    }
    (0..=data.len() - needle.len()).find(|&i| &data[i..i + needle.len()] == needle)
}
