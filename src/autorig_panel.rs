//! Auto Rig Studio panel & interactive Stage joint/bone overlay.
//!
//! Provides one-click Hybrid CV + Geometric character rigging (Stickman, Noir Silhouette,
//! Realistic Cartoon, Stylized Human, Exaggerated Proportions, SVG & Layered Artwork),
//! draggable joint dots on the Stage, confidence & warning inspection, IK/FK controls,
//! independent eye/eyebrow/eyelid/jaw/viseme controls, audio lip-sync, and reusable presets.

use crate::app::{App, Drag};
use crate::pathtools::Xf;
use crate::theme::{T, Type, text, to_color32};
use anicraft_engine::doc::autorig::{CharacterRig, FacePose, RigViseme, deform_skinned_mesh, face_pose_at};
use anicraft_engine::doc::{LayerId, LayerKind, Rgba, armature, evaluate_layer_elements};
use anicraft_engine::geom::{Affine, Vec2};
use egui::{Pos2, Stroke};
use serde_json::json;

/// Screen hit radius (points) for grabbing a joint dot on the Stage in Edit Joints mode.
const JOINT_HIT_RADIUS: f32 = 9.5;

/// View state of the Auto Rig Studio panel & Stage joint editor.
#[derive(Clone, Debug)]
pub struct AutoRigUi {
    /// When true, dragging a joint dot on the Stage moves & rebinds that joint (`rig.set_joint`).
    /// When false, dragging poses the rig via IK/FK (`armature::solve`).
    pub edit_joints: bool,
    /// Show the character's constrained triangle skin mesh wireframe on the Stage.
    pub show_mesh: bool,
    /// Show 21-point MediaPipe finger joints on the Stage.
    pub show_fingers: bool,
    /// Show facial rig handles (eyes, eyelids, brows, jaw, mouth) on the Stage.
    pub show_face_handles: bool,
    /// Currently selected joint index in `CharacterRig::joints`.
    pub selected_joint: Option<usize>,
}

impl Default for AutoRigUi {
    fn default() -> Self {
        AutoRigUi {
            edit_joints: false,
            show_mesh: false,
            show_fingers: true,
            show_face_handles: true,
            selected_joint: None,
        }
    }
}

fn is_finger_joint(id: &str) -> bool {
    id.contains("thumb_")
        || id.contains("index_")
        || id.contains("middle_")
        || id.contains("ring_")
        || id.contains("pinky_")
}

fn is_face_joint(id: &str) -> bool {
    id.starts_with("eye")
        || id.starts_with("jaw")
        || id.starts_with("chin")
        || id.starts_with("mouth")
        || id.starts_with("lip_")
}

fn active_layer_rig(app: &App) -> Option<(LayerId, std::sync::Arc<CharacterRig>, Affine, FacePose)> {
    let lid = app.s.layer;
    let layer = app.s.timeline().layer(lid)?;
    if layer.kind != LayerKind::Armature || !layer.visible {
        return None;
    }
    let arm = layer.armature.as_ref()?;
    let rig = arm.char_rig.clone()?;
    let (_, els) = evaluate_layer_elements(layer, app.s.frame)?;
    let xf = rig
        .target_shape
        .and_then(|tid| els.iter().find(|e| e.id == tid))
        .map_or(Affine::IDENTITY, |e| e.transform);
    let face = face_pose_at(layer, app.s.frame);
    Some((lid, rig, xf, face))
}

/// Hit-tests visible joint dots on the active rigged character layer when `edit_joints` is enabled.
pub fn hit_joint(app: &App, xf: Xf, screen_pt: Pos2) -> Option<(LayerId, usize)> {
    if !app.autorig.edit_joints {
        return None;
    }
    let (lid, rig, elem_xf, _) = active_layer_rig(app)?;
    let layer = app.s.timeline().layer(lid)?;
    if layer.locked || app.s.timeline().locked_by_folder(lid) {
        return None;
    }
    let mut best: Option<(usize, f32)> = None;
    for (idx, j) in rig.joints.iter().enumerate() {
        if !j.present {
            continue;
        }
        if !app.autorig.show_fingers && is_finger_joint(&j.id) {
            continue;
        }
        if !app.autorig.show_face_handles && is_face_joint(&j.id) {
            continue;
        }
        let stage_p = elem_xf.apply(j.pos_px);
        let sp = xf.to_screen(stage_p);
        let d = sp.distance(screen_pt);
        if d <= JOINT_HIT_RADIUS && best.is_none_or(|(_, bd)| d < bd) {
            best = Some((idx, d));
        }
    }
    best.map(|(idx, _)| (lid, idx))
}

/// Starts a Stage drag on an Auto-Rig joint dot when `edit_joints` is active.
pub fn drag_start(app: &mut App, xf: Xf, press: Pos2) -> Option<Drag> {
    let (lid, idx) = hit_joint(app, xf, press)?;
    app.autorig.selected_joint = Some(idx);
    app.s.begin_gesture(tr_noop!("Adjust Rig Joint"));
    Some(Drag::AutoRigJoint {
        layer: lid,
        joint_idx: idx,
    })
}

/// Updates joint coordinates (`pos_px` + normalized `norm_uv`) live while dragging on the Stage.
pub fn drag_to(app: &mut App, layer: LayerId, joint_idx: usize, stage_pt: Pos2) {
    let Some(jid) = app
        .s
        .timeline()
        .layer(layer)
        .and_then(|l| l.armature.as_ref())
        .and_then(|a| a.char_rig.as_ref())
        .and_then(|r| r.joints.get(joint_idx))
        .map(|j| j.id.clone())
    else {
        return;
    };
    app.run(
        "rig.set_joint",
        json!({
            "layer": layer.0,
            "joint": jid,
            "x": stage_pt.x,
            "y": stage_pt.y,
            "space": "stage",
        }),
    );
}

/// Draws the character's constrained skin mesh wireframe, connected bones, and interactive
/// confidence-colored joint dots over the Stage.
pub fn draw_stage_overlay(app: &App, painter: &egui::Painter, xf: Xf) {
    let Some((lid, rig, elem_xf, face)) = active_layer_rig(app) else {
        return;
    };
    let Some(layer) = app.s.timeline().layer(lid) else {
        return;
    };
    let Some(arm) = layer.armature.as_deref() else {
        return;
    };

    let c_outline = to_color32(Rgba::hex(0x121318));
    let c_good = to_color32(Rgba::hex(0x3ce882));
    let c_warn = to_color32(Rgba::hex(0xff9f2e));
    let c_face = to_color32(Rgba::hex(0xf472d0));
    let c_finger = to_color32(Rgba::hex(0x4dd4ff));
    let c_mesh = to_color32(Rgba::new(0.32, 0.78, 0.96, 0.30));

    // 1. Optional deformed or rest triangle skin-mesh wireframe overlay
    if app.autorig.show_mesh
        && let Some(sm) = &rig.skin_mesh
    {
        let shown_verts: Option<Vec<Vec2>> = evaluate_layer_elements(layer, app.s.frame)
            .and_then(|(_, els)| {
                let pose = armature::pose_at(layer, app.s.frame).unwrap_or_default();
                deform_skinned_mesh(arm, &els, &pose, &face).map(|(_, wm)| wm.shown)
            });
        let verts = shown_verts.as_deref().unwrap_or(&sm.rest_vertices);
        let stroke = Stroke::new(1.0, c_mesh);
        for &[i0, i1, i2] in &sm.triangles {
            let (Some(&a), Some(&b), Some(&c)) = (
                verts.get(i0 as usize),
                verts.get(i1 as usize),
                verts.get(i2 as usize),
            ) else {
                continue;
            };
            let sa = xf.to_screen(elem_xf.apply(a));
            let sb = xf.to_screen(elem_xf.apply(b));
            let sc = xf.to_screen(elem_xf.apply(c));
            painter.add(egui::Shape::closed_line(vec![sa, sb, sc], stroke));
        }
    }

    // 2. Draw connected bones + interactive confidence-colored joint dots whenever
    //    `edit_joints` is active (rest pose) OR `Bone` / `Bind` tool is active (live posed IK/FK)!
    let show_rig_handles =
        app.autorig.edit_joints || matches!(app.tool, crate::app::Tool::Bone | crate::app::Tool::Bind);
    if show_rig_handles {
        let posed_map: std::collections::HashMap<anicraft_engine::doc::BoneId, (Vec2, Vec2)> =
            if !app.autorig.edit_joints
                && let Some((_, els)) = evaluate_layer_elements(layer, app.s.frame)
            {
                let pose = armature::pose_at(layer, app.s.frame).unwrap_or_default();
                armature::bones_at(arm, &els, &pose)
                    .into_iter()
                    .map(|b| (b.id, (b.head, b.tail)))
                    .collect()
            } else {
                std::collections::HashMap::new()
            };

        let mut posed_joint_px: std::collections::HashMap<&str, Vec2> = std::collections::HashMap::new();
        for rb in &rig.bones {
            let (w0, w1) = if let Some(bid) = rb.bone_id
                && let Some(&(ph, pt)) = posed_map.get(&bid)
            {
                posed_joint_px.entry(rb.from_joint.as_str()).or_insert(ph);
                posed_joint_px.insert(rb.to_joint.as_str(), pt);
                (ph, pt)
            } else {
                (elem_xf.apply(rb.head), elem_xf.apply(rb.tail))
            };
            if !app.autorig.show_fingers && is_finger_joint(&rb.to_joint) {
                continue;
            }
            if !app.autorig.show_face_handles && is_face_joint(&rb.to_joint) {
                continue;
            }
            let p0 = xf.to_screen(w0);
            let p1 = xf.to_screen(w1);
            let col = if is_finger_joint(&rb.to_joint) {
                c_finger
            } else if is_face_joint(&rb.to_joint) {
                c_face
            } else {
                c_good
            };
            painter.line_segment([p0, p1], Stroke::new(3.5, c_outline));
            painter.line_segment([p0, p1], Stroke::new(1.8, col));
        }

        for (idx, j) in rig.joints.iter().enumerate() {
            if !j.present {
                continue;
            }
            if !app.autorig.show_fingers && is_finger_joint(&j.id) {
                continue;
            }
            if !app.autorig.show_face_handles && is_face_joint(&j.id) {
                continue;
            }
            let world_pt = posed_joint_px
                .get(j.id.as_str())
                .copied()
                .unwrap_or_else(|| elem_xf.apply(j.pos_px));
            let sp = xf.to_screen(world_pt);
            let is_sel = app.autorig.selected_joint == Some(idx);
            let rad = if is_finger_joint(&j.id) {
                4.2
            } else if is_face_joint(&j.id) {
                4.8
            } else {
                6.4
            };
            let fill = if is_sel {
                T.selection
            } else if j.low_confidence {
                c_warn
            } else if is_face_joint(&j.id) {
                c_face
            } else if is_finger_joint(&j.id) {
                c_finger
            } else {
                c_good
            };
            painter.circle_filled(sp, rad + 2.0, c_outline);
            painter.circle_filled(sp, rad, fill);
            if j.low_confidence || is_sel {
                painter.circle_stroke(sp, rad + 4.5, Stroke::new(1.6, fill));
            }
        }
    }
}

/// Draws the dockable **Auto Rig Studio** panel.
pub fn panel(app: &mut App, ui: &mut egui::Ui) {
    ui.label(text(Type::Heading, tr!("Auto Rig Studio")).strong());
    ui.label(
        text(
            Type::Caption,
            tr!("Hybrid DWPose + MediaPipe + SAM 2 + OpenCV/SVG Engine"),
        )
        .color(T.text_dim),
    );
    ui.add_space(6.0);

    // 1. Primary Auto-Rig Action Buttons
    ui.horizontal_wrapped(|ui| {
        if ui
            .button(text(Type::Body, tr!("Import & Auto-Rig PNG / SVG…")))
            .clicked()
        {
            app.invoke("rig.auto_detect");
        }
        if ui
            .button(text(Type::Body, tr!("Auto-Rig Stage Selection")))
            .clicked()
        {
            app.run("rig.auto_detect", json!({ "layer": app.s.layer.0 }));
        }
    });

    ui.separator();

    // 2. Active Rig Status, Style Badge, Confidence & Warnings
    if let Some((lid, rig, elem_xf, face)) = active_layer_rig(app) {
        let present_count = rig.joints.iter().filter(|j| j.present).count();
        let tri_count = rig.skin_mesh.as_ref().map_or(0, |m| m.triangles.len());

        ui.horizontal_wrapped(|ui| {
            ui.label(text(Type::Label, tr!("Character:")).color(T.text_dim));
            ui.label(text(Type::Body, &rig.name).strong());
            ui.label(text(Type::DataSmall, format!("[{}]", rig.character_style.label())));
        });
        ui.horizontal_wrapped(|ui| {
            let conf_str = format!("{:.1}", rig.overall_confidence * 100.0);
            ui.label(
                text(
                    Type::DataSmall,
                    tr!(
                        "View: {view}  |  Conf: {conf}%  |  Joints: {joints}  |  Tris: {tris}",
                        view = rig.view_angle.label(),
                        conf = conf_str,
                        joints = present_count,
                        tris = tri_count
                    ),
                )
                .color(T.text_dim),
            );
        });

        // Low-confidence / occlusion warnings box
        let low_joints: Vec<&str> = rig
            .joints
            .iter()
            .filter(|j| j.present && j.low_confidence)
            .map(|j| j.id.as_str())
            .collect();
        if !low_joints.is_empty() || !rig.warnings.is_empty() {
            ui.add_space(4.0);
            let warn_list = if !low_joints.is_empty() {
                low_joints.join(", ")
            } else {
                rig.warnings.first().cloned().unwrap_or_default()
            };
            ui.label(
                text(
                    Type::Caption,
                    tr!(
                        "Warnings ({count}): {warnings}",
                        count = low_joints.len() + rig.warnings.len(),
                        warnings = warn_list
                    ),
                )
                .color(T.accent_text),
            );
        }

        ui.add_space(6.0);

        // 3. Stage Interaction Mode Toggle: Edit Joint Dots vs Pose IK / FK
        ui.horizontal_wrapped(|ui| {
            let mode_edit = app.autorig.edit_joints;
            if ui
                .selectable_label(!mode_edit, text(Type::Label, tr!("Pose IK / FK")))
                .clicked()
            {
                app.autorig.edit_joints = false;
                app.tool = crate::app::Tool::Select;
            }
            if ui
                .selectable_label(mode_edit, text(Type::Label, tr!("Edit Joint Dots")))
                .clicked()
            {
                app.autorig.edit_joints = true;
                app.tool = crate::app::Tool::Select;
            }
            if ui.button(text(Type::Label, tr!("Rebind Skin"))).clicked() {
                app.run("rig.rebind", json!({ "layer": lid.0 }));
            }
        });

        ui.horizontal_wrapped(|ui| {
            ui.checkbox(
                &mut app.autorig.show_mesh,
                text(Type::Caption, tr!("Mesh")),
            );
            ui.checkbox(
                &mut app.autorig.show_fingers,
                text(Type::Caption, tr!("Fingers")),
            );
            ui.checkbox(
                &mut app.autorig.show_face_handles,
                text(Type::Caption, tr!("Face")),
            );
        });

        ui.separator();

        // 4. Joint Selector, Pixel + Normalized [0..1] Coordinates & FK Angle Slider
        ui.label(text(Type::Label, tr!("Joint & Bone Inspector")).strong());
        let sel_idx = app
            .autorig
            .selected_joint
            .filter(|&i| i < rig.joints.len())
            .unwrap_or(0);
        if !rig.joints.is_empty() {
            let cur_j = &rig.joints[sel_idx];
            egui::ComboBox::from_id_salt("autorig_joint_pick")
                .selected_text(text(Type::DataSmall, &cur_j.id))
                .show_ui(ui, |ui| {
                    for (idx, j) in rig.joints.iter().enumerate() {
                        if !j.present {
                            continue;
                        }
                        let badge = if j.low_confidence { " (!)" } else { "" };
                        if ui
                            .selectable_label(
                                sel_idx == idx,
                                text(Type::DataSmall, format!("{}{badge}", j.id)),
                            )
                            .clicked()
                        {
                            app.autorig.selected_joint = Some(idx);
                        }
                    }
                });

            let stage_pt = elem_xf.apply(cur_j.pos_px);
            let px_x = format!("{:.1}", cur_j.pos_px.x);
            let px_y = format!("{:.1}", cur_j.pos_px.y);
            let uv_x = format!("{:.3}", cur_j.norm_uv.x);
            let uv_y = format!("{:.3}", cur_j.norm_uv.y);
            let conf_pct = format!("{:.0}", cur_j.confidence * 100.0);
            ui.label(
                text(
                    Type::DataSmall,
                    tr!(
                        "Px: ({px}, {py})  |  Norm UV: ({u}, {v})  |  Conf: {conf}%",
                        px = px_x,
                        py = px_y,
                        u = uv_x,
                        v = uv_y,
                        conf = conf_pct
                    ),
                )
                .color(T.text_dim),
            );
            let _ = stage_pt;

            // FK Rotation Slider for the bone ending at (or starting from) `cur_j`
            if let Some(rb) = rig
                .bones
                .iter()
                .find(|b| b.to_joint == cur_j.id)
                .or_else(|| rig.bones.iter().find(|b| b.from_joint == cur_j.id))
                && let Some(bid) = rb.bone_id
            {
                let cur_deg = app
                    .s
                    .timeline()
                    .layer(lid)
                    .and_then(|l| armature::pose_at(l, app.s.frame))
                    .map_or(0.0, |p| p.angle(bid).to_degrees());
                let mut deg = cur_deg;
                let [lo, hi] = rb.limits_deg;
                ui.horizontal(|ui| {
                    ui.label(text(Type::Caption, tr!("FK Angle (°):")));
                    let resp = ui.add(egui::Slider::new(&mut deg, lo..=hi));
                    if resp.changed() {
                        app.run(
                            "rig.pose_fk",
                            json!({
                                "layer": lid.0,
                                "bone": bid.0,
                                "angle_deg": deg,
                            }),
                        );
                    }
                });
            }
        }

        ui.separator();

        // 5. Independent Facial & Lip-Sync Controls
        ui.label(text(Type::Label, tr!("Facial & Lip-Sync Controls")).strong());

        let mut blink_l = face.blink_l;
        let mut blink_r = face.blink_r;
        let mut brow_l = face.eyebrow_l;
        let mut brow_r = face.eyebrow_r;
        let mut eye_x = face.eye_gaze.x;
        let mut eye_y = face.eye_gaze.y;
        let mut jaw = face.jaw_open;
        let mut smile = face.mouth_smile;
        let mut face_changed = false;

        egui::Grid::new("autorig_face_grid")
            .num_columns(2)
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                ui.label(text(Type::Caption, tr!("Eye Gaze X")));
                face_changed |= ui.add(egui::Slider::new(&mut eye_x, -1.0..=1.0)).changed();
                ui.end_row();

                ui.label(text(Type::Caption, tr!("Eye Gaze Y")));
                face_changed |= ui.add(egui::Slider::new(&mut eye_y, -1.0..=1.0)).changed();
                ui.end_row();

                ui.label(text(Type::Caption, tr!("Blink L")));
                face_changed |= ui.add(egui::Slider::new(&mut blink_l, 0.0..=1.0)).changed();
                ui.end_row();

                ui.label(text(Type::Caption, tr!("Blink R")));
                face_changed |= ui.add(egui::Slider::new(&mut blink_r, 0.0..=1.0)).changed();
                ui.end_row();

                ui.label(text(Type::Caption, tr!("Eyebrow L")));
                face_changed |= ui.add(egui::Slider::new(&mut brow_l, -1.0..=1.0)).changed();
                ui.end_row();

                ui.label(text(Type::Caption, tr!("Eyebrow R")));
                face_changed |= ui.add(egui::Slider::new(&mut brow_r, -1.0..=1.0)).changed();
                ui.end_row();

                ui.label(text(Type::Caption, tr!("Jaw Open")));
                face_changed |= ui.add(egui::Slider::new(&mut jaw, 0.0..=1.0)).changed();
                ui.end_row();

                ui.label(text(Type::Caption, tr!("Smile / Wide")));
                face_changed |= ui.add(egui::Slider::new(&mut smile, -1.0..=1.0)).changed();
                ui.end_row();
            });

        if face_changed {
            app.run(
                "rig.set_face",
                json!({
                    "layer": lid.0,
                    "eye_x": eye_x,
                    "eye_y": eye_y,
                    "blink_l": blink_l,
                    "blink_r": blink_r,
                    "brow_l": brow_l,
                    "brow_r": brow_r,
                    "jaw_open": jaw,
                    "smile": smile,
                }),
            );
        }

        ui.add_space(4.0);
        ui.label(text(Type::Caption, tr!("Mouth Visemes:")));
        ui.horizontal_wrapped(|ui| {
            for vis in RigViseme::ALL {
                if ui
                    .selectable_label(face.viseme == vis, text(Type::DataSmall, vis.label()))
                    .clicked()
                {
                    app.run(
                        "rig.set_face",
                        json!({ "layer": lid.0, "viseme": vis.key() }),
                    );
                }
            }
        });

        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            if ui
                .button(text(Type::Label, tr!("Auto Lip-Sync Audio")))
                .clicked()
            {
                app.run("rig.lip_sync", json!({ "layer": lid.0 }));
            }
            if ui
                .button(text(Type::Label, tr!("Save Rig Preset…")))
                .clicked()
            {
                app.invoke("rig.save_preset");
            }
        });
    } else {
        ui.label(
            text(
                Type::Body,
                tr!("Import a PNG or SVG character, or select artwork on the Stage to generate an automatic rig."),
            )
            .color(T.text_dim),
        );
    }

    ui.separator();

    // 6. Reusable Character Rigs Across Scenes & Presets
    ui.label(text(Type::Label, tr!("Reusable Rigs (All Scenes)")).strong());
    ui.horizontal_wrapped(|ui| {
        if ui
            .button(text(Type::Label, tr!("Load Rig Preset…")))
            .clicked()
        {
            app.invoke("rig.load_preset");
        }
    });

    let saved_rigs: Vec<(u64, String, &'static str)> = app
        .s
        .doc
        .character_rigs
        .iter()
        .map(|r| (r.id.0, r.name.clone(), r.character_style.label()))
        .collect();
    for (rid, rname, rstyle) in saved_rigs {
        ui.horizontal(|ui| {
            ui.label(text(Type::Body, &rname));
            ui.label(text(Type::DataSmall, format!("({rstyle})")).color(T.text_dim));
            if ui
                .button(text(Type::Caption, tr!("Add to Scene")))
                .clicked()
            {
                app.run("rig.instantiate", json!({ "rig_id": rid }));
            }
        });
    }
}
