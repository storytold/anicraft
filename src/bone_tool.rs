//! The Bone tool and armatures on the stage (ADR 0043, ADR 0071, ADR 0098).
//!
//! Bones show over the artwork of every visible armature layer as wedges, wide at the round
//! joint they turn about and narrowing to a smaller round end, in the layer's colour; the
//! selected bone in the selection colour. With the **Bone tool** a drag from one symbol
//! instance to another adds a bone; a drag inside a shape draws a bone inside it (the shape
//! bends with it: a shape armature); a drag starting on a bone's end continues the chain from
//! it (onto a symbol instance from a shape's bone, or into a shape from an instance's: the two
//! mix in one armature), and a click picks a bone. With the **Selection tool** a drag on a bone, or on an
//! instance a bone moves, poses the armature by inverse kinematics (the point grabbed follows the
//! pointer); a drag on the root instance, the root's joints (or a shape armature's shape) moves
//! the whole armature. Each drag is one undo step, and the
//! pose keys the frame.

use crate::app::{App, Drag, Tool};
use crate::pathtools::Xf;
use crate::theme::{T, to_color32};
use anicraft_engine::doc::armature::{self, BoneAt};
use anicraft_engine::doc::{BoneId, Element, ElementId, ElementKind, Layer, LayerId, LayerKind, evaluate_layer_elements, evaluate_shown};
use anicraft_engine::geom::Vec2;
use egui::{Color32, Key, Modifiers, Pos2, Stroke, pos2};
use serde_json::json;

/// How close (screen points) a press must be to a bone's end to grab it, or to its wedge.
const GRAB_END: f32 = 8.0;
const GRAB_BODY: f32 = 6.0;
/// Joint radius on screen, and the widest a wedge gets.
const JOINT: f32 = 5.0;
const END: f32 = 3.5;
const WIDE: f32 = 9.0;

/// One armature as the stage shows it at the playhead.
pub struct Shown {
    pub layer: LayerId,
    pub color: egui::Color32,
    pub locked: bool,
    pub bones: Vec<BoneAt>,
}

/// Every visible armature on the scene at the playhead, bottom layer first.
pub fn shown(app: &App) -> Vec<Shown> {
    let tl = app.s.timeline();
    tl.layers
        .iter()
        .enumerate()
        .rev()
        .filter(|(i, l)| l.kind == LayerKind::Armature && l.visible && !tl.hidden_by_folder(*i))
        .filter_map(|(_, l)| {
            let arm = l.armature.as_deref()?;
            // As the stage draws the armature: swung by its springs (ADR 0084).
            let fps = app.s.doc.stage.fps;
            let (_, els) = evaluate_shown(l, app.s.frame, None, fps)?;
            let pose = anicraft_engine::doc::spring::sprung_pose(l, app.s.frame, fps).unwrap_or_default();
            Some(Shown { layer: l.id, color: to_color32(l.color), locked: l.locked || tl.locked_by_folder(l.id), bones: armature::bones_at(arm, &els, &pose) })
        })
        .collect()
}

/// What part of a bone a point is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    End,
    Body,
}

/// The bone under screen point `p` (ends before bodies, top layer first) on an unlocked
/// armature.
pub fn hit(app: &App, xf: Xf, p: Pos2) -> Option<(LayerId, BoneId, Part)> {
    let all = shown(app);
    let open = || all.iter().rev().filter(|s| !s.locked);
    for s in open() {
        if let Some(b) = s.bones.iter().find(|b| xf.to_screen(b.tail).distance(p) <= GRAB_END) {
            return Some((s.layer, b.id, Part::End));
        }
    }
    for s in open() {
        if let Some(b) = s.bones.iter().find(|b| segment_distance(p, xf.to_screen(b.head), xf.to_screen(b.tail)) <= GRAB_BODY) {
            return Some((s.layer, b.id, Part::Body));
        }
    }
    None
}

fn segment_distance(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let (ab, ap) = (b - a, p - a);
    let t = if ab.length_sq() > 0.0 { (ap.dot(ab) / ab.length_sq()).clamp(0.0, 1.0) } else { 0.0 };
    (a + ab * t).distance(p)
}

/// What a Bone tool press can start a bone on: an instance, a shape on a normal layer, or a
/// shape of an armature.
fn rig_target(l: &Layer, e: &Element) -> bool {
    match e.kind {
        ElementKind::Symbol { .. } => true,
        ElementKind::Shape(_) => l.kind == LayerKind::Normal || l.armature.as_ref().is_some_and(|a| a.contains(e.id)),
        _ => false,
    }
}

/// The topmost item under stage point `sp` other than `skip`, on an unlocked layer, that `want`
/// accepts: its layer and id.
fn item_at(app: &App, sp: Vec2, slop: f32, skip: Option<ElementId>, want: impl Fn(&Layer, &Element) -> bool) -> Option<(LayerId, ElementId)> {
    let dl = app.s.display_list();
    let tl = app.s.timeline();
    for item in dl.items.iter().rev() {
        if Some(item.element) == skip || item.guide || item.masks.is_some() {
            continue;
        }
        let Some(layer) = tl.layer(item.layer).filter(|l| !l.locked && !tl.locked_by_folder(l.id)) else { continue };
        if item.clip.is_some_and(|c| !dl.clip_contains(c, sp)) {
            continue;
        }
        let hw = item.stroke.as_ref().map_or(0.0, |s| s.reach() * item.transform.mean_scale());
        let inside = item.fill.is_some() && item.path.contains(sp, &item.transform, item.fill_rule);
        if !(inside || item.path.distance_to_edge(sp, &item.transform) <= hw + slop) {
            continue;
        }
        let wanted = evaluate_layer_elements(layer, app.s.frame).is_some_and(|(_, els)| els.iter().any(|e| e.id == item.element && want(layer, e)));
        if wanted {
            return Some((layer.id, item.element));
        }
    }
    None
}

/// Selects bone `bone` on armature layer `layer` (its layer becomes the active one), and the
/// instance it moves, so Properties shows that instance's colour effect, blending and filters
/// to edit in place (ADR 0084).
pub fn select(app: &mut App, layer: LayerId, bone: Option<BoneId>) {
    app.bone_sel = bone;
    if bone.is_some() && app.s.layer != layer {
        app.run("layer.select", json!({"layer": layer.0}));
    }
    // (A bone inside a shape moves points, and a bone into a whole shape has no such look.)
    let child = bone.and_then(|b| app.s.timeline().layer(layer).and_then(|l| l.armature.as_ref()).and_then(|a| a.bone(b)).filter(|b| b.shape.is_none()).map(|b| b.child));
    if let Some(child) = child.filter(|c| is_shape(app, *c) == Some(false)) {
        app.run("element.select", json!({"ids": [child.0], "layer": layer.0}));
    }
}

/// A Bone tool drag starting at screen point `press`: a new bone from the instance or shape
/// there, or from the end of the bone there.
pub fn drag_start(app: &mut App, xf: Xf, press: Pos2) -> Drag {
    let sp = xf.to_stage(press);
    if let Some((layer, id, Part::End)) = hit(app, xf, press)
        && let Some(child) = app.s.timeline().layer(layer).and_then(|l| l.armature.as_ref()).and_then(|a| a.bone(id)).map(|b| b.child)
        && let Some(b) = shown(app).into_iter().find(|s| s.layer == layer).and_then(|s| s.bones.into_iter().find(|b| b.id == id))
    {
        // A chain joins end to start.
        let head = pos2(b.tail.x, b.tail.y);
        return Drag::Bone { from: child, parent: Some(id), head, cur: head };
    }
    match item_at(app, sp, 3.0 / xf.zoom.max(1e-3), None, rig_target) {
        Some((_, from)) => Drag::Bone { from, parent: None, head: pos2(sp.x, sp.y), cur: pos2(sp.x, sp.y) },
        None => {
            app.status_ok(tr!("Start a bone inside a shape, or on a symbol instance and drag to the instance or shape it moves."));
            Drag::None
        }
    }
}

/// What kind of element `id` is at the playhead: `Some(true)` a shape, `Some(false)` a symbol
/// instance, `None` neither (a joint node inside a shape is no element).
fn is_shape(app: &App, id: ElementId) -> Option<bool> {
    app.s.timeline().layers.iter().find_map(|l| {
        let (_, els) = evaluate_layer_elements(l, app.s.frame)?;
        els.iter().find(|e| e.id == id).and_then(|e| match e.kind {
            ElementKind::Shape(_) => Some(true),
            ElementKind::Symbol { .. } => Some(false),
            _ => None,
        })
    })
}

/// Selects the new bone a command made and says what to do next.
fn added(app: &mut App, v: &serde_json::Value, hint: String) {
    let (layer, bone) = (LayerId(v["layer"].as_u64().unwrap_or(0)), v["bone"].as_u64().map(BoneId));
    select(app, layer, bone);
    app.status_ok(hint);
}

/// Ends a Bone tool drag that began on `from` (an instance, a shape, or with `parent` the end
/// of that bone) at joint `head`: the bone goes to the instance or shape under the release, or
/// stays inside the shape it began in. Instances and shapes mix in one armature (ADR 0098):
///
/// * from an instance (or the end of a bone that moves one): the bone moves the instance or
///   shape it is released over;
/// * from inside a shape: the bone is drawn inside it, ending where it is released;
/// * from the end of a bone inside a shape, or of a bone that ends on a shape: released over a
///   symbol instance that is in no armature yet, the bone carries that instance; anywhere else
///   it continues inside the shape.
pub fn drag_end(app: &mut App, xf: Xf, from: ElementId, parent: Option<BoneId>, head: Pos2, cur: Pos2) {
    let tail = Vec2::new(cur.x, cur.y);
    let slop = 3.0 / xf.zoom.max(1e-3);
    let undo = app.undo_key();
    // Whether the bone can go on inside a shape: it began in one, or on the end of a bone that
    // is inside one or ends on one.
    let node = parent.is_some() && is_shape(app, from).is_none();
    let shape = node || is_shape(app, from) == Some(true);
    let inside_hint = tr!("Added a bone inside the shape; its points follow the nearest bones. Drag a bone with the Selection tool to bend it, or adjust the points with the Bind tool. {undo} to undo.", undo = undo);
    if shape {
        // An instance on a normal layer under the release joins the armature on this bone.
        let free = |l: &Layer, e: &Element| l.kind == LayerKind::Normal && matches!(e.kind, ElementKind::Symbol { .. });
        if let Some(p) = parent
            && let Some((_, to)) = item_at(app, tail, slop, Some(from), |_, e| matches!(e.kind, ElementKind::Symbol { .. } | ElementKind::Shape(_))).filter(|(l, id)| app.s.timeline().layer(*l).is_some_and(|l| evaluate_layer_elements(l, app.s.frame).is_some_and(|(_, els)| els.iter().any(|e| e.id == *id && free(l, e)))))
        {
            if let Some(v) = app.run("bone.add", json!({"parent": p.0, "to": to.0, "tail": [tail.x, tail.y]})) {
                added(app, &v, tr!("Added a bone from the shape to the instance: it follows the shape's bones. Drag bones or the instance with the Selection tool to pose them together. {undo} to undo.", undo = undo));
            }
            return;
        }
        let p = match parent {
            Some(b) => json!({"parent": b.0, "tail": [tail.x, tail.y]}),
            None => json!({"shape": from.0, "head": [head.x, head.y], "tail": [tail.x, tail.y]}),
        };
        if let Some(v) = app.run("bone.add_in_shape", p) {
            added(app, &v, inside_hint);
        }
        return;
    }
    let Some((_, to)) = item_at(app, tail, slop, Some(from), |_, e| matches!(e.kind, ElementKind::Symbol { .. } | ElementKind::Shape(_))) else {
        app.status_ok(tr!("A bone ends on another symbol instance or a shape: release over the one it should move."));
        return;
    };
    let p = match parent {
        Some(b) => json!({"parent": b.0, "to": to.0, "tail": [tail.x, tail.y]}),
        None => json!({"from": from.0, "to": to.0, "head": [head.x, head.y], "tail": [tail.x, tail.y]}),
    };
    if let Some(v) = app.run("bone.add", p) {
        let hint = if is_shape(app, to) == Some(true) {
            tr!("Added a bone to the shape: it moves as a whole. Drag from this bone's end inside the shape to give it bones of its own. {undo} to undo.", undo = undo)
        } else {
            tr!("Added a bone. Drag a bone or an instance with the Selection tool to pose the armature. {undo} to undo.", undo = undo)
        };
        added(app, &v, hint);
    }
}

/// A Bone tool click: picks the bone there (or none).
pub fn click(app: &mut App, xf: Xf, p: Pos2) {
    match hit(app, xf, p) {
        Some((layer, id, _)) => select(app, layer, Some(id)),
        None => app.bone_sel = None,
    }
}

/// A Selection tool drag starting at screen point `press` on an armature: posing (on a bone, or
/// an instance a bone moves) or moving the whole armature (on its root). `None` when the press
/// isn't on an armature.
pub fn select_drag_start(app: &mut App, xf: Xf, press: Pos2) -> Option<Drag> {
    let sp = xf.to_stage(press);
    let tl = app.s.timeline();
    let (layer, bone, local) = match hit(app, xf, press) {
        Some((layer, id, Part::End)) => (layer, Some(id), None),
        // Along a bone: the point grabbed follows the pointer (pulling the bone's end there would
        // make the chain jump, #239); on the joint a root's bone starts at, the armature moves.
        Some((layer, id, Part::Body)) => {
            let l = tl.layer(layer)?;
            let arm = l.armature.as_deref()?;
            let (_, els) = evaluate_layer_elements(l, app.s.frame)?;
            let pose = armature::pose_at(l, app.s.frame).unwrap_or_default();
            let from_root = arm.bone(id).is_some_and(|b| arm.bone_into(b.parent).is_none());
            let at_head = armature::bones_at(arm, &els, &pose).iter().find(|b| b.id == id).is_some_and(|b| xf.to_screen(b.head).distance(press) <= GRAB_END);
            if from_root && at_head {
                (layer, None, None)
            } else {
                (layer, Some(id), armature::grab_point(arm, &els, &pose, id, sp))
            }
        }
        None => {
            let (layer, el) = item_at(app, sp, 3.0 / xf.zoom.max(1e-3), None, rig_target).filter(|(l, _)| tl.layer(*l).is_some_and(|l| l.kind == LayerKind::Armature))?;
            let l = tl.layer(layer)?;
            let into = l.armature.as_ref().and_then(|a| a.bone_into(el)).map(|b| b.id);
            // The point grabbed, in the instance's own space: it follows the pointer.
            let w = evaluate_layer_elements(l, app.s.frame).and_then(|(_, els)| els.iter().find(|e| e.id == el).map(|e| e.transform))?;
            (layer, into, w.inverse().map(|m| m.apply(sp)))
        }
    };
    match bone {
        Some(id) => {
            select(app, layer, Some(id));
            let arm = app.s.timeline().layer(layer).and_then(|l| l.armature.clone())?;
            if armature::movable_chain(&arm, id).is_empty() {
                app.status_ok(tr!("That bone is pinned (or hangs from a pinned bone): unpin it in Properties to drag it."));
                return Some(Drag::None);
            }
            app.s.begin_gesture(tr_noop!("Pose Armature"));
            let local = local.or_else(|| arm.bone(id).map(|b| b.tail)).unwrap_or(Vec2::new(0.0, 0.0));
            Some(Drag::BonePose { bone: id, local: pos2(local.x, local.y) })
        }
        None => {
            select(app, layer, None);
            app.run("layer.select", json!({"layer": layer.0}));
            app.s.begin_gesture(tr_noop!("Move Armature"));
            Some(Drag::ArmatureMove { layer, last: pos2(sp.x, sp.y) })
        }
    }
}

/// Continues a pose or armature drag to stage point `c`.
pub fn drag_to(app: &mut App, drag: &mut Drag, c: Pos2) {
    match drag {
        Drag::BonePose { bone, local } => {
            app.run("bone.solve", json!({"bone": bone.0, "x": c.x, "y": c.y, "local": [local.x, local.y]}));
        }
        Drag::ArmatureMove { layer, last } => {
            let d = c - *last;
            if d.length_sq() > 0.0 {
                *last = c;
                app.run("bone.pose", json!({"layer": layer.0, "dx": d.x, "dy": d.y}));
            }
        }
        Drag::Bone { cur, .. } => *cur = c,
        _ => {}
    }
}

/// Delete removes the selected bone (and those below it); Esc deselects it.
pub fn consume_keys(app: &mut App, ctx: &egui::Context) {
    let Some(id) = app.bone_sel else { return };
    if !matches!(app.tool, Tool::Bone | Tool::Bind | Tool::Select) {
        return;
    }
    let take = |k: Key| ctx.input_mut(|i| i.consume_key(Modifiers::NONE, k));
    if take(Key::Delete) || take(Key::Backspace) {
        app.request("bone.remove", json!({"bone": id.0}));
    } else if take(Key::Escape) {
        app.bone_sel = None;
    }
}

/// Drops a selected bone that no longer exists.
pub fn upkeep(app: &mut App) {
    if let Some(id) = app.bone_sel {
        let tl = app.s.timeline();
        if !tl.layers.iter().any(|l| l.kind == LayerKind::Armature && l.armature.as_ref().is_some_and(|a| a.bone(id).is_some())) {
            app.bone_sel = None;
        }
    }
}

/// The bones of every visible armature, and the bone being drawn.
pub fn draw(app: &App, painter: &egui::Painter, xf: Xf) {
    let tool = matches!(app.tool, Tool::Bone | Tool::Bind | Tool::Select);
    for s in shown(app) {
        for b in &s.bones {
            let picked = app.bone_sel == Some(b.id);
            let color = if picked { T.selection } else { s.color };
            // Bones fade when they can't be edited (another tool, a locked layer).
            let color = if tool && !s.locked { color } else { color.gamma_multiply(0.5) };
            wedge(painter, xf.to_screen(b.head), xf.to_screen(b.tail), color, picked);
        }
    }
    if let Drag::Bone { head, cur, .. } = app.drag {
        let (a, b) = (xf.to_screen(Vec2::new(head.x, head.y)), xf.to_screen(Vec2::new(cur.x, cur.y)));
        wedge(painter, a, b, T.selection, true);
    }
}

/// A bone from joint `a` to end `b` on screen: a wedge, widest just past the joint, with a
/// round joint and a smaller round end.
fn wedge(painter: &egui::Painter, a: Pos2, b: Pos2, color: Color32, strong: bool) {
    let d = b - a;
    let len = d.length();
    let edge = Stroke::new(if strong { 1.5 } else { 1.0 }, color);
    if len > JOINT * 2.0 {
        let dir = d / len;
        let n = egui::vec2(-dir.y, dir.x);
        let w = (len * 0.12).clamp(JOINT, WIDE);
        let shoulder = a + dir * (w * 1.2).min(len * 0.3);
        let pts = vec![a, shoulder + n * w, b, shoulder - n * w];
        painter.add(egui::Shape::convex_polygon(pts, color.gamma_multiply(if strong { 0.55 } else { 0.35 }), edge));
    }
    painter.circle(a, JOINT, Color32::WHITE, edge);
    painter.circle(b, END, Color32::WHITE, edge);
}

#[cfg(test)]
mod tests {
    use super::*;
    use anicraft_engine::Session;
    use anicraft_engine::doc::Document;

    const XF: Xf = Xf::flat(Pos2::ZERO, 1.0);

    /// An app with a body shape standing on (300, 400) up to (300, 200) and a head instance
    /// centred on (300, 160), both on a normal layer: the body's and the head's ids.
    fn app() -> (App, u64, u64) {
        let ctx = egui::Context::default();
        let mut app = App::with_context(&ctx, Session::new(Document::default()), Some(std::path::PathBuf::from("unused.png")));
        let id = |v: Option<serde_json::Value>, k: &str| v.and_then(|v| v[k].as_u64()).expect("an id");
        let body = id(app.run("draw.path", json!({"d": "M280 400 L280 300 L280 200 L320 200 L320 300 L320 400 Z", "fill": "#335577", "object": true})), "element");
        app.run("draw.rect", json!({"x": 270, "y": 130, "w": 60, "h": 60, "fill": "#ffcc99"}));
        let head = id(app.run("modify.convert_to_symbol", json!({"name": "Head"})), "instance");
        app.tool = Tool::Bone;
        (app, body, head)
    }

    /// Drags the Bone tool from stage point `a` to `b`.
    fn drag(app: &mut App, a: (f32, f32), b: (f32, f32)) {
        let d = drag_start(app, XF, pos2(a.0, a.1));
        let Drag::Bone { from, parent, head, .. } = d else { panic!("no bone starts at {a:?}: {d:?}") };
        drag_end(app, XF, from, parent, head, pos2(b.0, b.1));
    }

    fn armature(app: &App) -> std::sync::Arc<anicraft_engine::doc::Armature> {
        app.s.timeline().layers.iter().find_map(|l| l.armature.clone()).expect("an armature")
    }

    #[test]
    fn a_chain_runs_from_a_shapes_bones_onto_an_instance() {
        let (mut app, body, head) = app();
        drag(&mut app, (300.0, 395.0), (300.0, 300.0));
        drag(&mut app, (300.0, 300.0), (300.0, 205.0));
        let arm = armature(&app);
        assert_eq!(arm.bones.len(), 2);
        assert!(arm.bones.iter().all(|b| b.shape.map(|s| s.0) == Some(body)), "both bones are inside the body");
        // From the neck joint, released over the head: the head joins on that joint.
        drag(&mut app, (300.0, 205.0), (300.0, 160.0));
        let arm = armature(&app);
        assert_eq!(arm.bones.len(), 3);
        let neck = &arm.bones[2];
        assert_eq!((neck.shape, neck.child.0, neck.parent), (None, head, arm.bones[1].child), "it starts on the joint node and moves the head");
        assert!(arm.is_mixed());
        assert_eq!(app.bone_sel, Some(neck.id), "the new bone is picked");
        assert!(app.s.selection.contains(app.s.layer, ElementId(head)), "and the instance it moves, for its look in Properties");
        // Dragging the head with the Selection tool bends the body under it.
        app.tool = Tool::Select;
        let before = app.s.doc.clone();
        let mut d = select_drag_start(&mut app, XF, pos2(300.0, 160.0)).expect("a pose drag");
        assert!(matches!(d, Drag::BonePose { .. }));
        drag_to(&mut app, &mut d, pos2(360.0, 200.0));
        app.s.end_gesture();
        assert_ne!(app.s.doc, before);
        app.s.undo().expect("one step");
        assert_eq!(app.s.doc, before);
        // From the neck joint again, released on the empty stage: a bone inside the body.
        app.tool = Tool::Bone;
        drag(&mut app, (300.0, 205.0), (340.0, 230.0));
        let arm = armature(&app);
        assert_eq!(arm.bones.last().and_then(|b| b.shape).map(|s| s.0), Some(body));
    }

    #[test]
    fn a_chain_runs_from_an_instance_into_a_shape() {
        let (mut app, body, head) = app();
        // From the head, released over the body: the body hangs from the head as a whole.
        drag(&mut app, (300.0, 160.0), (300.0, 210.0));
        let arm = armature(&app);
        assert_eq!(arm.bones.len(), 1);
        assert_eq!((arm.bones[0].parent.0, arm.bones[0].child.0, arm.bones[0].shape), (head, body, None));
        assert!(arm.skins.is_empty());
        // From that bone's end, on inside the body: the body's first bone, and its skin.
        drag(&mut app, (300.0, 210.0), (300.0, 300.0));
        let arm = armature(&app);
        assert_eq!(arm.bones.len(), 2);
        assert_eq!((arm.bones[1].shape.map(|s| s.0), arm.bones[1].parent.0), (Some(body), body));
        assert_eq!(arm.skins.len(), 1);
        assert!(arm.is_mixed());
        // A drag that begins inside the body draws a bone inside it, wherever it ends.
        drag(&mut app, (300.0, 380.0), (300.0, 160.0));
        assert_eq!(armature(&app).bones.len(), 3);
        assert_eq!(armature(&app).bones[2].shape.map(|s| s.0), Some(body));
    }
}
