//! Auto Rig Studio commands: Hybrid CV + Deterministic Geometry character rigging,
//! interactive joint editing, IK/FK posing, facial/lip-sync controls, reusable rig presets,
//! and deterministic geometric rig validation.

use crate::access::{self, Access};
use crate::cmds_timeline::insert_keyframe_at;
use crate::params::*;
use crate::{CommandSpec, EngineError, KeyEdit, MAX_FRAME, Result, Session, vfs};
use anicraft_doc::armature::{self, Armature, Bone, BoneId, Limits};
use anicraft_doc::audio::analysis::{self, LipOptions, Span, Viseme12};
use anicraft_doc::autorig::{
    CharacterRig, CharacterStyle, DEFAULT_LOW_CONFIDENCE, FacePose, RigBoneBinding, RigId,
    RigJoint, RigSourceKind, RigViseme, SkinnedMesh, VertexWeight, ViewAngle, deform_skinned_mesh,
    face_pose_at,
};
use anicraft_doc::{
    Bitmap, BitmapId, ColorEffect, Element, ElementId, ElementKind, Keyframe, Layer, LayerId,
    LayerKind, Paint, Rgba, Shape, SoundSync, Tween, evaluate_layer_elements,
};
use anicraft_geom::{Affine, FillRule, Path as GeomPath, Rect, Vec2, segment_distance};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};

pub fn specs() -> Vec<CommandSpec> {
    let c = |id, label, menu, params, mutates, run| CommandSpec {
        id,
        label,
        menu,
        shortcut: None,
        params,
        mutates,
        run,
    };
    vec![
        c(
            "rig.auto_detect",
            "Auto-Rig Character…",
            "Modify",
            "{path?: .png or .svg character file, element?: shape id on stage, layer?: layer id, name?, x?, y?, scale?, style_override?} → {rig_id, layer, element, character_style, view_angle, overall_confidence, warnings, joints, bones, triangles}",
            true,
            auto_detect as crate::RunFn,
        ),
        c(
            "rig.set_joint",
            "Adjust Rig Joint",
            "",
            "{joint: joint id (e.g. 'elbow_l'), x, y, space?: 'stage'|'local' (default 'stage'), layer?} moves a joint dot, updates normalized [0..1] UV and fixed bone lengths, and refreshes skin weights",
            true,
            set_joint,
        ),
        c(
            "rig.rebind",
            "Rebuild Rig Skin Weights",
            "Modify",
            "{layer?} recomputes normalized skin weights around current joint locations while preserving original artwork",
            true,
            rebind_cmd,
        ),
        c(
            "rig.pose_fk",
            "Pose Rig Joint (FK)",
            "",
            "{joint?: joint id, bone?: bone id, angle_deg?: f32, rotate_by_deg?: f32, frame?, layer?} rotates a bone within its anatomical limits with strictly fixed bone length",
            true,
            pose_fk,
        ),
        c(
            "rig.pose_ik",
            "Pose Rig Chain (IK)",
            "",
            "{joint?: joint id, bone?: bone id, x: stage px, y: stage px, frame?, layer?} solves constrained CCD IK to target while keeping fixed bone lengths and joint limits",
            true,
            pose_ik,
        ),
        c(
            "rig.set_face",
            "Set Facial Rig Pose",
            "",
            "{layer?, frame?, eye_x?, eye_y?, brow_l?, brow_r?, brow_tilt_l?, brow_tilt_r?, blink_l?, blink_r?, jaw_open?, smile?, viseme?} keys independent eye, eyebrow, eyelid, jaw, mouth and viseme controls",
            true,
            set_face,
        ),
        c(
            "rig.lip_sync",
            "Auto Lip-Sync Character Rig",
            "Modify",
            "{layer?, sound_layer?, threshold?, hold?, from?, to?} analyses timeline speech audio and keys facial visemes & jaw motion across the speech span",
            true,
            lip_sync_rig,
        ),
        c(
            "rig.save_preset",
            "Save Character Rig Preset…",
            "",
            "{path: .json preset output file, layer?} writes permanent reusable rig with pixel & normalized [0..1] coordinates",
            false,
            save_preset,
        ),
        c(
            "rig.load_preset",
            "Load Character Rig Preset…",
            "",
            "{path: .json preset file, artwork_path?: optional .png/.svg override, x?, y?, scale?} loads a saved rig preset into the current scene",
            true,
            load_preset,
        ),
        c(
            "rig.instantiate",
            "Instantiate Reusable Character Rig",
            "",
            "{rig_id?: saved rig id in document, name?: rig name, x?, y?, scale?} places a reusable character rig from this document into the active scene",
            true,
            instantiate_rig,
        ),
        c(
            "rig.info",
            "Character Rig Info",
            "",
            "{layer?, frame?} → complete rig details: character style, view angle, joints (stage px, local px, normalized [0..1], confidence, warnings), bones, facial pose, and reusable rigs",
            false,
            info,
        ),
        c(
            "rig.validate",
            "Validate Character Rig",
            "",
            "{layer?, frame?} → deterministic geometric & deformation audit (fixed bone lengths, normalized skin weights, zero detached joints, zero inverted/torn mesh triangles)",
            false,
            validate,
        ),
    ]
}

// ---------------------------------------------------------------------------
// Optional External Vision Worker Bridge (`ANICRAFT_AUTORIG_VISION`)
// ---------------------------------------------------------------------------

struct VisionProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

static VISION_WORKER: OnceLock<Mutex<Option<VisionProcess>>> = OnceLock::new();

fn default_vision_paths() -> Option<(PathBuf, PathBuf)> {
    if let Some(ov) = std::env::var_os("ANICRAFT_AUTORIG_VISION") {
        let script = PathBuf::from(ov);
        if script.is_file() {
            let py = script
                .parent()
                .map(|d| d.join(".venv").join("bin").join("python"))
                .filter(|p| p.is_file())
                .unwrap_or_else(|| PathBuf::from("python3"));
            return Some((py, script));
        }
    }
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share")))?;
    let dir = base.join("anicraft").join("autorig-vision");
    let py = dir.join(".venv").join("bin").join("python");
    let script = dir.join("autorig_vision_server.py");
    if py.is_file() && script.is_file() {
        Some((py, script))
    } else {
        None
    }
}

fn call_vision_worker(input_file: &Path, clean_png_out: &Path) -> Option<Value> {
    if cfg!(target_arch = "wasm32") || vfs::in_memory() {
        return None;
    }
    let (py, script) = default_vision_paths()?;
    let mutex = VISION_WORKER.get_or_init(|| Mutex::new(None));
    let mut guard = mutex.lock().unwrap_or_else(|e| e.into_inner());

    if guard.is_none() {
        let mut child = Command::new(&py)
            .arg(&script)
            .arg("serve")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let stdin = child.stdin.take()?;
        let stdout = child.stdout.take()?;
        let mut reader = BufReader::new(stdout);
        let mut greeting = String::new();
        if reader.read_line(&mut greeting).ok()? == 0 {
            let _ = child.kill();
            return None;
        }
        *guard = Some(VisionProcess { child, stdin, stdout: reader, next_id: 1 });
    }

    let proc = guard.as_mut()?;
    let req_id = proc.next_id;
    proc.next_id = proc.next_id.saturating_add(1);
    let req = json!({
        "id": req_id,
        "op": "analyze",
        "input": input_file.to_string_lossy(),
        "clean_png": clean_png_out.to_string_lossy(),
    });
    let send_ok = writeln!(proc.stdin, "{req}").and_then(|_| proc.stdin.flush()).is_ok();
    if !send_ok {
        if let Some(mut dead) = guard.take() {
            let _ = dead.child.kill();
        }
        return None;
    }
    let mut line = String::new();
    if proc.stdout.read_line(&mut line).ok()? == 0 {
        if let Some(mut dead) = guard.take() {
            let _ = dead.child.kill();
        }
        return None;
    }
    let parsed: Value = serde_json::from_str(&line).ok()?;
    if parsed.get("ok").and_then(Value::as_bool) == Some(true) {
        parsed.get("result").cloned()
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Deterministic Pure-Rust Contour & Medial-Axis Fallback (Offline / In-Memory)
// ---------------------------------------------------------------------------

fn fallback_geometric_rig_spec(width: u32, height: u32, bounds: Rect, is_svg: bool) -> Value {
    let w = width.max(1) as f32;
    let h = height.max(1) as f32;
    let bx0 = bounds.min.x.clamp(0.0, w);
    let by0 = bounds.min.y.clamp(0.0, h);
    let bw = (bounds.max.x - bounds.min.x).max(8.0);
    let bh = (bounds.max.y - bounds.min.y).max(16.0);
    let pt = |u: f32, v: f32| -> (f32, f32, f32, f32) {
        (bx0 + u * bw, by0 + v * bh, u.clamp(0.0, 1.0), v.clamp(0.0, 1.0))
    };
    let specs: &[(&str, Option<&str>, f32, f32)] = &[
        ("pelvis", None, 0.50, 0.52),
        ("spine", Some("pelvis"), 0.50, 0.39),
        ("chest", Some("spine"), 0.50, 0.26),
        ("neck", Some("chest"), 0.50, 0.18),
        ("head", Some("neck"), 0.50, 0.10),
        ("head_tip", Some("head"), 0.50, 0.02),
        ("clavicle_r", Some("chest"), 0.41, 0.24),
        ("shoulder_r", Some("clavicle_r"), 0.32, 0.24),
        ("elbow_r", Some("shoulder_r"), 0.21, 0.36),
        ("wrist_r", Some("elbow_r"), 0.12, 0.47),
        ("clavicle_l", Some("chest"), 0.59, 0.24),
        ("shoulder_l", Some("clavicle_l"), 0.68, 0.24),
        ("elbow_l", Some("shoulder_l"), 0.79, 0.36),
        ("wrist_l", Some("elbow_l"), 0.88, 0.47),
        ("hip_r", Some("pelvis"), 0.41, 0.53),
        ("knee_r", Some("hip_r"), 0.39, 0.74),
        ("ankle_r", Some("knee_r"), 0.37, 0.93),
        ("hip_l", Some("pelvis"), 0.59, 0.53),
        ("knee_l", Some("hip_l"), 0.61, 0.74),
        ("ankle_l", Some("knee_l"), 0.63, 0.93),
        ("eye_r", Some("head"), 0.45, 0.09),
        ("eye_l", Some("head"), 0.55, 0.09),
        ("eyebrow_r", Some("head"), 0.45, 0.07),
        ("eyebrow_l", Some("head"), 0.55, 0.07),
        ("jaw", Some("head"), 0.50, 0.13),
        ("mouth_center", Some("jaw"), 0.50, 0.14),
    ];

    let mut joints = Vec::new();
    let mut map: HashMap<&str, (f32, f32)> = HashMap::new();
    for &(id, parent, u, v) in specs {
        let (x, y, nu, nv) = pt(u, v);
        map.insert(id, (x, y));
        joints.push(json!({
            "id": id,
            "parent": parent,
            "x": x,
            "y": y,
            "norm_uv": [nu, nv],
            "confidence": 0.72,
            "low_confidence": false,
            "present": true,
            "warning": Value::Null,
        }));
    }

    let mut bones = Vec::new();
    for &(id, parent, _, _) in specs {
        let Some(par) = parent else { continue };
        let Some(&(hx, hy)) = map.get(par) else { continue };
        let Some(&(tx, ty)) = map.get(id) else { continue };
        let len = ((tx - hx).hypot(ty - hy)).max(1.0);
        let deform = !matches!(id, "eye_l" | "eye_r" | "eyebrow_l" | "eyebrow_r" | "mouth_center");
        bones.push(json!({
            "id": format!("bone_{par}_to_{id}"),
            "from_joint": par,
            "to_joint": id,
            "head": [hx, hy],
            "tail": [tx, ty],
            "rest_length_px": len,
            "fixed_length": true,
            "limits_deg": [-90.0, 90.0],
            "deform": deform,
        }));
    }

    // Build clean grid mesh across bounds
    let nx = 10u32;
    let ny = 16u32;
    let mut verts = Vec::new();
    for jy in 0..=ny {
        for ix in 0..=nx {
            verts.push([
                bx0 + bw * (ix as f32 / nx as f32),
                by0 + bh * (jy as f32 / ny as f32),
            ]);
        }
    }
    let mut tris = Vec::new();
    for jy in 0..ny {
        for ix in 0..nx {
            let a = jy * (nx + 1) + ix;
            let b = a + 1;
            let c = a + (nx + 1);
            let d = c + 1;
            tris.push([a, b, d]);
            tris.push([a, d, c]);
        }
    }

    json!({
        "source_kind": if is_svg { "svg_vector" } else { "png_bitmap" },
        "image_size": [width.max(1), height.max(1)],
        "character_bounds_px": [bx0, by0, bw, bh],
        "character_style": if is_svg { "stickman" } else { "stylized_human" },
        "view_angle": "front",
        "overall_confidence": 0.72,
        "warnings": ["Fallback geometric rig used (in-memory or CV worker offline)"],
        "joints": joints,
        "bones": bones,
        "skin_mesh": {
            "rest_vertices": verts,
            "triangles": tris,
            "deform_bone_ids": [],
            "weights": [],
        }
    })
}

/// Recomputes normalized Geodesic + Finger-Localized + Bilaterally-Isolated skin weights
/// over the silhouette triangle graph `triangles` (so weights NEVER bleed across air gaps,
/// left/right limbs, or 20-to-1 finger clusters).
fn compute_local_skin_weights(
    rest_vertices: &[Vec2],
    triangles: &[[u32; 3]],
    deform_segments: &[(BoneId, Vec2, Vec2, &str)],
) -> Vec<Vec<VertexWeight>> {
    use std::cmp::Ordering;
    use std::collections::BinaryHeap;

    #[derive(Copy, Clone)]
    struct Node {
        cost: f32,
        v: usize,
    }
    impl PartialEq for Node {
        fn eq(&self, other: &Self) -> bool {
            self.cost.to_bits() == other.cost.to_bits() && self.v == other.v
        }
    }
    impl Eq for Node {}
    impl Ord for Node {
        fn cmp(&self, other: &Self) -> Ordering {
            other.cost.total_cmp(&self.cost)
        }
    }
    impl PartialOrd for Node {
        fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
            Some(self.cmp(other))
        }
    }

    let n_verts = rest_vertices.len();
    let n_bones = deform_segments.len();
    if n_bones == 0 || n_verts == 0 {
        return vec![Vec::new(); n_verts];
    }

    // Build undirected mesh adjacency list from silhouette triangles
    let mut adj: Vec<Vec<(usize, f32)>> = vec![Vec::new(); n_verts];
    for &[i0, i1, i2] in triangles {
        let (a, b, c) = (i0 as usize, i1 as usize, i2 as usize);
        if a < n_verts && b < n_verts && c < n_verts {
            for (u, v) in [(a, b), (b, c), (c, a)] {
                let d = (rest_vertices[u] - rest_vertices[v]).length().max(0.5);
                if !adj[u].iter().any(|&(nb, _)| nb == v) {
                    adj[u].push((v, d));
                    adj[v].push((u, d));
                }
            }
        }
    }

    let mut raw_matrix = vec![vec![0.0f32; n_bones]; n_verts];
    let mut dist = vec![f32::INFINITY; n_verts];
    let mut heap = BinaryHeap::new();

    for (b_idx, &(_, head, tail, to_joint)) in deform_segments.iter().enumerate() {
        let b_len = (tail - head).length().max(2.0);
        let is_finger = ["thumb", "index", "middle", "ring", "pinky"]
            .iter()
            .any(|k| to_joint.contains(k));
        let seed_r = if is_finger {
            (b_len * 0.45).clamp(4.0, 12.0)
        } else {
            (b_len * 0.18).clamp(8.0, 20.0)
        };
        let sigma = if is_finger {
            (b_len * 0.85).max(6.0)
        } else {
            (b_len * 0.45).clamp(16.0, 36.0)
        };

        dist.fill(f32::INFINITY);
        heap.clear();
        let mut best_seed = 0usize;
        let mut best_d = f32::INFINITY;
        for (vi, &vp) in rest_vertices.iter().enumerate() {
            let ed = segment_distance(vp, head, tail);
            if ed < best_d {
                best_d = ed;
                best_seed = vi;
            }
            if ed <= seed_r {
                dist[vi] = ed;
                heap.push(Node { cost: ed, v: vi });
            }
        }
        if heap.is_empty() {
            dist[best_seed] = best_d;
            heap.push(Node { cost: best_d, v: best_seed });
        }

        while let Some(Node { cost, v }) = heap.pop() {
            if cost > dist[v] {
                continue;
            }
            for &(nb, w_edge) in &adj[v] {
                let next = cost + w_edge;
                if next < dist[nb] {
                    dist[nb] = next;
                    heap.push(Node { cost: next, v: nb });
                }
            }
        }

        let max_finger_d = (b_len * 2.1).max(18.0);
        for vi in 0..n_verts {
            let gd = if dist[vi].is_finite() {
                dist[vi]
            } else if triangles.is_empty() {
                segment_distance(rest_vertices[vi], head, tail)
            } else {
                continue;
            };
            if is_finger && gd > max_finger_d {
                continue;
            }
            let q = gd / sigma + 0.35;
            let mut w = 1.0 / (q * q * q * q.sqrt());
            if is_finger {
                w *= 0.20;
            }
            if w.is_finite() && w > 0.0 {
                raw_matrix[vi][b_idx] = w;
            }
        }
    }

    // Bilateral left / right limb isolation
    let left_idxs: Vec<usize> = deform_segments
        .iter()
        .enumerate()
        .filter_map(|(i, s)| s.3.ends_with("_l").then_some(i))
        .collect();
    let right_idxs: Vec<usize> = deform_segments
        .iter()
        .enumerate()
        .filter_map(|(i, s)| s.3.ends_with("_r").then_some(i))
        .collect();
    if !left_idxs.is_empty() && !right_idxs.is_empty() {
        for row in &mut raw_matrix {
            let l_sum: f32 = left_idxs.iter().map(|&i| row[i]).sum();
            let r_sum: f32 = right_idxs.iter().map(|&i| row[i]).sum();
            if l_sum >= r_sum * 1.15 {
                for &ri in &right_idxs {
                    row[ri] = 0.0;
                }
            } else if r_sum > l_sum * 1.15 {
                for &li in &left_idxs {
                    row[li] = 0.0;
                }
            }
        }
    }

    raw_matrix
        .into_iter()
        .enumerate()
        .map(|(vi, row)| {
            let mut pairs: Vec<(u16, f32)> = row
                .into_iter()
                .enumerate()
                .filter(|&(_, w)| w > 0.0)
                .map(|(i, w)| (i as u16, w))
                .collect();
            if pairs.is_empty() {
                // Nearest segment fallback for isolated halo island vertices
                let best_i = deform_segments
                    .iter()
                    .enumerate()
                    .min_by(|a, b| {
                        segment_distance(rest_vertices[vi], a.1.1, a.1.2)
                            .total_cmp(&segment_distance(rest_vertices[vi], b.1.1, b.1.2))
                    })
                    .map_or(0, |(i, _)| i as u16);
                return vec![VertexWeight(best_i, 1.0)];
            }
            pairs.sort_by(|a, b| b.1.total_cmp(&a.1));
            pairs.truncate(4);
            let sum: f32 = pairs.iter().map(|x| x.1).sum();
            let mut kept: Vec<VertexWeight> = pairs
                .into_iter()
                .filter_map(|(i, w)| {
                    let nw = w / sum;
                    (nw >= 0.015).then_some(VertexWeight(i, nw))
                })
                .collect();
            let s2: f32 = kept.iter().map(|vw| vw.1).sum();
            if s2 > 0.0 {
                for vw in &mut kept {
                    vw.1 /= s2;
                }
            } else {
                kept.push(VertexWeight(0, 1.0));
            }
            kept
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Building Native Armature + CharacterRig Inside Session
// ---------------------------------------------------------------------------

fn fresh_rig_layer_name(s: &Session, base: &str) -> String {
    let tl = s.timeline();
    if tl.layers.iter().all(|l| l.name != base) {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base}_{n}"))
        .find(|cand| tl.layers.iter().all(|l| &l.name != cand))
        .unwrap_or_else(|| base.to_string())
}

fn assemble_armature_and_rig(
    s: &mut Session,
    spec: &Value,
    target_shape: ElementId,
    bitmap_id: Option<BitmapId>,
    rig_name: String,
    style_override: Option<CharacterStyle>,
) -> Result<(Armature, CharacterRig)> {
    let img_w = spec
        .get("image_size")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(Value::as_u64)
        .unwrap_or(512) as u32;
    let img_h = spec
        .get("image_size")
        .and_then(Value::as_array)
        .and_then(|a| a.get(1))
        .and_then(Value::as_u64)
        .unwrap_or(512) as u32;

    let b_arr = spec.get("character_bounds_px").and_then(Value::as_array);
    let bx = b_arr.and_then(|a| a.first()).and_then(Value::as_f64).unwrap_or(0.0) as f32;
    let by = b_arr.and_then(|a| a.get(1)).and_then(Value::as_f64).unwrap_or(0.0) as f32;
    let bw = b_arr.and_then(|a| a.get(2)).and_then(Value::as_f64).unwrap_or(f64::from(img_w)) as f32;
    let bh = b_arr.and_then(|a| a.get(3)).and_then(Value::as_f64).unwrap_or(f64::from(img_h)) as f32;
    let bounds_px = Rect::new(bx, by, bx + bw.max(1.0), by + bh.max(1.0));

    let character_style = style_override.unwrap_or_else(|| {
        spec.get("character_style")
            .and_then(Value::as_str)
            .and_then(CharacterStyle::from_key)
            .unwrap_or_default()
    });
    let view_angle = spec
        .get("view_angle")
        .and_then(Value::as_str)
        .and_then(ViewAngle::from_key)
        .unwrap_or_default();
    let source_kind = spec
        .get("source_kind")
        .and_then(Value::as_str)
        .map_or(RigSourceKind::PngBitmap, RigSourceKind::from_key);
    let overall_confidence = spec
        .get("overall_confidence")
        .and_then(Value::as_f64)
        .unwrap_or(0.85) as f32;

    let warnings: Vec<String> = spec
        .get("warnings")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(Value::as_str).map(String::from).collect())
        .unwrap_or_default();

    // Parse joints (pixel-accurate + normalized [0..1] UV)
    let mut joints: Vec<RigJoint> = Vec::new();
    if let Some(j_arr) = spec.get("joints").and_then(Value::as_array) {
        for jv in j_arr {
            let Some(id) = jv.get("id").and_then(Value::as_str).map(String::from) else {
                continue;
            };
            let parent = jv.get("parent").and_then(Value::as_str).map(String::from);
            let x = jv.get("x").and_then(Value::as_f64).unwrap_or(0.0) as f32;
            let y = jv.get("y").and_then(Value::as_f64).unwrap_or(0.0) as f32;
            let pos_px = Vec2::new(x, y);
            let norm_uv = jv
                .get("norm_uv")
                .and_then(Value::as_array)
                .and_then(|uv| {
                    Some(Vec2::new(
                        uv.first()?.as_f64()? as f32,
                        uv.get(1)?.as_f64()? as f32,
                    ))
                })
                .unwrap_or_else(|| {
                    Vec2::new(
                        ((x - bx) / bw.max(1.0)).clamp(0.0, 1.0),
                        ((y - by) / bh.max(1.0)).clamp(0.0, 1.0),
                    )
                });
            let confidence = jv.get("confidence").and_then(Value::as_f64).unwrap_or(0.8) as f32;
            let present = jv.get("present").and_then(Value::as_bool).unwrap_or(true);
            let low_confidence = present
                && jv
                    .get("low_confidence")
                    .and_then(Value::as_bool)
                    .unwrap_or(confidence < DEFAULT_LOW_CONFIDENCE);
            let warning = jv.get("warning").and_then(Value::as_str).map(String::from);
            joints.push(RigJoint {
                id,
                parent,
                pos_px,
                norm_uv,
                confidence,
                low_confidence,
                present,
                manual_override: false,
                warning,
            });
        }
    }

    // Parse connected bones in topological parent-first order and allocate native `armature::Bone`s
    let mut raw_bones: Vec<RigBoneBinding> = Vec::new();
    if let Some(b_list) = spec.get("bones").and_then(Value::as_array) {
        for bv in b_list {
            let Some(id) = bv.get("id").and_then(Value::as_str).map(String::from) else {
                continue;
            };
            let Some(from_joint) = bv.get("from_joint").and_then(Value::as_str).map(String::from) else {
                continue;
            };
            let Some(to_joint) = bv.get("to_joint").and_then(Value::as_str).map(String::from) else {
                continue;
            };
            let h_arr = bv.get("head").and_then(Value::as_array);
            let t_arr = bv.get("tail").and_then(Value::as_array);
            let head = Vec2::new(
                h_arr.and_then(|a| a.first()).and_then(Value::as_f64).unwrap_or(0.0) as f32,
                h_arr.and_then(|a| a.get(1)).and_then(Value::as_f64).unwrap_or(0.0) as f32,
            );
            let tail = Vec2::new(
                t_arr.and_then(|a| a.first()).and_then(Value::as_f64).unwrap_or(0.0) as f32,
                t_arr.and_then(|a| a.get(1)).and_then(Value::as_f64).unwrap_or(0.0) as f32,
            );
            let rest_length_px = bv
                .get("rest_length_px")
                .and_then(Value::as_f64)
                .map_or_else(|| (tail - head).length(), |v| v as f32)
                .max(1.0);
            let l_arr = bv.get("limits_deg").and_then(Value::as_array);
            let min_d = l_arr.and_then(|a| a.first()).and_then(Value::as_f64).unwrap_or(-90.0) as f32;
            let max_d = l_arr.and_then(|a| a.get(1)).and_then(Value::as_f64).unwrap_or(90.0) as f32;
            let deform = bv.get("deform").and_then(Value::as_bool).unwrap_or(true);
            raw_bones.push(RigBoneBinding {
                id,
                from_joint,
                to_joint,
                bone_id: None,
                head,
                tail,
                rest_length_px,
                fixed_length: true,
                limits_deg: [min_d.min(max_d), min_d.max(max_d)],
                deform,
            });
        }
    }

    // Topological sort of bones from root (`pelvis`) outward so `armature::place` evaluates parents before children
    let mut ordered_bones: Vec<RigBoneBinding> = Vec::with_capacity(raw_bones.len());
    let mut joint_node_map: HashMap<String, ElementId> = HashMap::new();
    let mut arm_bones: Vec<Bone> = Vec::with_capacity(raw_bones.len());
    let mut remaining = raw_bones;

    for pass in 0..32 {
        if remaining.is_empty() {
            break;
        }
        let mut next_rem = Vec::new();
        let mut progressed = false;
        for mut rb in remaining {
            let parent_node = if rb.from_joint == "pelvis" || pass > 16 {
                Some(*joint_node_map.get(&rb.from_joint).unwrap_or(&target_shape))
            } else {
                joint_node_map.get(&rb.from_joint).copied()
            };
            if let Some(p_node) = parent_node {
                let bid = BoneId(s.doc.alloc_id());
                let c_node = ElementId(s.doc.alloc_id());
                joint_node_map.insert(rb.to_joint.clone(), c_node);
                let mut b = Bone::in_shape(bid, target_shape, p_node, c_node, rb.head, rb.tail);
                b.name = rb.to_joint.clone();
                // Enforce anatomical rotation limits & strictly fixed bone length (`slide_x/y: None`)
                b.limits = Limits {
                    min: rb.limits_deg[0].to_radians(),
                    max: rb.limits_deg[1].to_radians(),
                }
                .sanitized();
                b.slide_x = None;
                b.slide_y = None;
                rb.bone_id = Some(bid);
                arm_bones.push(b);
                ordered_bones.push(rb);
                progressed = true;
            } else {
                next_rem.push(rb);
            }
        }
        remaining = next_rem;
        if !progressed {
            // Attach any disconnected secondary chain directly to `target_shape`
            for mut rb in std::mem::take(&mut remaining) {
                let bid = BoneId(s.doc.alloc_id());
                let c_node = ElementId(s.doc.alloc_id());
                joint_node_map.insert(rb.to_joint.clone(), c_node);
                let mut b = Bone::in_shape(bid, target_shape, target_shape, c_node, rb.head, rb.tail);
                b.name = rb.to_joint.clone();
                b.limits = Limits {
                    min: rb.limits_deg[0].to_radians(),
                    max: rb.limits_deg[1].to_radians(),
                }
                .sanitized();
                rb.bone_id = Some(bid);
                arm_bones.push(b);
                ordered_bones.push(rb);
            }
            break;
        }
    }

    // Build SkinnedMesh from CV geodesic SAM-2 mesh (or compute local fallback weights)
    let bone_by_spec_id: HashMap<String, BoneId> = ordered_bones
        .iter()
        .filter_map(|rb| rb.bone_id.map(|bid| (rb.id.clone(), bid)))
        .collect();

    let skin_mesh = if let Some(sm) = spec.get("skin_mesh") {
        let rest_vertices: Vec<Vec2> = sm
            .get("rest_vertices")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| {
                        let a = v.as_array()?;
                        Some(Vec2::new(a.first()?.as_f64()? as f32, a.get(1)?.as_f64()? as f32))
                    })
                    .collect()
            })
            .unwrap_or_default();

        let triangles: Vec<[u32; 3]> = sm
            .get("triangles")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|t| {
                        let a = t.as_array()?;
                        Some([
                            a.first()?.as_u64()? as u32,
                            a.get(1)?.as_u64()? as u32,
                            a.get(2)?.as_u64()? as u32,
                        ])
                    })
                    .collect()
            })
            .unwrap_or_default();

        let mut deform_bones: Vec<BoneId> = sm
            .get("deform_bone_ids")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|idv| idv.as_str().and_then(|k| bone_by_spec_id.get(k).copied()))
                    .collect()
            })
            .unwrap_or_default();

        let mut weights: Vec<Vec<VertexWeight>> = sm
            .get("weights")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .map(|row| {
                        row.as_array()
                            .map(|pairs| {
                                pairs
                                    .iter()
                                    .filter_map(|pair| {
                                        let pa = pair.as_array()?;
                                        let bi = pa.first()?.as_u64()? as u16;
                                        let w = pa.get(1)?.as_f64()? as f32;
                                        ((bi as usize) < deform_bones.len() && w > 0.0)
                                            .then_some(VertexWeight(bi, w))
                                    })
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default()
                    })
                    .collect()
            })
            .unwrap_or_default();

        if deform_bones.is_empty() || weights.len() != rest_vertices.len() {
            let segs: Vec<(BoneId, Vec2, Vec2, &str)> = ordered_bones
                .iter()
                .filter(|rb| rb.deform)
                .filter_map(|rb| rb.bone_id.map(|bid| (bid, rb.head, rb.tail, rb.to_joint.as_str())))
                .collect();
            deform_bones = segs.iter().map(|s| s.0).collect();
            weights = compute_local_skin_weights(&rest_vertices, &triangles, &segs);
        }

        Some(SkinnedMesh {
            rest_vertices,
            triangles,
            deform_bones,
            weights,
        })
    } else {
        None
    };

    let rig_id = RigId(s.doc.alloc_id());
    let rig = CharacterRig {
        id: rig_id,
        name: rig_name,
        target_shape: Some(target_shape),
        bitmap_id,
        symbol_id: None,
        source_kind,
        character_style,
        view_angle,
        image_size: [img_w.max(1), img_h.max(1)],
        bounds_px,
        overall_confidence,
        warnings,
        joints,
        bones: ordered_bones,
        skin_mesh,
    };

    let arm = Armature {
        bones: arm_bones,
        skins: Vec::new(),
        springs_off: false,
        char_rig: Some(Arc::new(rig.clone())),
    };
    Ok((arm, rig))
}

// ---------------------------------------------------------------------------
// Command Implementations
// ---------------------------------------------------------------------------

fn auto_detect(s: &mut Session, p: &Value) -> Result<Value> {
    let style_override = opt_str(p, "style_override")?
        .filter(|v| !v.trim().is_empty())
        .map(|v| {
            CharacterStyle::from_key(v)
                .ok_or_else(|| EngineError::Param(format!("unknown character style '{v}'")))
        })
        .transpose()?;

    let place_x = opt_f32(p, "x")?;
    let place_y = opt_f32(p, "y")?;
    let place_scale = opt_f32(p, "scale")?.unwrap_or(1.0).clamp(0.05, 20.0);

    // Case A: Auto-rig from an external `.png` or `.svg` file
    if let Some(raw_path) = opt_str(p, "path")?.filter(|x| !x.trim().is_empty()) {
        let file_path = Path::new(raw_path);
        let bytes = vfs::read(file_path)?;
        let is_svg = file_path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("svg"));

        let base_name: String = opt_str(p, "name")?
            .filter(|n| !n.trim().is_empty())
            .map(String::from)
            .unwrap_or_else(|| {
                file_path
                    .file_stem()
                    .map_or_else(|| "Character".into(), |st| st.to_string_lossy().into_owned())
            });

        // Write temporary input & clean_png output marked as `access::own` for the CV worker
        let tmp_dir = std::env::temp_dir();
        let suffix = vfs::random_suffix();
        let tmp_in = tmp_dir.join(format!(
            "anicraft-autorig-in-{suffix}.{}",
            if is_svg { "svg" } else { "png" }
        ));
        let tmp_clean = tmp_dir.join(format!("anicraft-autorig-clean-{suffix}.png"));
        let _own_in = access::own(&tmp_in);
        let _own_clean = access::own(&tmp_clean);

        let cv_spec = if !vfs::in_memory() && std::fs::write(&tmp_in, &bytes).is_ok() {
            call_vision_worker(&tmp_in, &tmp_clean)
        } else {
            None
        };

        // Load clean chroma-keyed RGBA PNG if CV worker produced one, else original bytes
        let (target_el, bitmap_id, w, h, bounds) = if is_svg && cv_spec.is_none() {
            let imported = anicraft_svg::import(&bytes)
                .map_err(|e| EngineError::Format(format!("SVG import: {e}")))?;
            let (iw, ih) = (imported.width.max(1.0), imported.height.max(1.0));
            let merged_path = imported
                .elements()
                .find_map(|ie| match &ie.kind {
                    ElementKind::Shape(sh) => Some((*sh.path).clone()),
                    _ => None,
                })
                .unwrap_or_else(|| GeomPath::rect(0.0, 0.0, iw, ih));
            let b = merged_path.bounds();
            let eid = ElementId(s.doc.alloc_id());
            let sh = Shape::filled(merged_path, Paint::Solid(Rgba::hex(0x181820)));
            let el = Element {
                id: eid,
                name: base_name.clone(),
                transform: Affine::IDENTITY,
                color: ColorEffect::default(),
                transform_3d: None,
                kind: ElementKind::Shape(sh),
            };
            (el, None, iw as u32, ih as u32, b)
        } else {
            let png_bytes = std::fs::read(&tmp_clean).ok().unwrap_or(bytes);
            let bid = BitmapId(s.doc.alloc_id());
            let bmp = Bitmap::from_image(bid, format!("{base_name}.png"), png_bytes)
                .map_err(|e| EngineError::Format(format!("{}: {e}", file_path.display())))?;
            let (bw, bh) = (bmp.width, bmp.height);
            s.doc.bitmaps.push(Arc::new(bmp));
            let eid = ElementId(s.doc.alloc_id());
            let sh = Shape {
                path: Arc::new(GeomPath::rect(0.0, 0.0, bw as f32, bh as f32)),
                fill: Some(Paint::Bitmap {
                    bitmap: bid,
                    repeat: false,
                    matrix: Affine::IDENTITY,
                }),
                stroke: None,
                fill_rule: FillRule::NonZero,
                merge: false,
                warp: None,
                fill_tag: None,
                stroke_tag: None,
            };
            let el = Element {
                id: eid,
                name: base_name.clone(),
                transform: Affine::IDENTITY,
                color: ColorEffect::default(),
                transform_3d: None,
                kind: ElementKind::Shape(sh),
            };
            (
                el,
                Some(bid),
                bw,
                bh,
                Rect::new(0.0, 0.0, bw as f32, bh as f32),
            )
        };

        let _ = std::fs::remove_file(&tmp_in);
        let _ = std::fs::remove_file(&tmp_clean);

        let spec = cv_spec.unwrap_or_else(|| fallback_geometric_rig_spec(w, h, bounds, is_svg));
        let target_id = target_el.id;

        // Place character centered cleanly on Stage unless explicit (x, y) given
        let (sw, sh) = (s.doc.stage.width as f32, s.doc.stage.height as f32);
        let fit_scale = if place_x.is_none() && place_y.is_none() && p.get("scale").is_none() {
            ((sh * 0.86) / (h.max(1) as f32)).min((sw * 0.86) / (w.max(1) as f32)).min(1.5)
        } else {
            place_scale
        };
        let tx = place_x.unwrap_or((sw - (w as f32) * fit_scale) * 0.5);
        let ty = place_y.unwrap_or((sh - (h as f32) * fit_scale) * 0.5);

        let mut placed_el = target_el;
        placed_el.transform = Affine::translate(tx, ty).mul(&Affine::scale(fit_scale, fit_scale));

        let (arm, rig) = assemble_armature_and_rig(
            s,
            &spec,
            target_id,
            bitmap_id,
            base_name.clone(),
            style_override,
        )?;

        // Save reusable character rig template in Document::character_rigs
        Arc::make_mut(&mut s.doc.character_rigs).push(rig.clone());

        // Create new Armature Layer holding the rigged character across the timeline
        let lid = LayerId(s.doc.alloc_id());
        let lname = fresh_rig_layer_name(s, &format!("Rig_{base_name}"));
        let mut layer = Layer::new(lid, lname);
        layer.kind = LayerKind::Armature;
        layer.color = Rgba::hex(0x2fb67c);
        layer.armature = Some(Arc::new(arm));
        let dur = s.frame_count().max(1);
        let mut kf = Keyframe::new(0, dur);
        kf.elements = vec![Arc::new(placed_el)];
        kf.tween = Some(Tween::default());
        layer.keyframes = vec![Arc::new(kf)];

        let tl = crate::editing::edited_mut(&mut s.doc, s.scene, &s.editing)?;
        tl.layers.insert(0, Arc::new(layer));
        tl.repair_masks();
        s.layer = lid;
        s.selection.clear();

        return Ok(rig_summary_json(&rig, lid, target_id));
    }

    // Case B: Auto-rig an existing Shape/Bitmap element already on the active or specified layer
    let src_layer_id = opt_u64(p, "layer")?.map_or(s.layer, LayerId);
    let wanted_el = opt_u64(p, "element")?.map(ElementId);
    let (_, els) = s
        .timeline()
        .layer(src_layer_id)
        .and_then(|l| evaluate_layer_elements(l, s.frame))
        .ok_or_else(|| EngineError::State("no artwork on the active layer at this frame".into()))?;

    let found_el = els
        .iter()
        .find(|e| {
            wanted_el.is_none_or(|id| e.id == id) && matches!(e.kind, ElementKind::Shape(_))
        })
        .cloned()
        .ok_or_else(|| {
            EngineError::State(
                "select a bitmap or vector shape on the stage, or pass 'path' to rig.auto_detect"
                    .into(),
            )
        })?;

    let ElementKind::Shape(sh) = &found_el.kind else {
        return Err(EngineError::State("selected item is not a shape or bitmap".into()));
    };
    let bounds = sh.path.bounds();
    let (w, h, bmp_id, cv_spec) = match &sh.fill {
        Some(Paint::Bitmap { bitmap, .. }) => {
            let bmp = s
                .doc
                .bitmap(*bitmap)
                .ok_or_else(|| EngineError::State("missing bitmap in library".into()))?;
            let (bw, bh) = (bmp.width, bmp.height);
            let mut spec_opt = None;
            if !vfs::in_memory() {
                let suffix = vfs::random_suffix();
                let tmp_in = std::env::temp_dir().join(format!("anicraft-stage-bmp-{suffix}.png"));
                let tmp_clean =
                    std::env::temp_dir().join(format!("anicraft-stage-clean-{suffix}.png"));
                let _own1 = access::own(&tmp_in);
                let _own2 = access::own(&tmp_clean);
                if std::fs::write(&tmp_in, bmp.png.as_slice()).is_ok() {
                    spec_opt = call_vision_worker(&tmp_in, &tmp_clean);
                }
                let _ = std::fs::remove_file(&tmp_in);
                let _ = std::fs::remove_file(&tmp_clean);
            }
            (bw, bh, Some(*bitmap), spec_opt)
        }
        _ => (
            (bounds.max.x - bounds.min.x).max(64.0) as u32,
            (bounds.max.y - bounds.min.y).max(64.0) as u32,
            None,
            None,
        ),
    };

    let rig_name = opt_str(p, "name")?
        .filter(|n| !n.trim().is_empty())
        .map(String::from)
        .unwrap_or_else(|| {
            if found_el.name.is_empty() {
                "Character".into()
            } else {
                found_el.name.clone()
            }
        });

    let spec = cv_spec.unwrap_or_else(|| fallback_geometric_rig_spec(w, h, bounds, bmp_id.is_none()));
    let (arm, rig) =
        assemble_armature_and_rig(s, &spec, found_el.id, bmp_id, rig_name, style_override)?;
    Arc::make_mut(&mut s.doc.character_rigs).push(rig.clone());

    let l = s.layer_mut(Some(src_layer_id))?;
    l.kind = LayerKind::Armature;
    l.armature = Some(Arc::new(arm));
    for kf in &mut l.keyframes {
        let k = Arc::make_mut(kf);
        if k.tween.is_none() {
            k.tween = Some(Tween::default());
        }
        for e in &mut k.elements {
            if e.id == found_el.id
                && let ElementKind::Shape(s_mut) = &mut Arc::make_mut(e).kind
            {
                s_mut.merge = false;
            }
        }
    }
    s.layer = src_layer_id;
    Ok(rig_summary_json(&rig, src_layer_id, found_el.id))
}

fn rig_summary_json(rig: &CharacterRig, lid: LayerId, eid: ElementId) -> Value {
    let present_joints = rig.joints.iter().filter(|j| j.present).count();
    let low_conf_joints: Vec<&str> = rig
        .joints
        .iter()
        .filter(|j| j.present && j.low_confidence)
        .map(|j| j.id.as_str())
        .collect();
    let tri_count = rig.skin_mesh.as_ref().map_or(0, |m| m.triangles.len());
    json!({
        "rig_id": rig.id.0,
        "name": rig.name,
        "layer": lid.0,
        "element": eid.0,
        "character_style": rig.character_style.key(),
        "character_style_label": rig.character_style.label(),
        "view_angle": rig.view_angle.key(),
        "view_angle_label": rig.view_angle.label(),
        "overall_confidence": rig.overall_confidence,
        "joint_count": present_joints,
        "bone_count": rig.bones.len(),
        "triangle_count": tri_count,
        "low_confidence_joints": low_conf_joints,
        "warnings": rig.warnings,
    })
}

fn find_rig_layer(s: &Session, layer_param: Option<u64>) -> Result<(LayerId, Arc<Armature>, Arc<CharacterRig>)> {
    let lid = layer_param.map_or(s.layer, LayerId);
    let l = s
        .timeline()
        .layer(lid)
        .ok_or_else(|| EngineError::State(format!("no layer with id {}", lid.0)))?;
    let arm = l
        .armature
        .clone()
        .ok_or_else(|| EngineError::State("the active layer has no armature".into()))?;
    let rig = arm
        .char_rig
        .clone()
        .ok_or_else(|| EngineError::State("the active layer has no Auto-Rigged character".into()))?;
    Ok((lid, arm, rig))
}

fn set_joint(s: &mut Session, p: &Value) -> Result<Value> {
    let joint_id = str_req(p, "joint")?.trim().to_string();
    let x = f32_req(p, "x")?;
    let y = f32_req(p, "y")?;
    if !x.is_finite() || !y.is_finite() {
        return Err(EngineError::Param("params 'x' and 'y' must be finite numbers".into()));
    }
    let space = opt_str(p, "space")?.unwrap_or("stage");
    let (lid, arm_arc, rig_arc) = find_rig_layer(s, opt_u64(p, "layer")?)?;

    // Convert stage px -> shape local px if `space == "stage"`
    let local_pt = if space.eq_ignore_ascii_case("local") {
        Vec2::new(x, y)
    } else {
        let inv = s
            .timeline()
            .layer(lid)
            .and_then(|l| evaluate_layer_elements(l, s.frame))
            .and_then(|(_, els)| {
                let tid = rig_arc.target_shape?;
                els.iter().find(|e| e.id == tid)?.transform.inverse()
            })
            .unwrap_or(Affine::IDENTITY);
        inv.apply(Vec2::new(x, y))
    };

    let mut rig = (*rig_arc).clone();
    let new_uv = rig.to_norm_uv(local_pt);
    let j = rig
        .joint_mut(&joint_id)
        .ok_or_else(|| EngineError::Param(format!("no joint '{joint_id}' in character rig")))?;
    j.pos_px = local_pt;
    j.norm_uv = new_uv;
    j.present = true;
    j.manual_override = true;
    j.low_confidence = false;
    j.warning = None;

    // Update all connected RigBoneBindings AND native `armature::Bone`s + refresh fixed rest lengths
    let mut arm = (*arm_arc).clone();
    for rb in &mut rig.bones {
        if rb.from_joint == joint_id {
            rb.head = local_pt;
        }
        if rb.to_joint == joint_id {
            rb.tail = local_pt;
        }
        rb.rest_length_px = (rb.tail - rb.head).length().max(1.0);
        if let Some(bid) = rb.bone_id
            && let Some(b) = arm.bones.iter_mut().find(|b| b.id == bid)
        {
            b.head = rb.head;
            b.tail = rb.tail;
        }
    }

    // Only rebind skin weights globally when `rebind: true` or when weights are missing;
    // otherwise preserve SAM 2's semantic body-part weights on untouched limbs and only rebind
    // vertices whose primary influence belongs to one of the edited bone segments!
    let force_rebind = opt_bool(p, "rebind")?.unwrap_or(false);
    if let Some(sm) = &mut rig.skin_mesh {
        let touched_bone_indices: Vec<u16> = sm
            .deform_bones
            .iter()
            .enumerate()
            .filter_map(|(idx, &bid)| {
                let rb = rig.bones.iter().find(|r| r.bone_id == Some(bid))?;
                (rb.from_joint == joint_id || rb.to_joint == joint_id).then_some(idx as u16)
            })
            .collect();

        if force_rebind || sm.weights.len() != sm.rest_vertices.len() {
            let segs: Vec<(BoneId, Vec2, Vec2, &str)> = sm
                .deform_bones
                .iter()
                .filter_map(|&bid| {
                    let rb = rig.bones.iter().find(|r| r.bone_id == Some(bid))?;
                    Some((bid, rb.head, rb.tail, rb.to_joint.as_str()))
                })
                .collect();
            if !segs.is_empty() {
                sm.weights = compute_local_skin_weights(&sm.rest_vertices, &sm.triangles, &segs);
            }
        } else if !touched_bone_indices.is_empty() {
            let segs: Vec<(BoneId, Vec2, Vec2, &str)> = sm
                .deform_bones
                .iter()
                .filter_map(|&bid| {
                    let rb = rig.bones.iter().find(|r| r.bone_id == Some(bid))?;
                    Some((bid, rb.head, rb.tail, rb.to_joint.as_str()))
                })
                .collect();
            if !segs.is_empty() {
                let fresh = compute_local_skin_weights(&sm.rest_vertices, &sm.triangles, &segs);
                for (vi, row) in sm.weights.iter_mut().enumerate() {
                    let uses_touched = row
                        .iter()
                        .any(|vw| touched_bone_indices.contains(&vw.0) && vw.1 >= 0.25);
                    if uses_touched && let Some(new_row) = fresh.get(vi) {
                        *row = new_row.clone();
                    }
                }
            }
        }
    }

    // Sync saved copy in Document::character_rigs too so saved rig stays permanently updated!
    for saved in Arc::make_mut(&mut s.doc.character_rigs).iter_mut() {
        if saved.id == rig.id {
            *saved = rig.clone();
        }
    }

    arm.char_rig = Some(Arc::new(rig));
    s.layer_mut(Some(lid))?.armature = Some(Arc::new(arm));

    Ok(json!({
        "layer": lid.0,
        "joint": joint_id,
        "local_px": [local_pt.x, local_pt.y],
        "norm_uv": [new_uv.x, new_uv.y],
        "manual_override": true,
    }))
}

fn rebind_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let (lid, arm_arc, rig_arc) = find_rig_layer(s, opt_u64(p, "layer")?)?;
    let mut rig = (*rig_arc).clone();
    let mut updated_verts = 0usize;
    if let Some(sm) = &mut rig.skin_mesh {
        let segs: Vec<(BoneId, Vec2, Vec2, &str)> = rig
            .bones
            .iter()
            .filter(|rb| rb.deform)
            .filter_map(|rb| rb.bone_id.map(|bid| (bid, rb.head, rb.tail, rb.to_joint.as_str())))
            .collect();
        sm.deform_bones = segs.iter().map(|s| s.0).collect();
        sm.weights = compute_local_skin_weights(&sm.rest_vertices, &sm.triangles, &segs);
        updated_verts = sm.rest_vertices.len();
    }
    let mut arm = (*arm_arc).clone();
    arm.char_rig = Some(Arc::new(rig));
    s.layer_mut(Some(lid))?.armature = Some(Arc::new(arm));
    Ok(json!({ "layer": lid.0, "vertices_rebound": updated_verts }))
}

fn resolve_rig_bone(arm: &Armature, rig: &CharacterRig, p: &Value) -> Result<BoneId> {
    if let Some(bid) = opt_u64(p, "bone")?.map(BoneId) {
        if arm.bone(bid).is_some() {
            return Ok(bid);
        }
        return Err(EngineError::Param(format!("no bone with id {}", bid.0)));
    }
    if let Some(jname) = opt_str(p, "joint")?.filter(|s| !s.trim().is_empty()) {
        // Match bone whose `to_joint` is `jname`, or whose `from_joint` is `jname`
        if let Some(bid) = rig
            .bones
            .iter()
            .find(|b| b.to_joint == jname)
            .or_else(|| rig.bones.iter().find(|b| b.from_joint == jname))
            .and_then(|b| b.bone_id)
        {
            return Ok(bid);
        }
        return Err(EngineError::Param(format!("no bone bound to joint '{jname}'")));
    }
    Err(EngineError::Param("provide 'joint' (e.g. 'elbow_l') or 'bone' id".into()))
}

fn edit_rig_keyframe<T>(
    s: &mut Session,
    lid: LayerId,
    frame: u32,
    edit: impl FnOnce(&Armature, &mut Keyframe) -> Result<T>,
) -> Result<T> {
    if frame > MAX_FRAME {
        return Err(EngineError::Param(format!("param 'frame' must be at most {MAX_FRAME}")));
    }
    let arm = s
        .timeline()
        .layer(lid)
        .and_then(|l| l.armature.clone())
        .ok_or_else(|| EngineError::State("not an armature layer".into()))?;
    {
        let l = s.layer_mut(Some(lid))?;
        if l.keyframe_at(frame).is_none() && frame >= l.end_frame() && !l.locked {
            insert_keyframe_at(l, frame, true)?;
        }
    }
    let (prev_layer, prev_frame) = (s.layer, s.frame);
    s.layer = lid;
    s.frame = frame;
    let res = (|| {
        let kf = s.editable_keyframe_in(KeyEdit::Pose)?;
        let out = edit(&arm, kf)?;
        kf.pose = kf.pose.allowed(&arm);
        armature::apply(&arm, &mut kf.elements, &kf.pose);
        Ok(out)
    })();
    s.layer = prev_layer;
    s.frame = prev_frame;
    res
}

fn pose_fk(s: &mut Session, p: &Value) -> Result<Value> {
    let (lid, arm, rig) = find_rig_layer(s, opt_u64(p, "layer")?)?;
    let bid = resolve_rig_bone(&arm, &rig, p)?;
    let angle_deg = opt_f32(p, "angle_deg")?;
    let rotate_by_deg = opt_f32(p, "rotate_by_deg")?;
    let frame = opt_u32(p, "frame")?.unwrap_or(s.frame);

    edit_rig_keyframe(s, lid, frame, |arm_ref, kf| {
        let bone = arm_ref
            .bone(bid)
            .ok_or_else(|| EngineError::State("missing bone in armature".into()))?;
        let cur = kf.pose.angle(bid);
        let target = angle_deg.map_or(cur, f32::to_radians) + rotate_by_deg.unwrap_or(0.0).to_radians();
        let clamped = bone.allow(target);
        kf.pose.set(bid, clamped);
        Ok(json!({
            "layer": lid.0,
            "frame": frame,
            "bone": bid.0,
            "angle_deg": clamped.to_degrees(),
        }))
    })
}

fn pose_ik(s: &mut Session, p: &Value) -> Result<Value> {
    let (lid, arm, rig) = find_rig_layer(s, opt_u64(p, "layer")?)?;
    let bid = resolve_rig_bone(&arm, &rig, p)?;
    let tx = f32_req(p, "x")?;
    let ty = f32_req(p, "y")?;
    let frame = opt_u32(p, "frame")?.unwrap_or(s.frame);
    let space_inv = s.layer_space(lid).inverse().unwrap_or(Affine::IDENTITY);
    let target = space_inv.apply(Vec2::new(tx, ty));

    edit_rig_keyframe(s, lid, frame, |arm_ref, kf| {
        let bone = arm_ref
            .bone(bid)
            .ok_or_else(|| EngineError::State("missing bone in armature".into()))?;
        let solved = armature::solve(arm_ref, &kf.elements, &kf.pose, bid, bone.tail, target)
            .ok_or_else(|| EngineError::State("IK chain could not be evaluated at this frame".into()))?;
        kf.pose = solved.pose;
        Ok(json!({
            "layer": lid.0,
            "frame": frame,
            "bone": bid.0,
            "reached": solved.error <= armature::IK_TOLERANCE,
            "error_px": solved.error,
            "chain_length": solved.chain,
        }))
    })
}

fn set_face(s: &mut Session, p: &Value) -> Result<Value> {
    let (lid, _, _) = find_rig_layer(s, opt_u64(p, "layer")?)?;
    let frame = opt_u32(p, "frame")?.unwrap_or(s.frame);
    let cur_interp = s
        .timeline()
        .layer(lid)
        .map_or_else(FacePose::default, |l| face_pose_at(l, frame));

    let viseme_ov = opt_str(p, "viseme")?
        .filter(|v| !v.trim().is_empty())
        .map(|v| {
            RigViseme::from_key(v)
                .ok_or_else(|| EngineError::Param(format!("unknown viseme '{v}'")))
        })
        .transpose()?;

    let eye_x = opt_f32(p, "eye_x")?;
    let eye_y = opt_f32(p, "eye_y")?;
    let brow_l = opt_f32(p, "brow_l")?;
    let brow_r = opt_f32(p, "brow_r")?;
    let brow_tilt_l = opt_f32(p, "brow_tilt_l")?;
    let brow_tilt_r = opt_f32(p, "brow_tilt_r")?;
    let blink_l = opt_f32(p, "blink_l")?;
    let blink_r = opt_f32(p, "blink_r")?;
    let jaw_open = opt_f32(p, "jaw_open")?;
    let smile = opt_f32(p, "smile")?;

    edit_rig_keyframe(s, lid, frame, |_, kf| {
        let mut fp = kf.face_pose.unwrap_or(cur_interp);
        if let Some(v) = eye_x {
            fp.eye_gaze.x = v;
        }
        if let Some(v) = eye_y {
            fp.eye_gaze.y = v;
        }
        if let Some(v) = brow_l {
            fp.eyebrow_l = v;
        }
        if let Some(v) = brow_r {
            fp.eyebrow_r = v;
        }
        if let Some(v) = brow_tilt_l {
            fp.eyebrow_tilt_l = v;
        }
        if let Some(v) = brow_tilt_r {
            fp.eyebrow_tilt_r = v;
        }
        if let Some(v) = blink_l {
            fp.blink_l = v;
        }
        if let Some(v) = blink_r {
            fp.blink_r = v;
        }
        if let Some(v) = jaw_open {
            fp.jaw_open = v;
        }
        if let Some(v) = smile {
            fp.mouth_smile = v;
        }
        if let Some(vis) = viseme_ov {
            fp.viseme = vis;
        }
        let clean = fp.sanitized();
        kf.face_pose = if clean.is_default() { None } else { Some(clean) };
        Ok(json!({
            "layer": lid.0,
            "frame": frame,
            "viseme": clean.viseme.key(),
            "jaw_open": clean.jaw_open,
            "blink": [clean.blink_l, clean.blink_r],
            "eyebrow": [clean.eyebrow_l, clean.eyebrow_r],
            "eye_gaze": [clean.eye_gaze.x, clean.eye_gaze.y],
        }))
    })
}

fn lip_sync_rig(s: &mut Session, p: &Value) -> Result<Value> {
    let (lid, _, _) = find_rig_layer(s, opt_u64(p, "layer")?)?;
    let threshold = opt_f32(p, "threshold")?.unwrap_or(0.07).clamp(0.0, 0.98);
    let hold = opt_u32(p, "hold")?.unwrap_or(2).clamp(1, 12);
    let wanted_sound_layer = opt_u64(p, "sound_layer")?.map(LayerId);

    // Find active speech sound on the timeline
    let fps = s.doc.stage.fps;
    let found_sound = s.timeline().layers.iter().find_map(|l| {
        if wanted_sound_layer.is_some_and(|w| w != l.id) || l.muted {
            return None;
        }
        l.keyframes.iter().find_map(|k| {
            let fs = k.sound.as_ref().filter(|fs| fs.sync != SoundSync::Stop)?;
            let snd = s.doc.sound(fs.sound)?;
            Some((k.start, fs.clone(), snd.clone()))
        })
    });

    let Some((kf_start_f, fs, snd)) = found_sound else {
        return Err(EngineError::State(
            "no sound layer found on this timeline: import speech audio onto a layer first".into(),
        ));
    };
    let dec = snd
        .decoded()
        .ok_or_else(|| EngineError::State("could not decode speech audio".into()))?;
    let pcm = &dec.pcm;
    let total_frames = s.frame_count().max(1);
    let from = opt_u32(p, "from")?.unwrap_or(kf_start_f);
    let to = opt_u32(p, "to")?.unwrap_or_else(|| total_frames.saturating_sub(1)).max(from);
    let max_f = u32::try_from(analysis::MAX_ANALYSIS_FRAMES).unwrap_or(100_000);
    let heard = if fs.looping {
        u64::MAX
    } else {
        snd.frames.saturating_mul(u64::from(fs.repeat.max(1)))
    };
    let span = Span {
        fps,
        offset: from.saturating_sub(kf_start_f),
        frames: (to - from + 1).min(max_f),
        heard,
    };
    let visemes = analysis::visemes_12(pcm, &span, &LipOptions { threshold, hold });
    let levels = analysis::normalised(&analysis::levels(pcm, &span, analysis::Channel::Both));

    let mut keys_written = 0u32;
    let mut prev_v: Option<RigViseme> = None;
    for (idx, &av) in visemes.iter().enumerate() {
        let f = from + idx as u32;
        let rv = match av {
            Viseme12::Neutral => RigViseme::Rest,
            Viseme12::Ah => RigViseme::Open,
            Viseme12::D => RigViseme::D,
            Viseme12::Ee => RigViseme::Wide,
            Viseme12::F => RigViseme::Fv,
            Viseme12::L => RigViseme::L,
            Viseme12::M => RigViseme::Closed,
            Viseme12::Oh => RigViseme::Round,
            Viseme12::R => RigViseme::R,
            Viseme12::S => RigViseme::S,
            Viseme12::Uh => RigViseme::Uh,
            Viseme12::Woo => RigViseme::Wq,
        };
        let lvl = levels.get(idx).copied().unwrap_or(0.0);
        if prev_v != Some(rv) || idx + 1 == visemes.len() {
            let _ = edit_rig_keyframe(s, lid, f, |_, kf| {
                let mut fp = kf.face_pose.unwrap_or_default();
                fp.viseme = rv;
                fp.jaw_open = if rv == RigViseme::Rest || rv == RigViseme::Closed {
                    0.0
                } else {
                    (lvl * 0.65).clamp(0.08, 0.85)
                };
                kf.face_pose = if fp.is_default() { None } else { Some(fp) };
                Ok(())
            });
            keys_written += 1;
            prev_v = Some(rv);
        }
    }

    Ok(json!({
        "layer": lid.0,
        "from": from,
        "to": to,
        "keys_written": keys_written,
    }))
}

fn save_preset(s: &mut Session, p: &Value) -> Result<Value> {
    let out_path = Path::new(str_req(p, "path")?);
    let (_, _, rig) = find_rig_layer(s, opt_u64(p, "layer")?)?;
    let payload = serde_json::to_vec_pretty(&*rig)
        .map_err(|e| EngineError::Format(format!("serialize rig preset: {e}")))?;
    vfs::write(out_path, &payload)?;
    Ok(json!({
        "saved": true,
        "path": out_path.to_string_lossy(),
        "rig_id": rig.id.0,
        "name": rig.name,
        "joints": rig.joints.len(),
    }))
}

fn instantiate_from_template(
    s: &mut Session,
    template: &CharacterRig,
    x: Option<f32>,
    y: Option<f32>,
    scale: Option<f32>,
) -> Result<Value> {
    let (w, h) = (template.image_size[0].max(1) as f32, template.image_size[1].max(1) as f32);
    let eid = ElementId(s.doc.alloc_id());
    let fill = match template.bitmap_id.filter(|&bid| s.doc.bitmap(bid).is_some()) {
        Some(bid) => Paint::Bitmap {
            bitmap: bid,
            repeat: false,
            matrix: Affine::IDENTITY,
        },
        None => Paint::Solid(Rgba::hex(0x20242c)),
    };
    let sh = Shape {
        path: Arc::new(GeomPath::rect(0.0, 0.0, w, h)),
        fill: Some(fill),
        stroke: None,
        fill_rule: FillRule::NonZero,
        merge: false,
        warp: None,
        fill_tag: None,
        stroke_tag: None,
    };

    let (sw, sh_stage) = (s.doc.stage.width as f32, s.doc.stage.height as f32);
    let sc = scale.unwrap_or_else(|| ((sh_stage * 0.84) / h).min((sw * 0.84) / w).min(1.5));
    let px = x.unwrap_or((sw - w * sc) * 0.5);
    let py = y.unwrap_or((sh_stage - h * sc) * 0.5);

    let el = Element {
        id: eid,
        name: template.name.clone(),
        transform: Affine::translate(px, py).mul(&Affine::scale(sc, sc)),
        color: ColorEffect::default(),
        transform_3d: None,
        kind: ElementKind::Shape(sh),
    };

    // Clone rig with freshly allocated BoneIds for this scene instance!
    let mut new_rig = template.clone();
    new_rig.id = RigId(s.doc.alloc_id());
    new_rig.target_shape = Some(eid);

    let mut old_to_new_bone: HashMap<BoneId, BoneId> = HashMap::new();
    let mut joint_nodes: HashMap<String, ElementId> = HashMap::new();
    let mut arm_bones: Vec<Bone> = Vec::with_capacity(new_rig.bones.len());

    for rb in &mut new_rig.bones {
        let new_bid = BoneId(s.doc.alloc_id());
        if let Some(old_bid) = rb.bone_id {
            old_to_new_bone.insert(old_bid, new_bid);
        }
        rb.bone_id = Some(new_bid);
        let p_node = joint_nodes.get(&rb.from_joint).copied().unwrap_or(eid);
        let c_node = ElementId(s.doc.alloc_id());
        joint_nodes.insert(rb.to_joint.clone(), c_node);
        let mut b = Bone::in_shape(new_bid, eid, p_node, c_node, rb.head, rb.tail);
        b.name = rb.to_joint.clone();
        b.limits = Limits {
            min: rb.limits_deg[0].to_radians(),
            max: rb.limits_deg[1].to_radians(),
        }
        .sanitized();
        arm_bones.push(b);
    }

    if let Some(sm) = &mut new_rig.skin_mesh {
        sm.deform_bones = sm
            .deform_bones
            .iter()
            .map(|old| old_to_new_bone.get(old).copied().unwrap_or(*old))
            .collect();
    }

    let summary = rig_summary_json(&new_rig, LayerId(0), eid);
    let arm = Armature {
        bones: arm_bones,
        skins: Vec::new(),
        springs_off: false,
        char_rig: Some(Arc::new(new_rig)),
    };

    let lid = LayerId(s.doc.alloc_id());
    let lname = fresh_rig_layer_name(s, &format!("Rig_{}", template.name));
    let mut layer = Layer::new(lid, lname);
    layer.kind = LayerKind::Armature;
    layer.color = Rgba::hex(0x2fb67c);
    layer.armature = Some(Arc::new(arm));
    let dur = s.frame_count().max(1);
    let mut kf = Keyframe::new(0, dur);
    kf.elements = vec![Arc::new(el)];
    kf.tween = Some(Tween::default());
    layer.keyframes = vec![Arc::new(kf)];

    let tl = crate::editing::edited_mut(&mut s.doc, s.scene, &s.editing)?;
    tl.layers.insert(0, Arc::new(layer));
    tl.repair_masks();
    s.layer = lid;

    let mut out = summary;
    out["layer"] = json!(lid.0);
    Ok(out)
}

fn load_preset(s: &mut Session, p: &Value) -> Result<Value> {
    let preset_path = Path::new(str_req(p, "path")?);
    let bytes = vfs::read(preset_path)?;
    let mut rig: CharacterRig = serde_json::from_slice(&bytes)
        .map_err(|e| EngineError::Format(format!("invalid character rig preset: {e}")))?;

    // Optional artwork_path override to attach a fresh PNG to a normalized preset!
    if let Some(art_str) = opt_str(p, "artwork_path")?.filter(|s| !s.trim().is_empty()) {
        let art_path = Path::new(art_str);
        let art_bytes = vfs::read(art_path)?;
        let bid = BitmapId(s.doc.alloc_id());
        let bmp = Bitmap::from_image(bid, rig.name.clone(), art_bytes)
            .map_err(|e| EngineError::Format(format!("{}: {e}", art_path.display())))?;
        let new_bounds = Rect::new(0.0, 0.0, bmp.width as f32, bmp.height as f32);
        rig.image_size = [bmp.width, bmp.height];
        rig.bitmap_id = Some(bid);
        // Remap normalized [0..1] UVs if image resolution changed!
        if (new_bounds.max.x - rig.bounds_px.max.x).abs() > 2.0
            || (new_bounds.max.y - rig.bounds_px.max.y).abs() > 2.0
        {
            for j in &mut rig.joints {
                j.pos_px = CharacterRig::from_norm_uv(j.norm_uv, new_bounds);
            }
            rig.bounds_px = new_bounds;
        }
        s.doc.bitmaps.push(Arc::new(bmp));
    }

    Arc::make_mut(&mut s.doc.character_rigs).push(rig.clone());
    instantiate_from_template(s, &rig, opt_f32(p, "x")?, opt_f32(p, "y")?, opt_f32(p, "scale")?)
}

fn instantiate_rig(s: &mut Session, p: &Value) -> Result<Value> {
    let rig_id = opt_u64(p, "rig_id")?.map(RigId);
    let name_q = opt_str(p, "name")?.map(str::trim).filter(|n| !n.is_empty());
    let template = s
        .doc
        .character_rigs
        .iter()
        .find(|r| rig_id.is_some_and(|id| r.id == id) || name_q.is_some_and(|n| r.name == n))
        .or_else(|| {
            if rig_id.is_none() && name_q.is_none() {
                s.doc.character_rigs.last()
            } else {
                None
            }
        })
        .cloned()
        .ok_or_else(|| EngineError::State("no matching saved character rig in this document".into()))?;

    instantiate_from_template(
        s,
        &template,
        opt_f32(p, "x")?,
        opt_f32(p, "y")?,
        opt_f32(p, "scale")?,
    )
}

fn info(s: &mut Session, p: &Value) -> Result<Value> {
    let frame = opt_u32(p, "frame")?.unwrap_or(s.frame);
    let reusable: Vec<Value> = s
        .doc
        .character_rigs
        .iter()
        .map(|r| {
            json!({
                "rig_id": r.id.0,
                "name": r.name,
                "character_style": r.character_style.key(),
                "view_angle": r.view_angle.key(),
                "joints": r.joints.iter().filter(|j| j.present).count(),
            })
        })
        .collect();

    let Ok((lid, arm, rig)) = find_rig_layer(s, opt_u64(p, "layer")?) else {
        return Ok(json!({ "active_rig": Value::Null, "reusable_rigs": reusable }));
    };

    let layer_ref = s.timeline().layer(lid);
    let elem_xf = layer_ref
        .and_then(|l| evaluate_layer_elements(l, frame))
        .and_then(|(_, els)| {
            let tid = rig.target_shape?;
            Some(els.iter().find(|e| e.id == tid)?.transform)
        })
        .unwrap_or(Affine::IDENTITY);

    let pose = layer_ref
        .and_then(|l| armature::pose_at(l, frame))
        .unwrap_or_default();
    let face = layer_ref.map_or_else(FacePose::default, |l| face_pose_at(l, frame));

    let joints_json: Vec<Value> = rig
        .joints
        .iter()
        .map(|j| {
            let st = elem_xf.apply(j.pos_px);
            json!({
                "id": j.id,
                "parent": j.parent,
                "present": j.present,
                "local_px": [j.pos_px.x, j.pos_px.y],
                "stage_px": [st.x, st.y],
                "norm_uv": [j.norm_uv.x, j.norm_uv.y],
                "confidence": j.confidence,
                "low_confidence": j.low_confidence,
                "manual_override": j.manual_override,
                "warning": j.warning,
            })
        })
        .collect();

    let bones_json: Vec<Value> = rig
        .bones
        .iter()
        .map(|rb| {
            let angle_deg = rb
                .bone_id
                .map_or(0.0, |bid| pose.angle(bid).to_degrees());
            json!({
                "id": rb.id,
                "bone": rb.bone_id.map(|b| b.0),
                "from_joint": rb.from_joint,
                "to_joint": rb.to_joint,
                "rest_length_px": rb.rest_length_px,
                "fixed_length": rb.fixed_length,
                "limits_deg": rb.limits_deg,
                "angle_deg": angle_deg,
            })
        })
        .collect();

    let _ = arm;
    Ok(json!({
        "active_rig": {
            "rig_id": rig.id.0,
            "name": rig.name,
            "layer": lid.0,
            "character_style": rig.character_style.key(),
            "view_angle": rig.view_angle.key(),
            "overall_confidence": rig.overall_confidence,
            "warnings": rig.warnings,
            "face_pose": {
                "viseme": face.viseme.key(),
                "jaw_open": face.jaw_open,
                "mouth_smile": face.mouth_smile,
                "blink_l": face.blink_l,
                "blink_r": face.blink_r,
                "eyebrow_l": face.eyebrow_l,
                "eyebrow_r": face.eyebrow_r,
                "eye_gaze": [face.eye_gaze.x, face.eye_gaze.y],
            },
            "joints": joints_json,
            "bones": bones_json,
        },
        "reusable_rigs": reusable,
    }))
}

fn validate(s: &mut Session, p: &Value) -> Result<Value> {
    let frame = opt_u32(p, "frame")?.unwrap_or(s.frame);
    let (lid, arm, rig) = find_rig_layer(s, opt_u64(p, "layer")?)?;
    let layer = s
        .timeline()
        .layer(lid)
        .ok_or_else(|| EngineError::State("missing rig layer".into()))?;
    let (_, els) = evaluate_layer_elements(layer, frame)
        .ok_or_else(|| EngineError::State("rig layer is empty at this frame".into()))?;
    let pose = armature::pose_at(layer, frame).unwrap_or_default();
    let face = face_pose_at(layer, frame);

    let tid = rig
        .target_shape
        .ok_or_else(|| EngineError::State("rig has no target shape".into()))?;
    let inv_xf = els
        .iter()
        .find(|e| e.id == tid)
        .and_then(|e| e.transform.inverse())
        .unwrap_or(Affine::IDENTITY);

    // 1. Check bone length invariance (`fixed_length == true` -> 0.0 px limb stretching!)
    let posed_bones = armature::bones_at(&arm, &els, &pose);
    let mut max_len_drift = 0.0f32;
    let mut max_joint_gap = 0.0f32;
    let mut posed_tails: HashMap<String, Vec2> = HashMap::new();

    for rb in &rig.bones {
        if let Some(bid) = rb.bone_id
            && let Some(pb) = posed_bones.iter().find(|x| x.id == bid)
        {
            let lh = inv_xf.apply(pb.head);
            let lt = inv_xf.apply(pb.tail);
            let cur_len = (lt - lh).length();
            max_len_drift = max_len_drift.max((cur_len - rb.rest_length_px).abs());
            if let Some(&parent_tail) = posed_tails.get(&rb.from_joint) {
                max_joint_gap = max_joint_gap.max((lh - parent_tail).length());
            }
            posed_tails.insert(rb.to_joint.clone(), lt);
        }
    }

    // 2. Check normalized skin weights (`sum == 1.0`, zero orphan vertices)
    let mut unnormalized_verts = 0usize;
    let mut inverted_tris = 0usize;
    let mut total_tris = 0usize;

    if let Some(sm) = &rig.skin_mesh {
        for row in &sm.weights {
            let sum: f32 = row.iter().map(|vw| vw.1).sum();
            if row.is_empty() || (sum - 1.0).abs() > 1e-3 {
                unnormalized_verts += 1;
            }
        }
        total_tris = sm.triangles.len();
        if let Some((_, deformed)) = deform_skinned_mesh(&arm, &els, &pose, &face) {
            for &[i0, i1, i2] in &deformed.tris {
                let (Some(&ra), Some(&rb_v), Some(&rc)) = (
                    deformed.rest.get(i0 as usize),
                    deformed.rest.get(i1 as usize),
                    deformed.rest.get(i2 as usize),
                ) else {
                    continue;
                };
                let (Some(&sa), Some(&sb), Some(&sc)) = (
                    deformed.shown.get(i0 as usize),
                    deformed.shown.get(i1 as usize),
                    deformed.shown.get(i2 as usize),
                ) else {
                    continue;
                };
                let rest_cross = (rb_v - ra).cross(rc - ra);
                let shown_cross = (sb - sa).cross(sc - sa);
                if rest_cross.abs() > 4.0 && rest_cross * shown_cross < -4.0 {
                    inverted_tris += 1;
                }
            }
        }
    }

    let passed = max_len_drift < 0.25 && max_joint_gap < 0.25 && unnormalized_verts == 0;
    Ok(json!({
        "passed": passed,
        "layer": lid.0,
        "frame": frame,
        "character_style": rig.character_style.key(),
        "view_angle": rig.view_angle.key(),
        "max_bone_length_drift_px": max_len_drift,
        "max_joint_detachment_px": max_joint_gap,
        "unnormalized_skin_vertices": unnormalized_verts,
        "inverted_triangles": inverted_tris,
        "total_triangles": total_tris,
    }))
}

/// Register `PATH_PARAMS` entries for `rig.*` commands that touch disk paths.
pub const RIG_PATH_PARAMS: &[(&str, &[(&str, Access)])] = &[
    ("rig.auto_detect", &[("path", Access::Read)]),
    ("rig.save_preset", &[("path", Access::Write)]),
    ("rig.load_preset", &[("path", Access::Read), ("artwork_path", Access::Read)]),
];
