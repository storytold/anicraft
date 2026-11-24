//! Importing video (ADR 0116): frames into the document, or a reference layer to trace over.

use crate::params::*;
use crate::video::{self, Options, Prepared, Request, What};
use crate::{CommandSpec, EngineError, MAX_FRAME, Result, Session};
use anicraft_doc::video::VideoRef;
use anicraft_doc::{BitmapId, ColorEffect, Element, ElementId, ElementKind, Keyframe, Layer, LayerId, LayerKind, Paint, Shape, SoundId};
use anicraft_geom::{Affine, FillRule};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "file.import_video",
            label: "Import Video…",
            menu: "File/Import",
            shortcut: None,
            params: "{path (any video your ffmpeg reads: .mp4, .mov, .webm, .mkv, .avi, .gif…), mode? (frames (default): the video's frames become bitmaps on a new layer from the playhead, one keyframe each at the document's frame rate, with its sound as a stream sound on a layer of its own | reference: a locked guide layer right under the active layer (under its folder, if it is in one) that shows the video beneath what you draw, for tracing; the video stays a file and is in no export), step? (frames: one picture every 1–60 document frames; default 1), max_edge? (frames: longest edge of the pictures in pixels, 16–4096; default 1280; a smaller video is never enlarged), start? (seconds into the video to begin at), length? (frames: seconds of video to take), max_frames? (frames: most pictures to take, 1–2000), audio? (frames: default true), ffmpeg? (path; else ANICRAFT_FFMPEG, else ffmpeg on the PATH)} → {mode, layer, frames, width, height, step, truncated, sound, note}. Decoding is done by your ffmpeg (ADR 0116); at most 2000 pictures and 1 GB of decoded pixels are taken, and the rest is left out (truncated)",
            mutates: true,
            run: import_video,
        },
        CommandSpec {
            id: "file.video_info",
            label: "Video Information",
            menu: "",
            shortcut: None,
            params: "{path, step?, max_edge?, start?, length?, max_frames?, ffmpeg?} what importing the video with these settings would take, before anything is decoded → {width, height, seconds, fps, audio, plan: {frames, width, height, limit, truncated, memory (bytes of decoded pixels)}}",
            mutates: false,
            run: video_info,
        },
        CommandSpec {
            id: "layer.set_video",
            label: "Reference Video",
            menu: "",
            shortcut: None,
            params: "{layer?, path? (relink: another file, or the same one where it moved), start? (the timeline frame, from 0, where the video begins), offset? (seconds into the video shown there), remove? (true: the layer keeps its artwork and loses the video), ffmpeg?} changes a layer's reference video (ADR 0116) → {layer, video}",
            mutates: true,
            run: set_video,
        },
    ]
}

fn seconds(p: &Value, key: &str) -> Result<Option<f64>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => match v.as_f64().filter(|x| x.is_finite() && (0.0..=anicraft_doc::video::MAX_VIDEO_SECONDS).contains(x)) {
            Some(x) => Ok(Some(x)),
            None => Err(EngineError::Param(format!("param '{key}' must be seconds, from 0 to 86400"))),
        },
    }
}

fn file_stem(path: &Path) -> String {
    let stem: String = path.file_stem().map_or_else(String::new, |n| n.to_string_lossy().trim().chars().take(96).collect());
    if stem.is_empty() { "Video".into() } else { stem }
}

/// The import the params ask for, checked. Nothing is started.
pub(crate) fn request(s: &Session, p: &Value) -> Result<Request> {
    let path = PathBuf::from(str_req(p, "path")?);
    if crate::vfs::in_memory() {
        return Err(EngineError::State("importing video runs ffmpeg, which can't run in a web browser: use the desktop app, or import the frames as PNG images".into()));
    }
    let reference = match opt_str(p, "mode")? {
        None | Some("frames") => false,
        Some("reference") => true,
        Some(other) => return Err(EngineError::Param(format!("param 'mode' must be frames or reference, not '{other}'"))),
    };
    let step = opt_u32(p, "step")?.unwrap_or(1);
    if !(1..=video::MAX_STEP).contains(&step) {
        return Err(EngineError::Param(format!("param 'step' must be between 1 and {}", video::MAX_STEP)));
    }
    let max_edge = opt_u32(p, "max_edge")?.unwrap_or(video::DEFAULT_EDGE);
    if !(16..=video::MAX_DECODE_EDGE).contains(&max_edge) {
        return Err(EngineError::Param(format!("param 'max_edge' must be between 16 and {}", video::MAX_DECODE_EDGE)));
    }
    let start = seconds(p, "start")?.unwrap_or(0.0);
    let length = seconds(p, "length")?;
    if length.is_some_and(|l| l <= 0.0) {
        return Err(EngineError::Param("param 'length' must be more than 0 seconds".into()));
    }
    let max_frames = opt_u32(p, "max_frames")?.unwrap_or(video::MAX_EMBED_FRAMES);
    if !(1..=video::MAX_EMBED_FRAMES).contains(&max_frames) {
        return Err(EngineError::Param(format!("param 'max_frames' must be between 1 and {}", video::MAX_EMBED_FRAMES)));
    }
    // The timeline's room from the playhead.
    let room = MAX_FRAME.saturating_sub(s.frame) / step;
    if room == 0 {
        return Err(EngineError::State("the playhead is at the end of the timeline: there is no room for a video".into()));
    }
    if !crate::vfs::is_file(&path) {
        return Err(EngineError::Param(format!("{} is not a file", path.display())));
    }
    let stage = s.doc.stage;
    Ok(Request {
        path,
        ffmpeg: crate::cmds_file::ffmpeg_program(p)?,
        reference,
        options: Options { step, max_edge, start, length, max_frames: max_frames.min(room) },
        audio: opt_bool(p, "audio")?.unwrap_or(true),
        fps: stage.fps,
        stage: (stage.width, stage.height),
    })
}

fn plan_json(plan: &video::Plan) -> Value {
    json!({"frames": plan.frames, "width": plan.width, "height": plan.height, "limit": plan.limit, "truncated": plan.truncated, "memory": plan.memory, "seconds": plan.seconds})
}

fn video_info(s: &mut Session, p: &Value) -> Result<Value> {
    let req = request(s, p)?;
    let probe = video::probe(&req.ffmpeg, &req.path)?;
    let plan = video::plan(&probe, req.fps, req.stage, &req.options);
    Ok(json!({"width": probe.width, "height": probe.height, "seconds": probe.duration, "fps": probe.fps, "audio": probe.audio, "plan": plan_json(&plan)}))
}

/// Import Video: the slow part (ffmpeg) first, then the document changes in one step. A
/// result the editor prepared for the same request (with progress and Cancel) is taken as it
/// is; a failure or a cancel before this point leaves the document untouched.
fn import_video(s: &mut Session, p: &Value) -> Result<Value> {
    let req = request(s, p)?;
    let staged = s.staged_video.take().filter(|x| x.request == req);
    if s.timeline().layers.len() + 2 > 1000 {
        return Err(EngineError::State("too many layers (limit 1000)".into()));
    }
    if req.reference && !s.editing.is_empty() {
        return Err(EngineError::State("a reference video goes on a scene's timeline: leave the symbol first".into()));
    }
    let prepared = match staged {
        Some(x) => x,
        None => video::prepare(&req, s.job.as_deref())?,
    };
    apply(s, prepared)
}

/// Puts a prepared import in the document.
fn apply(s: &mut Session, prepared: Prepared) -> Result<Value> {
    let Prepared { request: req, plan, width, height, what, .. } = prepared;
    let name = file_stem(&req.path);
    let at = s.frame;
    let stage = s.doc.stage;
    // Shown at its own size (smaller on a smaller stage), in the middle of the stage.
    let up = if plan.upscale.is_finite() { plan.upscale.clamp(1.0, 1024.0) } else { 1.0 };
    let (shown_w, shown_h) = (width as f32 * up, height as f32 * up);
    let place = Affine::translate((stage.width as f32 - shown_w) * 0.5, (stage.height as f32 - shown_h) * 0.5);
    match what {
        What::Reference { duration } => {
            let path = std::path::absolute(&req.path).unwrap_or_else(|_| req.path.clone());
            let v = VideoRef {
                id: ElementId(s.doc.alloc_id()),
                path: path.to_string_lossy().into_owned(),
                width: (shown_w.round() as u32).clamp(1, anicraft_doc::video::MAX_VIDEO_EDGE),
                height: (shown_h.round() as u32).clamp(1, anicraft_doc::video::MAX_VIDEO_EDGE),
                transform: place,
                start: at,
                offset: req.options.start,
                duration,
            };
            let v = v.sanitized().ok_or_else(|| EngineError::Format(format!("{} can't be a reference video", req.path.display())))?;
            let frames = v.frames(stage.fps).min(MAX_FRAME.saturating_sub(at)).max(1);
            let id = LayerId(s.doc.alloc_id());
            let mut layer = Layer::new(id, name);
            // A guide layer: never in the movie. Locked, so nothing is drawn on it by mistake
            // (artwork on a guide layer isn't exported either).
            layer.kind = LayerKind::Guide;
            layer.locked = true;
            layer.keyframes = vec![Arc::new(Keyframe::new(0, at + frames))];
            let result = json!({"mode": "reference", "layer": id.0, "frames": frames, "width": v.width, "height": v.height, "seconds": duration, "path": v.path});
            layer.video = Some(Arc::new(v));
            let active = s.active_layer_index();
            let tl = crate::editing::edited_mut(&mut s.doc, s.scene, &s.editing)?;
            layer.color = crate::cmds_timeline::layer_color(tl.layers.len() + 1);
            // Right under the layer being drawn on (under its whole folder or mask group, at
            // the top level): what is traced sits on the video, and a backdrop further down
            // doesn't cover it. That layer stays active.
            let mut top = active.unwrap_or(0).min(tl.layers.len().saturating_sub(1));
            for _ in 0..=anicraft_doc::MAX_LAYER_DEPTH {
                let Some(parent) = tl.layers.get(top).and_then(|l| l.parent) else { break };
                let Some(i) = tl.layers.iter().position(|l| l.id == parent) else { break };
                top = i;
            }
            let at = tl.group_range(top).end.max(top + 1).min(tl.layers.len());
            tl.layers.insert(at, Arc::new(layer));
            tl.repair_masks();
            Ok(result)
        }
        What::Frames { bitmaps, sound, truncated, note } => {
            let step = req.options.step.max(1);
            // The Library keeps the pictures (and the sound) together in a folder.
            let folder = s.doc.ensure_folder_path(&s.doc.unique_name(None, &name));
            let mut sound_id = None;
            if let Some(mut snd) = sound {
                let id = SoundId(s.doc.alloc_id());
                snd.id = id;
                snd.name = s.doc.unique_name(folder, &format!("{name} sound"));
                s.doc.sounds.push(Arc::new(snd));
                s.doc.place_entry(anicraft_doc::library::LibraryEntry::Item(anicraft_doc::library::LibraryItem::Sound(id)), folder);
                // Under the pictures' layer, a stream sound from the playhead: it keeps time
                // with the frames, as an embedded video's sound does.
                crate::cmds_timeline::new_layer(s, &json!({"name": format!("{name} sound")}))?;
                // The sound starts where the pictures do: on a keyframe of its own there.
                let layer = s.layer_mut(None)?;
                if layer.keyframe_at(at).is_none_or(|k| k.start != at) {
                    crate::cmds_timeline::insert_keyframe_at(layer, at, false)?;
                }
                crate::cmds_sound::place(s, &json!({"sound": id.0, "frame": at, "extend": true, "sync": "stream"}))?;
                sound_id = Some(id.0);
            }
            crate::cmds_timeline::new_layer(s, &json!({"name": name}))?;
            let mut keys: Vec<Arc<Keyframe>> = Vec::with_capacity(bitmaps.len() + 1);
            if at > 0 {
                keys.push(Arc::new(Keyframe::new(0, at)));
            }
            let mut first = None;
            let count = bitmaps.len();
            for (j, mut b) in bitmaps.into_iter().enumerate() {
                // Picture `j` shows from frame `at + j × step`, for `step` frames: exactly.
                let Some(frame) = (j as u32).checked_mul(step).and_then(|d| at.checked_add(d)).filter(|f| *f < MAX_FRAME) else { break };
                let bid = BitmapId(s.doc.alloc_id());
                b.id = bid;
                b.name = format!("{name} {:04}", j + 1);
                let shape = Shape {
                    path: Arc::new(anicraft_geom::Path::rect(0.0, 0.0, b.width as f32, b.height as f32)),
                    fill: Some(Paint::Bitmap { bitmap: bid, repeat: false, matrix: Affine::IDENTITY }),
                    stroke: None,
                    fill_rule: FillRule::NonZero,
                    merge: false,
                    warp: None,
                    fill_tag: None,
                    stroke_tag: None,
                };
                let eid = ElementId(s.doc.alloc_id());
                let el = Element { id: eid, name: b.name.clone(), transform: place.mul(&Affine::scale(up, up)), color: ColorEffect::opacity(1.0), transform_3d: None, kind: ElementKind::Shape(shape) };
                first.get_or_insert(eid);
                s.doc.bitmaps.push(Arc::new(b));
                s.doc.place_entry(anicraft_doc::library::LibraryEntry::Item(anicraft_doc::library::LibraryItem::Bitmap(bid)), folder);
                let mut key = Keyframe::new(frame, step);
                key.elements = vec![Arc::new(el)];
                keys.push(Arc::new(key));
            }
            let layer = s.layer_mut(None)?;
            layer.keyframes = keys;
            layer.normalize();
            let lid = layer.id;
            s.select_only(first);
            Ok(json!({"mode": "frames", "layer": lid.0, "frames": count, "width": width, "height": height, "step": step, "from": at, "truncated": truncated, "sound": sound_id, "note": note, "memory": plan.memory}))
        }
    }
}

/// A layer's reference video as JSON (`null` without one).
pub(crate) fn video_json(l: &Layer) -> Value {
    match l.video.as_deref() {
        Some(v) => json!({"path": v.path, "width": v.width, "height": v.height, "start": v.start, "offset": v.offset, "seconds": v.duration, "missing": !crate::vfs::in_memory() && !crate::vfs::is_file(Path::new(&v.path))}),
        None => Value::Null,
    }
}

fn set_video(s: &mut Session, p: &Value) -> Result<Value> {
    let id = opt_u64(p, "layer")?.map_or(s.layer, LayerId);
    let layer = s.timeline().layer(id).ok_or_else(|| EngineError::State(format!("no layer with id {}", id.0)))?;
    let Some(old) = layer.video.as_deref() else {
        return Err(EngineError::State(format!("'{}' has no reference video: add one with File › Import › Import Video (file.import_video {{\"mode\": \"reference\"}})", layer.name)));
    };
    let mut v = old.clone();
    if opt_bool(p, "remove")?.unwrap_or(false) {
        s.layer_mut(Some(id))?.video = None;
        return Ok(json!({"layer": id.0, "video": Value::Null}));
    }
    if let Some(path) = opt_str(p, "path")? {
        // Another file: ffmpeg says what it is. The picture keeps its top-left corner.
        let mut q = p.clone();
        if let Some(o) = q.as_object_mut() {
            o.insert("mode".into(), json!("reference"));
            o.insert("path".into(), json!(path));
            o.retain(|k, _| matches!(k.as_str(), "mode" | "path" | "ffmpeg"));
        }
        let req = request(s, &q)?;
        let prepared = video::prepare(&req, None)?;
        let What::Reference { duration } = prepared.what else { return Err(EngineError::State("the video could not be read".into())) };
        let up = if prepared.plan.upscale.is_finite() { prepared.plan.upscale.clamp(1.0, 1024.0) } else { 1.0 };
        v.path = std::path::absolute(&req.path).unwrap_or(req.path).to_string_lossy().into_owned();
        v.width = ((prepared.width as f32 * up).round() as u32).max(1);
        v.height = ((prepared.height as f32 * up).round() as u32).max(1);
        v.duration = duration;
    }
    if let Some(start) = opt_u32(p, "start")? {
        if start >= MAX_FRAME {
            return Err(EngineError::Param(format!("param 'start' must be below {MAX_FRAME}")));
        }
        v.start = start;
    }
    if let Some(offset) = seconds(p, "offset")? {
        if offset >= v.duration {
            return Err(EngineError::Param(format!("param 'offset' must be inside the video ({:.2} s long)", v.duration)));
        }
        v.offset = offset;
    }
    let v = v.sanitized().ok_or_else(|| EngineError::Param("that is not a video AniCraft can refer to".into()))?;
    let layer = s.layer_mut(Some(id))?;
    layer.video = Some(Arc::new(v));
    Ok(json!({"layer": id.0, "video": video_json(layer)}))
}
